//! Runtime verification harness for `tests/scenarios/`.
//!
//! A scenario runs the real `kurama` binary the way a user or an agent
//! would, inside an isolated sandbox:
//!
//! - `HOME`, `TMPDIR`, the AWS config/credentials files and the kurama
//!   config file all live in a temporary directory.
//! - STS, the console federation endpoint, an OAuth authorization server
//!   (`/oauth/token`, `/oauth/device`, OpenID Connect discovery) and an API
//!   (`/api/...`) are one local fake (`wiremock`) reached through
//!   `AWS_ENDPOINT_URL_STS` / `KURAMA_FEDERATION_ENDPOINT` and, for OAuth and
//!   the API, through `{server}` in the kurama config; every request is
//!   captured and decoded. A SigV4 signature on an API request is checked
//!   by recomputing it from the request as
//!   received, with the role credentials the fake STS handed out.
//! - An OpenAPI description (`/spec/<fixture>`, from `tests/fixtures/openapi/`)
//!   is served with an `ETag` and answers 304 to a matching `If-None-Match`,
//!   or 503 when the scenario says the server is down.
//! - 1Password is `tests/fakes/op`, the browser is `tests/fakes/open`, the
//!   clipboard is `tests/fakes/pbcopy` (and `xclip`) and `$EDITOR` is
//!   `tests/fakes/editor` and the Obsidian CLI is `tests/fakes/obsidian`
//!   (answering from `tests/fixtures/obsidian-vault`); each logs what it was
//!   given instead of doing
//!   anything (the editor also replaces the file with a fixed body). The fake
//!   browser answers an OAuth authorization request by sending the redirect
//!   with a code.
//! - The keychain is never touched. Scenarios that need a session cache or a
//!   token store use the file backends compiled in with `--features test-fakes`.
//!
//! A scenario may run the binary several times in the same sandbox
//! (`then_run`) to observe state carried between processes. Whatever a
//! scenario asserts, the harness also fails it when role credentials land in
//! a file or a successful run logs an error. After the run the harness
//! reports everything it observed and writes one JSON file per scenario under
//! `target/agent/scenarios/` so `cargo xtask verify` can assemble a
//! verification report.
//!
//! `tui` drives the binary through a pseudo terminal (keys, resize, screen
//! capture) inside the same sandbox; `png` draws a captured screen.

pub mod cases;
pub mod configs;
pub mod data;
pub mod expect_detail;
pub mod mcp_http;
pub mod png;
pub mod s3_browse;
pub mod tui;

pub use expect_detail::ApiCallExpect;

// The fixtures read by more than one feature's scenarios are imported the way
// the constants declared here are, so a scenario file has one import path.
pub use configs::*;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use sha2::{Digest, Sha256};
use wiremock::matchers::{body_string_contains, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Long-term key stored in the sandbox `~/.aws/credentials` (no 1Password).
pub const SOURCE_ACCESS_KEY: &str = "AKIASOURCEKEY000001";
pub const SOURCE_SECRET: &str = "source-secret-key";
/// Long-term key served by `tests/fakes/op`.
pub const OP_ACCESS_KEY: &str = "AKIAOPSOURCEKEY";
pub const OP_SECRET: &str = "op-source-secret";
pub const OP_TOTP: &str = "123456";
/// The code in the JSON of the item, which only the fallback reads.
pub const OP_JSON_TOTP: &str = "654321";
/// MFA session returned by the fake GetSessionToken. It may be cached by the
/// test-only file backend, so it is deliberately distinct from `RESULT_*`.
pub const SESSION_ACCESS_KEY: &str = "ASIASESSIONKEY00001";
/// The key of the second session the fake GetSessionToken hands out in one
/// scenario: each call gets a new one, so a run can show which it signed with.
pub const SECOND_SESSION_ACCESS_KEY: &str = "ASIASESSIONKEY00002";
pub const SESSION_SECRET: &str = "session-secret-key";
pub const SESSION_TOKEN: &str = "session-token";
/// Role credentials returned by the fake AssumeRole. These must only ever
/// appear on stdout, never in a file.
pub const RESULT_ACCESS_KEY: &str = "ASIARESULTKEY000001";
pub const RESULT_SECRET: &str = "result-secret-key";
pub const RESULT_TOKEN: &str = "result-session-token";
pub const SIGNIN_TOKEN: &str = "fake-signin-token";
/// Access token the fake authorization server issues for a grant.
pub const GRANTED_ACCESS_TOKEN: &str = "fake-granted-access-token";
/// Access token the fake authorization server issues for a refresh.
pub const REFRESHED_ACCESS_TOKEN: &str = "fake-refreshed-access-token";
/// Refresh token that comes with every granted fake token; the fake token
/// endpoint accepts only it (or the rotated one) for a refresh.
pub const FAKE_REFRESH_TOKEN: &str = "fake-refresh-token";
/// Refresh token a refresh answer carries: the authorization server rotates it.
pub const ROTATED_REFRESH_TOKEN: &str = "fake-rotated-refresh-token";
/// Every token the fakes hand out or a scenario stores; none may land in a
/// file other than the test-only token store.
const OAUTH_SECRETS: [&str; 4] = [
    GRANTED_ACCESS_TOKEN,
    REFRESHED_ACCESS_TOKEN,
    FAKE_REFRESH_TOKEN,
    ROTATED_REFRESH_TOKEN,
];
/// What every value the fake Secrets Manager and Parameter Store hand out
/// begins with. The id decides the rest, so the prefix is what catches any of
/// them wherever one lands on disk.
const AWS_SECRET_PREFIXES: [&str; 4] = [
    "password-of-",
    "user-of-",
    "parameter-of/",
    "plain-secret-of-",
];
/// Client secret `tests/fakes/op` prints for `op read`; with a service
/// account token it prints this followed by `-for-<token>`.
pub const FAKE_CLIENT_SECRET: &str = "fake-client-secret";
/// What `tests/fakes/op` hands out that no file may hold. Its TOTP is six
/// digits any file could contain by chance, so the scan cannot look for it.
const OP_SECRETS: [&str; 3] = [FAKE_CLIENT_SECRET, OP_ACCESS_KEY, OP_SECRET];
/// Code the fake browser sends to the loopback redirect.
pub const FAKE_AUTH_CODE: &str = "fake-auth-code";
/// User code of the fake device flow.
pub const FAKE_USER_CODE: &str = "ABCD-1234";

/// Upper bound for one run; a hung binary fails the scenario instead of the suite.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// The keychain service a scenario's service account token lives under.
const SERVICE_ACCOUNT_KEYCHAIN: &str = "kurama-op-service-account";
/// The account it lives under, as `security -a "$USER"` would have written it.
const SERVICE_ACCOUNT_USER: &str = "scenario-user";

pub const AWS_CONFIG: &str = "\
[default]
region = us-east-1

[profile dev]
role_arn = arn:aws:iam::123456789012:role/Dev
source_profile = default
region = ap-northeast-1

[profile ops-mfa]
role_arn = arn:aws:iam::123456789012:role/Ops
source_profile = default
mfa_serial = arn:aws:iam::123456789012:mfa/agent
region = ap-northeast-1
";

/// What the fake STS answers.
#[derive(Clone, Copy)]
pub enum StsFake {
    /// Every operation succeeds (`SESSION_*` for GetSessionToken, `RESULT_*`
    /// for AssumeRole).
    Success,
    /// Every operation fails with the given AWS error code and message.
    Error {
        code: &'static str,
        message: &'static str,
    },
    /// As `Success`, but AssumeRole credentials end this many seconds after
    /// each answer, so a long session has to assume the role again.
    ExpiresIn(i64),
}

/// How the fake 1Password CLI behaves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OnePassword {
    /// `[onepassword] enabled = false`: manual MFA, which fails without a TTY.
    Disabled,
    /// The fake serves keys, TOTP codes and `op read` secrets.
    Enabled,
    /// The fake answers like the real CLI before `op signin`.
    NotSignedIn,
    /// The fake blocks the way the real CLI does while a person is asked to
    /// approve the read.
    Hangs,
    /// Only `op --version` blocks: the availability probe, which every other
    /// mode leaves alone.
    VersionHangs,
    /// Only `op item get --otp` fails, so the TOTP comes from the item's JSON.
    OtpFails,
}

/// What the fake authorization server (`/oauth/token`) answers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OAuthFake {
    /// Every grant gets `GRANTED_ACCESS_TOKEN`, a refresh
    /// `REFRESHED_ACCESS_TOKEN`; the device flow is pending on its first poll.
    Success,
    /// Every token request fails with this OAuth error.
    Rejected { error: &'static str },
    /// The server answers 500.
    Unavailable,
    /// The device authorization answers this lifetime and polling interval,
    /// and every device poll is pending.
    DevicePending { expires_in: u64, interval: u64 },
}

/// What the fake API (`/api/...`) answers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ApiFake {
    /// Echo the request with the requested number of padding bytes.
    Padding(usize),
    /// 200 with a JSON echo of the request (path, method, authorization, body, login).
    Ok,
    /// 401 on the first request, 200 afterwards.
    UnauthorizedOnce,
    /// 401 always.
    Unauthorized,
    /// 404 with a JSON message.
    NotFound,
    /// 200 after a delay longer than a short `--timeout`.
    Slow,
    /// 200 with the requested path inside a compact one-line JSON array
    /// whose `10.00` a re-serialized value would print as `10.0`.
    CompactArray,
    /// The request body's own bytes back as `application/octet-stream`,
    /// with the status the `status` query parameter names (200 without
    /// one) after the seconds `delay` names (none without one); with a
    /// `set_cookies` parameter, one `Set-Cookie` for each line of the body,
    /// in order.
    Echo,
    /// A paged listing built from the query it receives: `pages` pages
    /// (3 without one) in the `style` it names. `link` answers page `page`
    /// with a relative `Link` to `page + 1` (`rel="prev"` only on the last,
    /// none at all for a one-page listing); `cursor` answers the page the
    /// `cursor` parameter `c<N>` names with `meta.next` = `c<N+1>` (null on
    /// the last); `none` has no marker. The page `fail` names answers 500;
    /// with `bare_last` the last page carries no marker at all.
    /// Every body is `{"page", "items"}` of that page.
    Pages,
    /// A GraphQL endpoint: 200 with `{"data": {"query", "variables"}}`, the
    /// document and the variables it received; when the variables carry an
    /// `id` starting with `missing`, 200 with `data: null` and two
    /// `errors`, the first naming that id at the path `["issue"]`.
    GraphQl,
}

/// What the fake Secrets Manager and Parameter Store answer. Every answer is
/// built from the id it was asked for, so a scenario can say which secret a
/// value came from instead of trusting a constant -- and one mounted fake
/// answers every shape, because the id says which one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AwsSecrets {
    /// Nothing is mounted: a request would 404, so a scenario that expects no
    /// read fails loudly if one happens.
    Absent,
    /// The id decides the shape. A `GetParameter` answers the name it was
    /// given; a `GetSecretValue` answers a managed-secret JSON document with
    /// `username` and `password`, except for an id ending in `-text`
    /// (a `SecretString` that is not JSON) or `-binary` (`SecretBinary`).
    FromId,
    /// Every read is refused, the way a role without the action is refused.
    AccessDenied,
    /// Every read is answered with the store's own "no such name" code:
    /// `ParameterNotFound` or `ResourceNotFoundException`.
    NotFound,
}

/// What the fake description endpoint (`/spec/<fixture>`) answers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpecFake {
    /// 200 with the fixture, an `ETag` and a `Last-Modified`; 304 when the
    /// request carries the `ETag` back.
    Etag,
    /// 503 always: the server is down.
    Down,
}

/// One marker for both entry paths; PTY children do not expose separate streams/statuses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CompletionSession {
    Direct,
    RealZsh,
}

/// Executable specification of one user-visible behavior.
pub struct Scenario {
    pub id: &'static str,
    pub feature: &'static str,
    /// One argument list per run; all runs share the sandbox and the fakes.
    /// For a TUI scenario these runs prepare the sandbox (for example a
    /// `login`) before the TUI is launched.
    pub runs: Vec<&'static [&'static str]>,
    pub sts: StsFake,
    pub onepassword: OnePassword,
    /// `[aws.session_cache] enabled = true` with the file backend.
    pub session_cache: bool,
    /// Put `tests/fakes` on PATH so `open` / `xdg-open` are fakes, and wait
    /// for the fake browser's log after each run.
    pub browser: bool,
    /// Put `tests/fakes` on PATH for `pbcopy` / `xclip` and `editor`.
    pub tools: bool,
    /// The fake federation endpoint answers 500 instead of a sign-in token.
    pub federation_fails: bool,
    /// The fake federation endpoint answers 200 with a body that is not JSON.
    pub federation_invalid_json: bool,
    /// The file-backed session cache holds invalid JSON before the first run.
    pub corrupt_session_cache: bool,
    /// Replaces the generated kurama config file (`{server}` is the fake server).
    pub config_override: Option<String>,
    /// TOML appended to the generated kurama config file (`{server}` is the
    /// fake server).
    pub extra_config: Vec<String>,
    /// Replaces the sandbox `~/.aws/config`.
    pub aws_config: &'static str,
    /// Set `KURAMA_ENV_SCRIPT` to a file in the sandbox, as the zsh wrapper does.
    pub env_script: bool,
    /// The hand-off file already exists with these permission bits before
    /// the first run, holding a stale line.
    pub existing_env_script_mode: Option<u32>,
    completion_session: Option<CompletionSession>,
    home_files: Vec<(&'static str, &'static [u8])>,
    /// Set through `with_env` so completion cannot bypass its session marker.
    env: Vec<(&'static str, &'static str)>,
    pub stdin: Option<String>,
    pub oauth: OAuthFake,
    pub api: ApiFake,
    /// Use the file-backed token store.
    pub token_store: bool,
    /// Tokens in the store before the first run: `(auth name, token JSON)`.
    pub stored_tokens: Vec<(&'static str, String)>,
    /// The file-backed token store holds invalid JSON before the first run.
    pub corrupt_token_store: bool,
    /// After `runs`, start this many processes with these arguments at once
    /// and record them as one run.
    pub parallel: Option<(usize, &'static [&'static str])>,
    pub spec: SpecFake,
    /// A description already in `~/.cache/kurama/openapi/` before the first
    /// run: `(path on the fake server, fixture file, age in seconds)`, cached
    /// with the same ETag the fixture endpoint returns. A negative age seeds a future timestamp.
    pub seeded_spec: Option<(&'static str, &'static str, i64)>,
    pub corrupt_spec_metadata: bool,
    /// The config names a keychain entry in `[onepassword]
    /// service_account_keychain`.
    pub service_account_keychain: bool,
    /// The token seeded into that entry; `None` leaves it absent, which is
    /// the first run on a build the keychain has not granted access to.
    pub service_account_token: Option<&'static str>,
    /// What the fake Secrets Manager and Parameter Store answer.
    pub aws_secrets: AwsSecrets,
    /// Talk to AWS itself: no fake STS, S3, SSM, Secrets Manager or
    /// federation endpoint, the person's own `~/.aws` files and `[onepassword]`
    /// section, the real PATH. The sandbox HOME, config file and token store
    /// stay isolated. What `cargo xtask verify --layer throwaway` sets, and
    /// nothing else: every fake-side expectation on STS or the API is empty.
    pub real_aws: bool,
    /// Requests sent once the first run, `kurama mcp --listen`, listens; it
    /// is stopped after the last answer.
    pub http: Vec<mcp_http::HttpRequest>,
}

impl Scenario {
    pub fn serving_http(mut self, requests: Vec<mcp_http::HttpRequest>) -> Self {
        self.http = requests;
        self
    }
    pub fn with_stdin(mut self, input: String) -> Self {
        self.stdin = Some(input);
        self
    }
    pub fn with_real_aws(mut self) -> Self {
        self.real_aws = true;
        self
    }
    pub fn new(id: &'static str, feature: &'static str, args: &'static [&'static str]) -> Self {
        Self {
            id,
            feature,
            runs: vec![args],
            sts: StsFake::Success,
            onepassword: OnePassword::Disabled,
            session_cache: false,
            browser: false,
            tools: false,
            federation_fails: false,
            federation_invalid_json: false,
            corrupt_session_cache: false,
            config_override: None,
            extra_config: Vec::new(),
            aws_config: AWS_CONFIG,
            env_script: false,
            existing_env_script_mode: None,
            env: Vec::new(),
            completion_session: None,
            home_files: Vec::new(),
            stdin: None,
            oauth: OAuthFake::Success,
            api: ApiFake::Ok,
            token_store: false,
            stored_tokens: Vec::new(),
            corrupt_token_store: false,
            parallel: None,
            spec: SpecFake::Etag,
            seeded_spec: None,
            corrupt_spec_metadata: false,
            service_account_keychain: false,
            service_account_token: None,
            aws_secrets: AwsSecrets::Absent,
            real_aws: false,
            http: Vec::new(),
        }
    }

    /// Mount the fake Secrets Manager and Parameter Store. Without this, a
    /// read of either is a request nothing answers.
    pub fn aws_secrets(mut self, fake: AwsSecrets) -> Self {
        self.aws_secrets = fake;
        self
    }

    /// Name the keychain entry in the config; `None` leaves it unseeded, so
    /// kurama looks up an entry it was told about and does not find.
    pub fn with_service_account_keychain(mut self, token: Option<&'static str>) -> Self {
        self.service_account_keychain = true;
        self.service_account_token = token;
        self
    }

    /// A direct zsh completion request, before configuration/logging/bootstrap.
    pub fn completion(
        id: &'static str,
        args: &'static [&'static str],
        index: &'static str,
    ) -> Self {
        Self::new(id, "shell-integration", args)
            .with_fake_tools()
            .with_env("COMPLETE", "zsh")
            .with_env("_CLAP_COMPLETE_INDEX", index)
            .with_env("_CLAP_IFS", "\n")
    }

    /// A scenario for `tui::launch`: no CLI run of its own; `then_run` adds
    /// preparation runs.
    pub fn tui(id: &'static str) -> Self {
        let mut scenario = Self::new(id, "tui", &[]);
        scenario.runs.clear();
        scenario
    }

    /// Report the scenario under another feature (an explorer scenario on
    /// the terminal belongs to `api-explorer`).
    pub fn for_feature(mut self, feature: &'static str) -> Self {
        self.feature = feature;
        self
    }

    pub fn then_run(mut self, args: &'static [&'static str]) -> Self {
        self.runs.push(args);
        self
    }

    pub fn sts(mut self, sts: StsFake) -> Self {
        self.sts = sts;
        self
    }

    pub fn onepassword(mut self, mode: OnePassword) -> Self {
        self.onepassword = mode;
        self
    }

    pub fn with_session_cache(mut self) -> Self {
        self.session_cache = true;
        self
    }

    pub fn with_fake_browser(mut self) -> Self {
        self.browser = true;
        self
    }

    pub fn with_failing_federation(mut self) -> Self {
        self.federation_fails = true;
        self
    }

    pub fn with_corrupt_session_cache(mut self) -> Self {
        self.session_cache = true;
        self.corrupt_session_cache = true;
        self
    }

    pub fn with_config(mut self, toml: &str) -> Self {
        self.config_override = Some(toml.to_string());
        self
    }

    pub fn with_aws_config(mut self, config: &'static str) -> Self {
        self.aws_config = config;
        self
    }

    pub fn with_env_script(mut self) -> Self {
        self.env_script = true;
        self
    }

    /// [`Scenario::with_env_script`] on a file that already exists with
    /// `mode`, as a caller that made the file itself would hand it over.
    pub fn with_existing_env_script(mut self, mode: u32) -> Self {
        self.env_script = true;
        self.existing_env_script_mode = Some(mode);
        self
    }

    pub fn with_env(mut self, name: &'static str, value: &'static str) -> Self {
        if (name, value) == ("COMPLETE", "zsh") {
            self.completion_session = Some(CompletionSession::Direct);
        }
        self.env.push((name, value));
        self
    }

    /// Seed a relative HOME path before the file-write baseline is captured.
    pub fn with_home_file(mut self, path: &'static str, content: &'static str) -> Self {
        self.home_files.push((path, content.as_bytes()));
        self
    }

    /// [`Scenario::with_home_file`] for content that is not UTF-8.
    pub fn with_home_bytes(mut self, path: &'static str, content: &'static [u8]) -> Self {
        self.home_files.push((path, content));
        self
    }

    /// Append TOML to the generated config; `{server}` is the fake server URI.
    pub fn with_extra_config(mut self, toml: &str) -> Self {
        self.extra_config.push(toml.to_string());
        self
    }

    pub fn oauth(mut self, fake: OAuthFake) -> Self {
        self.oauth = fake;
        self
    }

    pub fn api(mut self, fake: ApiFake) -> Self {
        self.api = fake;
        self
    }

    pub fn with_token_store(mut self) -> Self {
        self.token_store = true;
        self
    }

    /// A token in the store before the first run (implies the token store).
    pub fn with_stored_token(mut self, name: &'static str, token_json: &str) -> Self {
        self.token_store = true;
        self.stored_tokens.push((name, token_json.to_string()));
        self
    }

    pub fn with_corrupt_token_store(mut self) -> Self {
        self.token_store = true;
        self.corrupt_token_store = true;
        self
    }

    /// After the sequential runs, run `args` in `count` processes at once.
    pub fn with_parallel_runs(mut self, count: usize, args: &'static [&'static str]) -> Self {
        self.parallel = Some((count, args));
        self
    }

    pub fn spec(mut self, fake: SpecFake) -> Self {
        self.spec = fake;
        self
    }

    /// The fixture is in the description cache before the first run, as if
    /// fetched from `path` on the fake server earlier.
    pub fn with_seeded_spec_cache(
        mut self,
        path: &'static str,
        fixture: &'static str,
        age_seconds: i64,
    ) -> Self {
        self.seeded_spec = Some((path, fixture, age_seconds));
        self
    }

    /// Corrupt the metadata of the seeded description before the first run.
    pub fn with_corrupt_spec_metadata(mut self) -> Self {
        self.corrupt_spec_metadata = true;
        self
    }

    /// Put `tests/fakes` on PATH: `pbcopy` / `xclip` (and `editor`) are
    /// fakes that log instead of acting.
    pub fn with_fake_tools(mut self) -> Self {
        self.tools = true;
        self
    }
}

/// `tests/fixtures/openapi`.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/openapi")
}

/// `tests/fakes/editor`, for `EDITOR`.
pub const FAKE_EDITOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fakes/editor");

/// What `--fingerprint` prints for `value`, computed here rather than by the
/// binary, so a check against it says the binary hashed the value itself.
pub fn sha256_fingerprint(value: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(value.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

/// The keys of the file token store, as `Observed::token_store` reads them.
fn stored_sources(path: &Path) -> Option<Vec<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&content)
            .ok()
            .map(|tokens| tokens.into_keys().collect()),
        Err(_) => Some(Vec::new()),
    }
}

/// Token JSON for `with_stored_token`: usable when `expires_in_secs` is
/// positive, expired otherwise; a refresh token when `refreshable`.
pub fn stored_token_json(access_token: &str, expires_in_secs: i64, refreshable: bool) -> String {
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in_secs);
    let mut token = serde_json::json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_at": expires_at.to_rfc3339(),
    });
    if refreshable {
        token["refresh_token"] = serde_json::Value::from(FAKE_REFRESH_TOKEN);
    }
    token.to_string()
}

/// One decoded STS request (AWS Query protocol: form-encoded POST body).
#[derive(Debug, Clone, serde::Serialize)]
pub struct StsCall {
    pub action: String,
    /// Access key id from the SigV4 `Authorization` header: which long-term
    /// or session credentials signed this call.
    pub signing_access_key: Option<String>,
    /// Region of the same credential scope: where STS was called, which is
    /// the AWS profile's region and not where a bucket lives.
    pub signing_region: Option<String>,
    pub role_arn: Option<String>,
    pub serial_number: Option<String>,
    pub token_code: Option<String>,
    pub policy_arns: Vec<String>,
    pub duration_seconds: Option<String>,
    pub role_session_name: Option<String>,
}

impl Run {
    /// The bytes the run asked the fake API for: each GET's `Range`, or
    /// `whole_object` for a GET that carried none. The same arithmetic as the
    /// binary's `bytes_transferred`, from the other side: what the server was
    /// asked for rather than what the engine logged about itself.
    pub fn api_bytes_requested(&self, whole_object: u64) -> u64 {
        self.api_calls
            .iter()
            .filter(|call| call.method == "GET")
            .map(|call| {
                match call.headers.get("range").and_then(|range| {
                    let (from, to) = range.trim_start_matches("bytes=").split_once('-')?;
                    Some((from.parse::<u64>().ok()?, to.parse::<u64>().ok()?))
                }) {
                    Some((from, to)) => to - from + 1,
                    None => whole_object,
                }
            })
            .sum()
    }
}

/// One console federation request (`Action=getSigninToken`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct FederationCall {
    pub action: Option<String>,
    /// Access key id inside the `Session` JSON parameter.
    pub session_access_key: Option<String>,
}

/// One request to the fake authorization server: the token endpoint, the
/// device authorization endpoint or the discovery document.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OAuthCall {
    pub method: String,
    pub path: String,
    /// `grant_type` of a token request.
    pub grant_type: Option<String>,
    /// The decoded form of a token or device request.
    pub params: BTreeMap<String, String>,
}

/// One request to the fake API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiCall {
    pub method: String,
    /// Path and query.
    pub path: String,
    pub authorization: Option<String>,
    /// Every request header, names lower-cased.
    pub headers: BTreeMap<String, String>,
    pub body: String,
    /// The SigV4 signature, when the `Authorization` header carries one.
    pub sigv4: Option<Sigv4Signature>,
}

/// One request for the API description on the fake server.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpecCall {
    pub method: String,
    pub path: String,
    /// `If-None-Match` / `If-Modified-Since` were sent.
    pub conditional: bool,
    pub authorization: Option<String>,
    /// What the fake answered (200, 304, 503).
    pub status: u16,
}

/// A decoded `AWS4-HMAC-SHA256` signature and whether it matches the
/// request it came with, signed with the fake AssumeRole credentials.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Sigv4Signature {
    pub access_key: String,
    pub date: String,
    pub region: String,
    pub service: String,
    pub signed_headers: Vec<String>,
    pub security_token: Option<String>,
    pub valid: bool,
    pub reason: Option<String>,
}

/// `Sigv4Signature` from the `Authorization` and `x-amz-date` headers; the
/// signature is recomputed over the received method, URL, signed headers
/// and body with `RESULT_*`, the only credentials a signature may carry.
fn decode_sigv4(request: &Request) -> Option<Sigv4Signature> {
    let header = |name: &str| {
        request
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let parameters = header("authorization")?.strip_prefix("AWS4-HMAC-SHA256 ")?;
    let decoded = (|| -> Result<Sigv4Signature, &'static str> {
        let parameter = |name: &str| {
            parameters
                .split(',')
                .map(str::trim)
                .find_map(|part| part.strip_prefix(name))
                .map(str::to_string)
        };
        let credential = parameter("Credential=").ok_or("missing Credential")?;
        let [access_key, date, region, service, _] = credential.split('/').collect::<Vec<_>>()[..]
        else {
            return Err("invalid credential scope");
        };
        let signed_headers: Vec<String> = parameter("SignedHeaders=")
            .ok_or("missing SignedHeaders")?
            .split(';')
            .map(str::to_string)
            .collect();
        let signature = parameter("Signature=").ok_or("missing Signature")?;
        let time = chrono::NaiveDateTime::parse_from_str(
            header("x-amz-date").ok_or("missing x-amz-date")?,
            "%Y%m%dT%H%M%SZ",
        )
        .map_err(|_| "invalid x-amz-date")?
        .and_utc();
        let identity = aws_credential_types::Credentials::new(
            RESULT_ACCESS_KEY,
            RESULT_SECRET,
            Some(RESULT_TOKEN.to_string()),
            None,
            "scenario",
        )
        .into();
        let mut settings = SigningSettings::default();
        if matches!(service, "s3" | "aoss") {
            settings.payload_checksum_kind =
                aws_sigv4::http_request::PayloadChecksumKind::XAmzSha256;
        }
        if service == "s3" {
            settings.uri_path_normalization_mode =
                aws_sigv4::http_request::UriPathNormalizationMode::Disabled;
            settings.percent_encoding_mode = aws_sigv4::http_request::PercentEncodingMode::Single;
        }
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region(region)
            .name(service)
            .time(time.into())
            .settings(settings)
            .build()
            .map_err(|_| "invalid signing parameters")?
            .into();
        let headers: Vec<(String, String)> = signed_headers
            .iter()
            .map(|name| {
                let values: Result<Vec<_>, _> = request
                    .headers
                    .get_all(name)
                    .iter()
                    .map(|v| v.to_str())
                    .collect();
                values.map(|values| (name.clone(), values.join(",")))
            })
            .collect::<Result<_, _>>()
            .map_err(|_| "invalid signed header")?;
        let signable = SignableRequest::new(
            request.method.as_str(),
            request.url.as_str(),
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            if header("x-amz-content-sha256") == Some("UNSIGNED-PAYLOAD") {
                SignableBody::UnsignedPayload
            } else {
                SignableBody::Bytes(&request.body)
            },
        )
        .map_err(|_| "invalid signable request")?;
        let expected = sign(signable, &params)
            .map_err(|_| "signature calculation failed")?
            .signature()
            .to_string();
        let valid = access_key == RESULT_ACCESS_KEY && expected == signature;
        Ok(Sigv4Signature {
            access_key: access_key.to_string(),
            date: date.to_string(),
            region: region.to_string(),
            service: service.to_string(),
            signed_headers,
            security_token: header("x-amz-security-token").map(str::to_string),
            valid,
            reason: (!valid).then(|| "signature mismatch".into()),
        })
    })();
    Some(decoded.unwrap_or_else(|reason| Sigv4Signature {
        reason: Some(reason.into()),
        ..Sigv4Signature::default()
    }))
}

/// What one process invocation did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Run {
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// For a TUI run: what the terminal showed after the TUI exited.
    #[serde(skip)]
    pub stdout: String,
    pub stderr: String,
    pub sts_calls: Vec<StsCall>,
    /// The Systems Manager operations asked for, by `x-amz-target`.
    pub ssm_calls: Vec<String>,
    /// The secrets read from Secrets Manager and Parameter Store, in order.
    pub secret_calls: Vec<SecretCall>,
    pub federation_calls: Vec<FederationCall>,
    pub oauth_calls: Vec<OAuthCall>,
    pub api_calls: Vec<ApiCall>,
    pub spec_calls: Vec<SpecCall>,
    pub op_calls: Vec<String>,
    pub open_calls: Vec<String>,
    /// The Obsidian CLI argv of each call, its arguments joined by a tab.
    pub obsidian_calls: Vec<String>,
    /// Texts handed to the fake clipboard.
    pub clipboard_calls: Vec<String>,
    /// The file contents handed to the fake `$EDITOR`.
    pub editor_calls: Vec<String>,
    /// Environment variable names exported on stdout (values are secrets).
    pub exported_variables: Vec<String>,
    /// What a `kurama mcp --listen` run answered the requests sent to it.
    pub http_responses: Vec<mcp_http::HttpResponse>,
    /// For a server that listened: whether it was still serving when the
    /// harness stopped it after the last answer.
    pub stopped_while_serving: Option<bool>,
}

/// One read of a secret AWS holds: which store, and which secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecretCall {
    /// `secretsmanager:GetSecretValue` or `ssm:GetParameter`.
    pub operation: String,
    /// The secret id or parameter name the request asked for.
    pub id: String,
    /// The region of the SigV4 credential scope: where the secret was read.
    /// Both stores are mounted on one server, so the request itself is the
    /// only thing that says which region kurama meant.
    pub region: Option<String>,
    /// The key that signed it, which has to be the assumed role's
    /// (`RESULT_ACCESS_KEY`) and never the source profile's.
    pub signing_access_key: Option<String>,
    /// `WithDecryption` of a `GetParameter` request; `None` when not sent.
    pub with_decryption: Option<bool>,
}

/// The hand-off file `KURAMA_ENV_SCRIPT` names, under the sandbox's `tmp`.
const ENV_SCRIPT_FILE: &str = "kurama-env.sh";

/// The file `KURAMA_ENV_SCRIPT` named, when a run wrote it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvScript {
    pub path: String,
    /// Permission bits, expected `0o600`.
    pub mode: u32,
    pub exported_variables: Vec<String>,
}

/// One captured TUI screen: `screen.txt`, `screen.ansi` and `screen.png`
/// under `dir`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScreenRecord {
    pub step: String,
    pub cols: u16,
    pub rows: u16,
    pub dir: String,
}

/// Everything the scenario could observe, across all runs.
#[derive(Debug, serde::Serialize)]
pub struct Observed {
    pub runs: Vec<Run>,
    /// Sandbox files created or modified by any run (relative to the sandbox),
    /// excluding the fakes' own logs.
    pub files_written: Vec<String>,
    /// Sandbox files that contain a `RESULT_*` credential, a token or a secret
    /// the fakes hand out, or a stored token; the env script hand-off file and
    /// the test-only token store are excluded.
    pub secrets_on_disk: Vec<String>,
    /// Sandbox paths the secret scan could not list or read: each one is a
    /// file that may hold a secret nobody looked at.
    pub unscanned: Vec<String>,
    pub env_script: Option<EnvScript>,
    pub screens: Vec<ScreenRecord>,
    /// The sources the test-only token store holds after the last run, in
    /// name order (none when the file does not exist); `None` when the file
    /// is not a JSON object of tokens.
    pub token_store: Option<Vec<String>>,
}

/// What the harness itself put in place before the run. Kept apart from
/// `Observed` on purpose: a check built only from these values tests the
/// harness's own arithmetic and passes whatever the binary does. If a check
/// reads `seeded` it must compare it against something from `observed`.
#[derive(Debug, serde::Serialize)]
pub struct Seeded {
    /// The timestamp written into the description cache, so a scenario can
    /// say what the warning line should contain and how old the copy is.
    pub spec_fetched_at: Option<String>,
}

impl Observed {
    /// The last run; most scenarios have exactly one.
    pub fn last(&self) -> &Run {
        self.runs.last().expect("at least one run")
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct Check {
    name: String,
    ok: bool,
    detail: String,
}

/// Collects checks against an [`Observed`] scenario and writes the report.
pub struct Verification {
    id: &'static str,
    feature: &'static str,
    /// The coordinates of the combination a TOML case verifies
    /// (`tests/support/cases.rs`); a Rust scenario declares none.
    combination: Option<BTreeMap<String, String>>,
    /// What the run was observed against: every scenario in this binary
    /// runs against the fakes.
    evidence: &'static str,
    pub observed: Observed,
    pub seeded: Seeded,
    checks: Vec<Check>,
    /// The run the `expect_*` methods read; the last one unless
    /// [`Verification::keyed_run`] points them at another.
    focus: Option<usize>,
}

/// Files the harness itself owns: their writes are the record, not a leak.
/// `completion-stderr.log` has its own check, which prints what landed in it.
const FAKE_LOGS: [&str; 6] = [
    "op.log",
    "open.log",
    "obsidian.log",
    "pbcopy.log",
    "editor.log",
    "completion-stderr.log",
];

/// What the real 1Password CLI writes for itself in the HOME and TMPDIR it
/// is given -- its config and its daemon's socket, pid and lock -- even with
/// a service account token. On the real and throwaway layers these are the
/// CLI's own state, not a file kurama wrote; the secret scan still reads them.
fn is_op_cli_state(path: &str) -> bool {
    path.starts_with("home/.config/op/") || path.starts_with("tmp/com.agilebits.op.")
}

/// The isolated environment one scenario runs in: directories, config files,
/// the fake services and what they have recorded so far.
pub struct Sandbox {
    dir: tempfile::TempDir,
    pub home: PathBuf,
    aws_dir: PathBuf,
    tmp: PathBuf,
    kurama_config: PathBuf,
    op_log: PathBuf,
    open_log: PathBuf,
    obsidian_log: PathBuf,
    pbcopy_log: PathBuf,
    editor_log: PathBuf,
    /// Where `init zsh` sends the completing child's stderr, so a diagnostic
    /// the user never sees is still observed by the scenario.
    completion_stderr: PathBuf,
    session_cache_file: PathBuf,
    token_store_file: PathBuf,
    seeded_spec_fetched_at: Option<String>,
    /// Set only when the scenario seeded a service account token.
    keychain_secrets: Option<PathBuf>,
    env_script: Option<PathBuf>,
    path_env: String,
    onepassword: OnePassword,
    session_cache: bool,
    browser: bool,
    /// Credentials and tokens that must not land in a file.
    secrets: Vec<String>,
    completion_session: Option<CompletionSession>,
    extra_env: Vec<(&'static str, &'static str)>,
    stdin: Option<String>,
    real_aws: bool,
    before: BTreeMap<PathBuf, Stamp>,
    runtime: tokio::runtime::Runtime,
    server: MockServer,
    seen_requests: usize,
    seen_op: usize,
    seen_open: usize,
    seen_obsidian: usize,
    seen_pbcopy: usize,
    seen_editor: usize,
    /// The statuses the fake description endpoint answered, in order.
    spec_statuses: Arc<Mutex<Vec<u16>>>,
    seen_spec: usize,
}

impl Sandbox {
    pub fn create(scenario: &Scenario) -> Self {
        let dir = sandbox_root(scenario.real_aws);
        let home = dir.path().join("home");
        let tmp = dir.path().join("tmp");
        let aws_dir = home.join(".aws");
        let kurama_dir = home.join(".config").join("kurama");
        for directory in [&tmp, &aws_dir, &kurama_dir] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(aws_dir.join("config"), scenario.aws_config).unwrap();
        std::fs::write(
            aws_dir.join("credentials"),
            format!(
                "[default]\naws_access_key_id = {SOURCE_ACCESS_KEY}\naws_secret_access_key = {SOURCE_SECRET}\n"
            ),
        )
        .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let spec_statuses = Arc::new(Mutex::new(Vec::new()));
        let server = runtime.block_on(start_fakes(scenario, Arc::clone(&spec_statuses)));

        let session_cache_file = dir.path().join("session-cache.json");
        let token_store_file = dir.path().join("token-store.json");
        let keychain_secrets = dir.path().join("keychain-secrets");
        if scenario.service_account_keychain {
            std::fs::create_dir_all(&keychain_secrets).unwrap();
        }
        if let Some(token) = scenario.service_account_token {
            let entry = keychain_secrets.join(SERVICE_ACCOUNT_KEYCHAIN);
            std::fs::create_dir_all(&entry).unwrap();
            std::fs::write(entry.join(SERVICE_ACCOUNT_USER), token).unwrap();
        }
        // Deliberately not `~/.config/kurama/config.toml`: a hint that names
        // the default location instead of the file in use must be visible.
        let kurama_config = dir.path().join("kurama-config.toml");
        let config = scenario
            .config_override
            .clone()
            .unwrap_or_else(|| kurama_config_toml(scenario))
            .replace("{server}", &server.uri())
            .replace("{fixtures}", &fixtures_dir().display().to_string());
        std::fs::write(&kurama_config, config).unwrap();
        let seeded_spec_fetched_at = scenario
            .seeded_spec
            .map(|(_, _, age)| (chrono::Utc::now() - chrono::Duration::seconds(age)).to_rfc3339());
        if let Some((path, fixture, _)) = scenario.seeded_spec {
            let fetched_at = seeded_spec_fetched_at.as_deref().unwrap();
            // Written the way the binary writes it, so a format change is
            // caught here instead of invalidating every user's cache.
            let url = format!("{}{path}", server.uri());
            let key: String = Sha256::digest(url.as_bytes())
                .iter()
                .take(8)
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let cache_dir = home.join(".cache").join("kurama").join("openapi");
            std::fs::create_dir_all(&cache_dir).unwrap();
            let body = std::fs::read(fixtures_dir().join(fixture)).unwrap();
            std::fs::write(cache_dir.join(format!("{key}.body")), &body).unwrap();
            let metadata_path = cache_dir.join(format!("{key}.json"));
            let metadata = serde_json::json!({
                "url":url, "etag":format!("\"{fixture}-{}\"", body.len()),
                "last_modified":null, "fetched_at":fetched_at,
            });
            std::fs::write(&metadata_path, metadata.to_string()).unwrap();
            if scenario.corrupt_spec_metadata {
                std::fs::write(&metadata_path, "invalid metadata").unwrap();
            }
        }
        if scenario.corrupt_session_cache {
            std::fs::write(&session_cache_file, "not json").unwrap();
        }
        let mut secrets: Vec<String> = [RESULT_ACCESS_KEY, RESULT_SECRET, RESULT_TOKEN]
            .into_iter()
            .chain(OAUTH_SECRETS)
            .chain(AWS_SECRET_PREFIXES)
            .chain(OP_SECRETS)
            .map(str::to_string)
            .collect();
        if scenario.corrupt_token_store {
            std::fs::write(&token_store_file, "not json").unwrap();
        } else if !scenario.stored_tokens.is_empty() {
            let tokens: serde_json::Map<String, serde_json::Value> = scenario
                .stored_tokens
                .iter()
                .map(|(name, json)| (name.to_string(), serde_json::from_str(json).unwrap()))
                .collect();
            secrets.extend(
                tokens
                    .values()
                    .filter_map(|token| token["access_token"].as_str())
                    .map(str::to_string),
            );
            std::fs::write(
                &token_store_file,
                serde_json::Value::Object(tokens).to_string(),
            )
            .unwrap();
        }

        // Scenarios that use the session cache or an `[auth.*]` source are
        // `#[ignore]`d unless the crate is built with `--features test-fakes`,
        // which compiles the file backends in; without the feature the
        // binary would reach the real keychain.

        let fakes_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fakes");
        let path_env = if scenario.real_aws {
            // The real `aws`, `op` and `session-manager-plugin`: the scripts
            // and the tunnel are the person's own.
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string())
        } else if scenario.browser || scenario.tools {
            format!("{}:/usr/bin:/bin", fakes_dir.display())
        } else {
            "/usr/bin:/bin".to_string()
        };
        for (path, content) in &scenario.home_files {
            let path = home.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        if let Some(mode) = scenario.existing_env_script_mode {
            let path = tmp.join(ENV_SCRIPT_FILE);
            std::fs::write(&path, "stale\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        if scenario.completion_session == Some(CompletionSession::RealZsh) {
            tui::prepare_zsh_home(&home);
        }
        // Created before the baseline: `2>>` on an existing empty file leaves
        // its size and mtime alone, so only an actual diagnostic shows up.
        let completion_stderr = dir.path().join("completion-stderr.log");
        std::fs::write(&completion_stderr, "").unwrap();
        let (before, _) = snapshot(dir.path());

        Self {
            op_log: dir.path().join("op.log"),
            open_log: dir.path().join("open.log"),
            obsidian_log: dir.path().join("obsidian.log"),
            pbcopy_log: dir.path().join("pbcopy.log"),
            editor_log: dir.path().join("editor.log"),
            completion_stderr,
            env_script: scenario.env_script.then(|| tmp.join(ENV_SCRIPT_FILE)),
            home,
            aws_dir,
            tmp,
            kurama_config,
            session_cache_file,
            token_store_file,
            seeded_spec_fetched_at,
            keychain_secrets: scenario
                .service_account_keychain
                .then(|| keychain_secrets.clone()),
            path_env,
            onepassword: scenario.onepassword,
            session_cache: scenario.session_cache,
            browser: scenario.browser,
            secrets,
            completion_session: scenario.completion_session,
            extra_env: scenario.env.clone(),
            stdin: scenario.stdin.clone(),
            real_aws: scenario.real_aws,
            before,
            runtime,
            server,
            seen_requests: 0,
            seen_op: 0,
            seen_open: 0,
            seen_obsidian: 0,
            seen_pbcopy: 0,
            seen_editor: 0,
            spec_statuses,
            seen_spec: 0,
            dir,
        }
    }

    pub fn binary() -> &'static str {
        env!("CARGO_BIN_EXE_kurama")
    }

    /// The file `KURAMA_CONFIG_PATH` names, for a scenario that has to put
    /// something other than a plain file there (a link, a mode, nothing).
    pub fn kurama_config(&self) -> &Path {
        &self.kurama_config
    }

    /// The complete environment of a kurama process in this sandbox.
    /// What `init zsh` redirected the completing child's stderr to.
    pub fn completion_stderr(&self) -> String {
        std::fs::read_to_string(&self.completion_stderr).unwrap_or_default()
    }

    /// The scenario's own variables; `{home}` in a value is the sandbox
    /// HOME, as in an argument.
    fn extra_env(&self) -> impl Iterator<Item = (String, OsString)> + '_ {
        let home = self.home.display().to_string();
        self.extra_env.iter().map(move |(name, value)| {
            (
                name.to_string(),
                OsString::from(value.replace("{home}", &home)),
            )
        })
    }

    pub fn env(&self) -> Vec<(String, OsString)> {
        if self.real_aws {
            return self.real_aws_env();
        }
        let mut env: Vec<(String, OsString)> = vec![
            ("PATH".into(), self.path_env.clone().into()),
            ("HOME".into(), self.home.clone().into()),
            ("TMPDIR".into(), self.tmp.clone().into()),
            ("AWS_CONFIG_FILE".into(), self.aws_dir.join("config").into()),
            (
                "AWS_SHARED_CREDENTIALS_FILE".into(),
                self.aws_dir.join("credentials").into(),
            ),
            (
                "KURAMA_CONFIG_PATH".into(),
                self.kurama_config.clone().into(),
            ),
            ("AWS_ENDPOINT_URL_STS".into(), self.server.uri().into()),
            ("KURAMA_TEST_S3_ENDPOINT".into(), self.server.uri().into()),
            ("KURAMA_TEST_SSM_ENDPOINT".into(), self.server.uri().into()),
            (
                "KURAMA_TEST_SECRETSMANAGER_ENDPOINT".into(),
                self.server.uri().into(),
            ),
            (
                "KURAMA_FEDERATION_ENDPOINT".into(),
                format!("{}/federation", self.server.uri()).into(),
            ),
            ("AWS_EC2_METADATA_DISABLED".into(), "true".into()),
            ("AWS_MAX_ATTEMPTS".into(), "1".into()),
            (
                "KURAMA_COMPLETION_STDERR".into(),
                self.completion_stderr.clone().into(),
            ),
            ("KURAMA_FAKE_OP_LOG".into(), self.op_log.clone().into()),
            ("KURAMA_FAKE_OPEN_LOG".into(), self.open_log.clone().into()),
            (
                "KURAMA_FAKE_OBSIDIAN_LOG".into(),
                self.obsidian_log.clone().into(),
            ),
            (
                "KURAMA_FAKE_PBCOPY_LOG".into(),
                self.pbcopy_log.clone().into(),
            ),
            (
                "KURAMA_FAKE_EDITOR_LOG".into(),
                self.editor_log.clone().into(),
            ),
        ];
        // Keep instrumented child processes in cargo-llvm-cov's output directory,
        // rather than writing default .profraw files into the isolated HOME.
        if let Some(profile_file) = std::env::var_os("LLVM_PROFILE_FILE") {
            env.push(("LLVM_PROFILE_FILE".into(), profile_file));
        }
        if self.onepassword == OnePassword::Hangs {
            env.push(("KURAMA_FAKE_OP_MODE".into(), "hang".into()));
        }
        if self.onepassword == OnePassword::VersionHangs {
            env.push(("KURAMA_FAKE_OP_MODE".into(), "hang-version".into()));
        }
        if self.onepassword == OnePassword::NotSignedIn {
            env.push(("KURAMA_FAKE_OP_MODE".into(), "not-signed-in".into()));
        }
        if self.onepassword == OnePassword::OtpFails {
            env.push(("KURAMA_FAKE_OP_MODE".into(), "otp-fails".into()));
        }
        if self.session_cache {
            env.push((
                "KURAMA_TEST_SESSION_CACHE_FILE".into(),
                self.session_cache_file.clone().into(),
            ));
        }
        // Every run gets the isolated token store, so no scenario reads the
        // real keychain; `Scenario::token_store` only seeds it.
        env.push((
            "KURAMA_TEST_TOKEN_STORE_FILE".into(),
            self.token_store_file.clone().into(),
        ));
        if let Some(dir) = &self.keychain_secrets {
            env.push(("KURAMA_TEST_KEYCHAIN_SECRET_DIR".into(), dir.clone().into()));
            // The entry is keyed by account too, so the lookup has to use it.
            env.push(("USER".into(), SERVICE_ACCOUNT_USER.into()));
        }
        if let Some(script) = &self.env_script {
            env.push(("KURAMA_ENV_SCRIPT".into(), script.clone().into()));
        }
        env.extend(self.extra_env());
        env
    }

    /// The environment of a run that talks to AWS itself (`Scenario::real_aws`):
    /// the person's own `~/.aws` files (or what `AWS_CONFIG_FILE` /
    /// `AWS_SHARED_CREDENTIALS_FILE` name), no fake endpoint, the real PATH
    /// and user, and the MFA session cache the runner shares across the run
    /// through `KURAMA_THROWAWAY_SESSION_CACHE` (a file the runner removes),
    /// so one TOTP serves every case. The HOME, config file and token store
    /// are still the sandbox's, so nothing of the person's is written to.
    fn real_aws_env(&self) -> Vec<(String, OsString)> {
        let real_home = std::env::var_os("HOME").unwrap_or_default();
        let aws_file = |variable: &str, name: &str| -> OsString {
            std::env::var_os(variable)
                .unwrap_or_else(|| PathBuf::from(&real_home).join(".aws").join(name).into())
        };
        let mut env: Vec<(String, OsString)> = vec![
            ("PATH".into(), self.path_env.clone().into()),
            ("HOME".into(), self.home.clone().into()),
            ("TMPDIR".into(), self.tmp.clone().into()),
            (
                "AWS_CONFIG_FILE".into(),
                aws_file("AWS_CONFIG_FILE", "config"),
            ),
            (
                "AWS_SHARED_CREDENTIALS_FILE".into(),
                aws_file("AWS_SHARED_CREDENTIALS_FILE", "credentials"),
            ),
            (
                "KURAMA_CONFIG_PATH".into(),
                self.kurama_config.clone().into(),
            ),
            ("AWS_EC2_METADATA_DISABLED".into(), "true".into()),
            (
                "KURAMA_TEST_SESSION_CACHE_FILE".into(),
                std::env::var_os("KURAMA_THROWAWAY_SESSION_CACHE")
                    .unwrap_or_else(|| self.session_cache_file.clone().into()),
            ),
            (
                "KURAMA_TEST_TOKEN_STORE_FILE".into(),
                self.token_store_file.clone().into(),
            ),
        ];
        // The keychain entry of the 1Password service account is read for
        // `$USER`, and an exported token wins over it, as in a shell.
        for variable in [
            "USER",
            "OP_SERVICE_ACCOUNT_TOKEN",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
        ] {
            if let Some(value) = std::env::var_os(variable) {
                env.push((variable.into(), value));
            }
        }
        if let Some(script) = &self.env_script {
            env.push(("KURAMA_ENV_SCRIPT".into(), script.clone().into()));
        }
        env.extend(self.extra_env());
        env
    }

    fn create_cli_command(&self, args: &[&str]) -> Command {
        if !cfg!(feature = "test-fakes") && args.first() == Some(&"data") {
            let configuration = std::fs::read_to_string(&self.kurama_config).unwrap();
            let uses_s3 = args
                .iter()
                .copied()
                .chain(self.stdin.as_deref())
                .chain(std::iter::once(configuration.as_str()))
                .any(|text| {
                    text.contains("s3://")
                        || text.contains(".s3.amazonaws.com")
                        || text.contains(".s3.")
                });
            assert!(
                !uses_s3,
                "S3 scenarios require --features test-fakes; refusing to launch a binary that ignores the fake endpoint"
            );
        }
        let mut command = Command::new(Self::binary());
        command.args(args).env_clear().envs(self.env());
        command
    }

    /// Run the binary with piped stdio (no terminal) and record what it did.
    pub fn run_cli(&mut self, args: &[&str]) -> Run {
        // `{home}` is the sandbox HOME and `{server}` the fake server, which
        // exist only once the sandbox does: an argument or stdin naming a
        // file under HOME, or an argument naming the server, says so here.
        let home = self.home.display().to_string();
        let server = self.server.uri();
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.replace("{home}", &home).replace("{server}", &server))
            .collect();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let stdin = self
            .stdin
            .as_deref()
            .map(|input| input.replace("{home}", &home));
        let output = run_with_timeout(self.create_cli_command(&args), stdin.as_deref());
        let command = std::iter::once("kurama")
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect();
        self.record_run(
            command,
            output.exit_code,
            output.timed_out,
            output.stdout,
            output.stderr,
        )
    }

    /// Interrupt only after an export has created its temporary output. This
    /// observes that the worker started instead of guessing a startup delay.
    pub fn run_cli_interrupted(&mut self, args: &[&str], export_directory: &Path) -> Run {
        let output = run_with_interrupt(
            self.create_cli_command(args),
            self.stdin.as_deref(),
            Some(export_directory),
            RUN_TIMEOUT,
        );
        self.record_run(
            std::iter::once("kurama")
                .chain(args.iter().copied())
                .map(str::to_string)
                .collect(),
            output.exit_code,
            output.timed_out,
            output.stdout,
            output.stderr,
        )
    }

    /// Start `count` processes with `args` at once, wait for all of them and
    /// record them as one run: the exit code is 0 only when every process
    /// exited 0 (otherwise the first other code), stdout and stderr are
    /// concatenated in start order.
    pub fn run_cli_parallel(&mut self, args: &[&str], count: usize) -> Run {
        let children: Vec<std::process::Child> = (0..count)
            .map(|_| {
                self.create_cli_command(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("run kurama")
            })
            .collect();
        let mut exit_code = Some(0);
        let mut timed_out = false;
        let mut stdout = String::new();
        let mut stderr = String::new();
        let started = Instant::now();
        for mut child in children {
            let out = drain(child.stdout.take().unwrap());
            let err = drain(child.stderr.take().unwrap());
            let code = loop {
                match child.try_wait().expect("wait for kurama") {
                    Some(status) => break status.code(),
                    None if started.elapsed() > RUN_TIMEOUT => {
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break None;
                    }
                    None => std::thread::sleep(Duration::from_millis(20)),
                }
            };
            if code != Some(0) && exit_code == Some(0) {
                exit_code = code;
            }
            stdout.push_str(&out.join().unwrap());
            stderr.push_str(&err.join().unwrap());
        }
        let command = std::iter::once("kurama".to_string())
            .chain(args.iter().map(|arg| arg.to_string()))
            .chain(std::iter::once(format!("(x{count} parallel)")))
            .collect();
        self.record_run(command, exit_code, timed_out, stdout, stderr)
    }

    /// Collect the fake services' records since the previous run into a [`Run`].
    pub fn record_run(
        &mut self,
        command: Vec<String>,
        exit_code: Option<i32>,
        timed_out: bool,
        stdout: String,
        stderr: String,
    ) -> Run {
        let requests = self
            .runtime
            .block_on(self.server.received_requests())
            .unwrap_or_default();
        let new_requests = &requests[self.seen_requests..];
        self.seen_requests = requests.len();
        // STS and Systems Manager both POST to `/`; only the second names its
        // operation in a header.
        let target = |request: &wiremock::Request| {
            request
                .headers
                .get("x-amz-target")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let ssm_calls = new_requests
            .iter()
            .filter_map(|request| Some(target(request)?.strip_prefix("AmazonSSM.")?.to_owned()))
            .collect();
        let secret_calls = new_requests
            .iter()
            .filter_map(|request| {
                let operation = match target(request)?.as_str() {
                    "secretsmanager.GetSecretValue" => "secretsmanager:GetSecretValue",
                    "AmazonSSM.GetParameter" => "ssm:GetParameter",
                    _ => return None,
                };
                let body: serde_json::Value = serde_json::from_slice(&request.body).ok()?;
                let (signing_access_key, region) = signing_scope(
                    request
                        .headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                );
                Some(SecretCall {
                    operation: operation.to_owned(),
                    id: secret_id(&body).unwrap_or_default(),
                    region,
                    signing_access_key,
                    with_decryption: body
                        .get("WithDecryption")
                        .and_then(serde_json::Value::as_bool),
                })
            })
            .collect();
        let sts_calls = new_requests
            .iter()
            .filter(|request| {
                request.method == "POST" && request.url.path() == "/" && target(request).is_none()
            })
            .map(|request| {
                let authorization = request
                    .headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok());
                decode_sts_call(&request.body, authorization)
            })
            .collect();
        let federation_calls = new_requests
            .iter()
            .filter(|request| request.method == "GET" && request.url.path() == "/federation")
            .map(|request| decode_federation_call(request.url.query().unwrap_or("")))
            .collect();
        let oauth_calls = new_requests
            .iter()
            .filter(|request| {
                request.url.path().starts_with("/oauth/")
                    || request.url.path().starts_with("/.well-known/")
            })
            .map(|request| {
                let params: BTreeMap<String, String> = url::form_urlencoded::parse(&request.body)
                    .into_owned()
                    .collect();
                OAuthCall {
                    method: request.method.to_string(),
                    path: request.url.path().to_string(),
                    grant_type: params.get("grant_type").cloned(),
                    params,
                }
            })
            .collect();
        let api_calls = new_requests
            .iter()
            .filter(|request| {
                request.url.path().starts_with("/api/")
                    || request.url.path().starts_with("/data-bucket")
                    || request.url.path().starts_with("/west-bucket")
                    || request.url.path().starts_with("/moved-bucket")
                    || request.url.path().starts_with("/browse-")
                    // ListBuckets; STS is a POST to the same path.
                    || (request.method == "GET" && request.url.path() == "/")
            })
            .map(|request| ApiCall {
                method: request.method.to_string(),
                path: match request.url.query() {
                    Some(query) => format!("{}?{query}", request.url.path()),
                    None => request.url.path().to_string(),
                },
                authorization: request
                    .headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                headers: request
                    .headers
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str().to_ascii_lowercase(),
                            String::from_utf8_lossy(value.as_bytes()).into_owned(),
                        )
                    })
                    .collect(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
                sigv4: decode_sigv4(request),
            })
            .collect();
        // The fake answers the description requests in order; pair them up.
        let statuses: Vec<u16> = self.spec_statuses.lock().unwrap()[self.seen_spec..].to_vec();
        let spec_calls: Vec<SpecCall> = new_requests
            .iter()
            .filter(|request| request.url.path().starts_with("/spec/"))
            .zip(statuses)
            .map(|(request, status)| SpecCall {
                method: request.method.to_string(),
                path: request.url.path().to_string(),
                conditional: request.headers.contains_key("if-none-match")
                    || request.headers.contains_key("if-modified-since"),
                authorization: request
                    .headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                status,
            })
            .collect();
        self.seen_spec += spec_calls.len();
        let op_calls = new_lines(&self.op_log, &mut self.seen_op);
        let clipboard_calls = new_lines(&self.pbcopy_log, &mut self.seen_pbcopy);
        let editor_calls = new_lines(&self.editor_log, &mut self.seen_editor);
        if self.browser {
            // A TUI or an OAuth callback may hand the URL over after the run
            // reads its output; give the fake a moment to log.
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline && count_lines(&self.open_log) <= self.seen_open {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let open_calls = new_lines(&self.open_log, &mut self.seen_open);
        let obsidian_calls = new_lines(&self.obsidian_log, &mut self.seen_obsidian);
        let exported_variables = exported_variables(&stdout);
        Run {
            command,
            exit_code,
            timed_out,
            stdout,
            stderr,
            sts_calls,
            ssm_calls,
            secret_calls,
            federation_calls,
            oauth_calls,
            api_calls,
            spec_calls,
            op_calls,
            open_calls,
            obsidian_calls,
            clipboard_calls,
            editor_calls,
            exported_variables,
            http_responses: Vec::new(),
            stopped_while_serving: None,
        }
    }

    /// Everything observed, plus the checks every scenario must pass.
    pub fn finish(
        self,
        id: &'static str,
        feature: &'static str,
        runs: Vec<Run>,
        screens: Vec<ScreenRecord>,
    ) -> Verification {
        let root = self.dir.path();
        let (after, unlisted) = snapshot(root);
        let files_written = after
            .iter()
            .filter(|(path, stamp)| self.before.get(*path) != Some(stamp))
            .map(|(path, _)| relative(root, path))
            .filter(|path| !FAKE_LOGS.contains(&path.as_str()))
            .filter(|path| !(self.real_aws && is_op_cli_state(path)))
            .collect();
        // The wrapper's env script and the test-only token store are the
        // two files that hold credentials by design; on the real layer the
        // file-backed session cache is the third, and the secrets are not
        // known in advance, so any file that looks like it holds one counts.
        let scanned = after.keys().filter(|path| {
            Some(*path) != self.env_script.as_ref()
                && **path != self.token_store_file
                && !(self.real_aws && **path == self.session_cache_file)
        });
        let (secrets_on_disk, unread) = scan_for_secrets(scanned, &self.secrets, self.real_aws);
        let unscanned = unlisted
            .iter()
            .chain(&unread)
            .map(|path| relative(root, path))
            .collect();
        let secrets_on_disk = secrets_on_disk
            .iter()
            .map(|path| relative(root, path))
            .collect();
        let env_script = self.env_script.as_ref().and_then(|path| {
            let content = std::fs::read_to_string(path).ok()?;
            let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
            Some(EnvScript {
                path: relative(root, path),
                mode,
                exported_variables: exported_variables(&content),
            })
        });

        let mut verification = Verification {
            id,
            feature,
            combination: None,
            evidence: "fake",
            focus: None,
            observed: Observed {
                runs,
                files_written,
                secrets_on_disk,
                unscanned,
                env_script,
                screens,
                token_store: stored_sources(&self.token_store_file),
            },
            seeded: Seeded {
                spec_fetched_at: self.seeded_spec_fetched_at.clone(),
            },
            checks: Vec::new(),
        };
        let timed_out: Vec<usize> = verification
            .observed
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.timed_out)
            .map(|(index, _)| index)
            .collect();
        let ended_early: Vec<usize> = verification
            .observed
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.stopped_while_serving == Some(false))
            .map(|(index, _)| index)
            .collect();
        verification.check(
            "every server was still serving when the harness stopped it",
            ended_early.is_empty(),
            format!("servers that ended on their own: {ended_early:?}"),
        );
        verification.check(
            &format!("every run finishes within {}s", RUN_TIMEOUT.as_secs()),
            timed_out.is_empty(),
            format!("timed out runs: {timed_out:?}"),
        );
        if self.completion_session == Some(CompletionSession::Direct) {
            let bad_exits: Vec<_> = verification
                .observed
                .runs
                .iter()
                .enumerate()
                .filter(|(_, run)| run.exit_code != Some(0))
                .map(|(i, _)| i)
                .collect();
            verification.check(
                "every completion request exits zero",
                bad_exits.is_empty(),
                format!("runs {bad_exits:?}"),
            );
            let noisy: Vec<_> = verification
                .observed
                .runs
                .iter()
                .enumerate()
                .filter(|(_, run)| !run.stderr.is_empty())
                .map(|(i, _)| i)
                .collect();
            verification.check(
                "every completion request keeps stderr empty",
                noisy.is_empty(),
                format!("runs {noisy:?}"),
            );
        }
        if self.completion_session.is_some() {
            let effects: Vec<_> = verification
                .observed
                .runs
                .iter()
                .enumerate()
                .filter(|(_, run)| {
                    !run.sts_calls.is_empty()
                        || !run.federation_calls.is_empty()
                        || !run.oauth_calls.is_empty()
                        || !run.api_calls.is_empty()
                        || !run.spec_calls.is_empty()
                        || !run.op_calls.is_empty()
                        || !run.open_calls.is_empty()
                        || !run.obsidian_calls.is_empty()
                        || !run.clipboard_calls.is_empty()
                        || !run.editor_calls.is_empty()
                })
                .map(|(i, _)| i)
                .collect();
            verification.check(
                "completion makes no service, secret, browser, clipboard or editor calls",
                effects.is_empty(),
                format!("runs {effects:?}"),
            );
            verification.expect_no_files_written();
        }
        // Invariants every scenario must hold, whatever it asserts on top.
        if self.real_aws {
            // A layer that forgot its own `config` still points at `{server}`,
            // and the fake would answer it as if it were the real service.
            verification.check(
                "a real run reaches no fake endpoint",
                self.seen_requests == 0,
                format!("the fake server received {} request(s)", self.seen_requests),
            );
        }
        let escaped_runs: Vec<usize> = verification
            .observed
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.stdout.contains('\x1b') || run.stderr.contains('\x1b'))
            .map(|(index, _)| index)
            .collect();
        verification.check(
            "stdout and stderr contain no terminal escape characters",
            escaped_runs.is_empty(),
            format!("runs containing ESC: {escaped_runs:?}"),
        );
        let secrets = verification.observed.secrets_on_disk.clone();
        verification.check(
            "credentials and tokens are not written to disk",
            secrets.is_empty(),
            format!("files containing secrets: {secrets:?}"),
        );
        let audit_problems = audit_log_problems(&self.home.join(".local/state/kurama"));
        verification.check(
            "the audit log holds no query string, header, body or field AuditEntry does not name",
            audit_problems.is_empty(),
            format!("{audit_problems:?}"),
        );
        let unscanned = verification.observed.unscanned.clone();
        verification.check(
            "every sandbox path is listed and read by the secret scan",
            unscanned.is_empty(),
            format!("paths the scan could not list or read: {unscanned:?}"),
        );
        if let Some(script) = verification.observed.env_script.clone() {
            verification.check(
                "the env script hand-off file has mode 600",
                script.mode == 0o600,
                format!("{} has mode {:o}", script.path, script.mode),
            );
        }
        let error_logs: Vec<String> = verification
            .observed
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.exit_code == Some(0))
            .flat_map(|(index, run)| {
                run.stderr
                    .lines()
                    .filter(|line| is_error_line(line))
                    .map(move |line| format!("run {index}: {line}"))
            })
            .collect();
        verification.check(
            "successful runs log no errors",
            error_logs.is_empty(),
            format!("error lines: {error_logs:?}"),
        );
        let sprawling: Vec<String> = verification
            .observed
            .runs
            .iter()
            .enumerate()
            .filter_map(|(index, run)| {
                let tail = error_tail(&run.stderr)?;
                (!tail.is_empty()
                    || run
                        .stderr
                        .lines()
                        .filter(|line| line.starts_with("hint:"))
                        .count()
                        > 1)
                .then(|| format!("run {index}: {tail:?}"))
            })
            .collect();
        verification.check(
            "a failure is one error line and at most one hint line",
            sprawling.is_empty(),
            format!("extra lines after error[...]: {sprawling:?}"),
        );
        let repeated_causes: Vec<usize> = verification
            .observed
            .runs
            .iter()
            .enumerate()
            .filter_map(|(index, run)| repeats_error_causes(&run.stderr).then_some(index))
            .collect();
        verification.check(
            "error messages do not repeat causes",
            repeated_causes.is_empty(),
            format!("runs with repeated error causes: {repeated_causes:?}"),
        );
        for (index, run) in verification.observed.runs.clone().iter().enumerate() {
            // The subcommands whose failures under --json / --jq are one JSON
            // error document: the binary's own `JsonErrorKind`, so this list
            // cannot drift from it.
            let Some(subcommand) = run
                .command
                .get(1)
                .map(String::as_str)
                .filter(|name| kurama::JsonErrorKind::parse(name).is_some())
            else {
                continue;
            };
            let is_data = subcommand == "data";
            // `env` and `token` print a credential on stdout by design, and
            // `db` prints what the database holds; stderr never carries one.
            let checked = if is_data {
                format!("{}{}", run.stdout, run.stderr)
            } else {
                run.stderr.clone()
            };
            verification.check(
                &format!("run {index}: {subcommand} output contains no credentials or tokens"),
                !self.secrets.iter().any(|secret| checked.contains(secret)),
                if is_data {
                    "checked both stdout and stderr against every synthetic credential and token"
                } else {
                    "checked stderr against every synthetic credential and token"
                },
            );
            // `api --dry-run` and `-v` print on stderr what they were asked for.
            let json = run
                .command
                .iter()
                .any(|arg| arg == "--json" || arg == "--jq" || arg.starts_with("--jq="))
                && !(subcommand == "api"
                    && run
                        .command
                        .iter()
                        .any(|arg| ["--dry-run", "-v", "--verbose"].contains(&arg.as_str())));
            let code = if json && run.exit_code != Some(0) {
                serde_json::from_str::<serde_json::Value>(&run.stderr)
                    .ok()
                    .and_then(|value| value["error"]["code"].as_str().map(str::to_string))
            } else {
                run.stderr.lines().find_map(|line| {
                    line.strip_prefix("error[")
                        .and_then(|rest| rest.split_once(']').map(|(code, _)| code.to_string()))
                })
            };
            verification.check(
                &format!("run {index}: {subcommand} errors stay inside documented codes"),
                code.as_deref() != Some("INTERNAL"),
                format!("reported code: {code:?}"),
            );
            if json && run.exit_code != Some(0) {
                let document = serde_json::from_str::<serde_json::Value>(&run.stderr);
                let valid = document.as_ref().is_ok_and(|document| {
                    let error = &document["error"];
                    let (category, retry) = match run.exit_code {
                        Some(1) => ("tool", None),
                        Some(2) => ("usage", Some("never")),
                        Some(3) => ("human", Some("after_human")),
                        Some(4) => ("remote", None),
                        _ => return false,
                    };
                    document["schema_version"] == 1
                        && error["exit_code"].as_i64() == run.exit_code.map(i64::from)
                        && error["category"] == category
                        && error["code"]
                            .as_str()
                            .is_some_and(|code| !code.is_empty() && code != "INTERNAL")
                        && error["message"].as_str().is_some_and(|message| {
                            !message.is_empty() && !message.contains(['\n', '\r'])
                        })
                        && error["retry"].as_str().is_some_and(|value| {
                            // Only `db` knows a read that sent nothing.
                            (subcommand == "db" && value == "read_only_retry")
                                || (["never", "after_human"].contains(&value)
                                    && retry.is_none_or(|expected| expected == value))
                        })
                        && error["next_actions"].as_array().is_some_and(|actions| {
                            !actions.is_empty()
                                && actions.iter().all(|action| {
                                    action["message"]
                                        .as_str()
                                        .is_some_and(|message| !message.is_empty())
                                })
                        })
                });
                verification.check(
                    &format!("run {index}: {subcommand} failure is one complete JSON error with matching category and recovery advice"),
                    valid,
                    run.stderr.clone(),
                );
            } else if json && run.exit_code == Some(0) {
                // `api --pages` says why paging stopped in one JSON document,
                // the only line stderr may then hold.
                let pages = subcommand == "api"
                    && run
                        .command
                        .iter()
                        .any(|arg| arg == "--pages" || arg.starts_with("--pages="));
                let ok = if pages {
                    run.stderr.lines().count() == 1
                        && serde_json::from_str::<serde_json::Value>(&run.stderr)
                            .is_ok_and(|report| report["pages"]["stopped"].is_string())
                } else {
                    run.stderr.is_empty()
                };
                verification.check(
                    &format!(
                        "run {index}: successful {subcommand} JSON contains no stderr diagnostics"
                    ),
                    ok,
                    run.stderr.clone(),
                );
            }
            let projected = run
                .command
                .iter()
                .any(|arg| arg == "--jq" || arg.starts_with("--jq="));
            if is_data && json && !projected && !run.stdout.is_empty() {
                let document = serde_json::from_str::<serde_json::Value>(&run.stdout);
                verification.check(
                    &format!("run {index}: data result is one versioned envelope"),
                    document.as_ref().is_ok_and(|document| {
                        document["schema_version"] == 1
                            && document["kind"] == "data"
                            && document["operation"].as_str().is_some_and(|operation| {
                                [
                                    "tables", "describe", "preview", "summary", "query", "explain",
                                    "export",
                                ]
                                .contains(&operation)
                            })
                            && document["target"].is_string()
                            && document["meta"].is_object()
                    }),
                    run.stdout.clone(),
                );
            }
        }
        verification
    }
}
/// Everything printed after the `error[...]` line, `hint:` excluded. A
/// failure is one greppable line, so anything left here is sprawl: a message
/// with embedded newlines, or a second line the caller has to read.
fn error_tail(stderr: &str) -> Option<Vec<&str>> {
    let lines: Vec<&str> = stderr.lines().collect();
    let start = lines.iter().position(|line| line.starts_with("error["))?;
    Some(
        lines[start + 1..]
            .iter()
            .copied()
            .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
            .collect(),
    )
}

/// `error[CODE]` lines and `ERROR` level log lines.
pub fn is_error_line(line: &str) -> bool {
    line.starts_with("error[") || line.split_whitespace().nth(1) == Some("ERROR")
}

/// Catch adjacent copies of the same cause chain in text and JSON errors.
fn repeats_error_causes(stderr: &str) -> bool {
    let message = stderr
        .lines()
        .find(|line| line.starts_with("error["))
        .and_then(|line| line.split_once("]: "))
        .map(|(_, message)| message.to_owned())
        .or_else(|| {
            serde_json::from_str::<serde_json::Value>(stderr)
                .ok()?
                .get("error")?
                .get("message")?
                .as_str()
                .map(str::to_owned)
        });
    let Some(message) = message else {
        return false;
    };
    let causes: Vec<&str> = message.split(": ").collect();
    (1..=causes.len() / 2).any(|width| {
        causes
            .windows(width * 2)
            .any(|pair| pair[..width] == pair[width..])
    })
}

#[test]
fn the_real_op_cli_state_is_not_a_file_kurama_wrote() {
    for (path, own_state) in [
        ("home/.config/op/config", true),
        ("home/.config/op/op-daemon.sock", true),
        ("tmp/com.agilebits.op.502/op-daemon.pid", true),
        (
            "tmp/com.agilebits.op.502/.BNwConlrwYiWG0V015lGWbcaLLE",
            true,
        ),
        ("home/.config/kurama/config.toml", false),
        ("home/.config/opx/config", false),
        ("tmp/kurama-env.sh", false),
    ] {
        assert_eq!(is_op_cli_state(path), own_state, "{path}");
    }
}

#[test]
fn error_code_repeated_causes_are_detected_in_text_and_json() {
    for (message, repeated) in [
        ("choose a workspace: choose a workspace", true),
        ("request failed: timed out: timed out", true),
        (
            "input missing: data: /tmp/input: input missing: data: /tmp/input",
            true,
        ),
        ("cannot read input: file missing", false),
        ("no no", false),
    ] {
        assert_eq!(
            repeats_error_causes(&format!(
                "error[DATA_INVALID]: {message}\nhint: fix input\n"
            )),
            repeated,
            "text: {message}"
        );
        let json = serde_json::json!({"error": {"code": "DATA_INVALID", "message": message}});
        assert_eq!(
            repeats_error_causes(&json.to_string()),
            repeated,
            "JSON: {message}"
        );
    }
    assert!(!repeats_error_causes("progress: progress\n"));
    assert!(!repeats_error_causes("{\"data\": \"data: data\"}"));
}

fn exported_variables(script: &str) -> Vec<String> {
    script
        .lines()
        .filter_map(|line| line.strip_prefix("export "))
        .filter_map(|line| line.split('=').next())
        .map(str::to_string)
        .collect()
}

pub fn run(scenario: Scenario) -> Verification {
    let mut sandbox = Sandbox::create(&scenario);
    let mut runs: Vec<Run> = scenario
        .runs
        .iter()
        .enumerate()
        .map(|(index, args)| match index {
            0 if !scenario.http.is_empty() => {
                sandbox.run_cli_serving(args, scenario.http.clone()).0
            }
            _ => sandbox.run_cli(args),
        })
        .collect();
    if let Some((count, args)) = scenario.parallel {
        runs.push(sandbox.run_cli_parallel(args, count));
    }
    sandbox.finish(scenario.id, scenario.feature, runs, Vec::new())
}

pub(crate) struct ProcessOutput {
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
    stdout: String,
    stderr: String,
}

fn run_with_timeout(command: Command, input: Option<&str>) -> ProcessOutput {
    run_with_deadline(command, input, RUN_TIMEOUT)
}

/// Run `command` with `input` on its stdin, killing it once `limit` has
/// passed since it started.
pub(crate) fn run_with_deadline(
    command: Command,
    input: Option<&str>,
    limit: Duration,
) -> ProcessOutput {
    run_with_interrupt(command, input, None, limit)
}

fn run_with_interrupt(
    mut command: Command,
    input: Option<&str>,
    export_directory: Option<&Path>,
    limit: Duration,
) -> ProcessOutput {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run kurama");
    let started = Instant::now();
    let stdout = drain(child.stdout.take().unwrap());
    let stderr = drain(child.stderr.take().unwrap());
    // A child that does not read its stdin blocks a write larger than the
    // pipe, so the write runs beside the deadline; one that exits without
    // reading it all breaks the pipe, which is its own business.
    let writer = input.map(|input| {
        let mut stdin = child.stdin.take().unwrap();
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        })
    });
    let mut interrupted = false;
    let (exit_code, timed_out) = loop {
        if !interrupted
            && export_directory
                .is_some_and(|path| std::fs::read_dir(path).unwrap().next().is_some())
        {
            assert!(
                Command::new("/bin/kill")
                    .args(["-INT", &child.id().to_string()])
                    .status()
                    .unwrap()
                    .success(),
                "signal the data worker"
            );
            interrupted = true;
        }
        match child.try_wait().expect("wait for kurama") {
            Some(status) => break (status.code(), false),
            None if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert!(
        export_directory.is_none() || interrupted,
        "data worker never created a temporary export before exiting"
    );
    if let Some(writer) = writer {
        writer.join().unwrap();
    }
    ProcessOutput {
        exit_code,
        timed_out,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn count_lines(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .map(|content| content.lines().count())
        .unwrap_or(0)
}

fn new_lines(log: &Path, seen: &mut usize) -> Vec<String> {
    let lines: Vec<String> = std::fs::read_to_string(log)
        .map(|content| content.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let fresh = lines[(*seen).min(lines.len())..].to_vec();
    *seen = lines.len();
    fresh
}

/// What a JSON value read from a run is expected to be.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonExpect {
    /// Exactly this value.
    Equals(serde_json::Value),
    /// A string containing every one of these texts.
    Contains(Vec<String>),
    /// An array holding this value.
    Has(serde_json::Value),
    /// An array or object of exactly this many elements.
    Length(usize),
    /// An array or object of at least this many elements.
    MinLength(usize),
    /// A number at least this large.
    AtLeast(f64),
    /// Any string.
    AnyString,
    /// Any object.
    AnyObject,
    /// `null`, or nothing at that pointer.
    Null,
    /// The number of requests the fake API received in the run.
    ApiCallCount,
    /// The bytes the run asked the fake API for: each GET's `Range`, or the
    /// whole object -- the `input_bytes` the run's own document reports --
    /// for a GET without one.
    ApiBytesRequested,
}

impl JsonExpect {
    fn matches(&self, found: Option<&serde_json::Value>, run: &Run) -> bool {
        let length = || {
            found.and_then(|value| match value {
                serde_json::Value::Array(items) => Some(items.len()),
                serde_json::Value::Object(fields) => Some(fields.len()),
                _ => None,
            })
        };
        match self {
            JsonExpect::Equals(value) => found == Some(value),
            JsonExpect::Contains(texts) => found
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| texts.iter().all(|text| s.contains(text.as_str()))),
            JsonExpect::Has(value) => found
                .and_then(serde_json::Value::as_array)
                .is_some_and(|items| items.contains(value)),
            JsonExpect::Length(count) => length() == Some(*count),
            JsonExpect::MinLength(count) => length().is_some_and(|found| found >= *count),
            JsonExpect::AtLeast(minimum) => found
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|number| number >= *minimum),
            JsonExpect::AnyString => found.is_some_and(serde_json::Value::is_string),
            JsonExpect::AnyObject => found.is_some_and(serde_json::Value::is_object),
            JsonExpect::Null => found.is_none_or(serde_json::Value::is_null),
            JsonExpect::ApiCallCount => found == Some(&serde_json::json!(run.api_calls.len())),
            JsonExpect::ApiBytesRequested => {
                let document: serde_json::Value =
                    serde_json::from_str(run.stdout.trim()).unwrap_or_default();
                let object = document["meta"]["input_bytes"].as_u64().unwrap_or_default();
                let asked = run.api_bytes_requested(object);
                asked > 0 && found == Some(&serde_json::json!(asked))
            }
        }
    }
}

/// What one STS call is expected to have carried. Every `Some` is
/// compared; `"<none>"` says the parameter was not sent at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StsCallExpect {
    pub action: Option<String>,
    pub role_arn: Option<String>,
    pub serial_number: Option<String>,
    pub token_code: Option<String>,
    pub signing_access_key: Option<String>,
    pub policy_arns: Option<Vec<String>>,
}

impl StsCallExpect {
    pub(super) fn matches(&self, call: &StsCall) -> bool {
        let field = |expected: &Option<String>, observed: &Option<String>| match expected.as_deref()
        {
            None => true,
            Some("<none>") => observed.is_none(),
            Some(value) => observed.as_deref() == Some(value),
        };
        self.action
            .as_ref()
            .is_none_or(|action| *action == call.action)
            && field(&self.role_arn, &call.role_arn)
            && field(&self.serial_number, &call.serial_number)
            && field(&self.token_code, &call.token_code)
            && field(&self.signing_access_key, &call.signing_access_key)
            && self
                .policy_arns
                .as_ref()
                .is_none_or(|arns| *arns == call.policy_arns)
    }
}

/// Every non-empty line of `text` starts with one of `prefixes`.
fn lines_start_with(text: &str, prefixes: &[&str]) -> bool {
    text.lines()
        .filter(|line| !line.is_empty())
        .all(|line| prefixes.iter().any(|prefix| line.starts_with(prefix)))
}

impl Verification {
    /// Record one named check. Failures are collected and reported together
    /// by [`Verification::finish`].
    pub fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) -> &mut Self {
        self.checks.push(Check {
            name: name.to_string(),
            ok,
            detail: detail.into(),
        });
        self
    }

    /// The coordinates the report carries under `combination`.
    pub fn with_combination(mut self, combination: BTreeMap<String, String>) -> Self {
        self.combination = Some(combination);
        self
    }

    /// What the run was observed against, and so where the report goes:
    /// `fake` (the default) writes under `scenarios/`, any other layer under
    /// `scenarios-<evidence>/`, so a local or real run never overwrites the
    /// fake evidence of the same case.
    pub fn with_evidence(mut self, evidence: &'static str) -> Self {
        self.evidence = evidence;
        self
    }

    /// What the report will say under `combination` and `evidence`.
    pub fn contract(&self) -> (Option<&BTreeMap<String, String>>, &str) {
        (self.combination.as_ref(), self.evidence)
    }

    /// Run `expect` and name every check it records after `key`, so a case
    /// that fails says which of its keys did: `expect.exit_code: exit code
    /// is 4: observed 2`.
    pub fn keyed(&mut self, key: &str, expect: impl FnOnce(&mut Self) -> &mut Self) -> &mut Self {
        let before = self.checks.len();
        expect(self);
        for check in &mut self.checks[before..] {
            check.name = format!("{key}: {}", check.name);
        }
        self
    }

    /// The run the `expect_*` methods read.
    fn focused_index(&self) -> usize {
        self.focus.unwrap_or(self.observed.runs.len() - 1)
    }

    fn focused(&self) -> &Run {
        &self.observed.runs[self.focused_index()]
    }

    /// Like [`Verification::keyed`], with every `expect_*` inside reading
    /// run `index` instead of the last one.
    pub fn keyed_run(
        &mut self,
        index: usize,
        key: &str,
        expect: impl FnOnce(&mut Self) -> &mut Self,
    ) -> &mut Self {
        assert!(
            index < self.observed.runs.len(),
            "{key}: run {index} does not exist; {} run(s) were made",
            self.observed.runs.len()
        );
        let outer = self.focus.replace(index);
        self.keyed(key, expect);
        self.focus = outer;
        self
    }

    /// stdout is exactly `text`.
    pub fn expect_stdout_equals(&mut self, text: &str) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            &format!("stdout is {text:?}"),
            stdout == text,
            format!("observed {stdout:?}"),
        )
    }

    /// Every non-empty line of stdout starts with one of `prefixes`.
    pub fn expect_stdout_lines_start_with(&mut self, prefixes: &[&str]) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            &format!("every stdout line starts with one of {prefixes:?}"),
            !stdout.is_empty() && lines_start_with(&stdout, prefixes),
            format!("observed {stdout:?}"),
        )
    }

    /// stderr has a non-empty line, and every one starts with one of
    /// `prefixes`.
    pub fn expect_stderr_lines_start_with(&mut self, prefixes: &[&str]) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        self.check(
            &format!("every stderr line starts with one of {prefixes:?}"),
            stderr.lines().any(|line| !line.is_empty()) && lines_start_with(&stderr, prefixes),
            format!("observed {stderr:?}"),
        )
    }

    /// Some line of stdout contains every one of `parts`.
    pub fn expect_stdout_line_containing(&mut self, parts: &[&str]) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            &format!("a stdout line contains all of {parts:?}"),
            stdout
                .lines()
                .any(|line| parts.iter().all(|part| line.contains(part))),
            format!("observed {stdout:?}"),
        )
    }

    /// The JSON document on `stream` (`"stdout"` or `"stderr"`) has `value`
    /// at the JSON pointer `pointer`.
    pub fn expect_json_at(&mut self, stream: &str, pointer: &str, value: &JsonExpect) -> &mut Self {
        let run = self.focused().clone();
        let text = match stream {
            "stdout" => run.stdout.clone(),
            "stderr" => run.stderr.clone(),
            other => panic!("{other}: a JSON document is read from stdout or stderr"),
        };
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(text.trim());
        let document = parsed.as_ref().cloned().unwrap_or_default();
        let found = document.pointer(pointer);
        // The detail describes what was there, not the whole document: a
        // real service's body (a public key, a listing) has no place in a
        // report, and the shape is what the expectation is about.
        let observed = match (&parsed, found) {
            (Err(error), _) => format!("not a JSON document ({error}), {} bytes", text.len()),
            (Ok(_), None) => "nothing at that pointer".to_string(),
            (Ok(_), Some(serde_json::Value::Object(fields))) => {
                format!("an object with {} key(s)", fields.len())
            }
            (Ok(_), Some(serde_json::Value::Array(items))) => {
                format!("an array of {}", items.len())
            }
            (Ok(_), Some(scalar)) => {
                let text = scalar.to_string();
                if text.len() > 120 {
                    format!("{}... ({} bytes)", &text[..120], text.len())
                } else {
                    text
                }
            }
        };
        self.check(
            &format!("{stream} JSON at {pointer} is {value:?}"),
            parsed.is_ok() && value.matches(found, &run),
            format!("observed {observed}"),
        )
    }

    /// The files the scenario wrote under the sandbox, exactly.
    pub fn expect_files_written(&mut self, files: &[&str]) -> &mut Self {
        let written = self.observed.files_written.clone();
        self.check(
            &format!("the files written are exactly {files:?}"),
            written == files,
            format!("written: {written:?}"),
        )
    }

    /// The `op read` and `op item get` calls of the run, exactly and in
    /// order: the calls that read a secret or an item, and not `--version`.
    pub fn expect_op_reads(&mut self, reads: &[&str]) -> &mut Self {
        let calls = self.focused().op_calls.clone();
        let observed: Vec<&str> = calls
            .iter()
            .map(String::as_str)
            .filter(|call| call.starts_with("read ") || call.starts_with("item get "))
            .collect();
        self.check(
            &format!("1Password reads are exactly {reads:?}"),
            observed == reads,
            format!("observed {calls:?}"),
        )
    }

    /// The URLs handed to the browser in the run, exactly.
    pub fn expect_open_calls(&mut self, urls: &[&str]) -> &mut Self {
        let calls = self.focused().open_calls.clone();
        self.check(
            &format!("the browser is opened on exactly {urls:?}"),
            calls == urls,
            format!("observed {calls:?}"),
        )
    }

    /// The Obsidian CLI calls of the run, exactly and in order, each argv
    /// joined by a tab.
    pub fn expect_obsidian_calls(&mut self, calls: &[&str]) -> &mut Self {
        let observed = self.focused().obsidian_calls.clone();
        self.check(
            &format!("the Obsidian CLI is called exactly with {calls:?}"),
            observed == calls,
            format!("observed {observed:?}"),
        )
    }

    /// The Systems Manager operations of the run, exactly and in order.
    pub fn expect_ssm_calls(&mut self, operations: &[&str]) -> &mut Self {
        let calls = self.focused().ssm_calls.clone();
        self.check(
            &format!("SSM operations are exactly {operations:?}"),
            calls == operations,
            format!("observed {calls:?}"),
        )
    }

    /// The paths requested of the authorization server in the run, exactly
    /// and in order.
    pub fn expect_oauth_paths(&mut self, paths: &[&str]) -> &mut Self {
        let calls = self.focused().oauth_calls.clone();
        let observed: Vec<&str> = calls.iter().map(|call| call.path.as_str()).collect();
        self.check(
            &format!("authorization server requests are exactly {paths:?}"),
            observed == paths,
            format!("observed {calls:?}"),
        )
    }

    /// Request `call` to the authorization server carried every one of
    /// `params` with these values.
    pub fn expect_oauth_params(&mut self, call: usize, params: &[(&str, &str)]) -> &mut Self {
        let calls = self.focused().oauth_calls.clone();
        let observed = calls.get(call).map(|call| call.params.clone());
        self.check(
            &format!("authorization server request {call} carries {params:?}"),
            observed.as_ref().is_some_and(|observed| {
                params
                    .iter()
                    .all(|(name, value)| observed.get(*name).map(String::as_str) == Some(*value))
            }),
            format!("observed {observed:?}"),
        )
    }

    /// Every STS call of the run was signed for `region`: where STS was
    /// called, which a secret's own region must not move.
    pub fn expect_sts_signing_region(&mut self, region: &str) -> &mut Self {
        let calls = self.focused().sts_calls.clone();
        self.check(
            &format!("every STS call is signed for {region}"),
            !calls.is_empty()
                && calls
                    .iter()
                    .all(|call| call.signing_region.as_deref() == Some(region)),
            format!("observed {calls:?}"),
        )
    }

    /// The checks that failed so far, each as `name: detail`.
    pub fn failed_checks(&self) -> Vec<String> {
        self.reported_checks()
            .iter()
            .filter(|check| !check.ok)
            .map(|check| format!("{}: {}", check.name, check.detail))
            .collect()
    }

    pub fn expect_exit_code(&mut self, code: i32) -> &mut Self {
        let actual = self.focused().exit_code;
        self.check(
            &format!("exit code is {code}"),
            actual == Some(code),
            format!("observed exit code {actual:?}"),
        )
    }

    /// `error[CODE]` on stderr, the matching exit code, and no stdout.
    pub fn expect_error(&mut self, code: &str, exit_code: i32) -> &mut Self {
        self.expect_error_beside_stdout(code, exit_code)
            .expect_stdout_empty()
    }

    /// The failure line and the exit code, whatever stdout holds: for a
    /// command whose report is on stdout whether or not the run fails
    /// (`config check`), and whose scenario says what the report holds.
    pub fn expect_error_beside_stdout(&mut self, code: &str, exit_code: i32) -> &mut Self {
        let marker = format!("error[{code}]");
        let stderr = self.focused().stderr.clone();
        self.check(
            &format!("stderr reports {marker}"),
            stderr.contains(&marker),
            format!("observed stderr: {stderr}"),
        );
        self.expect_exit_code(exit_code)
    }

    /// One data JSON error with the requested code and no successful stdout.
    pub fn expect_json_error(&mut self, code: &str, exit_code: i32) -> &mut Self {
        self.expect_json_error_beside_stdout(code, exit_code)
            .expect_stdout_empty()
    }

    /// [`Self::expect_json_error`] whatever stdout holds; see
    /// [`Self::expect_error_beside_stdout`].
    pub fn expect_json_error_beside_stdout(&mut self, code: &str, exit_code: i32) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        let error = serde_json::from_str::<serde_json::Value>(&stderr);
        self.check(
            &format!("stderr reports JSON error {code}"),
            error
                .as_ref()
                .is_ok_and(|error| error["error"]["code"] == code),
            stderr,
        );
        self.expect_exit_code(exit_code)
    }

    /// The exact sequence of STS actions in the last run.
    pub fn expect_sts_actions(&mut self, actions: &[&str]) -> &mut Self {
        let index = self.focused_index();
        self.expect_run_sts_actions(index, actions)
    }

    /// The exact sequence of STS actions in run `index`.
    pub fn expect_run_sts_actions(&mut self, index: usize, actions: &[&str]) -> &mut Self {
        let actual: Vec<String> = self.observed.runs[index]
            .sts_calls
            .iter()
            .map(|call| call.action.clone())
            .collect();
        self.check(
            &format!("run {index}: STS calls are exactly {actions:?}"),
            actual == actions,
            format!("observed {actual:?}"),
        )
    }

    /// The exact sequence of `grant_type`s posted to the token endpoint in run `index`.
    pub fn expect_run_token_grants(&mut self, index: usize, grants: &[&str]) -> &mut Self {
        let actual: Vec<String> = self.observed.runs[index]
            .oauth_calls
            .iter()
            .filter(|call| call.path == "/oauth/token")
            .filter_map(|call| call.grant_type.clone())
            .collect();
        self.check(
            &format!("run {index}: token requests are exactly {grants:?}"),
            actual == grants,
            format!("observed {actual:?}"),
        )
    }

    /// The test-only token store holds exactly these sources after the runs.
    pub fn expect_token_store(&mut self, sources: &[String]) -> &mut Self {
        let observed = self.observed.token_store.clone();
        self.check(
            &format!("the token store holds exactly {sources:?}"),
            observed.as_deref() == Some(sources),
            format!("observed {observed:?}"),
        )
    }

    /// The exact sequence of `grant_type`s posted to the token endpoint in the last run.
    pub fn expect_token_grants(&mut self, grants: &[&str]) -> &mut Self {
        let index = self.focused_index();
        self.expect_run_token_grants(index, grants)
    }

    /// The API received exactly `count` requests in the last run, every one
    /// with this `Authorization` header (`None`: no header at all).
    pub fn expect_api_calls(&mut self, count: usize, authorization: Option<&str>) -> &mut Self {
        let calls = self.focused().api_calls.clone();
        let ok = calls.len() == count
            && calls
                .iter()
                .all(|call| call.authorization.as_deref() == authorization);
        self.check(
            &format!("the API receives {count} request(s) with authorization {authorization:?}"),
            ok,
            format!("observed {calls:?}"),
        )
    }

    /// The API received exactly `count` requests in the last run, every one
    /// with a valid SigV4 signature by the fake AssumeRole credentials for
    /// `service` in `region`, and the session token alongside.
    pub fn expect_sigv4_api_calls(
        &mut self,
        count: usize,
        service: &str,
        region: &str,
    ) -> &mut Self {
        self.expect_run_sigv4_api_calls(self.focused_index(), count, service, region)
    }

    pub fn expect_run_sigv4_api_calls(
        &mut self,
        index: usize,
        count: usize,
        service: &str,
        region: &str,
    ) -> &mut Self {
        let calls = self.observed.runs[index].api_calls.clone();
        let ok = calls.len() == count
            && calls.iter().all(|call| {
                call.sigv4.as_ref().is_some_and(|signature| {
                    signature.valid
                        && signature.service == service
                        && signature.region == region
                        && signature.security_token.as_deref() == Some(RESULT_TOKEN)
                })
            });
        self.check(
            &format!(
                "the API receives {count} request(s) validly signed with the role credentials for {service} in {region}"
            ),
            ok,
            format!("observed {calls:?}"),
        )
    }

    /// The description was requested exactly `count` times in run `index`,
    /// each time with (`conditional`) or without validators, and the fake
    /// answered `status` every time.
    pub fn expect_run_spec_calls(
        &mut self,
        index: usize,
        count: usize,
        conditional: bool,
        status: u16,
    ) -> &mut Self {
        let calls = self.observed.runs[index].spec_calls.clone();
        let ok = calls.len() == count
            && calls
                .iter()
                .all(|call| call.conditional == conditional && call.status == status);
        self.check(
            &format!(
                "run {index}: the description is requested {count} time(s), {}, answered {status}",
                if conditional {
                    "with If-None-Match / If-Modified-Since"
                } else {
                    "unconditionally"
                }
            ),
            ok,
            format!("observed {calls:?}"),
        )
    }

    /// Every secret read from AWS across all runs, in order: which store,
    /// which secret and in which region. An exact sequence is what says a
    /// secret two references name was read once and that nothing else was
    /// asked for; the region is what says the read went where the reference
    /// points, which both stores sharing one fake server cannot show.
    /// Every read is checked to be signed by the assumed role as well, because
    /// a read signed with the source profile's own key is the one thing a fake
    /// that answers anyway would never complain about.
    pub fn expect_secret_reads(&mut self, reads: &[(&str, &str, &str)]) -> &mut Self {
        let calls: Vec<SecretCall> = self
            .observed
            .runs
            .iter()
            .flat_map(|run| run.secret_calls.clone())
            .collect();
        let observed: Vec<(String, String, String)> = calls
            .iter()
            .map(|call| {
                (
                    call.operation.clone(),
                    call.id.clone(),
                    call.region.clone().unwrap_or_default(),
                )
            })
            .collect();
        let expected: Vec<(String, String, String)> = reads
            .iter()
            .map(|(operation, id, region)| {
                (
                    (*operation).to_owned(),
                    (*id).to_owned(),
                    (*region).to_owned(),
                )
            })
            .collect();
        self.check(
            &format!("AWS is asked for exactly these secrets: {reads:?}"),
            observed == expected,
            format!("observed {calls:?}"),
        );
        // A SecureString read without it answers its ciphertext, which a
        // fake that answers anyway would pass on as the secret.
        let decrypted = calls
            .iter()
            .filter(|call| call.operation == "ssm:GetParameter")
            .all(|call| call.with_decryption == Some(true));
        self.check(
            "every Parameter Store read asks for WithDecryption=true",
            decrypted,
            format!("observed {calls:?}"),
        );
        let signed_by_the_role = calls
            .iter()
            .all(|call| call.signing_access_key.as_deref() == Some(RESULT_ACCESS_KEY));
        self.check(
            "every secret is read with the assumed role's credentials",
            signed_by_the_role,
            format!("observed {calls:?}"),
        )
    }

    /// Exactly `count` of the run's `op` calls contain `needle`.
    pub fn expect_op_calls_containing(&mut self, needle: &str, count: usize) -> &mut Self {
        let calls = self.focused().op_calls.clone();
        let actual = calls.iter().filter(|call| call.contains(needle)).count();
        self.check(
            &format!("{count} 1Password call(s) contain {needle:?}"),
            actual == count,
            format!("observed {calls:?}"),
        )
    }

    /// The console federation requests of the run, exactly: the action and
    /// the access key inside the `Session` parameter of each.
    pub fn expect_federation_calls(&mut self, calls: &[(&str, &str)]) -> &mut Self {
        let observed = self.focused().federation_calls.clone();
        self.check(
            &format!("federation requests are exactly {calls:?}"),
            observed.len() == calls.len()
                && observed.iter().zip(calls).all(|(call, (action, key))| {
                    call.action.as_deref() == Some(*action)
                        && call.session_access_key.as_deref() == Some(*key)
                }),
            format!("observed {observed:?}"),
        )
    }

    /// Exactly `calls.len()` URLs were handed to the browser, and URL N
    /// contains every part of `calls[N]`.
    pub fn expect_open_calls_containing(&mut self, calls: &[&[&str]]) -> &mut Self {
        let observed = self.focused().open_calls.clone();
        self.check(
            &format!(
                "the browser is opened {} time(s), each URL containing {calls:?}",
                calls.len()
            ),
            observed.len() == calls.len()
                && observed
                    .iter()
                    .zip(calls)
                    .all(|(url, parts)| parts.iter().all(|part| url.contains(part))),
            format!("observed {observed:?}"),
        )
    }

    /// stdout has exactly `count` lines.
    pub fn expect_stdout_line_count(&mut self, count: usize) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            &format!("stdout has {count} line(s)"),
            stdout.lines().count() == count,
            format!("observed {stdout:?}"),
        )
    }

    pub fn expect_op_calls(&mut self, count: usize) -> &mut Self {
        let actual = self.focused().op_calls.len();
        let calls = self.focused().op_calls.clone();
        self.check(
            &format!("1Password is consulted {count} time(s)"),
            actual == count,
            format!("observed {calls:?}"),
        )
    }

    pub fn expect_stdout_is_export_script(&mut self) -> &mut Self {
        let run = self.focused();
        let ok = !run.stdout.is_empty()
            && run
                .stdout
                .lines()
                .all(|line| line.starts_with("export ") || line.starts_with("unset "));
        let exported = run.exported_variables.clone();
        self.check(
            "stdout contains only unset/export lines",
            ok,
            format!("exported {exported:?}"),
        )
    }

    pub fn expect_stdout_empty(&mut self) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            "stdout is empty",
            stdout.is_empty(),
            format!("stdout: {stdout:?}"),
        )
    }

    pub fn expect_stdout_contains(&mut self, needle: &str) -> &mut Self {
        let ok = self.focused().stdout.contains(needle);
        self.check(&format!("stdout contains {needle:?}"), ok, "")
    }

    /// No ANSI escape sequence on stdout (the binary is never given a TTY here).
    pub fn expect_stdout_plain_text(&mut self) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            "stdout has no ANSI escape sequences",
            !stdout.contains('\x1b'),
            format!("stdout: {stdout:?}"),
        )
    }

    /// stderr carries none of `needle`: a credential that must not be in an
    /// error line.
    /// stderr has exactly `count` lines: one error document, one error line.
    pub fn expect_stderr_line_count(&mut self, count: usize) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        let actual = stderr.lines().count();
        self.check(
            &format!("stderr has {count} line(s)"),
            actual == count,
            format!("observed {actual} line(s): {stderr:?}"),
        )
    }

    /// Exactly `count` stderr lines contain `needle`: one line per event, so
    /// a line printed twice or not at all fails where `contains` passes.
    pub fn expect_stderr_lines_containing(&mut self, needle: &str, count: usize) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        let actual = stderr.lines().filter(|line| line.contains(needle)).count();
        self.check(
            &format!("{count} stderr line(s) contain {needle:?}"),
            actual == count,
            format!("observed {actual} line(s): {stderr:?}"),
        )
    }

    /// `path` does not exist after the runs: a file a failed call must not
    /// have created.
    pub fn expect_path_absent(&mut self, path: &str) -> &mut Self {
        self.check(
            &format!("{path} does not exist"),
            !Path::new(path).exists(),
            format!("exists: {}", Path::new(path).exists()),
        )
    }

    /// `path` holds exactly `before` after the runs: a database every refused
    /// write left byte for byte what it was. `!bytes.is_empty()` once stood
    /// for this and could not fail; a DROP TABLE leaves a file that is not
    /// empty.
    pub fn expect_file_unchanged(&mut self, path: &str, before: &[u8]) -> &mut Self {
        let now = std::fs::read(path).unwrap_or_default();
        self.check(
            &format!(
                "{path} is byte for byte what it was ({} bytes)",
                before.len()
            ),
            now == before,
            format!("observed {} bytes", now.len()),
        )
    }

    /// stderr is empty: no progress, no log, no diagnostic.
    pub fn expect_stderr_empty(&mut self) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        self.check(
            "stderr is empty",
            stderr.is_empty(),
            format!("observed {stderr:?}"),
        )
    }

    pub fn expect_stderr_excludes(&mut self, needle: &str) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        self.check(
            &format!("stderr excludes {needle:?}"),
            !stderr.contains(needle),
            format!("observed {stderr:?}"),
        )
    }

    /// Every API request of the last run carries `headers` (names
    /// lower-cased) with exactly these values.
    pub fn expect_api_call_headers(&mut self, headers: &[(&str, &str)]) -> &mut Self {
        let calls = self.focused().api_calls.clone();
        let observed: Vec<BTreeMap<&str, &str>> = calls
            .iter()
            .map(|call| {
                headers
                    .iter()
                    .map(|(name, _)| (*name, call.headers.get(*name).map_or("", String::as_str)))
                    .collect()
            })
            .collect();
        let ok = !calls.is_empty()
            && observed.iter().all(|call| {
                headers
                    .iter()
                    .all(|(name, value)| call.get(name) == Some(value))
            });
        self.check(
            &format!("every API request carries the headers {headers:?}"),
            ok,
            format!("observed {observed:?}"),
        )
    }

    pub fn expect_stderr_contains(&mut self, needle: &str) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        self.check(
            &format!("stderr contains {needle:?}"),
            stderr.contains(needle),
            format!("stderr: {stderr}"),
        )
    }

    /// Nothing under the sandbox was created or modified by the run.
    pub fn expect_no_files_written(&mut self) -> &mut Self {
        let files = self.observed.files_written.clone();
        self.check(
            "no file written under HOME or TMPDIR",
            files.is_empty(),
            format!("written: {files:?}"),
        )
    }

    /// The env script hand-off file exists and exports these variables.
    pub fn expect_env_script_exports(&mut self, variables: &[&str]) -> &mut Self {
        let exported = self
            .observed
            .env_script
            .as_ref()
            .map(|script| script.exported_variables.clone());
        let ok = exported
            .as_ref()
            .is_some_and(|exported| variables.iter().all(|v| exported.iter().any(|e| e == v)));
        self.check(
            &format!("the env script exports {variables:?}"),
            ok,
            format!("env script: {exported:?}"),
        )
    }

    /// The checks as the report and the failure message show them. On a
    /// layer other than `fake` the stdout of a run is a real service's answer
    /// -- an export script with a real role's secret key -- so a detail that
    /// quotes it says how long it was instead.
    fn reported_checks(&self) -> Vec<Check> {
        let mut checks = self.checks.clone();
        if self.evidence == "fake" {
            return checks;
        }
        for run in &self.observed.runs {
            if run.stdout.is_empty() {
                continue;
            }
            let shown = format!(
                "<{} bytes of stdout, not shown on the {} layer>",
                run.stdout.len(),
                self.evidence
            );
            for check in &mut checks {
                check.detail = check
                    .detail
                    .replace(&format!("{:?}", run.stdout), &shown)
                    .replace(&run.stdout, &shown);
            }
        }
        checks
    }

    /// Writes `target/agent/scenarios/<id>.json` and panics with every failed
    /// check so `cargo test` reports the scenario as failed.
    pub fn finish(self) {
        let checks = self.reported_checks();
        let passed = checks.iter().all(|check| check.ok);
        let report = serde_json::json!({
            "schema_version": 2,
            "scenario": self.id,
            "feature": self.feature,
            "combination": self.combination,
            "evidence": self.evidence,
            "command": self.focused().command,
            "passed": passed,
            "checks": checks,
            "observed": self.observed,
            "seeded": self.seeded,
        });
        let dir = layer_report_dir(self.evidence);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", self.id)),
            serde_json::to_string_pretty(&report).unwrap(),
        )
        .unwrap();

        let failures: Vec<String> = checks
            .iter()
            .filter(|check| !check.ok)
            .map(|check| format!("- {}: {}", check.name, check.detail))
            .collect();
        let last = self.focused();
        let stdout = if self.evidence == "fake" {
            last.stdout.clone()
        } else {
            format!(
                "<{} bytes, not shown on the {} layer>",
                last.stdout.len(),
                self.evidence
            )
        };
        assert!(
            passed,
            "scenario {} failed:\n{}\nstdout:\n{}\nstderr:\n{}",
            self.id,
            failures.join("\n"),
            stdout,
            last.stderr
        );
    }
}

/// `target/agent`, honoring `CARGO_TARGET_DIR`.
pub fn agent_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
        .join("agent")
}

/// `target/agent/scenarios`, honoring `CARGO_TARGET_DIR`: the fake evidence.
pub fn report_dir() -> PathBuf {
    layer_report_dir("fake")
}

/// Where a report with this `evidence` goes: `scenarios/` for `fake`,
/// `scenarios-<evidence>/` for every other layer, which is what
/// `cargo xtask verify-matrix` reads back by that name.
pub fn layer_report_dir(evidence: &str) -> PathBuf {
    if evidence == "fake" {
        agent_dir().join("scenarios")
    } else {
        agent_dir().join(format!("scenarios-{evidence}"))
    }
}

/// Text that looks like a credential a real run could have left behind: an
/// AWS access key id, a PEM block, a bearer token. The real layer's secrets
/// are not seeded, so this is what the on-disk check has to go by.
pub fn looks_like_a_credential(text: &str) -> bool {
    let key_id = text
        .match_indices("AKIA")
        .chain(text.match_indices("ASIA"))
        .any(|(at, _)| {
            text[at + 4..]
                .chars()
                .take(16)
                .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                .count()
                == 16
        });
    key_id || text.contains("-----BEGIN ") || text.contains("Bearer ")
}

fn kurama_config_toml(scenario: &Scenario) -> String {
    if scenario.real_aws {
        return real_aws_config_toml(scenario);
    }
    let op_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fakes/op");
    let onepassword_enabled = scenario.onepassword != OnePassword::Disabled;
    let mut config = format!(
        "[aws.session_cache]\nenabled = {}\n\n\
         [onepassword]\nenabled = {onepassword_enabled}\ncli_path = \"{}\"\nitem_name = \"aws-agent\"\n",
        scenario.session_cache,
        op_path.display()
    );
    if matches!(
        scenario.onepassword,
        OnePassword::Hangs | OnePassword::VersionHangs
    ) {
        // Short enough that the scenario is quick, long enough to be reached.
        config.push_str("timeout = 2\n");
    }
    if scenario.service_account_keychain {
        config.push_str(&format!(
            "service_account_keychain = \"{SERVICE_ACCOUNT_KEYCHAIN}\"\n"
        ));
    }
    for extra in &scenario.extra_config {
        config.push('\n');
        config.push_str(extra);
        config.push('\n');
    }
    config
}

/// The configuration of a run against AWS itself. The real layer
/// (`cargo xtask verify --layer real`) names the copy of the person's daily
/// configuration in `KURAMA_REAL_BASE_CONFIG`, and that whole file is the
/// base: its `[onepassword]` answers the MFA prompt without a person and its
/// `[auth.*]` sections are what a case's `requires` may name. Otherwise (the
/// throwaway layer) only the `[onepassword]` section of the person's own
/// configuration (`KURAMA_REAL_CONFIG`, else `~/.config/kurama/config.toml`)
/// is taken, with the session cache on: an `[api.*]` or `[db.*]` of the
/// person's is not what such a case verifies. The sections the case adds
/// follow either way.
fn real_aws_config_toml(scenario: &Scenario) -> String {
    let mut config = match std::env::var_os("KURAMA_REAL_BASE_CONFIG") {
        Some(path) => std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "KURAMA_REAL_BASE_CONFIG={}: {error}",
                Path::new(&path).display()
            )
        }),
        None => {
            let real = std::env::var_os("KURAMA_REAL_CONFIG")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|home| PathBuf::from(home).join(".config/kurama/config.toml"))
                });
            let onepassword = real
                .and_then(|path| std::fs::read_to_string(path).ok())
                .map(|text| onepassword_section(&text))
                .unwrap_or_default();
            format!("[aws.session_cache]\nenabled = true\n\n{onepassword}")
        }
    };
    for extra in &scenario.extra_config {
        config.push('\n');
        config.push_str(extra);
        config.push('\n');
    }
    config
}

/// The `[onepassword]` table of a configuration, up to its next table, or
/// nothing when the file has none (MFA then asks a terminal, and a run
/// without one fails the way the invariant says).
pub(crate) fn onepassword_section(config: &str) -> String {
    let mut out = String::new();
    let mut keep = false;
    for line in config.lines() {
        if line.trim_start().starts_with('[') {
            keep = line.trim() == "[onepassword]";
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The fake token endpoint; a device flow is pending on its first poll.
struct TokenEndpoint {
    fake: OAuthFake,
    device_polls: AtomicUsize,
}

impl Respond for TokenEndpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let params: BTreeMap<String, String> = url::form_urlencoded::parse(&request.body)
            .into_owned()
            .collect();
        match self.fake {
            OAuthFake::Unavailable => ResponseTemplate::new(500).set_body_string("upstream down"),
            OAuthFake::DevicePending { .. } => ResponseTemplate::new(400)
                .set_body_json(serde_json::json!({"error": "authorization_pending"})),
            OAuthFake::Rejected { error } => ResponseTemplate::new(400).set_body_json(
                serde_json::json!({"error": error, "error_description": "rejected by the fake"}),
            ),
            OAuthFake::Success => {
                let grant = params.get("grant_type").map(String::as_str).unwrap_or("");
                if grant.ends_with(":device_code")
                    && self.device_polls.fetch_add(1, Ordering::SeqCst) == 0
                {
                    return ResponseTemplate::new(400)
                        .set_body_json(serde_json::json!({"error": "authorization_pending"}));
                }
                let refreshing = grant == "refresh_token";
                let known = [FAKE_REFRESH_TOKEN, ROTATED_REFRESH_TOKEN];
                if refreshing
                    && !params
                        .get("refresh_token")
                        .is_some_and(|token| known.contains(&token.as_str()))
                {
                    return ResponseTemplate::new(400)
                        .set_body_json(serde_json::json!({"error": "invalid_grant"}));
                }
                // A refresh rotates the refresh token, as GitHub and Google do.
                let (access_token, refresh_token) = if refreshing {
                    (REFRESHED_ACCESS_TOKEN, ROTATED_REFRESH_TOKEN)
                } else {
                    (GRANTED_ACCESS_TOKEN, FAKE_REFRESH_TOKEN)
                };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": access_token,
                    "token_type": "bearer",
                    "expires_in": 3600,
                    "refresh_token": refresh_token,
                    "scope": params.get("scope"),
                }))
            }
        }
    }
}

/// The fake API: echoes the request as JSON.
struct ApiEndpoint {
    fake: ApiFake,
    calls: AtomicUsize,
}

impl Respond for ApiEndpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let unauthorized = || {
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"message": "Bad credentials"}))
        };
        match self.fake {
            ApiFake::Unauthorized => return unauthorized(),
            ApiFake::UnauthorizedOnce if call == 0 => return unauthorized(),
            ApiFake::NotFound => {
                return ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({"message": "Not Found"}));
            }
            ApiFake::Echo => {
                let query: BTreeMap<String, String> =
                    request.url.query_pairs().into_owned().collect();
                let status: u16 = query.get("status").map_or(200, |s| s.parse().unwrap());
                let delay: u64 = query.get("delay").map_or(0, |s| s.parse().unwrap());
                // The body, not the URL, carries the cookies: `-v` prints the
                // URL, and a scenario checks the value never reaches stderr.
                let body = String::from_utf8_lossy(&request.body);
                let cookies = body.lines().filter(|_| query.contains_key("set_cookies"));
                return cookies
                    .fold(ResponseTemplate::new(status), |template, cookie| {
                        template.append_header("Set-Cookie", cookie)
                    })
                    .set_body_raw(request.body.clone(), "application/octet-stream")
                    .set_delay(Duration::from_secs(delay));
            }
            ApiFake::Pages => return paged_response(request),
            ApiFake::GraphQl => return graphql_response(request),
            ApiFake::CompactArray => {
                return ResponseTemplate::new(200).set_body_raw(
                    format!(
                        r#"[{{"login":"octocat","path":"{}","amount":10.00}}]"#,
                        request.url.path()
                    ),
                    "application/json",
                );
            }
            _ => {}
        }
        let mut body = serde_json::json!({
            "path": request.url.path(),
            "method": request.method.to_string(),
            "authorization": request.headers.get("authorization").and_then(|v| v.to_str().ok()),
            "body": String::from_utf8_lossy(&request.body),
            "login": "octocat",
        });
        if let ApiFake::Padding(bytes) = self.fake {
            body["padding"] = "x".repeat(bytes).into();
        }
        let template = ResponseTemplate::new(200).set_body_json(body);
        if self.fake == ApiFake::Slow {
            template.set_delay(Duration::from_secs(3))
        } else {
            template
        }
    }
}

/// `ApiFake::GraphQl`: the answer to the document and variables received.
fn graphql_response(request: &Request) -> ResponseTemplate {
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
    let missing = sent["variables"]["id"]
        .as_str()
        .filter(|id| id.starts_with("missing"));
    let body = match missing {
        Some(id) => serde_json::json!({"data": null, "errors": [
            {"message": format!("Entity not found: {id}"), "path": ["issue"]},
            {"message": "a second error"}
        ]}),
        None => {
            serde_json::json!({"data": {"query": sent["query"], "variables": sent["variables"]}})
        }
    };
    ResponseTemplate::new(200).set_body_json(body)
}

/// `ApiFake::Pages`: the page the query asks for.
fn paged_response(request: &Request) -> ResponseTemplate {
    let query: BTreeMap<String, String> = request.url.query_pairs().into_owned().collect();
    let number =
        |name: &str, default: usize| query.get(name).map_or(default, |v| v.parse().unwrap());
    let style = query.get("style").map_or("link", String::as_str);
    let pages = number("pages", 3);
    let page = match style {
        "cursor" => query
            .get("cursor")
            .map_or(1, |cursor| cursor.trim_start_matches('c').parse().unwrap()),
        _ => number("page", 1),
    };
    if query
        .get("fail")
        .is_some_and(|fail| fail.parse() == Ok(page))
    {
        return ResponseTemplate::new(500)
            .set_body_json(serde_json::json!({"message": format!("page {page} failed")}));
    }
    let items: Vec<String> = (1..=2).map(|item| format!("item-{page}-{item}")).collect();
    let mut body = serde_json::json!({"page": page, "items": items});
    let mut template = ResponseTemplate::new(200);
    let bare = query.contains_key("bare_last") && page == pages;
    match style {
        _ if bare => {}
        "link" => {
            let at = |page: usize| {
                let mut url = request.url.clone();
                let kept: Vec<(String, String)> = query
                    .iter()
                    .filter(|(name, _)| *name != "page")
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect();
                url.query_pairs_mut()
                    .clear()
                    .extend_pairs(kept)
                    .append_pair("page", &page.to_string());
                format!("{}?{}", url.path(), url.query().unwrap_or_default())
            };
            let mut links = Vec::new();
            if page > 1 {
                links.push(format!("<{}>; rel=\"prev\"", at(page - 1)));
            }
            if page < pages {
                links.push(format!("<{}>; rel=\"next\"", at(page + 1)));
            }
            if !links.is_empty() {
                template = template.insert_header("Link", links.join(", ").as_str());
            }
        }
        "cursor" => {
            let next = (page < pages).then(|| format!("c{}", page + 1));
            body["meta"] = serde_json::json!({"next": next});
        }
        _ => {}
    }
    template.set_body_json(body)
}

async fn start_fakes(scenario: &Scenario, spec_statuses: Arc<Mutex<Vec<u16>>>) -> MockServer {
    let server = MockServer::start().await;
    if scenario.feature == "local-analytics" {
        data::mount(&server).await;
    }
    if scenario.feature == "s3-explorer" {
        s3_browse::mount(&server).await;
    }
    match scenario.sts {
        StsFake::Success | StsFake::ExpiresIn(_) => {
            let lifetime = match scenario.sts {
                StsFake::ExpiresIn(seconds) => Some(seconds),
                _ => None,
            };
            Mock::given(method("POST"))
                .and(path("/"))
                .and(body_string_contains("Action=AssumeRole"))
                .respond_with(move |_: &Request| {
                    let credentials =
                        credentials_xml(RESULT_ACCESS_KEY, RESULT_SECRET, RESULT_TOKEN);
                    let credentials = match lifetime {
                        Some(seconds) => credentials.replace(
                            "2030-01-01T12:00:00Z",
                            &(chrono::Utc::now() + chrono::Duration::seconds(seconds))
                                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                        ),
                        None => credentials,
                    };
                    ResponseTemplate::new(200)
                        .set_body_string(response_xml("AssumeRole", &credentials))
                })
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/"))
                .and(body_string_contains("Action=GetSessionToken"))
                .respond_with(SessionTokenEndpoint::default())
                .mount(&server)
                .await;
        }
        StsFake::Error { code, message } => {
            Mock::given(method("POST"))
                .and(path("/"))
                .respond_with(ResponseTemplate::new(403).set_body_string(error_xml(code, message)))
                .mount(&server)
                .await;
        }
    }
    // A bastion that is online and a session the fake plugin is handed. The
    // answers carry nothing of the request: what a scenario reads is which
    // operations were asked for, from the requests themselves.
    for (operation, body) in [
        (
            "DescribeInstanceInformation",
            serde_json::json!({"InstanceInformationList": [
                {"InstanceId": "i-0123456789abcdef0", "PingStatus": "Online"}
            ]}),
        ),
        (
            "StartSession",
            serde_json::json!({
                "SessionId": "s-kurama",
                "TokenValue": "fake-ssm-session-token",
                "StreamUrl": "wss://example.invalid/"
            }),
        ),
        (
            "TerminateSession",
            serde_json::json!({"SessionId": "s-kurama"}),
        ),
    ] {
        Mock::given(method("POST"))
            .and(path("/"))
            .and(wiremock::matchers::header(
                "x-amz-target",
                format!("AmazonSSM.{operation}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    mount_secret_stores(&server, scenario.aws_secrets).await;
    let federation = if scenario.federation_fails {
        ResponseTemplate::new(500)
    } else if scenario.federation_invalid_json {
        ResponseTemplate::new(200).set_body_string("<html>not a sign-in token</html>")
    } else {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "SigninToken": SIGNIN_TOKEN
        }))
    };
    Mock::given(method("GET"))
        .and(path("/federation"))
        .respond_with(federation)
        .mount(&server)
        .await;

    let base = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/oauth/authorize"),
            "token_endpoint": format!("{base}/oauth/token"),
            "device_authorization_endpoint": format!("{base}/oauth/device"),
        })))
        .mount(&server)
        .await;
    let (expires_in, interval) = match scenario.oauth {
        OAuthFake::DevicePending {
            expires_in,
            interval,
        } => (expires_in, interval),
        _ => (60, 1),
    };
    Mock::given(method("POST"))
        .and(path("/oauth/device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "fake-device-code",
            "user_code": FAKE_USER_CODE,
            "verification_uri": format!("{base}/device"),
            "expires_in": expires_in,
            "interval": interval,
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(TokenEndpoint {
            fake: scenario.oauth,
            device_polls: AtomicUsize::new(0),
        })
        .mount(&server)
        .await;
    Mock::given(path_regex("^/api/.*"))
        .respond_with(ApiEndpoint {
            fake: scenario.api,
            calls: AtomicUsize::new(0),
        })
        .mount(&server)
        .await;
    Mock::given(path_regex("^/spec/.*"))
        .respond_with(SpecEndpoint {
            fake: scenario.spec,
            statuses: spec_statuses,
        })
        .mount(&server)
        .await;
    server
}

/// The fake Secrets Manager and Parameter Store. Each answer is built from the
/// id the request asked for, so a scenario that reads two secrets can say which
/// value came from which -- a constant would pass for either.
async fn mount_secret_stores(server: &MockServer, fake: AwsSecrets) {
    if fake == AwsSecrets::Absent {
        return;
    }
    for target in ["secretsmanager.GetSecretValue", "AmazonSSM.GetParameter"] {
        Mock::given(method("POST"))
            .and(path("/"))
            .and(wiremock::matchers::header("x-amz-target", target))
            .respond_with(SecretStoreEndpoint { fake })
            .mount(server)
            .await;
    }
}

struct SecretStoreEndpoint {
    fake: AwsSecrets,
}

impl Respond for SecretStoreEndpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
        let parameter = request
            .headers
            .get("x-amz-target")
            .and_then(|value| value.to_str().ok())
            == Some("AmazonSSM.GetParameter");
        let id = secret_id(&body).unwrap_or_default();
        let refusal = match self.fake {
            AwsSecrets::AccessDenied => Some((
                "AccessDeniedException",
                format!("is not authorized to perform this action on {id}"),
            )),
            AwsSecrets::NotFound if parameter => {
                Some(("ParameterNotFound", format!("Parameter {id} not found.")))
            }
            AwsSecrets::NotFound => Some((
                "ResourceNotFoundException",
                format!("Secrets Manager can't find the specified secret {id}."),
            )),
            AwsSecrets::Absent | AwsSecrets::FromId => None,
        };
        if let Some((code, message)) = refusal {
            let service = if parameter {
                "AmazonSSM"
            } else {
                "secretsmanager"
            };
            return ResponseTemplate::new(400)
                .insert_header("x-amzn-errortype", code)
                .set_body_json(serde_json::json!({
                    "__type": format!("{service}#{code}"),
                    "message": message,
                }));
        }
        if parameter {
            return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Parameter": {
                    "Name": id,
                    "Type": "SecureString",
                    "Value": format!("parameter-of{id}"),
                }
            }));
        }
        if id.ends_with("-binary") {
            return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Name": id,
                "SecretBinary": "AAECAw==",
            }));
        }
        if id.ends_with("-text") {
            return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Name": id,
                "SecretString": format!("plain-secret-of-{id}"),
            }));
        }
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": id,
            "SecretString": serde_json::json!({
                "username": format!("user-of-{id}"),
                "password": format!("password-of-{id}"),
            })
            .to_string(),
        }))
    }
}

/// The secret or parameter a request asked for, whichever field names it.
fn secret_id(body: &serde_json::Value) -> Option<String> {
    ["SecretId", "Name"]
        .iter()
        .find_map(|field| body.get(field)?.as_str().map(str::to_owned))
}

/// The fake description server: a fixture with validators. Every answer's
/// status is recorded so a scenario can tell a 304 from a re-download.
struct SpecEndpoint {
    fake: SpecFake,
    statuses: Arc<Mutex<Vec<u16>>>,
}

impl Respond for SpecEndpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let (status, response) = self.answer(request);
        self.statuses.lock().unwrap().push(status);
        response
    }
}

impl SpecEndpoint {
    fn answer(&self, request: &Request) -> (u16, ResponseTemplate) {
        if self.fake == SpecFake::Down {
            return (
                503,
                ResponseTemplate::new(503).set_body_string("spec server down"),
            );
        }
        let name = request.url.path().trim_start_matches("/spec/").to_string();
        let Ok(body) = std::fs::read(fixtures_dir().join(&name)) else {
            return (
                404,
                ResponseTemplate::new(404).set_body_string("no such fixture"),
            );
        };
        let etag = format!("\"{}-{}\"", name, body.len());
        let matches = request
            .headers
            .get("if-none-match")
            .and_then(|value| value.to_str().ok())
            == Some(etag.as_str());
        if matches {
            return (
                304,
                ResponseTemplate::new(304).insert_header("ETag", etag.as_str()),
            );
        }
        let content_type = if name.ends_with(".yaml") || name.ends_with(".yml") {
            "application/yaml"
        } else {
            "application/json"
        };
        (
            200,
            ResponseTemplate::new(200)
                .insert_header("ETag", etag.as_str())
                .insert_header("Last-Modified", "Wed, 17 Sep 2026 00:00:00 GMT")
                .insert_header("Content-Type", content_type)
                .set_body_bytes(body),
        )
    }
}

/// GetSessionToken answering the nth call of a scenario with the session key
/// `ASIASESSIONKEY0000<n>`: the first is `SESSION_ACCESS_KEY`, the second
/// `SECOND_SESSION_ACCESS_KEY`, so a session that should have been replaced
/// cannot pass for its replacement.
#[derive(Default)]
struct SessionTokenEndpoint {
    calls: AtomicUsize,
}

impl Respond for SessionTokenEndpoint {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let access_key = format!("ASIASESSIONKEY{call:05}");
        ResponseTemplate::new(200).set_body_string(response_xml(
            "GetSessionToken",
            &credentials_xml(&access_key, SESSION_SECRET, SESSION_TOKEN),
        ))
    }
}

fn response_xml(operation: &str, result: &str) -> String {
    let ns = "https://sts.amazonaws.com/doc/2011-06-15/";
    format!(
        "<{operation}Response xmlns=\"{ns}\"><{operation}Result>{result}</{operation}Result>\
         <ResponseMetadata><RequestId>fake</RequestId></ResponseMetadata></{operation}Response>"
    )
}

fn credentials_xml(access_key: &str, secret: &str, token: &str) -> String {
    format!(
        "<Credentials><AccessKeyId>{access_key}</AccessKeyId>\
         <SecretAccessKey>{secret}</SecretAccessKey><SessionToken>{token}</SessionToken>\
         <Expiration>2030-01-01T12:00:00Z</Expiration></Credentials>"
    )
}

fn error_xml(code: &str, message: &str) -> String {
    format!(
        "<ErrorResponse><Error><Type>Sender</Type><Code>{code}</Code>\
         <Message>{message}</Message></Error><RequestId>fake</RequestId></ErrorResponse>"
    )
}

/// The credential scope of a SigV4 `Authorization` header:
/// "AWS4-HMAC-SHA256 Credential=<access key>/<date>/<region>/<service>/aws4_request, ...".
/// One decoder for every signed fake, so a store added later cannot record
/// less about its caller than STS does.
fn signing_scope(authorization: Option<&str>) -> (Option<String>, Option<String>) {
    let scope = authorization.and_then(|header| header.split("Credential=").nth(1));
    let part = |index: usize| {
        scope
            .and_then(|rest| rest.split('/').nth(index))
            .map(str::to_string)
    };
    (part(0), part(2))
}

fn decode_sts_call(body: &[u8], authorization: Option<&str>) -> StsCall {
    let params: BTreeMap<String, String> = url::form_urlencoded::parse(body).into_owned().collect();
    let policy_arns = params
        .iter()
        .filter(|(key, _)| key.starts_with("PolicyArns.member."))
        .map(|(_, value)| value.clone())
        .collect();
    let (signing_access_key, signing_region) = signing_scope(authorization);
    StsCall {
        action: params.get("Action").cloned().unwrap_or_default(),
        signing_access_key,
        signing_region,
        role_arn: params.get("RoleArn").cloned(),
        serial_number: params.get("SerialNumber").cloned(),
        token_code: params.get("TokenCode").cloned(),
        policy_arns,
        duration_seconds: params.get("DurationSeconds").cloned(),
        role_session_name: params.get("RoleSessionName").cloned(),
    }
}

fn decode_federation_call(query: &str) -> FederationCall {
    let params: BTreeMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let session_access_key = params
        .get("Session")
        .and_then(|session| serde_json::from_str::<serde_json::Value>(session).ok())
        .and_then(|session| session["sessionId"].as_str().map(str::to_string));
    FederationCall {
        action: params.get("Action").cloned(),
        session_access_key,
    }
}

type Stamp = (u64, Option<SystemTime>);

/// Every file under `root` with its size and modification time, and the
/// paths that could not be listed: a directory or entry the walk cannot read
/// hides whatever it holds.
pub(crate) fn snapshot(root: &Path) -> (BTreeMap<PathBuf, Stamp>, Vec<PathBuf>) {
    let mut files = BTreeMap::new();
    let mut unlisted = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            unlisted.push(dir);
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                unlisted.push(dir.clone());
                continue;
            };
            let path = entry.path();
            match entry.metadata() {
                Ok(metadata) if metadata.is_dir() => pending.push(path),
                Ok(metadata) => {
                    files.insert(path, (metadata.len(), metadata.modified().ok()));
                }
                Err(_) => unlisted.push(path),
            }
        }
    }
    (files, unlisted)
}

/// The regular files among `paths` whose bytes hold one of `secrets` (or, with
/// `any_credential`, look like a credential), and the files that could not be
/// read. A symbolic link or a socket is not read: the link's target is a file
/// of its own, and a socket holds nothing.
pub(crate) fn scan_for_secrets<'a>(
    paths: impl Iterator<Item = &'a PathBuf>,
    secrets: &[String],
    any_credential: bool,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut found = Vec::new();
    let mut unread = Vec::new();
    for path in paths {
        let bytes = match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.is_file() => continue,
            Ok(_) => std::fs::read(path),
            Err(error) => Err(error),
        };
        let Ok(bytes) = bytes else {
            unread.push(path.clone());
            continue;
        };
        let holds = |needle: &[u8]| bytes.windows(needle.len()).any(|window| window == needle);
        if secrets.iter().any(|secret| holds(secret.as_bytes()))
            || (any_credential && looks_like_a_credential(&String::from_utf8_lossy(&bytes)))
        {
            found.push(path.clone());
        }
    }
    (found, unread)
}

/// The fields an audit line may hold: `AuditEntry`'s, and nothing a request
/// carried beyond its method and its path.
const AUDIT_FIELDS: [&str; 12] = [
    "time",
    "command",
    "target",
    "agent",
    "method",
    "path",
    "status",
    "program",
    "sql_sha256",
    "exit_code",
    "error_code",
    "duration_ms",
];

/// What is wrong with the audit log generations in `directory`: a line that
/// is not a JSON object, a field outside [`AUDIT_FIELDS`], or a path with a
/// query or a fragment. No log is nothing wrong.
fn audit_log_problems(directory: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for name in ["audit.jsonl", "audit.jsonl.1"] {
        let Ok(content) = std::fs::read_to_string(directory.join(name)) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            let Ok(serde_json::Value::Object(entry)) = serde_json::from_str(line) else {
                problems.push(format!("{name} line {}: not a JSON object", index + 1));
                continue;
            };
            for key in entry
                .keys()
                .filter(|key| !AUDIT_FIELDS.contains(&key.as_str()))
            {
                problems.push(format!("{name} line {}: field {key:?}", index + 1));
            }
            if let Some(path) = entry.get("path").and_then(|path| path.as_str())
                && path.contains(['?', '#'])
            {
                problems.push(format!("{name} line {}: path {path:?}", index + 1));
            }
        }
    }
    problems
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Decode the simple names used by completion scenarios (without escaped delimiters).
pub fn completion_values(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .map(|line| line.split_once(':').map_or(line, |(name, _)| name))
        .collect()
}

/// The directory a sandbox lives in. The session manager plugin opens a
/// unix socket named `<fnv32 of the session id>_session_manager_plugin_mux.sock`
/// in `TMPDIR`, and macOS refuses a socket path longer than 103 bytes: under
/// the default macOS temp directory the sandbox's `tmp` leaves too little room
/// for that name. A run against AWS itself starts the real plugin, so its
/// sandbox lives under `/tmp` with a short name.
fn sandbox_root(real_aws: bool) -> tempfile::TempDir {
    if real_aws {
        tempfile::Builder::new()
            .prefix("k")
            .tempdir_in("/tmp")
            .expect("sandbox")
    } else {
        tempfile::tempdir().expect("sandbox")
    }
}

#[cfg(test)]
mod tests {
    use super::sandbox_root;

    #[test]
    fn a_real_aws_sandbox_leaves_room_for_the_plugin_socket_in_its_tmpdir() {
        let root = sandbox_root(true);
        let socket = root
            .path()
            .join("tmp")
            .join("4294967295_session_manager_plugin_mux.sock");
        assert!(
            socket.as_os_str().len() <= 103,
            "{} is {} bytes",
            socket.display(),
            socket.as_os_str().len()
        );
    }
}
