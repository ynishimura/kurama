//! `kurama api <API> --ops [QUERY]`, `--describe <OP>`, `--schema [OP]`,
//! `--skill` and `--refresh-spec`, and the operation targets of `kurama api <API> <OP>`:
//! everything that reads the API's OpenAPI description on the command line.
//!
//! stdout carries the operation table, the description, their `--json`
//! forms, the `--schema` contract, or the `--skill` SKILL.md; warnings about what the description holds that kurama cannot
//! represent go to stderr.

use std::io::Write;

use anyhow::Result;
use serde_json::Value;

use crate::adapters::config::{ApiDescription, ApiProfile, SpecSource};
use crate::adapters::jq::apply_filter;
use crate::console::progress;
use crate::domain::functions::api_request::{TargetLocation, parse_target};
use crate::domain::functions::api_schema::{SchemaContract, schema_document};
use crate::domain::functions::api_skill::{SkillAuth, SkillDescriptionSource, render_skill};
use crate::domain::functions::graphql_call::build_graphql_request;
use crate::domain::functions::operation_lookup::{
    OperationTarget, find_operation, parse_operation_target, search_operations, suggest_operations,
};
use crate::domain::functions::operation_request::build_operation_request;
use crate::domain::functions::spec_output::{
    operation_json, operations_json, render_operation_description, render_operations_table,
};
use crate::domain::types::AuthSource;
use crate::domain::types::api_spec::{ApiSpec, Operation, SpecFormat};
use crate::ports::HttpRequest;
use crate::shell::api_error::ApiError;
use crate::shell::api_runtime::ApiRuntime;
use crate::shell::cli::commands::api_command::options_contract;
use crate::shell::spec_loader::{
    LoadedSpec, SpecAccess, SpecOrigin, load_spec_verbose, load_spec_with,
};

/// Candidates named when an operation is not found.
const SUGGESTIONS: usize = 3;
/// Warning lines printed before `... and N more`.
const WARNINGS_SHOWN: usize = 5;

/// `--refresh-spec`: fetch the description again and say what came back.
pub async fn refresh_spec(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    access: SpecAccess,
) -> Result<LoadedSpec> {
    let loaded = load_spec_with(runtime, api, true, "--refresh-spec", access).await?;
    progress!(
        "# {}: {} {} with {} operations, from {}",
        api.name,
        loaded.spec.title,
        loaded.spec.version,
        loaded.spec.operations.len(),
        loaded.origin
    );
    print_warnings(&loaded.spec);
    Ok(loaded)
}

/// How `--ops` and `--describe` print: the table or description, the JSON
/// document, or the results of a jq filter on that document.
#[derive(Debug, Clone, Copy)]
pub struct ListingFormat<'a> {
    pub json: bool,
    pub jq: Option<&'a str>,
}

impl ListingFormat<'_> {
    fn write(self, text: String, document: serde_json::Value) -> Result<()> {
        let mut stdout = std::io::stdout().lock();
        match self.jq {
            Some(filter) => {
                for line in apply_filter(filter, &document, None).map_err(ApiError::Jq)? {
                    writeln!(stdout, "{line}")?;
                }
            }
            None if self.json => writeln!(stdout, "{document}")?,
            None => write!(stdout, "{text}")?,
        }
        Ok(())
    }
}

/// `--ops [QUERY]`: the matching operations as a table or JSON.
pub async fn list_operations(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    query: &str,
    format: ListingFormat<'_>,
    refresh: bool,
    verbose: bool,
) -> Result<()> {
    let loaded = load_spec_verbose(
        runtime,
        api,
        refresh,
        "--ops",
        SpecAccess::WithCredential,
        verbose,
    )
    .await?;
    print_warnings(&loaded.spec);
    let operations = search_operations(&loaded.spec, query);
    if operations.is_empty() {
        progress!("# no operation matches {query:?}");
    }
    format.write(
        render_operations_table(&operations),
        operations_json(&operations),
    )
}

/// `--describe <OP>`: one operation, its inputs and the example command.
pub async fn describe_operation(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    target: &str,
    format: ListingFormat<'_>,
    refresh: bool,
    verbose: bool,
) -> Result<()> {
    let loaded = load_spec_verbose(
        runtime,
        api,
        refresh,
        "--describe",
        SpecAccess::WithCredential,
        verbose,
    )
    .await?;
    let operation =
        find_operation(&loaded.spec, target).ok_or_else(|| not_found(api, &loaded.spec, target))?;
    format.write(
        render_operation_description(&api.name, operation),
        operation_json(&api.name, operation),
    )
}

/// `--schema [OP]`: the contract of every operation, or of `target`, as
/// one JSON document (or the results of `--jq` on it). The description's
/// warnings are in the document, so stderr stays quiet.
pub async fn print_schema(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    target: Option<&str>,
    jq: Option<&str>,
    refresh: bool,
    verbose: bool,
) -> Result<()> {
    let loaded = load_spec_verbose(
        runtime,
        api,
        refresh,
        "--schema",
        SpecAccess::WithCredential,
        verbose,
    )
    .await?;
    let operations = match target {
        Some(target) => vec![
            find_operation(&loaded.spec, target)
                .ok_or_else(|| not_found(api, &loaded.spec, target))?,
        ],
        None => loaded.spec.operations.iter().collect(),
    };
    ListingFormat { json: true, jq }
        .write(String::new(), contract_document(api, &loaded, &operations))
}

/// The `--schema` document of `operations`, which `--skill` renders too.
fn contract_document(api: &ApiProfile, loaded: &LoadedSpec, operations: &[&Operation]) -> Value {
    let contract = SchemaContract {
        api: &api.name,
        base_url: &api.base_url,
        spec: &loaded.spec,
        source: source_json(&loaded.origin),
        options: options_contract(),
    };
    schema_document(&contract, operations)
}

/// `--skill`: the SKILL.md of the API on stdout.
pub async fn print_skill(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    refresh: bool,
    verbose: bool,
) -> Result<()> {
    let skill =
        render_api_skill(runtime, api, refresh, SpecAccess::WithCredential, verbose).await?;
    write!(std::io::stdout().lock(), "{skill}")?;
    Ok(())
}

/// The SKILL.md of the API, rendered from the same contract `--schema`
/// prints. The description is loaded the way `--schema` loads it (with
/// `access`); the credential source is only named, never started.
pub async fn render_api_skill(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    refresh: bool,
    access: SpecAccess,
    verbose: bool,
) -> Result<String> {
    let loaded = load_spec_verbose(runtime, api, refresh, "--skill", access, verbose).await?;
    let source = runtime.auth_source(api)?;
    let auth = match (&api.aws_profile, &source) {
        (Some(aws_profile), _) => SkillAuth::SigV4 {
            aws_profile,
            service: api.signing.service.as_deref(),
            region: api.signing.region.as_deref(),
        },
        (None, None) => SkillAuth::None,
        (None, Some(AuthSource::OAuth(client))) => SkillAuth::OAuth {
            source: &client.name,
        },
        (None, Some(AuthSource::Token(token))) => SkillAuth::Token {
            source: &token.name,
            placement: &token.placement,
        },
    };
    let description_source = match &api.spec {
        Some(ApiDescription {
            format,
            source: SpecSource::Url(_),
        }) => SkillDescriptionSource::Url {
            with_credential: api.openapi_auth || matches!(format, SpecFormat::GraphQl(_)),
        },
        _ => SkillDescriptionSource::File,
    };
    let document = contract_document(
        api,
        &loaded,
        &loaded.spec.operations.iter().collect::<Vec<_>>(),
    );
    Ok(render_skill(&document, &auth, description_source))
}

/// Where the description came from, for the contract's `source`.
pub(crate) fn source_json(origin: &SpecOrigin) -> serde_json::Value {
    use serde_json::json;
    match origin {
        SpecOrigin::File(path) => json!({"kind": "file", "path": path.display().to_string()}),
        SpecOrigin::Fetched { url } => json!({"kind": "fetched", "url": url}),
        SpecOrigin::Cached { url, fetched_at } => {
            json!({"kind": "cached", "url": url, "fetched_at": fetched_at.to_rfc3339()})
        }
        SpecOrigin::Validated { url, fetched_at } => {
            json!({"kind": "validated", "url": url, "fetched_at": fetched_at.to_rfc3339()})
        }
        SpecOrigin::Stale {
            url,
            fetched_at,
            reason,
        } => json!({
            "kind": "stale",
            "url": url,
            "fetched_at": fetched_at.to_rfc3339(),
            "reason": reason,
        }),
        SpecOrigin::Unchecked { url, fetched_at } => {
            json!({"kind": "unchecked", "url": url, "fetched_at": fetched_at.to_rfc3339()})
        }
    }
}

/// How a TARGET is answered.
pub enum TargetKind {
    /// `/path`, a URL, or `METHOD /path` without a description: the request
    /// is built from the arguments alone.
    Plain,
    /// The description decides: an operationId, or `METHOD /path` that the
    /// description knows.
    Operation,
}

/// Whether `target` needs the description. A `METHOD /path` is looked up
/// in the description when the API has one, so `-P` parameters can fill
/// its template; a `/path` or a URL never is.
pub fn target_kind(api: &ApiProfile, target: &str) -> TargetKind {
    match parse_target(target) {
        Ok(parsed) => match (&parsed.location, parsed.method.is_some(), &api.spec) {
            // Every GraphQL operation is `POST <endpoint>`: that target is
            // the endpoint itself, sent as it is.
            (
                TargetLocation::Path(_),
                true,
                Some(ApiDescription {
                    format: SpecFormat::GraphQl(_),
                    ..
                }),
            ) => TargetKind::Plain,
            (TargetLocation::Path(_), true, Some(_)) => TargetKind::Operation,
            _ => TargetKind::Plain,
        },
        Err(_) => TargetKind::Operation,
    }
}

/// How an operation target loads the description: `--refresh-spec`,
/// whether it may use the API's credential, and `-v`.
#[derive(Debug, Clone, Copy)]
pub struct SpecLoad {
    pub refresh: bool,
    pub access: SpecAccess,
    pub verbose: bool,
}

/// The request of an operation target: the operation is found in the
/// description, the `-P` parameters checked and placed, the body attached;
/// a GraphQL operation is its query document with `select` as the
/// selection. A `METHOD /path` the description does not know falls back to
/// a plain request (`request` is `None`) when no `-P` was given.
pub struct OperationCall {
    /// Where the description came from, for the dry-run plan.
    pub origin: SpecOrigin,
    pub request: Option<HttpRequest>,
    /// Whether the request calls a GraphQL operation, whose answer may
    /// carry `errors` with a 200.
    pub graphql: bool,
}

pub async fn operation_request(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    target: &str,
    input: OperationInput<'_>,
    load: SpecLoad,
) -> Result<OperationCall> {
    if api.spec.is_none() {
        return Err(ApiError::ArgumentInvalid(format!(
            "TARGET must be a path starting with '/', an http(s) URL, or `METHOD /path`; an operationId needs openapi under [api.{}]; got {target:?}",
            api.name
        ))
        .into());
    }
    let loaded = load_spec_verbose(
        runtime,
        api,
        load.refresh,
        &format!("the target {target:?}"),
        load.access,
        load.verbose,
    )
    .await?;
    let Some(operation) = find_operation(&loaded.spec, target) else {
        let is_concrete_path = matches!(
            parse_operation_target(target),
            OperationTarget::MethodPath { path, .. } if !path.contains('{')
        );
        if is_concrete_path && input.params.is_empty() && input.select.is_none() {
            return Ok(OperationCall {
                origin: loaded.origin,
                request: None,
                graphql: false,
            });
        }
        return Err(not_found(api, &loaded.spec, target).into());
    };
    let request = match (&operation.graphql, input.select) {
        (Some(field), select) => build_graphql_request(
            &api.base_url,
            operation,
            field,
            input.params,
            input.body,
            select,
        ),
        (None, Some(_)) => return Err(select_needs_graphql(target).into()),
        (None, None) => build_operation_request(&api.base_url, operation, input.params, input.body),
    }
    .map_err(|error| ApiError::OperationInput {
        api: api.name.clone(),
        error,
    })?;
    Ok(OperationCall {
        origin: loaded.origin,
        request: Some(request),
        graphql: operation.graphql.is_some(),
    })
}

/// What an operation target is called with: `-P`, `-d` and `--select`.
pub struct OperationInput<'a> {
    pub params: &'a [(String, String)],
    pub body: Option<Vec<u8>>,
    pub select: Option<&'a str>,
}

/// `--select` on a target that is not a GraphQL operation.
pub fn select_needs_graphql(target: &str) -> ApiError {
    ApiError::ArgumentInvalid(format!(
        "--select applies to a GraphQL operation (query.<field> or mutation.<field>); TARGET {target:?} is not one"
    ))
}

/// `-P name=value` pairs as given.
pub fn parse_params(params: &[String]) -> Result<Vec<(String, String)>, ApiError> {
    params
        .iter()
        .map(|param| {
            param
                .split_once('=')
                .filter(|(name, _)| !name.is_empty())
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .ok_or_else(|| {
                    ApiError::ArgumentInvalid(format!("-P {param:?} is not `name=value`"))
                })
        })
        .collect()
}

pub fn not_found(api: &ApiProfile, spec: &ApiSpec, target: &str) -> ApiError {
    ApiError::OperationNotFound {
        api: api.name.clone(),
        target: target.to_string(),
        candidates: suggest_operations(spec, target, SUGGESTIONS)
            .iter()
            .map(|operation| operation.id.clone())
            .collect(),
    }
}

/// The description's warnings on stderr (held until the explorer exits).
pub fn print_warnings(spec: &ApiSpec) {
    for line in spec.warnings.iter().take(WARNINGS_SHOWN) {
        progress!("# warning: {line}");
    }
    if spec.warnings.len() > WARNINGS_SHOWN {
        progress!(
            "# warning: ... and {} more",
            spec.warnings.len() - WARNINGS_SHOWN
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::ApiToml;

    fn api(openapi: bool) -> ApiProfile {
        let toml = if openapi {
            "base_url = \"https://x\"\nopenapi = \"/tmp/spec.json\"\n"
        } else {
            "base_url = \"https://x\"\n"
        };
        toml::from_str::<ApiToml>(toml)
            .unwrap()
            .typed("x", &Default::default())
            .unwrap()
    }

    #[test]
    fn paths_and_urls_are_plain_and_ids_need_the_description() {
        for target in ["/user", "https://x/user", "GET /user"] {
            assert!(
                matches!(target_kind(&api(false), target), TargetKind::Plain),
                "{target}"
            );
        }
        assert!(matches!(
            target_kind(&api(true), "/user"),
            TargetKind::Plain
        ));
        assert!(matches!(
            target_kind(&api(true), "https://x/user"),
            TargetKind::Plain
        ));
        assert!(matches!(
            target_kind(&api(true), "GET /user"),
            TargetKind::Operation
        ));
        assert!(matches!(
            target_kind(&api(true), "issues/create"),
            TargetKind::Operation
        ));
        assert!(matches!(
            target_kind(&api(false), "issues/create"),
            TargetKind::Operation
        ));
    }

    /// Every GraphQL operation is `POST <endpoint>`, so a `METHOD /path`
    /// is the endpoint sent as it is, while an operation id is looked up.
    #[test]
    fn a_method_and_path_on_a_graphql_api_is_plain() {
        let graphql =
            toml::from_str::<ApiToml>("base_url = \"https://x\"\ngraphql = \"/graphql\"\n")
                .unwrap()
                .typed("x", &Default::default())
                .unwrap();
        assert!(matches!(
            target_kind(&graphql, "POST /graphql"),
            TargetKind::Plain
        ));
        assert!(matches!(
            target_kind(&graphql, "query.viewer"),
            TargetKind::Operation
        ));
    }

    #[test]
    fn params_are_name_value_pairs() {
        assert_eq!(
            parse_params(&["owner=o".into(), "q=a=b".into()]).unwrap(),
            [
                ("owner".to_string(), "o".to_string()),
                ("q".to_string(), "a=b".to_string())
            ]
        );
        assert!(parse_params(&["owner".into()]).is_err());
        assert!(parse_params(&["=x".into()]).is_err());
    }
}
