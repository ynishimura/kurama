//! Offline data capability schema and configured workspace status; no engine or credential lookup.
use crate::{
    adapters::config::Config,
    domain::types::dataset::{DataLimits, DataOperation, DataOutput, DataRequest},
};
use serde_json::{Value, json};

#[cfg(test)]
#[path = "data_contract_tests.rs"]
mod tests;

pub fn capabilities() -> Value {
    json!({"schema_version": 1, "kind":"data", "request_operations": DataOperation::REQUESTS, "result_operations": DataOperation::RESULTS,
        "operation_modifiers": {"explain":"query with args.explain = true", "export":"query with args.export set"},
        "request_schema": schemars::schema_for!(DataRequest), "result_schema": result_schema(),
        "defaults": DataLimits::default(), "preview_default_rows":100,
        "side_effects":{"tables":"read input metadata", "describe":"read input metadata", "preview":"read inputs", "summary":"scan inputs", "query":"scan inputs", "explain":"read input metadata", "export":"scan inputs and create a local file", "dry_run":"none"},
        "examples":[{"operation":"query","args":{"from":"orders.csv", "sql":"SELECT count(*) AS count FROM data", "max_rows":100}}],
        "capabilities":{"local":true,"s3":true,"tui":false,"s3_explorer":false},
        "input_formats":["csv","jsonl","ndjson","parquet"], "gzip_input_formats":["csv","jsonl","ndjson"],
        "input_syntax":{"positional":"a path/glob, s3 URI or S3 HTTPS URL is shorthand for --from; otherwise a workspace name", "ad_hoc_default_operation":"preview", "optional_table":"describe/preview/summary select the source when exactly one exists", "s3_https":"convert standard AWS S3 object URLs to s3:// and discard query credentials; use an explicit AWS profile or S3 source; versionId, partNumber and glob characters in URL keys are unsupported", "signed_url_input":"use --request - to keep the URL out of argv; never store signed URLs in workspace configuration"},
        "output_options":{"jq":"--jq FILTER implies --json and projects the envelope to exactly one JSON value; incomplete queries and byte limits exit nonzero; row-limited previews succeed"},
        "result_envelope":{"schema_version":1,"kind":"data","operation":"operation name","target":"workspace name or ad-hoc","meta":"effective limits, sources, bounded inputs, observed_at, input_count, input_bytes, inputs_omitted, returned_rows, returned_result_bytes, consistency, next_actions","engine":"name and version","elapsed_ms":"integer"},
        "error_schema": crate::shell::cli::client_error::schema()})
}

fn result_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(DataOutput)).expect("data output schema serializes")
}

/// Configuration-only data status; no engine or credentials have been opened.
#[derive(Debug, Clone)]
pub struct DataStatusRow {
    pub name: String,
    pub sources: Vec<crate::domain::types::dataset::DataSource>,
    pub aws_profile: Option<String>,
    pub region: Option<String>,
}

pub fn status_rows(config: &Config, name: Option<&str>) -> Vec<DataStatusRow> {
    config
        .data
        .iter()
        .filter(|(workspace, _)| name.is_none_or(|name| *workspace == name))
        .map(|(name, workspace)| {
            let source = workspace
                .s3_source
                .as_ref()
                .and_then(|name| config.s3.get(name));
            DataStatusRow {
                name: name.clone(),
                sources: workspace.sources.clone(),
                aws_profile: workspace
                    .aws_profile
                    .as_ref()
                    .or(source.map(|source| &source.aws_profile))
                    .cloned(),
                region: workspace
                    .region
                    .as_ref()
                    .or(source.and_then(|source| source.region.as_ref()))
                    .cloned(),
            }
        })
        .collect()
}

pub fn print_status(config: &Config, name: Option<&str>, json: bool) -> anyhow::Result<()> {
    let rows: Vec<_> = status_rows(config, name)
        .into_iter()
        .map(super::status::StatusRow::Data)
        .collect();
    if name.is_some() && rows.is_empty() {
        return Err(crate::domain::types::dataset::DataError::Invalid(
            crate::domain::types::dataset::InvalidInput::UnknownWorkspace,
        )
        .into());
    }
    super::status::print_rows(&rows, chrono::Utc::now(), json);
    Ok(())
}
