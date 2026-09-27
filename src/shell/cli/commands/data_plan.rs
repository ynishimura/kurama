//! Resolve workspace and request precedence into one validated analysis plan.
use super::data_command::DataCommand;
use crate::{
    adapters::{aws::s3_data, config::Config},
    domain::types::dataset::{DataError, DataLimits, DataRequest, DataSource, InvalidInput},
};
use std::path::Path;

pub(super) struct Plan {
    pub sources: Vec<DataSource>,
    pub limits: DataLimits,
    pub aws_profile: Option<String>,
    /// The region the request named, directly or through a regional S3 URL.
    /// Without one, S3 starts in the AWS profile's region and the bucket
    /// corrects it.
    pub region: Option<String>,
}

pub(super) fn plan(
    command: &DataCommand,
    request: &DataRequest,
    url_region: Option<String>,
    config: &Config,
) -> Result<Plan, DataError> {
    request.validate()?;
    let a = request.args();
    if command.workspace.is_some() == a.from.is_some() {
        return Err(DataError::Invalid(InvalidInput::WorkspaceOrInput));
    }
    let (mut sources, mut limits, root, aws_profile, s3_source, region) =
        if let Some(name) = &command.workspace {
            let w = config
                .data
                .get(name)
                .ok_or(DataError::Invalid(InvalidInput::UnknownWorkspace))?;
            if !a.types.is_empty()
                || a.header.is_some()
                || a.delimiter.is_some()
                || a.date_format.is_some()
                || a.timestamp_format.is_some()
            {
                return Err(DataError::Invalid(InvalidInput::WorkspaceCsvOptions));
            }
            (
                w.sources.clone(),
                w.limits.clone(),
                w.root_dir.clone(),
                if a.s3_source.is_some() {
                    None
                } else {
                    a.aws_profile.clone().or(w.aws_profile.clone())
                },
                if a.aws_profile.is_some() {
                    None
                } else {
                    a.s3_source.clone().or(w.s3_source.clone())
                },
                a.region.clone().or(w.region.clone()),
            )
        } else {
            let source = DataSource {
                name: "data".into(),
                path: a.from.clone().unwrap(),
                format: None,
                types: a.types.clone(),
                header: a.header,
                delimiter: a.delimiter.clone(),
                date_format: a.date_format.clone(),
                timestamp_format: a.timestamp_format.clone(),
                union_by_name: false,
                hive_partitioning: false,
            };
            (
                vec![source],
                DataLimits::default(),
                None,
                a.aws_profile.clone(),
                a.s3_source.clone(),
                a.region.clone(),
            )
        };
    macro_rules! replace { ($($name:ident),*) => { $(if let Some(value) = &a.$name { limits.$name = value.clone(); })* }; }
    replace!(
        memory_limit,
        threads,
        max_rows,
        max_result_bytes,
        max_temp_directory_size,
        allow_spill,
        max_source_objects,
        max_source_bytes,
        query_timeout_secs
    );

    if matches!(request, DataRequest::Preview(_)) && a.max_rows.is_none() {
        limits.max_rows = limits.max_rows.min(100);
    }
    limits.validate()?;
    if aws_profile.is_some() && s3_source.is_some() {
        return Err(DataError::Invalid(InvalidInput::CredentialSourceConflict));
    }
    let (aws_profile, region) = if let Some(name) = s3_source {
        let source = config
            .s3
            .get(&name)
            .ok_or(DataError::Invalid(InvalidInput::UnknownS3Source))?;
        (
            Some(source.aws_profile.clone()),
            region.or(source.region.clone()),
        )
    } else {
        (aws_profile, region)
    };
    if region.as_ref().is_some_and(|r| r.trim().is_empty()) {
        return Err(InvalidInput::EmptyRegion.into());
    }
    let region = region.or(url_region);
    if aws_profile.as_ref().is_some_and(|p| p.trim().is_empty()) {
        return Err(InvalidInput::EmptyAwsProfile.into());
    }
    let root = match root {
        Some(root) => std::path::PathBuf::from(root),
        None => std::env::current_dir().map_err(DataError::Io)?,
    };
    for source in &mut sources {
        source.format = Some(source.inferred_format()?);
        if source.path.starts_with("s3://") {
            s3_data::split_uri(&source.path)?;
        } else {
            if source.path.contains("://") {
                return Err(DataError::Invalid(InvalidInput::UnsupportedSourceProtocol));
            }
            source.path = root.join(&source.path).to_string_lossy().into_owned();
        }
    }
    if let Some(table) = &a.table
        && !sources.iter().any(|s| s.name.eq_ignore_ascii_case(table))
    {
        return Err(DataError::Invalid(InvalidInput::UnknownTable));
    }
    if let Some(path) = &a.export
        && (path.contains("://")
            || !Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| {
                    e.eq_ignore_ascii_case("csv") || e.eq_ignore_ascii_case("parquet")
                }))
    {
        return Err(DataError::Invalid(InvalidInput::InvalidExportPath));
    }
    Ok(Plan {
        sources,
        limits,
        aws_profile,
        region,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::parse(
            r#"
[s3.configured]
aws_profile = "workspace-profile"
region = "us-east-1"
[s3.override]
aws_profile = "override-profile"
region = "eu-west-1"
[data.lake]
s3_source = "configured"
[[data.lake.sources]]
name = "orders"
path = "s3://bucket/orders.csv"
"#,
        )
        .unwrap()
    }

    fn make_plan(args: &[&str], config: &Config) -> Result<Plan, DataError> {
        let matches = super::super::data_command::command()
            .try_get_matches_from(args)
            .unwrap();
        let command = DataCommand::parse(&matches);
        let (request, url_region) = command.read_request()?;
        plan(&command, &request, url_region, config)
    }

    #[test]
    fn data_plan_credential_override_replaces_the_inherited_source() {
        let config = config();
        let plan = make_plan(
            &["data", "lake", "--tables", "--aws-profile", "explicit"],
            &config,
        )
        .unwrap();
        assert_eq!(plan.aws_profile.as_deref(), Some("explicit"));
        assert_eq!(plan.region, None);
        let mut config = config;
        let workspace = config.data.get_mut("lake").unwrap();
        workspace.s3_source = None;
        workspace.aws_profile = Some("direct-profile".into());
        let plan = make_plan(
            &["data", "lake", "--tables", "--s3-source", "override"],
            &config,
        )
        .unwrap();
        assert_eq!(plan.aws_profile.as_deref(), Some("override-profile"));
        assert_eq!(plan.region.as_deref(), Some("eu-west-1"));
    }

    #[test]
    fn data_plan_reads_the_region_of_a_regional_url_unless_one_is_named() {
        let url = "https://bucket.s3.us-west-2.amazonaws.com/orders.parquet";
        let plan = make_plan(&["data", "--from", url, "--preview"], &Config::default()).unwrap();
        assert_eq!(plan.region.as_deref(), Some("us-west-2"));
        let plan = make_plan(
            &["data", "--from", url, "--preview", "--region", "eu-west-1"],
            &Config::default(),
        )
        .unwrap();
        assert_eq!(plan.region.as_deref(), Some("eu-west-1"));
        let plan = make_plan(
            &[
                "data",
                "--from",
                "https://bucket.s3.amazonaws.com/orders.parquet",
                "--preview",
            ],
            &Config::default(),
        )
        .unwrap();
        assert_eq!(plan.region, None);
    }

    #[test]
    fn data_plan_requires_a_workspace_or_input() {
        assert!(matches!(
            make_plan(&["data", "--tables"], &Config::default()),
            Err(DataError::Invalid(InvalidInput::WorkspaceOrInput))
        ));
    }

    #[test]
    fn data_plan_resolves_format_and_applies_request_limits() {
        let plan = make_plan(
            &[
                "data",
                "ORDERS.CSV",
                "--preview",
                "--timeout",
                "7",
                "--max-source-bytes",
                "123",
                "--max-rows",
                "42",
            ],
            &Config::default(),
        )
        .unwrap();
        assert!(matches!(
            plan.sources[0].format,
            Some(crate::domain::types::dataset::DataFormat::Csv)
        ));
        assert_eq!(plan.limits.query_timeout_secs, 7);
        assert_eq!(plan.limits.max_source_bytes, 123);
        assert_eq!(plan.limits.max_rows, 42);
        let plan = make_plan(&["data", "orders.csv", "--preview"], &Config::default()).unwrap();
        assert_eq!(plan.limits.max_rows, 100);
    }

    #[test]
    fn data_plan_workspace_options_and_unknown_tables_are_rejected() {
        let config = config();
        for args in [
            vec!["data", "lake", "--tables", "--delimiter", "|"],
            vec!["data", "lake", "--preview", "missing"],
            vec!["data", "lake", "--tables", "--s3-source", "missing"],
        ] {
            assert!(make_plan(&args, &config).is_err(), "{args:?}");
        }
    }
}
