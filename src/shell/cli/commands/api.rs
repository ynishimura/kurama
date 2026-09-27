//! `kurama api <API> [TARGET] [OPTIONS]`: one HTTP request to an `[api.*]`
//! profile with its source's bearer token, or with a SigV4 signature from
//! its `aws_profile`'s role credentials.
//!
//! TARGET is a path, a URL, `METHOD /path`, or an operation of the API's
//! OpenAPI description (`api_spec`); without one, the explorer opens on a
//! terminal. stdout carries the response body, the `--jq` results, or the
//! `--json` envelope `{status, headers, body}` -- or nothing, when
//! `--output PATH` takes a 2xx body; everything else goes to stderr. A status outside 2xx is `API_HTTP_ERROR` (exit 4); with `--json`
//! the envelope is still printed first so an agent sees the answer. A
//! GraphQL operation answered with `errors` is `API_GRAPHQL_ERROR` (exit 4)
//! the same way, whatever the status.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use super::api_command::{
    ApiCommand, ApiConnectionOptions, ApiOutputOptions, ApiPages, ApiRequestOptions, ApiSpecOptions,
};
use super::api_pages::{dry_run_note, fetch_pages};
use super::api_plan::DryRunPlan;
use super::api_spec::{
    OperationInput, SpecLoad, TargetKind, describe_operation, list_operations, operation_request,
    parse_params, print_schema, print_skill, refresh_spec, select_needs_graphql, target_kind,
};
use crate::adapters::config::{ApiProfile, Config};
use crate::adapters::jq::apply_filter;
use crate::console::progress;
use crate::domain::functions::agent_policy::AgentPolicy;
use crate::domain::functions::api_body_fit::fit_body;
use crate::domain::functions::api_request::{
    body_as_json, body_excerpt, indent_json, parse_target, reason_phrase, render_dry_run,
    resolve_url, response_envelope, with_default_headers, with_headers,
};
use crate::domain::functions::graphql_call::graphql_error_line;
use crate::domain::functions::signing_target::SigningHint;
use crate::domain::types::SecretRef;
use crate::domain::types::http::{mask_secret_headers, mask_secret_headers_with};
use crate::domain::types::request_history::HistoryEntry;
use crate::ports::{HttpRequest, HttpResponse};
use crate::shell::agent_policy::{api_policy, check_api_request, is_agent_run};
use crate::shell::api_error::ApiError;
use crate::shell::api_runtime::{ApiCredential, ApiRuntime, ApiRuntimeOptions};
use crate::shell::spec_loader::{SpecAccess, SpecOrigin};
use crate::shell::tui::explorer::handle_explorer_command;

/// Characters of a response body an error line carries.
const ERROR_EXCERPT_CHARS: usize = 512;

pub async fn handle_api_command(command: ApiCommand, config: Config) -> Result<()> {
    let api = resolve_api(&config, &command.api)?;
    let api = apply_signing_options(api, &command.signing)?;
    let params = parse_params(&command.request.params)?;
    let policy = api_policy(&config, &api.name, is_agent_run(), command.request.confirm);
    let runtime = create_runtime(config, &command.connection, command.output.verbose)?;
    let listing = command.output.listing_format();
    if let Some(query) = &command.spec.ops {
        return list_operations(
            &runtime,
            &api,
            query,
            listing,
            command.spec.refresh,
            command.output.verbose,
        )
        .await;
    }
    if let Some(target) = &command.spec.describe {
        return describe_operation(
            &runtime,
            &api,
            target,
            listing,
            command.spec.refresh,
            command.output.verbose,
        )
        .await;
    }
    if let Some(target) = &command.spec.schema {
        return print_schema(
            &runtime,
            &api,
            target.as_deref(),
            command.output.jq.as_deref(),
            command.spec.refresh,
            command.output.verbose,
        )
        .await;
    }
    if command.spec.skill {
        return print_skill(&runtime, &api, command.spec.refresh, command.output.verbose).await;
    }
    let Some(target) = command.target.as_deref() else {
        return handle_missing_target(&command.spec, api, runtime, command.output.verbose).await;
    };
    execute_target(&runtime, &api, &command, target, &params, policy.as_ref()).await
}

fn resolve_api(config: &Config, name: &str) -> Result<ApiProfile> {
    config
        .api_profile(name)?
        .ok_or_else(|| ApiError::NotFound(name.to_string()).into())
}

fn create_runtime(
    config: Config,
    options: &ApiConnectionOptions,
    verbose: bool,
) -> Result<ApiRuntime> {
    ApiRuntime::from_config(
        Arc::new(config),
        ApiRuntimeOptions {
            timeout: Duration::from_secs(options.timeout_secs),
            accept_invalid_certs: options.insecure,
            open_browser: true,
            report_secret_reads: verbose,
        },
    )
}

async fn handle_missing_target(
    spec: &ApiSpecOptions,
    api: ApiProfile,
    runtime: ApiRuntime,
    verbose: bool,
) -> Result<()> {
    if spec.refresh {
        refresh_spec(&runtime, &api, SpecAccess::WithCredential).await?;
        return Ok(());
    }
    // An agent or a pipe must get an answer instead of a blank TUI.
    if !crate::shell::tui::terminal::supports_tui() {
        return Err(ApiError::TargetRequired.into());
    }
    handle_explorer_command(api, runtime, verbose, None).await
}

/// The explorer the home screen's palette opens: `kurama api <API>` on a
/// terminal, with the form of `start` open once the description is loaded.
pub async fn handle_explorer_at(name: &str, start: HistoryEntry, config: Config) -> Result<()> {
    let matches =
        crate::shell::cli::build_command().try_get_matches_from(["kurama", "api", name])?;
    let crate::shell::cli::command::CliCommand::Api(command) =
        crate::shell::cli::parser::parse_cli_command(&matches)
    else {
        unreachable!("`kurama api <API>` parses as the api command");
    };
    let api = resolve_api(&config, &command.api)?;
    let api = apply_signing_options(api, &command.signing)?;
    let runtime = create_runtime(config, &command.connection, command.output.verbose)?;
    handle_explorer_command(api, runtime, command.output.verbose, Some(start)).await
}

async fn execute_target(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    command: &ApiCommand,
    target: &str,
    params: &[(String, String)],
    policy: Option<&AgentPolicy>,
) -> Result<()> {
    let (request_options, output_options, spec_options) =
        (&command.request, &command.output, &command.spec);
    let pages = command.pages.as_ref();
    output_options.check_output()?;
    if let Some(path) = &output_options.output {
        check_output_path(path)?;
    }
    let body = read_body(request_options.data.as_deref())?;
    // A dry run sends nothing with the API's credential, the description
    // fetch included.
    let access = if output_options.dry_run {
        SpecAccess::WithoutCredential
    } else {
        SpecAccess::WithCredential
    };
    let mut graphql = false;
    let (request, description) = match target_kind(api, target) {
        TargetKind::Operation => {
            let call = operation_request(
                runtime,
                api,
                target,
                OperationInput {
                    params,
                    body: body.clone(),
                    select: request_options.select.as_deref(),
                },
                SpecLoad {
                    refresh: spec_options.refresh,
                    access,
                    verbose: output_options.verbose,
                },
            )
            .await?;
            graphql = call.graphql;
            let request = match call.request {
                Some(request) => apply_options(request, api, request_options)?,
                None => build_request(api, request_options, target, body)?,
            };
            (request, Some(call.origin))
        }
        TargetKind::Plain if !params.is_empty() => {
            return Err(ApiError::ArgumentInvalid(format!(
                "-P applies to an operation of the API description; TARGET {target:?} is a plain request"
            ))
            .into());
        }
        TargetKind::Plain if request_options.select.is_some() => {
            return Err(select_needs_graphql(target).into());
        }
        TargetKind::Plain => {
            let description = if spec_options.refresh {
                Some(refresh_spec(runtime, api, access).await?.origin)
            } else {
                None
            };
            (
                build_request(api, request_options, target, body)?,
                description,
            )
        }
    };
    if pages.is_some() && request.method != "GET" {
        return Err(ApiError::ArgumentInvalid(format!(
            "--pages sends the request once per page, so it takes a GET only; this request is {}",
            request.method
        ))
        .into());
    }
    // Before any credential is read: a refused call reaches nothing. A dry
    // run sends nothing, so it shows what would be refused.
    if !output_options.dry_run {
        crate::shell::audit::note_request(&request.method, &request.url);
        check_api_request(policy, &request)?;
    }
    if output_options.plan_requested() {
        let body_file = request_options
            .data
            .as_deref()
            .and_then(|data| data.strip_prefix('@'))
            .filter(|path| *path != "-");
        return print_plan(
            runtime,
            api,
            output_options,
            PlanInput {
                target,
                description: description.as_ref(),
                refresh: spec_options.refresh,
                body_file,
                pages,
            },
            request,
        )
        .await;
    }

    execute_request(runtime, api, output_options, request, pages, graphql).await
}

/// What the target path knows about a plan besides the request.
struct PlanInput<'a> {
    target: &'a str,
    description: Option<&'a SpecOrigin>,
    refresh: bool,
    body_file: Option<&'a str>,
    pages: Option<&'a ApiPages>,
}

/// `--dry-run --json`: the plan document on stdout (or the `--jq` results
/// on it). The preview holds placeholder credentials only; `-v` adds the
/// human dry run on stderr.
async fn print_plan(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    options: &ApiOutputOptions,
    input: PlanInput<'_>,
    request: HttpRequest,
) -> Result<()> {
    let credential = runtime.credential(api, &request).await?;
    let reference_profile = match reference_aws_profile(&credential) {
        Some(name) => Some(runtime.load_aws_profile(name).await?),
        None => None,
    };
    let shown = ApiRuntime::preview(&credential, request)?;
    if options.verbose {
        progress!("# dry run: nothing is sent");
        progress!(
            "{}",
            render_dry_run(&shown, &credential.extra_secret_headers())
        );
    }
    let document = DryRunPlan {
        api: &api.name,
        target: input.target,
        request: &shown,
        credential: &credential,
        reference_profile: reference_profile.as_ref(),
        description: input.description,
        refresh: input.refresh,
        body_file: input.body_file,
        output: options.output.as_deref(),
        pages: input.pages,
    }
    .to_json();
    let lines = match options.jq.as_deref() {
        Some(filter) => apply_filter(filter, &document, None).map_err(ApiError::Jq)?,
        None => vec![document.to_string()],
    };
    let mut stdout = std::io::stdout().lock();
    for line in lines {
        writeln!(stdout, "{line}")?;
    }
    Ok(())
}

/// The AWS profile an `aws-*://` reference of the credential is read with.
fn reference_aws_profile(credential: &ApiCredential) -> Option<&str> {
    let secret = match credential {
        ApiCredential::OAuth(client) => client.client_secret.as_ref()?,
        ApiCredential::Token(source) => &source.token,
        ApiCredential::None | ApiCredential::SigV4 { .. } => return None,
    };
    match secret {
        SecretRef::Aws(reference) => Some(&reference.aws_profile),
        SecretRef::OnePassword(_) | SecretRef::Literal(_) => None,
    }
}

async fn execute_request(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    options: &ApiOutputOptions,
    request: HttpRequest,
    pages: Option<&ApiPages>,
    graphql: bool,
) -> Result<()> {
    if options.dry_run {
        let credential = runtime.credential(api, &request).await?;
        let shown = ApiRuntime::preview(&credential, request)?;
        // The header a `kind = "token"` source names is masked too: the
        // fixed list of credential headers cannot know what it is called.
        progress!("# dry run: nothing is sent");
        progress!(
            "{}",
            render_dry_run(&shown, &credential.extra_secret_headers())
        );
        if let Some(path) = &options.output {
            progress!("# a 2xx body would be written to {}", path.display());
        }
        if let Some(pages) = pages {
            progress!("{}", dry_run_note(pages));
        }
        return Ok(());
    }
    if let Some(pages) = pages {
        return fetch_pages(runtime, api, options, pages, request).await;
    }
    let response = send_request(runtime, api, options.verbose, request).await?;
    // A GraphQL server answers a failed operation with 200 and `errors`.
    let failure = if !response.is_success() {
        Some(http_error(&response))
    } else if graphql {
        graphql_error_line(&response.body).map(ApiError::GraphQl)
    } else {
        None
    };
    let failure = match failure {
        Some(failure) if !options.json => return Err(failure.into()),
        failure => failure,
    };
    match &options.output {
        Some(path) => save_body(path, &response.body)?,
        None => write_output(
            &fitted(&response, options),
            options,
            crate::shell::tui::terminal::stdout_is_interactive(),
        )?,
    }
    match failure {
        Some(failure) => Err(failure.into()),
        None => Ok(()),
    }
}

/// One request through `ApiRuntime::call`; with `-v`, the request (its
/// credentials masked) and the response headers on stderr.
pub(super) async fn send_request(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    verbose: bool,
    request: HttpRequest,
) -> Result<HttpResponse> {
    if verbose {
        let credential = runtime.credential(api, &request).await?;
        let shown = ApiRuntime::preview(&credential, request.clone())?;
        progress!("> {} {}", shown.method, shown.url);
        for (name, value) in
            mask_secret_headers_with(&shown.headers, &credential.extra_secret_headers())
        {
            progress!("> {name}: {value}");
        }
    }
    let response = runtime.call(api, request).await?;
    crate::shell::audit::note_status(response.status);
    if verbose {
        progress!("< HTTP {}", response.status);
        for (name, value) in mask_secret_headers(&response.headers) {
            progress!("< {name}: {value}");
        }
    }
    Ok(response)
}

/// The request from the profile, the TARGET and the options; `-X` wins over
/// a method in the TARGET, and a body makes the default method POST.
fn build_request(
    api: &ApiProfile,
    options: &ApiRequestOptions,
    target: &str,
    body: Option<Vec<u8>>,
) -> Result<HttpRequest, ApiError> {
    let target = parse_target(target).map_err(ApiError::ArgumentInvalid)?;
    let url = resolve_url(&api.base_url, &target.location).map_err(ApiError::ArgumentInvalid)?;
    let method = target
        .method
        .unwrap_or_else(|| if body.is_some() { "POST" } else { "GET" }.to_string());
    let mut request = HttpRequest::new(method, url);
    if let Some(body) = body {
        request = request.with_body(body);
    }
    apply_options(request, api, options)
}

/// `-X`, `-H`, the API's `headers` and the defaults on a request: the
/// method option wins, `headers` replace the ones of the same name, `-H`
/// replaces both, `Accept` defaults to JSON and so does `Content-Type` when
/// there is a body.
fn apply_options(
    mut request: HttpRequest,
    api: &ApiProfile,
    options: &ApiRequestOptions,
) -> Result<HttpRequest, ApiError> {
    if let Some(method) = options.method.as_deref() {
        request.method = method.to_ascii_uppercase();
    } else {
        request.method = request.method.to_ascii_uppercase();
    }
    let headers = options
        .headers
        .iter()
        .map(|header| {
            let (name, value) = header.split_once(':').ok_or_else(|| {
                ApiError::ArgumentInvalid(format!("header {header:?} is not `Name: value`"))
            })?;
            Ok((name.trim(), value.trim()))
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let request = with_headers(with_headers(request, api.headers.iter()), headers);
    Ok(with_default_headers(request))
}

/// `--service` / `--region` over the profile's `service` / `region`. They
/// belong to an `aws_profile` API; on any other they are a mistake.
fn apply_signing_options(
    mut api: ApiProfile,
    options: &SigningHint,
) -> Result<ApiProfile, ApiError> {
    if *options == SigningHint::default() {
        return Ok(api);
    }
    if api.aws_profile.is_none() {
        return Err(ApiError::ArgumentInvalid(format!(
            "--service and --region apply to an API with aws_profile; [api.{}] has none",
            api.name
        )));
    }
    api.signing = options.clone().or(api.signing);
    Ok(api)
}

/// `-d`: `@-` reads stdin, `@path` reads a file, anything else is literal.
fn read_body(data: Option<&str>) -> Result<Option<Vec<u8>>, ApiError> {
    let Some(data) = data else {
        return Ok(None);
    };
    match data.strip_prefix('@') {
        Some("-") => {
            let mut body = Vec::new();
            std::io::stdin()
                .read_to_end(&mut body)
                .map_err(|error| ApiError::ArgumentInvalid(format!("-d @-: {error}")))?;
            Ok(Some(body))
        }
        Some(path) => std::fs::read(path)
            .map(Some)
            .map_err(|error| ApiError::ArgumentInvalid(format!("-d @{path}: {error}"))),
        None => Ok(Some(data.as_bytes().to_vec())),
    }
}

pub(super) fn http_error(response: &HttpResponse) -> ApiError {
    ApiError::HttpStatus {
        status: response.status,
        reason: reason_phrase(response.status).to_string(),
        excerpt: body_excerpt(&response.body, ERROR_EXCERPT_CHARS),
    }
}

/// A PATH `--output` must not replace, refused before the request: a
/// read-only file (someone protected it, and a rename would not ask) or a
/// directory.
fn check_output_path(path: &Path) -> Result<(), ApiError> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            Err(output_failed(path, std::io::ErrorKind::IsADirectory.into()))
        }
        Ok(metadata) if metadata.permissions().readonly() => Err(output_failed(
            path,
            std::io::ErrorKind::PermissionDenied.into(),
        )),
        _ => Ok(()),
    }
}

/// `--output PATH`: the body's bytes in a temporary file beside PATH,
/// renamed over it once all of them are written, so PATH is either what it
/// was or the whole body. PATH is replaced, not written through: a symlink
/// or a hard link there is replaced by the new file, and its mode is not
/// carried over -- the file has the temporary file's mode 600. No fsync:
/// the rename is atomic against a failed request or write, not against a
/// power loss, and a process killed mid-write may leave the `.tmp` file.
fn save_body(path: &Path, body: &[u8]) -> Result<(), ApiError> {
    let failed = |source| output_failed(path, source);
    let mut file = tempfile::NamedTempFile::new_in(output_directory(path)).map_err(failed)?;
    file.write_all(body).map_err(failed)?;
    file.persist(path).map_err(|error| failed(error.error))?;
    Ok(())
}

/// The directory the temporary file goes in: PATH's own, `.` for a bare
/// name.
fn output_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// The failure with the path a person named. tempfile wraps an OS error in
/// one that ends with the temporary file's random path; the kind is kept
/// and that path dropped.
fn output_failed(path: &Path, source: std::io::Error) -> ApiError {
    let source = match source.raw_os_error() {
        Some(_) => source,
        None => std::io::Error::from(source.kind()),
    };
    ApiError::OutputFailed {
        path: path.display().to_string(),
        source,
    }
}

/// The body, the `--jq` results, or the `--json` envelope on stdout.
///
/// A person reading a JSON body on a terminal gets it laid out, and a
/// terminal gets a final newline; a pipe or a file gets the server's bytes
/// exactly, and `--json` and `--jq` one line per document.
fn write_output(
    response: &HttpResponse,
    options: &ApiOutputOptions,
    stdout_is_terminal: bool,
) -> Result<()> {
    let Some(lines) = result_lines(response, options)? else {
        let laid_out = stdout_is_terminal
            .then(|| indent_json(&response.body))
            .flatten();
        let mut stdout = std::io::stdout().lock();
        match &laid_out {
            Some(body) => {
                stdout.write_all(body.as_bytes())?;
                stdout.write_all(b"\n")?;
            }
            None => {
                stdout.write_all(&response.body)?;
                if stdout_is_terminal
                    && !response.body.is_empty()
                    && !response.body.ends_with(b"\n")
                {
                    stdout.write_all(b"\n")?;
                }
            }
        }
        return Ok(());
    };
    let mut stdout = std::io::stdout().lock();
    for line in lines {
        writeln!(stdout, "{line}")?;
    }
    Ok(())
}

/// The response as printed: with `--shape` / `--sample`, its body cut to
/// fit; the headers, the status and the bytes `--output` writes are the
/// server's.
pub(super) fn fitted(response: &HttpResponse, options: &ApiOutputOptions) -> HttpResponse {
    let mut shown = response.clone();
    if let Some(fit) = options.fit {
        shown.body = fit_body(&response.body, fit);
    }
    shown
}

/// The `--json` envelope or the `--jq` results, one line each; `None`
/// without either, when the body itself is the output.
pub(super) fn result_lines(
    response: &HttpResponse,
    options: &ApiOutputOptions,
) -> Result<Option<Vec<String>>, ApiError> {
    Ok(Some(match (options.jq.as_deref(), options.json) {
        (Some(filter), true) => {
            apply_filter(filter, &response_envelope(response), None).map_err(ApiError::Jq)?
        }
        (None, true) => vec![response_envelope(response).to_string()],
        (Some(filter), false) => {
            let body = body_as_json(&response.body).ok_or_else(|| {
                ApiError::Jq("the response body is not JSON; drop --jq or add --json".into())
            })?;
            apply_filter(filter, &body, None).map_err(ApiError::Jq)?
        }
        (None, false) => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::ApiHeaders;

    fn command(target: Option<&str>) -> ApiCommand {
        ApiCommand {
            api: "svc".into(),
            target: target.map(str::to_string),
            request: ApiRequestOptions {
                method: None,
                headers: vec![],
                data: None,
                params: vec![],
                confirm: false,
                select: None,
            },
            output: ApiOutputOptions {
                jq: None,
                json: false,
                dry_run: false,
                verbose: false,
                output: None,
                fit: None,
            },
            connection: ApiConnectionOptions {
                insecure: false,
                timeout_secs: 60,
            },
            signing: SigningHint::default(),
            spec: ApiSpecOptions {
                ops: None,
                describe: None,
                schema: None,
                skill: false,
                refresh: false,
            },
            pages: None,
        }
    }

    fn api() -> ApiProfile {
        ApiProfile {
            name: "svc".into(),
            description: None,
            base_url: "https://api.example.com/v1/".into(),
            auth: Some("svc".into()),
            aws_profile: None,
            signing: SigningHint::default(),
            spec: None,
            openapi_auth: false,
            headers: ApiHeaders::default(),
        }
    }

    fn aws_api(service: Option<&str>, region: Option<&str>) -> ApiProfile {
        ApiProfile {
            name: "apigw".into(),
            description: None,
            base_url: "https://api.example.com".into(),
            auth: None,
            aws_profile: Some("dev".into()),
            signing: SigningHint {
                service: service.map(str::to_string),
                region: region.map(str::to_string),
            },
            spec: None,
            openapi_auth: false,
            headers: ApiHeaders::default(),
        }
    }

    #[test]
    fn service_and_region_options_win_over_the_profile_and_need_aws_profile() {
        let mut with_region = command(None);
        with_region.signing.region = Some("us-east-1".into());
        let overridden = apply_signing_options(
            aws_api(Some("execute-api"), Some("ap-northeast-1")),
            &with_region.signing,
        )
        .unwrap();
        assert_eq!(
            overridden.signing,
            SigningHint {
                service: Some("execute-api".into()),
                region: Some("us-east-1".into()),
            }
        );
        assert_eq!(
            apply_signing_options(aws_api(None, None), &command(None).signing)
                .unwrap()
                .signing,
            SigningHint::default()
        );
        assert_eq!(
            apply_signing_options(api(), &command(None).signing).unwrap(),
            api()
        );
        assert!(matches!(
            apply_signing_options(api(), &with_region.signing).unwrap_err(),
            ApiError::ArgumentInvalid(message) if message.contains("[api.svc] has none")
        ));
    }

    #[test]
    fn a_path_becomes_a_get_under_the_base_url_with_json_accept() {
        let command = command(None);
        let request = build_request(&api(), &command.request, "/items", None).unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://api.example.com/v1/items");
        assert_eq!(request.header("accept"), Some("application/json"));
        assert!(request.body.is_none());
    }

    #[test]
    fn a_body_defaults_to_post_and_json_and_explicit_options_win() {
        let mut with_body = command(None);
        with_body.request.headers = vec!["X-Trace: 1".into(), "accept: text/plain".into()];
        let request =
            build_request(&api(), &with_body.request, "/items", Some(b"{}".to_vec())).unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(request.header("accept"), Some("text/plain"));
        assert_eq!(request.header("x-trace"), Some("1"));

        let mut explicit = command(None);
        explicit.request.method = Some("patch".into());
        let request = build_request(&api(), &explicit.request, "DELETE /items/1", None).unwrap();
        assert_eq!(request.method, "PATCH");
        let command = command(None);
        let request = build_request(&api(), &command.request, "DELETE /items/1", None).unwrap();
        assert_eq!(request.method, "DELETE");
    }

    /// The API's `headers` go on every request, over the defaults and over
    /// what an operation sets; `-H` replaces one of them by name.
    #[test]
    fn configured_headers_are_sent_and_an_option_replaces_them_by_name() {
        let mut configured = api();
        configured.headers = ApiHeaders::new(
            [
                (
                    "Accept".to_string(),
                    "application/vnd.github+json".to_string(),
                ),
                ("X-Api-Version".to_string(), "1".to_string()),
            ]
            .into(),
        )
        .unwrap();
        let request = build_request(&configured, &command(None).request, "/items", None).unwrap();
        assert_eq!(
            request.header("accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(request.header("x-api-version"), Some("1"));
        let mut with_header = command(None);
        with_header.request.headers = vec!["x-api-version: 2".into()];
        let request = apply_options(
            HttpRequest::new("get", "https://x/items").with_header("Accept", "text/csv"),
            &configured,
            &with_header.request,
        )
        .unwrap();
        assert_eq!(
            request.header("accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(request.header("x-api-version"), Some("2"));
        assert_eq!(request.headers.len(), 2);
    }

    #[test]
    fn options_on_an_operation_request_replace_its_headers_by_name() {
        let mut with_headers = command(None);
        with_headers.request.headers = vec!["X-Trace: 2".into()];
        let request = apply_options(
            HttpRequest::new("post", "https://x/items")
                .with_header("X-Trace", "1")
                .with_header("Content-Type", "application/vnd.item+json")
                .with_body(b"{}".to_vec()),
            &api(),
            &with_headers.request,
        )
        .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(
            request
                .headers
                .iter()
                .filter(|(n, _)| n == "X-Trace")
                .count(),
            1
        );
        assert_eq!(request.header("x-trace"), Some("2"));
        assert_eq!(
            request.header("content-type"),
            Some("application/vnd.item+json"),
            "the operation's media type stays"
        );
        assert_eq!(request.header("accept"), Some("application/json"));
    }

    #[test]
    fn bad_targets_and_headers_are_argument_errors() {
        assert!(matches!(
            build_request(
                &api(),
                &command(None).request,
                "items",
                None
            )
            .unwrap_err(),
            ApiError::ArgumentInvalid(message) if message.contains("TARGET")
        ));
        let mut bad_header = command(None);
        bad_header.request.headers = vec!["NoColon".into()];
        assert!(matches!(
            build_request(&api(), &bad_header.request, "/x", None).unwrap_err(),
            ApiError::ArgumentInvalid(message) if message.contains("NoColon")
        ));
    }

    #[test]
    fn body_sources() {
        assert_eq!(read_body(None).unwrap(), None);
        assert_eq!(read_body(Some("{}")).unwrap(), Some(b"{}".to_vec()));
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"from-file").unwrap();
        assert_eq!(
            read_body(Some(&format!("@{}", file.path().display()))).unwrap(),
            Some(b"from-file".to_vec())
        );
        assert!(matches!(
            read_body(Some("@/nonexistent/body.json")).unwrap_err(),
            ApiError::ArgumentInvalid(_)
        ));
    }

    #[test]
    fn a_saved_body_replaces_the_file_whole_and_a_failure_leaves_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("body.bin");
        std::fs::write(&path, b"the previous, longer content").unwrap();
        save_body(&path, b"\x00\xffnew").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"\x00\xffnew");

        let missing = directory.path().join("missing/body.bin");
        let error = save_body(&missing, b"x").unwrap_err();

        assert!(matches!(
            &error,
            ApiError::OutputFailed { path, source }
                if path == &missing.display().to_string()
                    && source.kind() == std::io::ErrorKind::NotFound
        ));
        let message =
            error.to_string() + ": " + &std::error::Error::source(&error).unwrap().to_string();
        assert!(
            !message.contains("missing/.tmp"),
            "the temporary file's path stays out of the message: {message}"
        );
        let left: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["body.bin"]);
    }

    #[test]
    fn a_bare_name_is_written_in_the_current_directory() {
        assert_eq!(output_directory(Path::new("body.json")), Path::new("."));
        assert_eq!(
            output_directory(Path::new("out/body.json")),
            Path::new("out")
        );
        assert_eq!(output_directory(Path::new("/body.json")), Path::new("/"));
    }

    #[test]
    fn a_read_only_file_or_a_directory_at_the_path_is_refused_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("kept.json");
        std::fs::write(&file, b"kept").unwrap();
        check_output_path(&file).unwrap();
        check_output_path(&directory.path().join("new.json")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(matches!(
            check_output_path(&file).unwrap_err(),
            ApiError::OutputFailed { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied
        ));
        assert!(matches!(
            check_output_path(directory.path()).unwrap_err(),
            ApiError::OutputFailed { source, .. } if source.kind() == std::io::ErrorKind::IsADirectory
        ));
        assert_eq!(std::fs::read(&file).unwrap(), b"kept");
    }

    #[test]
    fn output_path_is_refused_beside_json_or_jq_except_in_a_dry_run() {
        let options = |json: bool, jq: Option<&str>, dry_run: bool, output: Option<&str>| {
            ApiOutputOptions {
                jq: jq.map(str::to_string),
                json,
                dry_run,
                verbose: false,
                output: output.map(std::path::PathBuf::from),
                fit: None,
            }
            .check_output()
            .is_ok()
        };
        assert!(options(false, None, false, Some("/tmp/x")));
        assert!(!options(true, None, false, Some("/tmp/x")));
        assert!(!options(false, Some("."), false, Some("/tmp/x")));
        assert!(options(true, Some("."), true, Some("/tmp/x")));
        assert!(options(true, Some("."), false, None));
    }

    #[test]
    fn http_errors_carry_the_status_reason_and_an_excerpt() {
        let error = http_error(&HttpResponse {
            status: 404,
            headers: vec![],
            body: b"{\n \"message\": \"Not Found\"\n}".to_vec(),
        });
        assert_eq!(
            error.to_string(),
            "HTTP 404 Not Found: { \"message\": \"Not Found\" }"
        );
        assert_eq!(
            http_error(&HttpResponse {
                status: 599,
                headers: vec![],
                body: vec![],
            })
            .to_string(),
            "HTTP 599"
        );
    }
}
