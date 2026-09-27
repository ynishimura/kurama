//! A verification case declared in TOML (`tests/cases/<feature>/<id>.toml`):
//! what the user gives, how the fakes answer, and what is expected of the
//! observed run. `cargo xtask generate-cases` writes one `#[test]` per file that calls
//! [`run_case`], which turns the file into a [`Scenario`], runs the real
//! binary through the same harness as a Rust scenario, and maps every key of
//! `[expect]` onto the `expect_*` method of the same meaning -- so a case
//! never checks anything a Rust scenario could not, and the common checks
//! (no credential on disk, one error line, no unexpected call) apply as they
//! are. A check a case fails is named by its key: `expect.exit_code: exit
//! code is 4: observed 2`.
//!
//! A case references nothing inside kurama: only inputs a person could give
//! and results a person could see. What the fakes were given is named by a
//! placeholder rather than copied: `{granted_access_token}`,
//! `{refreshed_access_token}`, `{stored_access_token}`, `{fake_client_secret}`,
//! the STS keys and codes the fakes hold (`{source_access_key}`,
//! `{result_access_key}`, `{result_secret_key}`, `{result_session_token}`,
//! `{session_access_key}`,
//! `{second_session_access_key}`, `{op_access_key}`, `{op_totp}`,
//! `{op_json_totp}`, `{signin_token}`) and `{sha256:<text>}` are replaced in
//! the whole file before it is read; `{home}` in an argument, stdin or an `env`
//! value is the
//! sandbox HOME and `{server}` in an argument the fake server, which the
//! harness fills in once they exist.
//! `[expect]` speaks of the last run; `[[expect.runs]]` with `run = N` says
//! the same things about run N; a case whose `[expect]`, a `[[runs]]` block or
//! a layer's `expect` checks nothing is refused when it is read, because the
//! fake layer always runs the case and a layer's block replaces `[expect]`;
//! `files_unchanged`, `paths_absent` and `token_store` are about the whole
//! scenario and are read from `[expect]` (or `[local.expect]`) only. An `api_calls` or `spec_calls` that names no
//! `authorization` and no `sigv4` says the requests carry none: silence is a
//! decision, not a request left unchecked; a request checked one by one under
//! `api_calls.calls` says so for itself.
//!
//! `[local]` is the same case against the databases `cargo xtask db-up`
//! starts: its own `config` (with `{pg17_port}`, `{pg18_port}`,
//! `{mysql_port}` and `{db_tls_dir}` standing for where they listen and the
//! CA `up.sh` wrote), optionally its own `args` / `then_run` / `env`, the
//! `combination` coordinates it adds, and `[local.expect]` with every key of
//! `[expect]`. `cargo xtask verify --layer local` runs the generated test
//! with `KURAMA_CASE_LAYER=local`, which is what picks the layer; its report
//! says `"evidence": "local"` and lands under `scenarios-local/`. `[real]`
//! has the same shape plus `requires` (what the person's environment has to
//! hold, by kind), and `cargo xtask verify --layer real` runs it the same
//! way with `KURAMA_CASE_LAYER=real`: the scenario then runs without the
//! fakes (`Scenario::real`), on the configuration `KURAMA_REAL_BASE_CONFIG`
//! names plus the layer's own sections.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::expect_detail::ApiRequestMatch;
use super::{
    ApiCallExpect, ApiFake, AwsSecrets, FAKE_CLIENT_SECRET, GRANTED_ACCESS_TOKEN, JsonExpect,
    OAuthFake, OP_ACCESS_KEY, OP_JSON_TOTP, OP_TOTP, OnePassword, REFRESHED_ACCESS_TOKEN,
    RESULT_ACCESS_KEY, RESULT_SECRET, RESULT_TOKEN, SECOND_SESSION_ACCESS_KEY, SESSION_ACCESS_KEY,
    SIGNIN_TOKEN, SOURCE_ACCESS_KEY, STORED_ACCESS_TOKEN, Scenario, SpecFake, StsCallExpect,
    StsFake, Verification, run, sha256_fingerprint, stored_token_json,
};

/// One case file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    feature: String,
    /// The coordinates of the combination this case verifies, every value
    /// an enumerated one; the matrix groups cases by these.
    #[serde(default)]
    combination: BTreeMap<String, String>,
    input: Input,
    #[serde(default)]
    fakes: Fakes,
    #[serde(default)]
    expect: Expect,
    /// The same case against the databases `cargo xtask db-up` starts.
    #[serde(default)]
    local: Option<Layer>,
    /// The same case against the real service; another layer runs it.
    #[serde(default)]
    real: Option<Layer>,
    /// The same case against AWS resources `cargo xtask verify --layer
    /// throwaway` creates for the run and deletes after it; `stacks` names
    /// them, and the layer's `config` reads their addresses through
    /// placeholders (`{iam_api_url}`, `{rds_pg_host}`, ...) and the profile
    /// the run was given through `{throwaway_profile}`.
    #[serde(default)]
    throwaway: Option<Layer>,
}

/// The same case on another layer: what changes about the input, the
/// coordinates the layer adds, and what is expected there. The fakes stay as
/// declared -- a local database still takes its password from the fake `op`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct Layer {
    /// Replaces `input.args`.
    args: Option<Vec<String>>,
    /// Replaces `input.then_run`.
    then_run: Option<Vec<Vec<String>>>,
    /// Replaces `input.config`.
    config: Option<String>,
    /// Replaces `input.replace_config`.
    replace_config: Option<String>,
    /// Added to `input.env`, winning on a shared name.
    env: BTreeMap<String, String>,
    /// Added to the case's `combination`, winning on a shared key.
    combination: BTreeMap<String, String>,
    /// What the layer needs from the person's environment, `kind:name`:
    /// `profile:kurama-sandbox` (in `~/.aws/config`), `keychain:OP_SERVICE_ACCOUNT_TOKEN`
    /// (an entry the runner can read), `auth:kurama-real-oauth` / `api:github`
    /// (a section of the base configuration), `env:NAME`. `cargo xtask verify
    /// --layer real` checks each before running the case and leaves the
    /// reason behind when one is missing, so the matrix says what to prepare.
    requires: Vec<String>,
    expect: Expect,
    /// `[throwaway]` only: the disposable stacks the case needs, by the
    /// names `cargo xtask verify --layer throwaway` knows.
    stacks: Vec<String>,
}

/// The kinds a requirement may name.
const REQUIREMENT_KINDS: [&str; 5] = ["profile", "keychain", "auth", "api", "env"];

/// What the user gives.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    args: Vec<String>,
    /// Further runs in the same sandbox, in order.
    #[serde(default)]
    then_run: Vec<Vec<String>>,
    /// TOML appended to the generated config; `{server}` is the fake server.
    #[serde(default)]
    config: Option<String>,
    /// Replaces the generated config instead of appending to it.
    #[serde(default)]
    replace_config: Option<String>,
    /// Replaces the sandbox `~/.aws/config`.
    #[serde(default)]
    aws_config: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Files under the sandbox HOME before the first run.
    #[serde(default)]
    home_files: BTreeMap<String, String>,
    #[serde(default)]
    stdin: Option<String>,
    /// Tokens in the store before the first run (implies the token store).
    #[serde(default)]
    stored_tokens: Vec<StoredToken>,
    /// The 1Password service account token in the keychain entry the config
    /// names; `"<absent>"` names the entry without seeding it.
    #[serde(default)]
    service_account_keychain: Option<String>,
    /// Files copied before the first run: a fixture a write may touch, so the
    /// shared one is never opened for writing.
    #[serde(default)]
    copy_files: Vec<CopyFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyFile {
    from: String,
    to: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredToken {
    source: String,
    access_token: String,
    /// Seconds until it expires; negative is expired.
    expires_in: i64,
    #[serde(default)]
    refreshable: bool,
}

/// How the fakes answer; every field has the harness's default.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct Fakes {
    sts: Sts,
    op: Op,
    oauth: OAuth,
    api: Api,
    aws_secrets: Secrets,
    spec: Spec,
    session_cache: bool,
    corrupt_session_cache: bool,
    token_store: bool,
    corrupt_token_store: bool,
    /// The keychain refuses this build with the macOS status code given:
    /// both file-backed stores answer every read with it.
    keychain_denied: Option<i32>,
    browser: bool,
    tools: bool,
    federation_fails: bool,
    federation_invalid_json: bool,
    env_script: bool,
    /// The hand-off file already exists with these permission bits.
    existing_env_script_mode: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(untagged, deny_unknown_fields)]
enum Sts {
    Plain(StsPlain),
    #[default]
    Success,
    Error {
        error: StsError,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StsPlain {
    Success,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StsError {
    code: String,
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum Op {
    #[default]
    Disabled,
    Enabled,
    NotSignedIn,
    Hangs,
    VersionHangs,
    OtpFails,
}

#[derive(Deserialize, Default)]
#[serde(untagged, deny_unknown_fields)]
enum OAuth {
    Plain(OAuthPlain),
    #[default]
    Success,
    Rejected {
        rejected: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OAuthPlain {
    Success,
    Unavailable,
}

#[derive(Deserialize, Default)]
#[serde(untagged, deny_unknown_fields)]
enum Api {
    Plain(ApiPlain),
    #[default]
    Ok,
    Padding {
        padding: usize,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ApiPlain {
    Ok,
    UnauthorizedOnce,
    Unauthorized,
    NotFound,
    Slow,
    CompactArray,
    Echo,
    Pages,
    #[serde(rename = "graphql")]
    GraphQl,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum Secrets {
    #[default]
    Absent,
    FromId,
    AccessDenied,
    NotFound,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum Spec {
    #[default]
    Etag,
    Down,
}

/// What is expected of one run: the last one, or `run` in a `[[runs]]`
/// block. Every key is one `expect_*` of [`Verification`]; `secret_reads`,
/// `files_written`, `no_files_written` and `env_script_exports` are about the
/// whole scenario, whichever block names them.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct Expect {
    /// The run this block is about; only in a `[[runs]]` block.
    run: Option<usize>,
    runs: Vec<Expect>,
    exit_code: Option<i32>,
    error: Option<ErrorExpect>,
    /// A `data --json` / `db --json` error document with this code.
    json_error: Option<ErrorExpect>,
    /// The `hint:` line, without the `hint: ` prefix.
    hint: Option<String>,
    stdout: Option<Stdout>,
    stdout_excludes: Vec<String>,
    stdout_lines: Option<usize>,
    stderr_contains: Vec<String>,
    stderr_excludes: Vec<String>,
    /// Exactly this many stderr lines: one error document, one error line.
    stderr_lines: Option<usize>,
    /// Exactly this many stderr lines contain each key: one line per event.
    stderr_lines_containing: BTreeMap<String, usize>,
    /// stderr is empty: no progress, log or diagnostic.
    stderr_empty: bool,
    /// Every non-empty stderr line starts with one of these.
    stderr_lines_start_with: Option<Vec<String>>,
    /// The JSON document on stdout, by JSON pointer.
    stdout_json: BTreeMap<String, JsonValue>,
    /// The JSON document on stderr (an error document), by JSON pointer.
    stderr_json: BTreeMap<String, JsonValue>,
    /// STS actions of the run, in order; `[]` is none.
    sts_actions: Option<Vec<String>>,
    /// The STS calls of the run, exactly; `[]` is none. Every field a call
    /// names is compared, `"<none>"` says the parameter was not sent.
    sts_calls: Option<Vec<StsCallFields>>,
    /// Every STS call of the run is signed for this region.
    sts_signing_region: Option<String>,
    /// OAuth grant types of the run, in order; `[]` is none.
    token_grants: Option<Vec<String>>,
    /// Paths requested of the authorization server, in order; `[]` is none.
    oauth_paths: Option<Vec<String>>,
    /// Parameters request `call` to the authorization server carried.
    oauth_params: Vec<OAuthParams>,
    op_calls: Option<usize>,
    /// How many `op` calls of the run contain each text.
    op_calls_containing: BTreeMap<String, usize>,
    /// The console federation requests of the run, exactly; `[]` is none.
    federation_calls: Option<Vec<FederationCallFields>>,
    /// The `op read` calls, in order; `[]` is none.
    op_reads: Option<Vec<String>>,
    /// URLs handed to the browser, in order; `[]` is none.
    open_calls: Option<Vec<String>>,
    /// Exactly this many browser calls, call N containing all of list N.
    open_calls_containing: Option<Vec<Vec<String>>>,
    /// Systems Manager operations, in order; `[]` is none.
    ssm_calls: Option<Vec<String>>,
    api_calls: Option<ApiCalls>,
    /// Every API request of the run is validly signed for this service and
    /// region, however many there are: for a run whose request count varies.
    api_signed: Option<Sigv4>,
    /// How many requests of the run match each shape, in any order.
    api_requests_matching: Vec<ApiRequestMatch>,
    spec_calls: Option<SpecCalls>,
    /// Every secret read from AWS across all runs, in order; `[]` is none.
    secret_reads: Option<Vec<SecretRead>>,
    no_files_written: bool,
    /// The files written under the sandbox across all runs.
    files_written: Option<FilesWritten>,
    env_script_exports: Option<Vec<String>>,
    /// Files outside the sandbox that are byte for byte what they were
    /// before the first run; whole-scenario, `[expect]` only.
    files_unchanged: Vec<String>,
    /// Paths that do not exist after the runs; whole-scenario, `[expect]`
    /// only.
    paths_absent: Vec<String>,
    /// The sources the file token store holds after the runs, in name
    /// order; `[]` is none. Whole-scenario, `[expect]` only.
    token_store: Option<Vec<String>>,
}

/// The parameters of one STS call a case pins; an absent field is not
/// compared.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct StsCallFields {
    action: Option<String>,
    role_arn: Option<String>,
    serial_number: Option<String>,
    token_code: Option<String>,
    signing_access_key: Option<String>,
    policy_arns: Option<Vec<String>>,
}

impl StsCallFields {
    fn expectation(&self) -> StsCallExpect {
        StsCallExpect {
            action: self.action.clone(),
            role_arn: self.role_arn.clone(),
            serial_number: self.serial_number.clone(),
            token_code: self.token_code.clone(),
            signing_access_key: self.signing_access_key.clone(),
            policy_arns: self.policy_arns.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationCallFields {
    action: String,
    session_access_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthParams {
    call: usize,
    params: BTreeMap<String, String>,
}

/// A JSON value a case expects: a plain value (`"<null>"` inside it is
/// null, which TOML cannot write), `"<string>"` for any string, `"<object>"`
/// for any object, `"<null>"` for null or nothing, `"<api_call_count>"` and
/// `"<api_bytes_requested>"` for what the fake API received, or one of
/// `{ contains = "..." | ["...", ...] }`, `{ has = value }` (an array holding
/// it), `{ length = N }`, `{ min_length = N }`, `{ at_least = N }`.
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum JsonValue {
    Contains { contains: OneOrMany },
    Has { has: toml::Value },
    Length { length: usize },
    MinLength { min_length: usize },
    AtLeast { at_least: f64 },
    Value(toml::Value),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl JsonValue {
    /// `"<api_call_count>"` and `"<api_bytes_requested>"`: what the fake API
    /// received.
    fn reads_the_fakes(&self) -> bool {
        matches!(
            self.expectation(),
            JsonExpect::ApiCallCount | JsonExpect::ApiBytesRequested
        )
    }

    fn expectation(&self) -> JsonExpect {
        match self {
            JsonValue::Contains {
                contains: OneOrMany::One(text),
            } => JsonExpect::Contains(vec![text.clone()]),
            JsonValue::Contains {
                contains: OneOrMany::Many(texts),
            } => JsonExpect::Contains(texts.clone()),
            JsonValue::Has { has } => JsonExpect::Has(json_value(has)),
            JsonValue::Length { length } => JsonExpect::Length(*length),
            JsonValue::MinLength { min_length } => JsonExpect::MinLength(*min_length),
            JsonValue::AtLeast { at_least } => JsonExpect::AtLeast(*at_least),
            JsonValue::Value(toml::Value::String(s)) if s == "<string>" => JsonExpect::AnyString,
            JsonValue::Value(toml::Value::String(s)) if s == "<object>" => JsonExpect::AnyObject,
            JsonValue::Value(toml::Value::String(s)) if s == "<null>" => JsonExpect::Null,
            JsonValue::Value(toml::Value::String(s)) if s == "<api_call_count>" => {
                JsonExpect::ApiCallCount
            }
            JsonValue::Value(toml::Value::String(s)) if s == "<api_bytes_requested>" => {
                JsonExpect::ApiBytesRequested
            }
            JsonValue::Value(value) => JsonExpect::Equals(json_value(value)),
        }
    }
}

/// The TOML value as JSON, with every `"<null>"` string inside it null.
fn json_value(value: &toml::Value) -> serde_json::Value {
    let mut json = serde_json::to_value(value).expect("a TOML value is a JSON value");
    fn nulls(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) if s == "<null>" => *value = serde_json::Value::Null,
            serde_json::Value::Array(items) => items.iter_mut().for_each(nulls),
            serde_json::Value::Object(fields) => fields.values_mut().for_each(nulls),
            _ => {}
        }
    }
    nulls(&mut json);
    json
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorExpect {
    code: String,
    exit: i32,
    /// The whole `error[CODE]: message` line, exactly (text mode only).
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Stdout {
    Kind(StdoutKind),
    Detail(StdoutDetail),
}

/// Several things about stdout at once, each its own check.
#[derive(Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields, default)]
struct StdoutDetail {
    /// One of the plain kinds, next to the details.
    kind: Option<StdoutKind>,
    equals: Option<String>,
    starts_with: Option<String>,
    /// Exactly this many lines.
    line_count: Option<usize>,
    contains: Vec<String>,
    /// Every non-empty line starts with one of these.
    lines_start_with: Option<Vec<String>>,
    /// Some line contains all of each inner list.
    line_containing: Vec<Vec<String>>,
    /// Some line has exactly these whitespace-separated words.
    line_words: Vec<Vec<String>>,
    /// A table: `[header, cell]` pairs, the cell on some line at the column
    /// the header has on the first.
    aligned_under: Vec<Vec<String>>,
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum StdoutKind {
    Empty,
    ExportScript,
    PlainText,
}

/// The API requests of the run: how many, what every one carried, and what
/// particular ones carried.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiCalls {
    count: usize,
    /// The `Authorization` header value of every request; absent means the
    /// requests carry none, unless every request is checked for it under
    /// `calls`. Not with `sigv4`, whose signature is that header.
    #[serde(default)]
    authorization: Option<String>,
    /// Headers every request carries, by lower-cased name.
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    sigv4: Option<Sigv4>,
    /// Request by request, from the first; a request not listed is not
    /// looked at on its own.
    #[serde(default)]
    calls: Vec<ApiCallExpect>,
}

impl ApiCalls {
    /// Whether the `Authorization` header is checked request by request
    /// under `calls`, so the run-level check counts the requests only.
    fn authorization_per_request(&self) -> bool {
        self.authorization.is_none()
            && self.sigv4.is_none()
            && self.calls.len() == self.count
            && self.calls.iter().all(ApiCallExpect::names_authorization)
    }
}

/// The requests for the API description in the run.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecCalls {
    count: usize,
    /// Every request carried `If-None-Match` / `If-Modified-Since`.
    #[serde(default)]
    conditional: bool,
    /// What the fake answered every request; `0` when there was none.
    #[serde(default)]
    status: u16,
    /// The `Authorization` header of every request; absent means none.
    #[serde(default)]
    authorization: Option<String>,
}

/// The files written under the sandbox across all runs: the exact list, or
/// patterns (`*` is any text) some of which must match and none of which may.
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum FilesWritten {
    Exact(Vec<String>),
    Patterns {
        #[serde(default)]
        include: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sigv4 {
    service: String,
    region: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretRead {
    operation: String,
    id: String,
    region: String,
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn leak_args(args: Vec<String>) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = args.into_iter().map(leak).collect();
    Box::leak(leaked.into_boxed_slice())
}

/// Read a case. `path` is where the text came from: for a `.toml` file the
/// id has to be the file name and, under `tests/cases/`, the feature the
/// directory, so a case cannot report under one name and be selected by
/// another; a case written into a Rust test names its source file instead.
fn parse(text: &str, path: &str) -> Case {
    let text = expand_placeholders(text);
    let case: Case = toml::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
    let layers = layers(&case);
    for (name, expect) in [("expect".to_string(), &case.expect)].into_iter().chain(
        layers
            .iter()
            .map(|(name, layer)| (format!("{name}.expect"), &layer.expect)),
    ) {
        assert!(
            expect.run.is_none(),
            "{path}: `{name}.run` belongs in a `[[{name}.runs]]` block"
        );
        check_block(expect, path, &name);
        for block in &expect.runs {
            assert!(
                block.run.is_some() && block.runs.is_empty(),
                "{path}: every `[[{name}.runs]]` block names its `run` and holds no `runs`"
            );
            assert!(
                block.files_unchanged.is_empty()
                    && block.paths_absent.is_empty()
                    && block.token_store.is_none(),
                "{path}: `files_unchanged`, `paths_absent` and `token_store` are about the whole scenario and belong in `[{name}]`"
            );
            check_block(block, path, &format!("{name}.runs"));
        }
    }
    for (name, layer) in &layers {
        for requirement in &layer.requires {
            assert!(
                requirement
                    .split_once(':')
                    .is_some_and(
                        |(kind, what)| REQUIREMENT_KINDS.contains(&kind) && !what.is_empty()
                    ),
                "{path}: `{name}.requires` holds `<kind>:<name>` with a kind among {REQUIREMENT_KINDS:?}, not {requirement:?}"
            );
        }
        assert!(
            (*name == "throwaway") == !layer.stacks.is_empty(),
            "{path}: `[{name}]` {} `stacks`: only `[throwaway]` names the disposable stacks it needs",
            if *name == "throwaway" {
                "needs"
            } else {
                "takes no"
            }
        );
        if *name != "local" {
            let fake_only: Vec<&str> = [&layer.expect]
                .into_iter()
                .chain(&layer.expect.runs)
                .flat_map(Expect::fake_only_keys)
                .collect();
            assert!(
                fake_only.is_empty(),
                "{path}: `[{name}.expect]` names {fake_only:?}, which only the fakes record: against the real service they are always empty"
            );
        }
    }
    let mut parts = path.rsplit('/');
    if let Some(stem) = parts.next().and_then(|name| name.strip_suffix(".toml")) {
        assert_eq!(
            case.id, stem,
            "{path}: `id` has to be the file name without .toml"
        );
        if let Some(directory) = parts.next().filter(|_| path.starts_with("tests/cases/")) {
            assert_eq!(
                case.feature, directory,
                "{path}: `feature` has to be the directory the file is in"
            );
        }
    }
    for (name, combination) in [("combination".to_string(), &case.combination)]
        .into_iter()
        .chain(
            layers
                .iter()
                .map(|(name, layer)| (format!("{name}.combination"), &layer.combination)),
        )
    {
        assert!(
            combination
                .iter()
                .all(|(key, value)| is_enumerated(key.replace('.', "-").as_str())
                    && is_enumerated(value)),
            "{path}: `{name}` holds enumerated coordinates only (lower-case words, digits, `-`, `_`; a `.` joins a command and its option): {combination:?}"
        );
    }
    case
}

/// The layers the case declares besides the fake one, by name.
fn layers(case: &Case) -> Vec<(&'static str, &Layer)> {
    [
        ("local", &case.local),
        ("real", &case.real),
        ("throwaway", &case.throwaway),
    ]
    .into_iter()
    .filter_map(|(name, layer)| Some((name, layer.as_ref()?)))
    .collect()
}

/// What one block of expectations says that checks nothing at all.
fn check_block(expect: &Expect, path: &str, key: &str) {
    assert!(
        expect.checks_anything(),
        "{path}: `[{key}]` expects nothing: every layer a case runs on, the fake one always, needs at least one expectation"
    );
    check_calls(expect, path, key);
    assert!(
        !matches!(&expect.files_written, Some(FilesWritten::Patterns { include, exclude }) if include.is_empty() && exclude.is_empty()),
        "{path}: `{key}.files_written` names no pattern: give `include`, `exclude`, or a list"
    );
    assert!(
        !matches!(&expect.stdout, Some(Stdout::Detail(detail)) if *detail == StdoutDetail::default()),
        "{path}: `{key}.stdout` checks nothing"
    );
}

impl Expect {
    /// Whether the block, or one of its `[[runs]]` blocks, sets a key that
    /// checks something: an empty list of texts, a `false` and a block that
    /// names only its `run` check nothing. Every field is named, so a new key
    /// has to say here whether it is an expectation.
    fn checks_anything(&self) -> bool {
        let Expect {
            run: _,
            runs,
            exit_code,
            error,
            json_error,
            hint,
            stdout,
            stdout_excludes,
            stdout_lines,
            stderr_contains,
            stderr_excludes,
            stderr_lines,
            stderr_lines_containing,
            stderr_empty,
            stderr_lines_start_with,
            stdout_json,
            stderr_json,
            sts_actions,
            sts_calls,
            sts_signing_region,
            token_grants,
            oauth_paths,
            oauth_params,
            op_calls,
            op_calls_containing,
            federation_calls,
            op_reads,
            open_calls,
            open_calls_containing,
            ssm_calls,
            api_calls,
            api_signed,
            api_requests_matching,
            spec_calls,
            secret_reads,
            no_files_written,
            files_written,
            env_script_exports,
            files_unchanged,
            paths_absent,
            token_store,
        } = self;
        runs.iter().any(Expect::checks_anything)
            || exit_code.is_some()
            || error.is_some()
            || json_error.is_some()
            || hint.is_some()
            || stdout.is_some()
            || !stdout_excludes.is_empty()
            || stdout_lines.is_some()
            || !stderr_contains.is_empty()
            || !stderr_excludes.is_empty()
            || stderr_lines.is_some()
            || !stderr_lines_containing.is_empty()
            || *stderr_empty
            || stderr_lines_start_with.is_some()
            || !stdout_json.is_empty()
            || !stderr_json.is_empty()
            || sts_actions.is_some()
            || sts_calls.is_some()
            || sts_signing_region.is_some()
            || token_grants.is_some()
            || oauth_paths.is_some()
            || !oauth_params.is_empty()
            || op_calls.is_some()
            || !op_calls_containing.is_empty()
            || federation_calls.is_some()
            || op_reads.is_some()
            || open_calls.is_some()
            || open_calls_containing.is_some()
            || ssm_calls.is_some()
            || api_calls.is_some()
            || api_signed.is_some()
            || !api_requests_matching.is_empty()
            || spec_calls.is_some()
            || secret_reads.is_some()
            || *no_files_written
            || files_written.is_some()
            || env_script_exports.is_some()
            || !files_unchanged.is_empty()
            || !paths_absent.is_empty()
            || token_store.is_some()
    }

    /// The keys this block sets that only a fake records: the fake STS,
    /// authorization server, API, `op`, browser and AWS secret stores. A run
    /// against the real service leaves each of them empty, so an expectation
    /// on one passes whatever the run did.
    fn fake_only_keys(&self) -> Vec<&'static str> {
        [
            ("sts_actions", self.sts_actions.is_some()),
            ("sts_calls", self.sts_calls.is_some()),
            ("sts_signing_region", self.sts_signing_region.is_some()),
            ("token_grants", self.token_grants.is_some()),
            ("oauth_paths", self.oauth_paths.is_some()),
            ("oauth_params", !self.oauth_params.is_empty()),
            ("op_calls", self.op_calls.is_some()),
            ("op_calls_containing", !self.op_calls_containing.is_empty()),
            ("federation_calls", self.federation_calls.is_some()),
            ("op_reads", self.op_reads.is_some()),
            ("open_calls", self.open_calls.is_some()),
            (
                "open_calls_containing",
                self.open_calls_containing.is_some(),
            ),
            ("ssm_calls", self.ssm_calls.is_some()),
            ("api_calls", self.api_calls.is_some()),
            ("api_signed", self.api_signed.is_some()),
            (
                "api_requests_matching",
                !self.api_requests_matching.is_empty(),
            ),
            ("spec_calls", self.spec_calls.is_some()),
            ("secret_reads", self.secret_reads.is_some()),
            ("token_store", self.token_store.is_some()),
            (
                "a JSON value of `<api_call_count>` or `<api_bytes_requested>`",
                self.stdout_json
                    .values()
                    .chain(self.stderr_json.values())
                    .any(JsonValue::reads_the_fakes),
            ),
        ]
        .into_iter()
        .filter_map(|(key, set)| set.then_some(key))
        .collect()
    }
}

fn check_calls(expect: &Expect, path: &str, key: &str) {
    if let Some(calls) = &expect.api_calls {
        assert!(
            calls.authorization.is_none() || calls.sigv4.is_none(),
            "{path}: `{key}.api_calls` names `authorization` or `sigv4`, not both: a signature is the Authorization header"
        );
        assert!(
            calls.calls.len() <= calls.count,
            "{path}: `{key}.api_calls` lists more requests under `calls` than its `count`"
        );
        for (index, call) in calls.calls.iter().enumerate() {
            assert!(
                call.authorization.is_none() || !call.no_authorization,
                "{path}: `{key}.api_calls.calls[{index}]` names `authorization` or `no_authorization`, not both"
            );
        }
    }
}

/// The layer `KURAMA_CASE_LAYER` names, which `cargo xtask verify --layer`
/// sets for the whole test run; unset is the fake layer every case has.
fn layer_from_env() -> Option<String> {
    std::env::var("KURAMA_CASE_LAYER")
        .ok()
        .filter(|name| !name.is_empty())
}

/// Where the databases `cargo xtask db-up` started listen, and the CA
/// `tests/db/up.sh` wrote: the same variables and defaults as
/// `tests/real_db.rs`.
fn local_database_placeholders() -> [(&'static str, String); 4] {
    let port = |variable: &str, default: &str| std::env::var(variable).unwrap_or(default.into());
    [
        ("{pg17_port}", port("KURAMA_TEST_PG17_PORT", "55432")),
        ("{pg18_port}", port("KURAMA_TEST_PG18_PORT", "55433")),
        ("{mysql_port}", port("KURAMA_TEST_MYSQL_PORT", "55306")),
        (
            "{db_tls_dir}",
            format!("{}/tests/db/tls", env!("CARGO_MANIFEST_DIR")),
        ),
    ]
}

/// What `[throwaway]` reads from the run: the profile `cargo xtask verify
/// --layer throwaway --profile` was given and the outputs of the stacks it
/// created, each an environment variable the runner sets. One the runner did
/// not set stays as written, so the fake layer of the same case, which never
/// reads the layer's config, is not touched.
const STACK_PLACEHOLDERS: [(&str, &str); 15] = [
    ("{throwaway_profile}", "KURAMA_THROWAWAY_PROFILE"),
    ("{iam_api_url}", "KURAMA_STACK_IAM_API_URL"),
    ("{lambda_url}", "KURAMA_STACK_LAMBDA_URL"),
    ("{bastion_stack}", "KURAMA_STACK_BASTION_STACK"),
    ("{bastion_instance_id}", "KURAMA_STACK_BASTION_INSTANCE_ID"),
    ("{rds_iam_stack}", "KURAMA_STACK_RDS_IAM_STACK"),
    ("{rds_pg_host}", "KURAMA_STACK_RDS_PG_HOST"),
    ("{rds_mysql_host}", "KURAMA_STACK_RDS_MYSQL_HOST"),
    ("{rds_ca_file}", "KURAMA_STACK_RDS_CA_FILE"),
    ("{dsql_host}", "KURAMA_STACK_DSQL_HOST"),
    ("{dsql_identifier}", "KURAMA_STACK_DSQL_IDENTIFIER"),
    (
        "{s3_bucket_same_region}",
        "KURAMA_STACK_S3_BUCKET_SAME_REGION",
    ),
    ("{s3_same_region}", "KURAMA_STACK_S3_SAME_REGION"),
    (
        "{s3_bucket_other_region}",
        "KURAMA_STACK_S3_BUCKET_OTHER_REGION",
    ),
    ("{s3_other_region}", "KURAMA_STACK_S3_OTHER_REGION"),
];

/// `STACK_PLACEHOLDERS` replaced by what `lookup` answers for each variable.
fn expand_stack_placeholders(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = text.to_string();
    for (placeholder, variable) in STACK_PLACEHOLDERS {
        if let Some(value) = lookup(variable) {
            out = out.replace(placeholder, &value);
        }
    }
    out
}

/// Replace the placeholders that name what the fakes were given.
fn expand_placeholders(text: &str) -> String {
    let mut out = text
        .replace("{granted_access_token}", GRANTED_ACCESS_TOKEN)
        .replace("{refreshed_access_token}", REFRESHED_ACCESS_TOKEN)
        .replace("{stored_access_token}", STORED_ACCESS_TOKEN)
        .replace("{fake_client_secret}", FAKE_CLIENT_SECRET)
        .replace("{source_access_key}", SOURCE_ACCESS_KEY)
        .replace("{result_access_key}", RESULT_ACCESS_KEY)
        .replace("{result_secret_key}", RESULT_SECRET)
        .replace("{result_session_token}", RESULT_TOKEN)
        .replace("{session_access_key}", SESSION_ACCESS_KEY)
        .replace("{second_session_access_key}", SECOND_SESSION_ACCESS_KEY)
        .replace("{op_access_key}", OP_ACCESS_KEY)
        .replace("{op_totp}", OP_TOTP)
        .replace("{op_json_totp}", OP_JSON_TOTP)
        .replace("{signin_token}", SIGNIN_TOKEN)
        .replace("{repo}", env!("CARGO_MANIFEST_DIR"));
    for (placeholder, value) in local_database_placeholders() {
        out = out.replace(placeholder, &value);
    }
    out = expand_stack_placeholders(&out, |variable| std::env::var(variable).ok());
    while let Some(start) = out.find("{sha256:") {
        let end = out[start..]
            .find('}')
            .map(|end| start + end)
            .expect("`{sha256:` is closed by `}`");
        let digest = sha256_fingerprint(&out[start + "{sha256:".len()..end]);
        out.replace_range(start..=end, &digest);
    }
    out
}

fn is_enumerated(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The scenario of the fake layer, or of `layer` where it says otherwise.
fn scenario(case: &Case, layer: Option<&Layer>) -> Scenario {
    let input = &case.input;
    let fakes = &case.fakes;
    let args = layer
        .and_then(|layer| layer.args.as_ref())
        .unwrap_or(&input.args);
    let then_run = layer
        .and_then(|layer| layer.then_run.as_ref())
        .unwrap_or(&input.then_run);
    let config = layer
        .and_then(|layer| layer.config.as_ref())
        .or(input.config.as_ref());
    let replace_config = layer
        .and_then(|layer| layer.replace_config.as_ref())
        .or(input.replace_config.as_ref());
    let mut env = input.env.clone();
    if let Some(layer) = layer {
        env.extend(layer.env.clone());
    }
    let mut scenario = Scenario::new(
        leak(case.id.clone()),
        leak(case.feature.clone()),
        leak_args(args.clone()),
    );
    for run in then_run {
        scenario = scenario.then_run(leak_args(run.clone()));
    }
    if let Some(config) = config {
        scenario = scenario.with_extra_config(config);
    }
    if let Some(config) = replace_config {
        scenario = scenario.with_config(config);
    }
    if let Some(config) = &input.aws_config {
        scenario = scenario.with_aws_config(leak(config.clone()));
    }
    for (name, value) in &env {
        scenario = scenario.with_env(leak(name.clone()), leak(value.clone()));
    }
    for (path, content) in &input.home_files {
        scenario = scenario.with_home_file(leak(path.clone()), leak(content.clone()));
    }
    if let Some(stdin) = &input.stdin {
        scenario = scenario.with_stdin(stdin.clone());
    }
    for token in &input.stored_tokens {
        scenario = scenario.with_stored_token(
            leak(token.source.clone()),
            &stored_token_json(&token.access_token, token.expires_in, token.refreshable),
        );
    }
    if let Some(token) = &input.service_account_keychain {
        scenario = scenario.with_service_account_keychain(match token.as_str() {
            "<absent>" => None,
            token => Some(leak(token.to_string())),
        });
    }
    scenario = scenario.sts(match &fakes.sts {
        Sts::Success | Sts::Plain(StsPlain::Success) => StsFake::Success,
        Sts::Error { error } => StsFake::Error {
            code: leak(error.code.clone()),
            message: leak(error.message.clone()),
        },
    });
    scenario = scenario.onepassword(match fakes.op {
        Op::Disabled => OnePassword::Disabled,
        Op::Enabled => OnePassword::Enabled,
        Op::NotSignedIn => OnePassword::NotSignedIn,
        Op::Hangs => OnePassword::Hangs,
        Op::VersionHangs => OnePassword::VersionHangs,
        Op::OtpFails => OnePassword::OtpFails,
    });
    scenario = scenario.oauth(match &fakes.oauth {
        OAuth::Success | OAuth::Plain(OAuthPlain::Success) => OAuthFake::Success,
        OAuth::Plain(OAuthPlain::Unavailable) => OAuthFake::Unavailable,
        OAuth::Rejected { rejected } => OAuthFake::Rejected {
            error: leak(rejected.clone()),
        },
    });
    scenario = scenario.api(match &fakes.api {
        Api::Ok | Api::Plain(ApiPlain::Ok) => ApiFake::Ok,
        Api::Plain(ApiPlain::UnauthorizedOnce) => ApiFake::UnauthorizedOnce,
        Api::Plain(ApiPlain::Unauthorized) => ApiFake::Unauthorized,
        Api::Plain(ApiPlain::NotFound) => ApiFake::NotFound,
        Api::Plain(ApiPlain::Slow) => ApiFake::Slow,
        Api::Plain(ApiPlain::CompactArray) => ApiFake::CompactArray,
        Api::Plain(ApiPlain::Echo) => ApiFake::Echo,
        Api::Plain(ApiPlain::Pages) => ApiFake::Pages,
        Api::Plain(ApiPlain::GraphQl) => ApiFake::GraphQl,
        Api::Padding { padding } => ApiFake::Padding(*padding),
    });
    scenario = scenario.aws_secrets(match fakes.aws_secrets {
        Secrets::Absent => AwsSecrets::Absent,
        Secrets::FromId => AwsSecrets::FromId,
        Secrets::AccessDenied => AwsSecrets::AccessDenied,
        Secrets::NotFound => AwsSecrets::NotFound,
    });
    scenario = scenario.spec(match fakes.spec {
        Spec::Etag => SpecFake::Etag,
        Spec::Down => SpecFake::Down,
    });
    if fakes.session_cache {
        scenario = scenario.with_session_cache();
    }
    if fakes.corrupt_session_cache {
        scenario = scenario.with_corrupt_session_cache();
    }
    if fakes.token_store {
        scenario = scenario.with_token_store();
    }
    if fakes.corrupt_token_store {
        scenario = scenario.with_corrupt_token_store();
    }
    if let Some(code) = fakes.keychain_denied {
        scenario = scenario
            .with_session_cache()
            .with_env("KURAMA_TEST_KEYCHAIN_DENIED", leak(code.to_string()));
    }
    if fakes.browser {
        scenario = scenario.with_fake_browser();
    }
    if fakes.tools {
        scenario = scenario.with_fake_tools();
    }
    if fakes.federation_fails {
        scenario = scenario.with_failing_federation();
    }
    if fakes.federation_invalid_json {
        scenario.federation_invalid_json = true;
    }
    if fakes.env_script {
        scenario = scenario.with_env_script();
    }
    if let Some(mode) = fakes.existing_env_script_mode {
        scenario = scenario.with_existing_env_script(mode);
    }
    scenario
}

fn apply(v: &mut Verification, expect: &Expect) {
    apply_block(v, expect, "expect");
    for block in &expect.runs {
        let index = block.run.expect("checked by parse");
        v.keyed_run(index, &format!("expect.runs[{index}]"), |v| {
            apply_block(v, block, "");
            v
        });
    }
}

fn apply_block(v: &mut Verification, expect: &Expect, prefix: &str) {
    let key = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        }
    };
    if let Some(code) = expect.exit_code {
        v.keyed(&key("exit_code"), |v| v.expect_exit_code(code));
    }
    // A failure leaves stdout empty, unless the block says what stdout holds:
    // a command that reports on stdout and fails on what it found.
    // A kind (`stdout = "plain-text"`) says nothing about what the report
    // holds, so it never lifts the empty-stdout check.
    let reports_on_stdout = !expect.stdout_json.is_empty()
        || matches!(&expect.stdout, Some(Stdout::Detail(detail)) if !detail.contains.is_empty());
    if let Some(error) = &expect.error {
        v.keyed(&key("error"), |v| {
            if reports_on_stdout {
                v.expect_error_beside_stdout(&error.code, error.exit)
            } else {
                v.expect_error(&error.code, error.exit)
            }
        });
        if let Some(message) = &error.message {
            v.keyed(&key("error.message"), |v| {
                v.expect_error_line(&error.code, message)
            });
        }
    }
    if let Some(error) = &expect.json_error {
        v.keyed(&key("json_error"), |v| {
            if reports_on_stdout {
                v.expect_json_error_beside_stdout(&error.code, error.exit)
            } else {
                v.expect_json_error(&error.code, error.exit)
            }
        });
    }
    if let Some(hint) = &expect.hint {
        v.keyed(&key("hint"), |v| {
            v.expect_stderr_contains(&format!("hint: {hint}"))
        });
    }
    let stdout_kind = |v: &mut Verification, kind: StdoutKind| {
        v.keyed(
            &key("stdout"),
            match kind {
                StdoutKind::Empty => Verification::expect_stdout_empty,
                StdoutKind::ExportScript => Verification::expect_stdout_is_export_script,
                StdoutKind::PlainText => Verification::expect_stdout_plain_text,
            },
        );
    };
    match &expect.stdout {
        Some(Stdout::Kind(kind)) => stdout_kind(v, *kind),
        Some(Stdout::Detail(detail)) => {
            if let Some(kind) = detail.kind {
                stdout_kind(v, kind);
            }
            if let Some(text) = &detail.equals {
                v.keyed(&key("stdout.equals"), |v| v.expect_stdout_equals(text));
            }
            if let Some(prefix) = &detail.starts_with {
                v.keyed(&key("stdout.starts_with"), |v| {
                    v.expect_stdout_starts_with(prefix)
                });
            }
            if let Some(count) = detail.line_count {
                v.keyed(&key("stdout.line_count"), |v| {
                    v.expect_stdout_line_count(count)
                });
            }
            for needle in &detail.contains {
                v.keyed(&key("stdout.contains"), |v| {
                    v.expect_stdout_contains(needle)
                });
            }
            if let Some(prefixes) = &detail.lines_start_with {
                let prefixes: Vec<&str> = prefixes.iter().map(String::as_str).collect();
                v.keyed(&key("stdout.lines_start_with"), |v| {
                    v.expect_stdout_lines_start_with(&prefixes)
                });
            }
            for parts in &detail.line_containing {
                let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
                v.keyed(&key("stdout.line_containing"), |v| {
                    v.expect_stdout_line_containing(&parts)
                });
            }
            for words in &detail.line_words {
                let words: Vec<&str> = words.iter().map(String::as_str).collect();
                v.keyed(&key("stdout.line_words"), |v| {
                    v.expect_stdout_line_words(&words)
                });
            }
            for pair in &detail.aligned_under {
                let [header, cell] = pair.as_slice() else {
                    panic!("`stdout.aligned_under` holds [header, cell] pairs: {pair:?}")
                };
                v.keyed(&key("stdout.aligned_under"), |v| {
                    v.expect_stdout_aligned_under(header, cell)
                });
            }
        }
        None => {}
    }
    for needle in &expect.stdout_excludes {
        v.keyed(&key("stdout_excludes"), |v| {
            v.expect_stdout_excludes(needle)
        });
    }
    if let Some(count) = expect.stdout_lines {
        v.keyed(&key("stdout_lines"), |v| v.expect_stdout_lines(count));
    }
    for needle in &expect.stderr_contains {
        v.keyed(&key("stderr_contains"), |v| {
            v.expect_stderr_contains(needle)
        });
    }
    for needle in &expect.stderr_excludes {
        v.keyed(&key("stderr_excludes"), |v| {
            v.expect_stderr_excludes(needle)
        });
    }
    if let Some(count) = expect.stderr_lines {
        v.keyed(&key("stderr_lines"), |v| v.expect_stderr_line_count(count));
    }
    for (needle, count) in &expect.stderr_lines_containing {
        v.keyed(&key("stderr_lines_containing"), |v| {
            v.expect_stderr_lines_containing(needle, *count)
        });
    }
    if expect.stderr_empty {
        v.keyed(&key("stderr_empty"), Verification::expect_stderr_empty);
    }
    if let Some(prefixes) = &expect.stderr_lines_start_with {
        let prefixes: Vec<&str> = prefixes.iter().map(String::as_str).collect();
        v.keyed(&key("stderr_lines_start_with"), |v| {
            v.expect_stderr_lines_start_with(&prefixes)
        });
    }
    for (stream, values) in [
        ("stdout", &expect.stdout_json),
        ("stderr", &expect.stderr_json),
    ] {
        for (pointer, value) in values {
            v.keyed(&key(&format!("{stream}_json[{pointer}]")), |v| {
                v.expect_json_at(stream, pointer, &value.expectation())
            });
        }
    }
    if let Some(actions) = &expect.sts_actions {
        let actions: Vec<&str> = actions.iter().map(String::as_str).collect();
        v.keyed(&key("sts_actions"), |v| v.expect_sts_actions(&actions));
    }
    if let Some(calls) = &expect.sts_calls {
        let calls: Vec<StsCallExpect> = calls.iter().map(StsCallFields::expectation).collect();
        v.keyed(&key("sts_calls"), |v| v.expect_sts_calls(&calls));
    }
    if let Some(region) = &expect.sts_signing_region {
        v.keyed(&key("sts_signing_region"), |v| {
            v.expect_sts_signing_region(region)
        });
    }
    if let Some(grants) = &expect.token_grants {
        let grants: Vec<&str> = grants.iter().map(String::as_str).collect();
        v.keyed(&key("token_grants"), |v| v.expect_token_grants(&grants));
    }
    if let Some(paths) = &expect.oauth_paths {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        v.keyed(&key("oauth_paths"), |v| v.expect_oauth_paths(&paths));
    }
    for request in &expect.oauth_params {
        let params: Vec<(&str, &str)> = request
            .params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        v.keyed(&key("oauth_params"), |v| {
            v.expect_oauth_params(request.call, &params)
        });
    }
    if let Some(count) = expect.op_calls {
        v.keyed(&key("op_calls"), |v| v.expect_op_calls(count));
    }
    for (needle, count) in &expect.op_calls_containing {
        v.keyed(&key("op_calls_containing"), |v| {
            v.expect_op_calls_containing(needle, *count)
        });
    }
    if let Some(calls) = &expect.federation_calls {
        let calls: Vec<(&str, &str)> = calls
            .iter()
            .map(|call| (call.action.as_str(), call.session_access_key.as_str()))
            .collect();
        v.keyed(&key("federation_calls"), |v| {
            v.expect_federation_calls(&calls)
        });
    }
    if let Some(reads) = &expect.op_reads {
        let reads: Vec<&str> = reads.iter().map(String::as_str).collect();
        v.keyed(&key("op_reads"), |v| v.expect_op_reads(&reads));
    }
    if let Some(urls) = &expect.open_calls {
        let urls: Vec<&str> = urls.iter().map(String::as_str).collect();
        v.keyed(&key("open_calls"), |v| v.expect_open_calls(&urls));
    }
    if let Some(calls) = &expect.open_calls_containing {
        let calls: Vec<Vec<&str>> = calls
            .iter()
            .map(|parts| parts.iter().map(String::as_str).collect())
            .collect();
        let calls: Vec<&[&str]> = calls.iter().map(Vec::as_slice).collect();
        v.keyed(&key("open_calls_containing"), |v| {
            v.expect_open_calls_containing(&calls)
        });
    }
    if let Some(operations) = &expect.ssm_calls {
        let operations: Vec<&str> = operations.iter().map(String::as_str).collect();
        v.keyed(&key("ssm_calls"), |v| v.expect_ssm_calls(&operations));
    }
    if let Some(calls) = &expect.api_calls {
        match &calls.sigv4 {
            Some(sigv4) => v.keyed(&key("api_calls"), |v| {
                v.expect_sigv4_api_calls(calls.count, &sigv4.service, &sigv4.region)
            }),
            None if calls.authorization_per_request() => {
                v.keyed(&key("api_calls"), |v| v.expect_api_call_count(calls.count))
            }
            None => v.keyed(&key("api_calls"), |v| {
                v.expect_api_calls(calls.count, calls.authorization.as_deref())
            }),
        };
        if !calls.headers.is_empty() {
            let headers: Vec<(&str, &str)> = calls
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            v.keyed(&key("api_calls.headers"), |v| {
                v.expect_api_call_headers(&headers)
            });
        }
        for (call, expect) in calls.calls.iter().enumerate() {
            v.keyed(&key(&format!("api_calls.calls[{call}]")), |v| {
                v.expect_api_call(call, expect)
            });
        }
    }
    if let Some(sigv4) = &expect.api_signed {
        v.keyed(&key("api_signed"), |v| {
            v.expect_api_signed(&sigv4.service, &sigv4.region)
        });
    }
    if !expect.api_requests_matching.is_empty() {
        v.keyed(&key("api_requests_matching"), |v| {
            v.expect_api_requests_matching(&expect.api_requests_matching)
        });
    }
    if let Some(calls) = &expect.spec_calls {
        v.keyed(&key("spec_calls"), |v| {
            v.expect_spec_calls(calls.count, calls.conditional, calls.status)
        });
        if calls.count > 0 {
            v.keyed(&key("spec_calls.authorization"), |v| {
                v.expect_spec_authorization(calls.authorization.as_deref())
            });
        }
    }
    if let Some(reads) = &expect.secret_reads {
        let reads: Vec<(&str, &str, &str)> = reads
            .iter()
            .map(|read| {
                (
                    read.operation.as_str(),
                    read.id.as_str(),
                    read.region.as_str(),
                )
            })
            .collect();
        v.keyed(&key("secret_reads"), |v| v.expect_secret_reads(&reads));
    }
    if expect.no_files_written {
        v.keyed(
            &key("no_files_written"),
            Verification::expect_no_files_written,
        );
    }
    match &expect.files_written {
        Some(FilesWritten::Exact(files)) => {
            let files: Vec<&str> = files.iter().map(String::as_str).collect();
            v.keyed(&key("files_written"), |v| v.expect_files_written(&files));
        }
        Some(FilesWritten::Patterns { include, exclude }) => {
            let include: Vec<&str> = include.iter().map(String::as_str).collect();
            let exclude: Vec<&str> = exclude.iter().map(String::as_str).collect();
            v.keyed(&key("files_written"), |v| {
                v.expect_files_written_matching(&include, &exclude)
            });
        }
        None => {}
    }
    if let Some(variables) = &expect.env_script_exports {
        let variables: Vec<&str> = variables.iter().map(String::as_str).collect();
        v.keyed(&key("env_script_exports"), |v| {
            v.expect_env_script_exports(&variables)
        });
    }
}

/// Run the case in `text` (read from `path`) on the fake layer and return
/// its verification with every expectation checked, before the report is
/// written: what the harness's own scenario reads.
pub fn verification(text: &str, path: &str) -> Verification {
    verification_on(text, path, None)
}

/// The same, on the layer `layer` names (`None` is the fake layer). A layer
/// the case does not declare is a panic: `cargo xtask verify --layer` selects
/// only the cases that declare it, so reaching this is a hand-written filter.
fn verification_on(text: &str, path: &str, layer: Option<&str>) -> Verification {
    let case = parse(text, path);
    let (layer, evidence): (Option<&Layer>, &'static str) = match layer {
        None => (None, "fake"),
        Some("local") => (
            Some(case.local.as_ref().unwrap_or_else(|| {
                panic!("{path}: declares no [local]; `cargo xtask verify --layer local` runs the cases that do")
            })),
            "local",
        ),
        Some("real") => (
            Some(case.real.as_ref().unwrap_or_else(|| {
                panic!("{path}: declares no [real]; `cargo xtask verify --layer real` runs the cases that do")
            })),
            "real",
        ),
        Some("throwaway") => (
            Some(case.throwaway.as_ref().unwrap_or_else(|| {
                panic!("{path}: declares no [throwaway]; `cargo xtask verify --layer throwaway` runs the cases that do")
            })),
            "throwaway",
        ),
        Some(other) => panic!("{path}: no layer is named `{other}` (KURAMA_CASE_LAYER); `local`, `throwaway` and `real` are the ones a case can declare here"),
    };
    let mut combination = case.combination.clone();
    if let Some(layer) = layer {
        combination.extend(layer.combination.clone());
    }
    let expect = layer.map_or(&case.expect, |layer| &layer.expect);
    for copy in &case.input.copy_files {
        if let Some(parent) = std::path::Path::new(&copy.to).parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{path}: {}: {e}", copy.to));
        }
        std::fs::copy(&copy.from, &copy.to)
            .unwrap_or_else(|e| panic!("{path}: copy {} to {}: {e}", copy.from, copy.to));
    }
    let before: Vec<(&String, Vec<u8>)> = expect
        .files_unchanged
        .iter()
        .map(|file| {
            (
                file,
                std::fs::read(file).unwrap_or_else(|e| panic!("{path}: {file}: {e}")),
            )
        })
        .collect();
    let mut scenario = scenario(&case, layer);
    // The throwaway layer talks to AWS itself: nothing is faked but the
    // sandbox HOME, and the profile is the person's own.
    if evidence == "throwaway" || evidence == "real" {
        scenario = scenario.with_real_aws();
    }
    let mut v = run(scenario)
        .with_combination(combination)
        .with_evidence(evidence);
    apply(&mut v, expect);
    for (file, bytes) in &before {
        v.keyed("expect.files_unchanged", |v| {
            v.expect_file_unchanged(file, bytes)
        });
    }
    for file in &expect.paths_absent {
        v.keyed("expect.paths_absent", |v| v.expect_path_absent(file));
    }
    if let Some(sources) = &expect.token_store {
        v.keyed("expect.token_store", |v| v.expect_token_store(sources));
    }
    v
}

/// Why a case file is refused when it is read, or `None` when it reads.
pub fn refusal(text: &str, path: &str) -> Option<String> {
    let payload = std::panic::catch_unwind(|| {
        parse(text, path);
    })
    .err()?;
    Some(
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
            .unwrap_or_default(),
    )
}

/// Run one case file on the layer `KURAMA_CASE_LAYER` names and write its
/// report: what every generated test calls.
pub fn run_case(text: &str, path: &str) {
    verification_on(text, path, layer_from_env().as_deref()).finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
id = "x_runs"
feature = "api-client"
[combination]
command = "api"
[input]
args = ["api", "svc", "/items"]
[fakes]
api = "unauthorized"
[expect]
error = { code = "API_HTTP_ERROR", exit = 4 }
api_calls = { count = 1, headers = { "x-a" = "1" } }
"#;

    #[test]
    fn a_case_reads_its_fakes_and_expectations() {
        let case = parse(MINIMAL, "tests/cases/api-client/x_runs.toml");
        assert!(matches!(case.fakes.api, Api::Plain(ApiPlain::Unauthorized)));
        assert!(matches!(case.fakes.oauth, OAuth::Success));
        assert_eq!(case.expect.error.as_ref().map(|e| e.exit), Some(4));
        assert_eq!(case.combination["command"], "api");
        let s = scenario(&case, None);
        assert_eq!(s.runs, vec![&["api", "svc", "/items"][..]]);
        assert!(matches!(s.api, ApiFake::Unauthorized));
    }

    #[test]
    #[should_panic(expected = "`id` has to be the file name")]
    fn the_id_is_the_file_name() {
        parse(MINIMAL, "tests/cases/api-client/other.toml");
    }

    #[test]
    #[should_panic(expected = "`feature` has to be the directory")]
    fn the_feature_is_the_directory() {
        parse(MINIMAL, "tests/cases/oauth/x_runs.toml");
    }

    #[test]
    #[should_panic(expected = "unknown field `tty`")]
    fn an_input_the_harness_cannot_give_is_refused_by_name() {
        parse(
            &MINIMAL.replace("[input]\n", "[input]\ntty = true\n"),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "enumerated coordinates only")]
    fn a_combination_value_is_an_enumerated_word() {
        parse(
            &MINIMAL.replace("command = \"api\"", "command = \"api svc\""),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "not both")]
    fn a_signature_and_a_bearer_are_one_header() {
        parse(
            &MINIMAL.replace(
                "api_calls = { count = 1, headers = { \"x-a\" = \"1\" } }",
                "api_calls = { count = 1, authorization = \"Bearer x\", sigv4 = { service = \"s\", region = \"r\" } }",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    fn placeholders_name_what_the_fakes_were_given() {
        let text = expand_placeholders(
            "a = \"{granted_access_token}\"\nb = \"{sha256:abc}\"\nc = \"{stored_access_token} x\"",
        );
        assert_eq!(
            text,
            format!(
                "a = \"{GRANTED_ACCESS_TOKEN}\"\nb = \"{}\"\nc = \"{STORED_ACCESS_TOKEN} x\"",
                sha256_fingerprint("abc")
            )
        );
    }

    #[test]
    #[should_panic(expected = "names its `run`")]
    fn a_runs_block_names_its_run() {
        parse(
            &MINIMAL.replace("[expect]\n", "[[expect.runs]]\nexit_code = 0\n[expect]\n"),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    fn a_stored_token_and_a_json_expectation_are_read() {
        let case = parse(
            &MINIMAL
                .replace(
                    "[input]\n",
                    "[input]\nstored_tokens = [{ source = \"github\", access_token = \"{stored_access_token}\", expires_in = 60 }]\n",
                )
                .replace(
                    "[expect]\n",
                    "[expect]\nstdout_json = { \"/a\" = \"<string>\", \"/b\" = { contains = \"x\" }, \"/c\" = 1 }\n",
                ),
            "tests/cases/api-client/x_runs.toml",
        );
        let s = scenario(&case, None);
        assert!(s.token_store);
        assert_eq!(s.stored_tokens[0].0, "github");
        assert!(s.stored_tokens[0].1.contains(STORED_ACCESS_TOKEN));
        let json = &case.expect.stdout_json;
        assert_eq!(json["/a"].expectation(), JsonExpect::AnyString);
        assert_eq!(
            json["/b"].expectation(),
            JsonExpect::Contains(vec!["x".into()])
        );
        assert_eq!(
            json["/c"].expectation(),
            JsonExpect::Equals(serde_json::json!(1))
        );
    }

    const WITH_LOCAL: &str = r#"
id = "x_runs"
feature = "database"
[combination]
command = "db"
engine = "sqlite"
[input]
args = ["db", "app", "--tables", "--json"]
config = "[db.app]\nengine = \"sqlite\"\npath = \"{repo}/tests/fixtures/db/app.sqlite3\"\n"
[expect]
exit_code = 0
[local]
config = "[db.app]\nengine = \"postgresql\"\nport = {pg17_port}\nca_file = \"{db_tls_dir}/ca.pem\"\n"
env = { KURAMA_LOG = "debug" }
[local.combination]
engine = "postgresql"
tls = "verify-ca"
[local.expect]
exit_code = 0
stdout_json = { "/operation" = "tables" }
"#;

    #[test]
    fn a_local_layer_replaces_the_config_and_adds_its_coordinates() {
        let case = parse(WITH_LOCAL, "tests/cases/database/x_runs.toml");
        let layer = case.local.as_ref().expect("[local] is read");
        let config = layer.config.as_deref().unwrap();
        assert!(config.contains("engine = \"postgresql\""));
        assert!(
            !config.contains("{pg17_port}") && !config.contains("{db_tls_dir}"),
            "{config}"
        );
        assert!(config.contains("/tests/db/tls/ca.pem"), "{config}");
        assert!(
            case.input
                .config
                .as_deref()
                .unwrap()
                .contains("/tests/fixtures/db/app.sqlite3"),
            "{{repo}} is the checkout"
        );
        assert_eq!(layer.combination["engine"], "postgresql");
        assert_eq!(layer.expect.stdout_json.len(), 1);

        let fake = scenario(&case, None);
        assert!(fake.extra_config.concat().contains("sqlite"));
        assert!(fake.env.is_empty());
        let local = scenario(&case, Some(layer));
        assert!(local.extra_config.concat().contains("postgresql"));
        assert!(!local.extra_config.concat().contains("sqlite"));
        assert_eq!(local.env, vec![("KURAMA_LOG", "debug")]);
        assert_eq!(
            local.runs, fake.runs,
            "args are the case's unless the layer says otherwise"
        );
    }

    #[test]
    #[should_panic(expected = "`local.combination` holds enumerated coordinates only")]
    fn a_layer_coordinate_is_an_enumerated_word_too() {
        parse(
            &WITH_LOCAL.replace("tls = \"verify-ca\"", "tls = \"verify ca\""),
            "tests/cases/database/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "declares no [local]")]
    fn a_layer_the_case_does_not_declare_is_named() {
        verification_on(MINIMAL, "tests/cases/api-client/x_runs.toml", Some("local"));
    }

    const WITH_REAL: &str = r#"
id = "x_runs"
feature = "api-client"

[input]
args = ["api", "github-public", "/rate_limit"]
config = "[api.github-public]\nbase_url = \"{server}/api\"\n"

[expect]
exit_code = 0

[real]
requires = ["api:github-public", "profile:kurama-sandbox"]
config = "[api.github-public]\nbase_url = \"https://api.github.com\"\n"

[real.combination]
service = "github"

[real.expect]
exit_code = 0
"#;

    /// A `[real]` says what it needs by kind, and the scenario built for it
    /// runs without the fakes: that flag is what `Sandbox` reads to leave the
    /// fake endpoints, the fake `op` and the generated configuration out.
    #[test]
    fn a_real_layer_names_what_it_requires_and_runs_without_the_fakes() {
        let case = parse(WITH_REAL, "tests/cases/api-client/x_runs.toml");
        let layer = case.real.as_ref().expect("[real] is read");
        assert_eq!(
            layer.requires,
            ["api:github-public", "profile:kurama-sandbox"]
        );
        assert!(
            !scenario(&case, None).real_aws,
            "the fake layer keeps the fakes"
        );
        let real = scenario(&case, Some(layer)).with_real_aws();
        assert!(real.real_aws);
        assert_eq!(
            real.extra_config,
            ["[api.github-public]\nbase_url = \"https://api.github.com\"\n"]
        );
    }

    #[test]
    #[should_panic(expected = "`real.requires` holds `<kind>:<name>`")]
    fn a_requirement_without_a_known_kind_is_refused() {
        parse(
            &WITH_REAL.replace("\"profile:kurama-sandbox\"", "\"bucket:kurama-sandbox\""),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "declares no [real]")]
    fn a_real_layer_the_case_does_not_declare_is_named() {
        verification_on(MINIMAL, "tests/cases/api-client/x_runs.toml", Some("real"));
    }

    #[test]
    #[should_panic(expected = "no layer is named `staging`")]
    fn a_layer_nothing_knows_is_named() {
        verification_on(
            WITH_LOCAL,
            "tests/cases/database/x_runs.toml",
            Some("staging"),
        );
    }

    #[test]
    fn an_sts_call_a_keychain_entry_and_a_stdout_kind_are_read() {
        let case = parse(
            &MINIMAL
                .replace("[input]\n", "[input]\nservice_account_keychain = \"<absent>\"\n")
                .replace(
                    "[expect]\n",
                    "[expect]\nsts_calls = [{ action = \"AssumeRole\", token_code = \"<none>\", signing_access_key = \"{source_access_key}\" }]\nstdout = { kind = \"export-script\", line_count = 3 }\nop_calls_containing = { \"--otp\" = 1 }\nopen_calls_containing = [[\"Action=login\"]]\n",
                ),
            "tests/cases/api-client/x_runs.toml",
        );
        let s = scenario(&case, None);
        assert!(s.service_account_keychain && s.service_account_token.is_none());
        let call = case.expect.sts_calls.as_ref().unwrap()[0].expectation();
        assert_eq!(call.action.as_deref(), Some("AssumeRole"));
        assert_eq!(call.token_code.as_deref(), Some("<none>"));
        assert_eq!(call.signing_access_key.as_deref(), Some(SOURCE_ACCESS_KEY));
        assert!(call.role_arn.is_none());
        let Some(Stdout::Detail(detail)) = &case.expect.stdout else {
            panic!("a detailed stdout")
        };
        assert!(matches!(detail.kind, Some(StdoutKind::ExportScript)));
        assert_eq!(detail.line_count, Some(3));
        assert_eq!(case.expect.op_calls_containing["--otp"], 1);
        assert_eq!(
            case.expect.open_calls_containing.as_deref(),
            Some(&[vec!["Action=login".to_string()]][..])
        );
    }

    #[test]
    fn requests_checked_one_by_one_carry_their_own_authorization() {
        let case = parse(
            &MINIMAL.replace(
                "api_calls = { count = 1, headers = { \"x-a\" = \"1\" } }",
                "api_calls = { count = 2, calls = [{ authorization = \"Bearer a\" }, { no_authorization = true, path = \"/x\" }] }",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
        let calls = case.expect.api_calls.as_ref().unwrap();
        assert!(calls.authorization_per_request());
        assert_eq!(calls.calls[1].path.as_deref(), Some("/x"));
        let case = parse(
            &MINIMAL.replace(
                "api_calls = { count = 1, headers = { \"x-a\" = \"1\" } }",
                "api_calls = { count = 2, calls = [{ authorization = \"Bearer a\" }] }",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
        assert!(
            !case
                .expect
                .api_calls
                .as_ref()
                .unwrap()
                .authorization_per_request()
        );
    }

    #[test]
    #[should_panic(expected = "more requests under `calls`")]
    fn requests_listed_one_by_one_fit_the_count() {
        parse(
            &MINIMAL.replace(
                "api_calls = { count = 1, headers = { \"x-a\" = \"1\" } }",
                "api_calls = { count = 0, calls = [{ path = \"/x\" }] }",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    fn the_detailed_keys_are_read() {
        let case = parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[expect]\nstdout = { starts_with = \"a\" }\nstdout_excludes = [\"b\"]\nstdout_lines = 2\nsts_calls = [{ action = \"AssumeRole\", signing_access_key = \"AKIA\" }]\nspec_calls = { count = 1, conditional = true, status = 304 }\nfiles_written = { include = [\"*.body\"], exclude = [\"*.json\"] }\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
        let expect = &case.expect;
        assert!(
            matches!(&expect.stdout, Some(Stdout::Detail(d)) if d.starts_with.as_deref() == Some("a"))
        );
        assert_eq!(expect.stdout_excludes, ["b"]);
        assert_eq!(expect.stdout_lines, Some(2));
        assert_eq!(
            expect.sts_calls.as_ref().unwrap()[0].action.as_deref(),
            Some("AssumeRole")
        );
        assert!(expect.spec_calls.as_ref().unwrap().conditional);
        assert!(
            matches!(&expect.files_written, Some(FilesWritten::Patterns { include, .. }) if include == &["*.body"])
        );
        let case = parse(
            &MINIMAL.replace("[expect]\n", "[expect]\nfiles_written = [\"a\"]\n"),
            "tests/cases/api-client/x_runs.toml",
        );
        assert!(
            matches!(&case.expect.files_written, Some(FilesWritten::Exact(files)) if files == &["a"])
        );
    }

    /// The whole-scenario file checks and the array/presence JSON forms the
    /// database cases read; a `[[runs]]` block cannot carry the file checks.
    #[test]
    fn copied_files_unchanged_files_and_absent_paths_are_read() {
        let case = parse(
            &MINIMAL
                .replace(
                    "[input]\n",
                    "[input]\ncopy_files = [{ from = \"a\", to = \"b\" }]\n",
                )
                .replace(
                    "[expect]\n",
                    "[expect]\nfiles_unchanged = [\"b\"]\npaths_absent = [\"b-journal\"]\nstderr_lines = 1\nstdout_json = { \"/ops\" = { has = \"query\" }, \"/schema\" = \"<object>\" }\n",
                ),
            "tests/cases/api-client/x_runs.toml",
        );
        assert_eq!(case.input.copy_files[0].to, "b");
        assert_eq!(case.expect.files_unchanged, ["b"]);
        assert_eq!(case.expect.paths_absent, ["b-journal"]);
        assert_eq!(case.expect.stderr_lines, Some(1));
        assert_eq!(
            case.expect.stdout_json["/ops"].expectation(),
            JsonExpect::Has(serde_json::json!("query"))
        );
        assert_eq!(
            case.expect.stdout_json["/schema"].expectation(),
            JsonExpect::AnyObject
        );
    }

    #[test]
    #[should_panic(expected = "belong in `[expect]`")]
    fn a_run_block_carries_no_whole_scenario_file_check() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[expect]\n[[expect.runs]]\nrun = 0\npaths_absent = [\"b\"]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "`[expect]` expects nothing")]
    fn a_case_without_expect_is_refused() {
        let text = MINIMAL.split("[expect]").next().unwrap();
        parse(text, "tests/cases/api-client/x_runs.toml");
    }

    #[test]
    #[should_panic(expected = "`[expect]` expects nothing")]
    fn an_expect_whose_keys_check_nothing_is_refused() {
        let text = MINIMAL.split("[expect]").next().unwrap().to_string()
            + "[expect]\nstderr_empty = false\nstdout_excludes = []\n";
        parse(&text, "tests/cases/api-client/x_runs.toml");
    }

    #[test]
    #[should_panic(expected = "`[expect.runs]` expects nothing")]
    fn a_run_block_that_names_only_its_run_is_refused() {
        parse(
            &format!("{MINIMAL}[[expect.runs]]\nrun = 0\n"),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "`[local.expect]` expects nothing")]
    fn a_layer_that_expects_nothing_is_refused() {
        parse(
            &MINIMAL.replace("[expect]\n", "[local]\nconfig = \"\"\n[expect]\n"),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    /// Every case file in the tree reads in this format: a key one branch
    /// spelled differently is refused here, before any binary runs.
    #[test]
    fn every_case_file_parses() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
        let mut count = 0;
        for feature in std::fs::read_dir(&root).expect("tests/cases/ exists") {
            let feature = feature.unwrap().path();
            if !feature.is_dir() {
                continue;
            }
            for file in std::fs::read_dir(&feature).unwrap() {
                let file = file.unwrap().path();
                if file.extension().is_some_and(|e| e == "toml") {
                    let relative = file
                        .strip_prefix(root.parent().unwrap().parent().unwrap())
                        .unwrap();
                    parse(
                        &std::fs::read_to_string(&file).unwrap(),
                        relative.to_str().unwrap(),
                    );
                    count += 1;
                }
            }
        }
        assert!(count > 0, "no case file under {}", root.display());
    }

    #[test]
    fn the_json_forms_and_the_data_keys_are_read() {
        let case = parse(
            &MINIMAL.replace(
                "error = { code = \"API_HTTP_ERROR\", exit = 4 }",
                "error = { code = \"X\", exit = 2, message = \"m\" }\nstderr_empty = true\nstdout = { line_words = [[\"a\", \"b\"]], aligned_under = [[\"H\", \"c\"]] }\napi_signed = { service = \"s3\", region = \"r\" }\napi_requests_matching = [{ method = \"GET\", path_excludes = \"list-type=\", count = 2 }]\nstdout_json = { \"/a\" = { contains = [\"x\", \"y\"] }, \"/b\" = { has = \"jsonl\" }, \"/c\" = { length = 2 }, \"/d\" = { min_length = 1 }, \"/e\" = { at_least = 1000 }, \"/f\" = \"<object>\", \"/g\" = \"<api_call_count>\", \"/h\" = \"<api_bytes_requested>\", \"/i\" = [[\"a\", \"<null>\"]] }\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
        let expect = &case.expect;
        assert_eq!(expect.error.as_ref().unwrap().message.as_deref(), Some("m"));
        assert!(expect.stderr_empty);
        let Some(Stdout::Detail(detail)) = &expect.stdout else {
            panic!("a detailed stdout")
        };
        assert_eq!(detail.line_words, [["a", "b"]]);
        assert_eq!(detail.aligned_under, [["H", "c"]]);
        assert_eq!(expect.api_signed.as_ref().unwrap().region, "r");
        assert_eq!(expect.api_requests_matching[0].count, 2);
        assert_eq!(
            expect.api_requests_matching[0].path_excludes.as_deref(),
            Some("list-type=")
        );
        let json = &expect.stdout_json;
        assert_eq!(
            json["/a"].expectation(),
            JsonExpect::Contains(vec!["x".into(), "y".into()])
        );
        assert_eq!(
            json["/b"].expectation(),
            JsonExpect::Has(serde_json::json!("jsonl"))
        );
        assert_eq!(json["/c"].expectation(), JsonExpect::Length(2));
        assert_eq!(json["/d"].expectation(), JsonExpect::MinLength(1));
        assert_eq!(json["/e"].expectation(), JsonExpect::AtLeast(1000.0));
        assert_eq!(json["/f"].expectation(), JsonExpect::AnyObject);
        assert_eq!(json["/g"].expectation(), JsonExpect::ApiCallCount);
        assert_eq!(json["/h"].expectation(), JsonExpect::ApiBytesRequested);
        assert_eq!(
            json["/i"].expectation(),
            JsonExpect::Equals(serde_json::json!([["a", null]]))
        );
    }

    #[test]
    fn a_placeholder_names_the_repository_and_a_key_may_name_an_option() {
        let case = parse(
            &MINIMAL
                .replace("command = \"api\"", "command = \"status\"\n\"status.kind\" = \"data\"")
                .replace(
                    "args = [\"api\", \"svc\", \"/items\"]",
                    "args = [\"data\", \"{repo}/tests/fixtures/data/orders.csv\", \"--export\", \"{home}/out.csv\"]",
                ),
            "tests/cases/api-client/x_runs.toml",
        );
        assert_eq!(case.combination["status.kind"], "data");
        let s = scenario(&case, None);
        assert_eq!(
            s.runs[0][1],
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/data/orders.csv"
            )
        );
        assert_eq!(s.runs[0][3], "{home}/out.csv");
    }

    #[test]
    fn an_sts_error_and_an_oauth_rejection_carry_their_values() {
        let case = parse(
            &MINIMAL.replace(
                "[fakes]\n",
                "[fakes]\nsts = { error = { code = \"AccessDenied\", message = \"no\" } }\noauth = { rejected = \"invalid_client\" }\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
        let s = scenario(&case, None);
        assert!(matches!(
            s.sts,
            StsFake::Error {
                code: "AccessDenied",
                message: "no"
            }
        ));
        assert!(matches!(
            s.oauth,
            OAuthFake::Rejected {
                error: "invalid_client"
            }
        ));
    }

    #[test]
    fn a_throwaway_layer_names_its_stacks_and_reads_the_run_through_placeholders() {
        let case = parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[throwaway]\nstacks = [\"iam-api\"]\nconfig = \"[api.svc]\\nbase_url = \\\"{iam_api_url}\\\"\\naws_profile = \\\"{throwaway_profile}\\\"\\n\"\n[throwaway.combination]\ntls = \"verify-full\"\n[throwaway.expect]\nexit_code = 0\n[expect]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
        let layer = case.throwaway.as_ref().expect("[throwaway] is read");
        assert_eq!(layer.stacks, ["iam-api"]);
        assert_eq!(layer.combination["tls"], "verify-full");
        assert_eq!(layer.expect.exit_code, Some(0));
        let expanded =
            expand_stack_placeholders(
                layer.config.as_deref().unwrap(),
                |variable| match variable {
                    "KURAMA_STACK_IAM_API_URL" => Some("https://r.example/v1".to_string()),
                    "KURAMA_THROWAWAY_PROFILE" => Some("kurama-sandbox".to_string()),
                    _ => None,
                },
            );
        assert_eq!(
            expanded,
            "[api.svc]\nbase_url = \"https://r.example/v1\"\naws_profile = \"kurama-sandbox\"\n"
        );
        // A variable the runner did not set leaves the placeholder as written.
        assert_eq!(
            expand_stack_placeholders("{rds_pg_host}", |_| None),
            "{rds_pg_host}"
        );
    }

    #[test]
    #[should_panic(expected = "`[throwaway]` needs `stacks`")]
    fn a_throwaway_layer_without_stacks_is_refused() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[throwaway]\nconfig = \"\"\nexpect = { exit_code = 4 }\n[expect]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "`[local]` takes no `stacks`")]
    fn a_local_layer_with_stacks_is_refused() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[local]\nstacks = [\"bastion\"]\nexpect = { exit_code = 4 }\n[expect]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    fn the_onepassword_section_is_what_a_real_run_takes_from_the_configuration() {
        let config = "[aws.session_cache]\nenabled = true\n\n[onepassword]\nenabled = true\nitem_name = \"aws-agent\"\n\n[api.x]\nbase_url = \"https://x\"\n";
        assert_eq!(
            crate::support::onepassword_section(config),
            "[onepassword]\nenabled = true\nitem_name = \"aws-agent\"\n\n"
        );
        assert_eq!(
            crate::support::onepassword_section("[api.x]\nbase_url = \"y\"\n"),
            ""
        );
    }

    #[test]
    #[should_panic(expected = "FilesWritten")]
    fn a_misspelt_files_written_pattern_is_refused() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[expect]\nfiles_written = { include = [\"a\"], exlude = [\"b\"] }\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "`expect.files_written` names no pattern")]
    fn an_empty_files_written_table_is_refused() {
        parse(
            &MINIMAL.replace("[expect]\n", "[expect]\nfiles_written = {}\n"),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "`expect.stdout` checks nothing")]
    fn an_empty_stdout_table_is_refused() {
        parse(
            &MINIMAL.replace("[expect]\n", "[expect]\nstdout = {}\n"),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    fn a_json_operator_with_a_key_it_does_not_take_is_a_plain_value() {
        let case = parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[expect]\nstdout_json = { \"/a\" = { min_length = 1, max_length = 3 } }\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
        assert_eq!(
            case.expect.stdout_json["/a"].expectation(),
            JsonExpect::Equals(serde_json::json!({ "min_length": 1, "max_length": 3 }))
        );
    }

    #[test]
    #[should_panic(expected = "only the fakes record")]
    fn a_real_layer_expecting_what_only_the_fakes_record_is_refused() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[real]\n[real.expect]\nop_calls = 0\n[expect]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "only the fakes record")]
    fn a_throwaway_run_expecting_what_only_the_fakes_record_is_refused() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[throwaway]\nstacks = [\"bastion\"]\n[[throwaway.expect.runs]]\nrun = 0\nstdout_json = { \"/n\" = \"<api_call_count>\" }\n[expect]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "every `[[throwaway.expect.runs]]` block names its `run`")]
    fn a_throwaway_runs_block_names_its_run() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[throwaway]\nstacks = [\"bastion\"]\n[[throwaway.expect.runs]]\nexit_code = 0\n[expect]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    #[test]
    #[should_panic(expected = "`throwaway.requires` holds `<kind>:<name>`")]
    fn a_throwaway_requirement_names_a_known_kind() {
        parse(
            &MINIMAL.replace(
                "[expect]\n",
                "[throwaway]\nstacks = [\"bastion\"]\nrequires = [\"bogus\"]\nexpect = { exit_code = 4 }\n[expect]\n",
            ),
            "tests/cases/api-client/x_runs.toml",
        );
    }

    /// `kurama agent`: a text document on stdout, nothing on stderr.
    const AGENT: &str = r#"
id = "x_runs"
feature = "agent-guide"
[input]
args = ["agent"]
[expect]
exit_code = 0
stdout_json = { "/x" = "<null>" }
stderr_lines_start_with = ["warning:"]
[local]
[local.expect]
exit_code = 0
stdout = { equals = "not the guide" }
"#;

    #[test]
    fn a_document_that_is_not_json_and_an_empty_stderr_satisfy_no_expectation() {
        let failures = verification(AGENT, "tests/cases/agent-guide/x_runs.toml").failed_checks();
        assert!(
            failures
                .iter()
                .any(|line| line.starts_with("expect.stdout_json[/x]: ")),
            "{failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|line| line.starts_with("expect.stderr_lines_start_with: ")),
            "{failures:?}"
        );
    }

    #[test]
    fn a_layer_other_than_fake_reports_the_length_of_stdout_not_its_text() {
        let v = verification_on(AGENT, "tests/cases/agent-guide/x_runs.toml", Some("local"));
        let stdout = v.observed.runs[0].stdout.clone();
        let failures = v.failed_checks();
        let equals = failures
            .iter()
            .find(|line| line.starts_with("expect.stdout.equals: "))
            .unwrap_or_else(|| panic!("{failures:?}"));
        assert!(
            equals.contains(&format!(
                "<{} bytes of stdout, not shown on the local layer>",
                stdout.len()
            )),
            "{equals}"
        );
        let first_line = stdout.lines().find(|line| line.len() > 20).unwrap();
        assert!(
            failures.iter().all(|line| !line.contains(first_line)),
            "{failures:?}"
        );
    }

    #[test]
    fn a_real_run_that_reaches_the_fake_server_fails() {
        let case = r#"
id = "x_runs"
feature = "api-client"
[input]
args = ["api", "svc", "/items"]
config = "[api.svc]\nbase_url = \"{server}/api\"\n"
[expect]
exit_code = 0
[real]
[real.expect]
exit_code = 0
"#;
        let failures = verification_on(case, "tests/cases/api-client/x_runs.toml", Some("real"))
            .failed_checks();
        assert!(
            failures
                .iter()
                .any(|line| line.starts_with("a real run reaches no fake endpoint: ")),
            "{failures:?}"
        );
    }
}
