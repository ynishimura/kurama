//! Stable error codes and exit codes.
//!
//! Every failure leaves the process through `main` as
//! `error[<CODE>]: <message>` on stderr, optionally followed by a `hint:` line,
//! and one of the exit codes [`ErrorCode::EXITS`] describes. That table is
//! the one statement of what an exit code means: `kurama --help` and
//! `kurama agent --json` print it.
//!
//! To find where a code comes from: `rg -n "<CODE>" src tests` lands on the
//! `as_str` arm here, the `classify` arm names the typed error, and the
//! scenario that pins it lives in `tests/scenarios/`.

use crate::adapters::aws::federation::FederationError;
use crate::adapters::browser::BrowserError;
use crate::adapters::config::Config;
use crate::adapters::config::input::InputError;
use crate::adapters::config::writer::WriteError;
use crate::adapters::error::CoreError;
use crate::domain::functions::error_mapping::StsErrorKind;
use crate::domain::functions::operation_request::OperationRequestError;
use crate::domain::functions::preset_render::PresetError;
use crate::domain::types::SecretFailure;
use crate::domain::types::dataset::DataError;
use crate::domain::types::s3_browse::{S3Error, S3Invalid};
use crate::ports::{SecretError, SessionCacheError, TokenStoreError};
use crate::shell::api_error::ApiError;
use crate::shell::cli::commands::preset::PresetInputRejected;
use crate::shell::cli::executor::CliExecutorError;
use crate::shell::executor::ExecutorError;
use crate::shell::oauth_executor::OAuthError;

/// `strum::VariantArray` gives `ErrorCode::VARIANTS`, every code in
/// declaration order, which `kurama agent --json` groups by exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::VariantArray)]
pub enum ErrorCode {
    ProfileNotFound,
    ProfileInvalid,
    ConfigInvalid,
    KindUnsupported,
    MfaTokenUnavailable,
    MfaProviderFailed,
    SessionCacheError,
    StsAccessDenied,
    StsInvalidMfaToken,
    StsInvalidCredentials,
    StsRoleNotFound,
    StsMfaRequired,
    StsServiceError,
    FederationFailed,
    TerminalRequired,
    ExecFailed,
    OAuthLoginRequired,
    OAuthRejected,
    OAuthFailed,
    TokenStoreError,
    SecretUnavailable,
    SecretInvalid,
    SecretRejected,
    SecretFailed,
    ArgumentInvalid,
    ApiNotFound,
    ApiTargetRequired,
    ApiArgumentInvalid,
    ApiHttpError,
    ApiRequestFailed,
    ApiSigningTargetRequired,
    ApiSpecRequired,
    ApiSpecUnavailable,
    ApiSpecInvalid,
    ApiOperationNotFound,
    ApiParameterMissing,
    ApiOutputFailed,
    JqError,
    DataInvalid,
    DataFailed,
    DataRejected,
    DbInvalid,
    DbUnreachable,
    DbFailed,
    DbRejected,
    ConfigWriteFailed,
    PresetNotFound,
    BrowserFailed,
    S3Invalid,
    S3Rejected,
    S3Failed,
    Internal,
    AgentPolicyDenied,
    ApiIntrospectionRefused,
    ApiGraphqlError,
    AgentInstallFailed,
    McpListenFailed,
    ObsidianPathRefused,
    ObsidianUnavailable,
    ObsidianFailed,
}

/// A hint and what kind it is. The distinction is the point: text that is the
/// same every time cannot degrade without anyone noticing, and text read out
/// of an error can -- which is how a refusal once named every action kurama
/// knows instead of the one the role was missing, and passed every assertion
/// that looked for a piece of it.
enum Hint {
    /// This code needs no hint: the message already says everything.
    None,
    /// The same text every time.
    Fixed(&'static str),
    /// Built from the error. `Derived(None)` is the read finding nothing,
    /// which prints no hint rather than a general one that reads like one.
    Derived(Option<String>),
}

impl Hint {
    fn derived(text: String) -> Self {
        Self::Derived(Some(text))
    }
}

impl ErrorCode {
    /// What each exit code means.
    pub const EXITS: &'static [(u8, &'static str)] = &[
        (0, "success"),
        (
            1,
            "tool error: a bug, the network, an unexpected service response",
        ),
        (
            2,
            "usage: unknown profile, invalid configuration, bad arguments",
        ),
        (3, "a person must act; the `hint:` line says what"),
        (
            4,
            "the remote side rejected the request: AWS, an authorization server, an HTTP status outside 2xx",
        ),
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProfileNotFound => "PROFILE_NOT_FOUND",
            Self::ProfileInvalid => "PROFILE_INVALID",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::MfaTokenUnavailable => "MFA_TOKEN_UNAVAILABLE",
            Self::MfaProviderFailed => "MFA_PROVIDER_FAILED",
            Self::SessionCacheError => "SESSION_CACHE_ERROR",
            Self::StsAccessDenied => "STS_ACCESS_DENIED",
            Self::StsInvalidMfaToken => "STS_INVALID_MFA_TOKEN",
            Self::StsInvalidCredentials => "STS_INVALID_CREDENTIALS",
            Self::StsRoleNotFound => "STS_ROLE_NOT_FOUND",
            Self::StsMfaRequired => "STS_MFA_REQUIRED",
            Self::StsServiceError => "STS_SERVICE_ERROR",
            Self::FederationFailed => "FEDERATION_FAILED",
            Self::TerminalRequired => "TERMINAL_REQUIRED",
            Self::ExecFailed => "EXEC_FAILED",
            Self::KindUnsupported => "KIND_UNSUPPORTED",
            Self::OAuthLoginRequired => "OAUTH_LOGIN_REQUIRED",
            Self::OAuthRejected => "OAUTH_REJECTED",
            Self::OAuthFailed => "OAUTH_FAILED",
            Self::TokenStoreError => "TOKEN_STORE_ERROR",
            Self::SecretUnavailable => "SECRET_UNAVAILABLE",
            Self::SecretInvalid => "SECRET_INVALID",
            Self::SecretRejected => "SECRET_REJECTED",
            Self::SecretFailed => "SECRET_FAILED",
            Self::ArgumentInvalid => "ARGUMENT_INVALID",
            Self::ApiNotFound => "API_NOT_FOUND",
            Self::ApiTargetRequired => "API_TARGET_REQUIRED",
            Self::ApiArgumentInvalid => "API_ARGUMENT_INVALID",
            Self::ApiHttpError => "API_HTTP_ERROR",
            Self::ApiRequestFailed => "API_REQUEST_FAILED",
            Self::ApiSigningTargetRequired => "API_SIGNING_TARGET_REQUIRED",
            Self::ApiSpecRequired => "API_SPEC_REQUIRED",
            Self::ApiSpecUnavailable => "API_SPEC_UNAVAILABLE",
            Self::ApiSpecInvalid => "API_SPEC_INVALID",
            Self::ApiOperationNotFound => "API_OPERATION_NOT_FOUND",
            Self::ApiParameterMissing => "API_PARAMETER_MISSING",
            Self::ApiOutputFailed => "API_OUTPUT_FAILED",
            Self::JqError => "JQ_ERROR",
            Self::DataInvalid => "DATA_INVALID",
            Self::DataFailed => "DATA_FAILED",
            Self::DataRejected => "DATA_REJECTED",
            Self::DbInvalid => "DB_INVALID",
            Self::DbUnreachable => "DB_UNREACHABLE",
            Self::DbFailed => "DB_FAILED",
            Self::DbRejected => "DB_REJECTED",
            Self::ConfigWriteFailed => "CONFIG_WRITE_FAILED",
            Self::PresetNotFound => "PRESET_NOT_FOUND",
            Self::BrowserFailed => "BROWSER_FAILED",
            Self::S3Invalid => "S3_INVALID",
            Self::S3Rejected => "S3_REJECTED",
            Self::S3Failed => "S3_FAILED",
            Self::Internal => "INTERNAL",
            Self::AgentPolicyDenied => "AGENT_POLICY_DENIED",
            Self::ApiIntrospectionRefused => "API_INTROSPECTION_REFUSED",
            Self::ApiGraphqlError => "API_GRAPHQL_ERROR",
            Self::AgentInstallFailed => "AGENT_INSTALL_FAILED",
            Self::McpListenFailed => "MCP_LISTEN_FAILED",
            Self::ObsidianPathRefused => "OBSIDIAN_PATH_REFUSED",
            Self::ObsidianUnavailable => "OBSIDIAN_UNAVAILABLE",
            Self::ObsidianFailed => "OBSIDIAN_FAILED",
        }
    }

    pub fn exit_code(self) -> u8 {
        match self {
            Self::ProfileNotFound
            | Self::ProfileInvalid
            | Self::ConfigInvalid
            | Self::KindUnsupported
            | Self::TerminalRequired
            | Self::ExecFailed
            | Self::ArgumentInvalid
            | Self::ApiNotFound
            | Self::ApiTargetRequired
            | Self::ApiArgumentInvalid
            | Self::ApiSigningTargetRequired
            | Self::ApiSpecRequired
            | Self::ApiSpecInvalid
            | Self::ApiOperationNotFound
            | Self::ApiParameterMissing
            | Self::DataInvalid
            | Self::SecretInvalid
            | Self::DbInvalid
            | Self::PresetNotFound
            | Self::S3Invalid
            | Self::ObsidianPathRefused => 2,
            Self::MfaTokenUnavailable
            | Self::MfaProviderFailed
            | Self::SessionCacheError
            | Self::OAuthLoginRequired
            | Self::TokenStoreError
            | Self::SecretUnavailable
            | Self::AgentPolicyDenied
            | Self::ObsidianUnavailable => 3,
            Self::StsAccessDenied
            | Self::StsInvalidMfaToken
            | Self::StsInvalidCredentials
            | Self::StsRoleNotFound
            | Self::StsMfaRequired
            | Self::FederationFailed
            | Self::OAuthRejected
            | Self::ApiHttpError
            | Self::DataRejected
            | Self::SecretRejected
            | Self::DbRejected
            | Self::S3Rejected
            | Self::ApiIntrospectionRefused
            | Self::ApiGraphqlError => 4,
            Self::StsServiceError
            | Self::OAuthFailed
            | Self::ApiRequestFailed
            | Self::ApiSpecUnavailable
            | Self::ApiOutputFailed
            | Self::JqError
            | Self::Internal
            | Self::DataFailed
            | Self::SecretFailed
            | Self::DbUnreachable
            | Self::DbFailed
            | Self::ConfigWriteFailed
            | Self::AgentInstallFailed
            | Self::McpListenFailed
            | Self::ObsidianFailed
            | Self::BrowserFailed
            | Self::S3Failed => 1,
        }
    }

    /// The configuration file in use, so a hint names what kurama really read.
    fn config_file() -> String {
        Config::config_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "~/.config/kurama/config.toml".to_string())
    }

    /// What a person has to do before retrying, for exit code 3 and usage
    /// errors. `error` supplies the profile name where the hint names one.
    pub fn hint(self, error: &anyhow::Error) -> Option<String> {
        match self.hint_of(error) {
            Hint::None => None,
            Hint::Fixed(text) => Some(text.to_owned()),
            Hint::Derived(text) => text,
        }
    }

    /// The hint and what kind it is. A `Fixed` hint is the same text every
    /// time this code is printed and cannot quietly stop being right; a
    /// `Derived` one is read out of the error, so what it does when the read
    /// finds nothing is behaviour, and `tests/architecture/` asks a scenario
    /// to have seen it.
    fn hint_of(self, error: &anyhow::Error) -> Hint {
        use crate::domain::types::dataset::DataError;
        if let Some(hint) = error.chain().find_map(s3_hint) {
            return Hint::derived(hint.to_owned());
        }
        match error
            .chain()
            .find_map(|cause| cause.downcast_ref::<DataError>())
        {
            Some(DataError::Invalid(reason)) => {
                return Hint::Derived(reason.hint().map(str::to_owned));
            }
            Some(DataError::S3OtherRegion { bucket_region, .. }) => {
                return Hint::derived(format!(
                    "use --region {bucket_region} (or region in [s3.*] / [data.*]), or name no region so kurama follows the bucket's"
                ));
            }
            Some(DataError::S3Redirected { .. }) => {
                return Hint::Fixed(
                    "set --region (or region in [s3.*] / [data.*]) to the bucket's region; `aws s3api head-bucket` reports it as BucketRegion",
                );
            }
            _ => {}
        }
        if let Some(error) = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<crate::domain::types::database::DbError>())
        {
            use crate::domain::types::database::{DbError, DbFailure};
            return Hint::Derived(match error {
                DbError::Invalid(reason) => reason.hint().map(str::to_owned),
                DbError::Unreachable(_) => Some(
                    "check the host, the port and that the database accepts connections from here; no statement was sent"
                        .to_owned(),
                ),
                DbError::TunnelUnavailable(_) => Some(
                    "the bastion is what could not be reached, not the database: check that its SSM agent is online and that the tunnel names the right instance; no statement was sent"
                        .to_owned(),
                ),
                DbError::Rejected(server) if server.needs_a_parameter_cast() => Some(
                    "a parameter is sent as text; write the cast the column needs in the statement, for example $1::int or $1::date"
                        .to_owned(),
                ),
                DbError::Rejected(server) => Some(match server.next_operation() {
                    Some(operation) => format!(
                        "the database refused the name; run the same database with --{} to see what it has",
                        operation.as_str()
                    ),
                    None => "the database refused the statement; its own message is above".to_owned(),
                }),
                DbError::Failed(DbFailure::Locked) => Some(
                    "another process held the write lock for the whole wait; retry, or raise --timeout"
                        .to_owned(),
                ),
                DbError::Failed(DbFailure::AffectedRowsLimit { .. }) => Some(
                    "nothing was kept; narrow the statement, or raise max_affected_rows when the count is intended"
                        .to_owned(),
                ),
                DbError::Failed(DbFailure::CommitUnknown) => Some(
                    "read the rows the change touched to see what the database kept; do not send it again blindly"
                        .to_owned(),
                ),
                DbError::Failed(DbFailure::Incomplete) => Some(
                    "the partial result is on stdout; narrow the statement, or raise --max-rows / --max-result-bytes to get the rest"
                        .to_owned(),
                ),
                DbError::Failed(DbFailure::UnsupportedType { .. }) => {
                    Some("cast the column to text in the statement".to_owned())
                }
                _ => None,
            });
        }
        let hint = match self {
            Self::ProfileNotFound | Self::ApiNotFound => {
                "run `kurama status` to see the profile, source and API names; `kurama agent` shows how to add one"
            }
            Self::PresetNotFound => "run `kurama preset` to list the presets",
            Self::ConfigInvalid | Self::ArgumentInvalid
                if let Some(preset) = error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<PresetError>()) =>
            {
                return Hint::derived(preset.hint().to_owned());
            }
            Self::ConfigInvalid | Self::ArgumentInvalid
                if let Some(input) = error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<InputError>()) =>
            {
                return Hint::derived(input.hint().to_owned());
            }
            Self::ConfigInvalid => {
                let fix = format!(
                    "fix {} (KURAMA_CONFIG_PATH overrides the location); `kurama agent` lists the keys",
                    Self::config_file()
                );
                let elsewhere = error.chain().find_map(|cause| match cause.downcast_ref() {
                    Some(CoreError::KeyOfAnotherSection { key, sections, .. }) => Some(format!(
                        "; this kurama reads `{key}` only in {}, so the key is misplaced or this binary is older than the configuration",
                        sections.join(", ")
                    )),
                    _ => None,
                });
                return Hint::derived(fix + elsewhere.as_deref().unwrap_or_default());
            }
            Self::MfaTokenUnavailable => {
                let path = Self::config_file();
                return Hint::derived(format!(
                    "enable [onepassword] in {path}, or run kurama in a terminal"
                ));
            }
            Self::MfaProviderFailed => {
                "run `op signin` and check that the 1Password item has a one-time password; for an unattended run export OP_SERVICE_ACCOUNT_TOKEN or set [onepassword] service_account_keychain"
            }
            Self::TerminalRequired => {
                "run `kurama status` to list profiles and `kurama env <profile>` to export one; `kurama audit` lists the recorded calls"
            }
            Self::ExecFailed => {
                "check the command name after `--`; the command inherits PATH from this shell"
            }
            Self::SessionCacheError | Self::TokenStoreError => {
                "unlock the login keychain, or run `kurama logout --all` and retry"
            }
            Self::OAuthLoginRequired => {
                let profile = error
                    .chain()
                    .find_map(|cause| match cause.downcast_ref::<OAuthError>() {
                        Some(OAuthError::LoginRequired { profile, .. }) => Some(profile.as_str()),
                        _ => None,
                    })
                    .unwrap_or("<profile>");
                return Hint::derived(format!("run `kurama login {profile}` in a terminal"));
            }
            Self::SecretUnavailable => {
                "run `op signin`, or export OP_SERVICE_ACCOUNT_TOKEN (or set [onepassword] service_account_keychain) for an unattended run; check the op:// reference"
            }
            Self::SecretInvalid => {
                "a secret reference is op://<vault>/<item>/<field>, aws-secrets://<aws-profile>/<secret-id>[?region=<region>][#<json-key>] or aws-ssm://<aws-profile>/<parameter-name>[?region=<region>]"
            }
            Self::SecretRejected => {
                // Both carriers: `kurama db` hands the `SecretError` to
                // `main`, and the pure OAuth workflow reduces it to the same
                // `failure` inside `OAuthError::Secret`. No fallback naming
                // both stores -- a hint that cannot say which one refused is
                // one no assertion can tell from the right one.
                let store = error.chain().find_map(|cause| {
                    let failure = match cause.downcast_ref::<SecretError>() {
                        Some(error) => error.failure,
                        None => match cause.downcast_ref::<OAuthError>() {
                            Some(OAuthError::Secret { failure, .. }) => *failure,
                            _ => return None,
                        },
                    };
                    match failure {
                        SecretFailure::Rejected { store, not_found } => Some((store, not_found)),
                        _ => None,
                    }
                });
                return Hint::Derived(store.map(|(store, not_found)| {
                    let actions = store.iam_actions();
                    if not_found {
                        format!("check the name in the reference; the role also needs {actions}")
                    } else {
                        format!("the role of the AWS profile in the reference needs {actions}")
                    }
                }));
            }
            Self::SecretFailed => {
                "check the network and the region of the reference; nothing was read, so retrying can work"
            }
            Self::ApiOutputFailed => {
                "name a file in a directory that exists and is writable, not a directory or a read-only file; nothing at the path was changed"
            }
            Self::ApiTargetRequired => {
                "pass a TARGET (/path or operationId), or use --ops; the explorer requires a terminal with TERM other than dumb, in a run without KURAMA_AGENT"
            }
            Self::ApiSpecRequired
            | Self::ApiSpecUnavailable
            | Self::ApiSpecInvalid
            | Self::ApiOperationNotFound
            | Self::ApiParameterMissing => {
                let api = error
                    .chain()
                    .find_map(|cause| match cause.downcast_ref::<ApiError>() {
                        Some(
                            ApiError::SpecRequired { api, .. }
                            | ApiError::SpecUnavailable { api, .. }
                            | ApiError::SpecNeedsCredential { api, .. }
                            | ApiError::SpecInvalid { api, .. }
                            | ApiError::OperationNotFound { api, .. }
                            | ApiError::OperationInput { api, .. },
                        ) => Some(api.as_str()),
                        _ => None,
                    })
                    .unwrap_or("<name>");
                return Hint::derived(match self {
                    Self::ApiSpecRequired => {
                        format!("add openapi = \"<URL or file>\" under [api.{api}] in config.toml")
                    }
                    Self::ApiSpecUnavailable
                        if error.chain().any(|cause| {
                            matches!(
                                cause.downcast_ref::<ApiError>(),
                                Some(ApiError::SpecNeedsCredential { .. })
                            )
                        }) =>
                    {
                        format!(
                            "run `kurama api {api} --refresh-spec` without --dry-run to cache the description, then dry-run again"
                        )
                    }
                    Self::ApiSpecUnavailable => {
                        let Some((key, from_url)) = error.chain().find_map(|cause| match cause
                            .downcast_ref::<ApiError>(
                        ) {
                            Some(ApiError::SpecUnavailable { key, location, .. }) => {
                                Some((*key, location.starts_with("http")))
                            }
                            _ => None,
                        }) else {
                            return Hint::Derived(None);
                        };
                        if from_url {
                            format!(
                                "check the {key} URL under [api.{api}]; `kurama api {api} --refresh-spec` fetches it again"
                            )
                        } else {
                            format!("check the {key} file under [api.{api}]")
                        }
                    }
                    Self::ApiSpecInvalid => {
                        let Some(key) = error.chain().find_map(|cause| {
                            match cause.downcast_ref::<ApiError>() {
                                Some(ApiError::SpecInvalid { key, .. }) => Some(*key),
                                _ => None,
                            }
                        }) else {
                            return Hint::Derived(None);
                        };
                        match key {
                            "graphql" | "graphql_schema" => format!(
                                "the graphql schema of [api.{api}] must be a GraphQL introspection result: JSON with a __schema"
                            ),
                            "discovery" => format!(
                                "the discovery document under [api.{api}] must be a Google Discovery Document (discovery#restDescription) as JSON"
                            ),
                            _ => format!(
                                "the openapi document under [api.{api}] must be OpenAPI 3.0 / 3.1 or Swagger 2.0, as JSON or YAML"
                            ),
                        }
                    }
                    Self::ApiOperationNotFound => {
                        format!("run `kurama api {api} --ops [QUERY]` to list the operations")
                    }
                    _ => format!(
                        "run `kurama api {api} --describe <OP>` to see the parameters and the body"
                    ),
                });
            }
            Self::ConfigWriteFailed => {
                return Hint::Derived(
                    error
                        .chain()
                        .find_map(|cause| cause.downcast_ref::<WriteError>())
                        .map(|error| match error {
                            WriteError::Io { path, .. } => format!(
                                "check that the directory of {} exists and is writable; the file was not changed",
                                path.display()
                            ),
                            WriteError::Changed { path } => format!(
                                "{} was changed by something else after kurama read it; run the command again to start from the current file",
                                path.display()
                            ),
                        }),
                );
            }
            Self::ApiSigningTargetRequired => {
                let api = error
                    .chain()
                    .find_map(|cause| match cause.downcast_ref::<ApiError>() {
                        Some(ApiError::SigningTargetRequired { api, .. }) => Some(api.as_str()),
                        _ => None,
                    })
                    .unwrap_or("<name>");
                return Hint::derived(format!(
                    "set the missing SigV4 setting under [api.{api}] in config.toml, or pass its --service / --region option"
                ));
            }
            Self::BrowserFailed => {
                "open the URL printed above in a browser yourself; check that the system has a default browser for it"
            }
            Self::ApiIntrospectionRefused => {
                let Some(api) =
                    error
                        .chain()
                        .find_map(|cause| match cause.downcast_ref::<ApiError>() {
                            Some(ApiError::IntrospectionRefused { api, .. }) => Some(api.as_str()),
                            _ => None,
                        })
                else {
                    return Hint::Derived(None);
                };
                return Hint::derived(format!(
                    "save the schema as an introspection result (JSON with __schema) from where introspection is allowed, and set graphql_schema = \"<file>\" under [api.{api}]"
                ));
            }
            Self::ApiGraphqlError => {
                "rerun with --json to see every error and the data that came back with them"
            }
            Self::AgentInstallFailed => {
                "name a --dir that is a writable directory; the Skills written before this one stay, and a rerun writes only what differs"
            }
            Self::McpListenFailed => {
                "choose another [mcp] listen port, or stop the process holding this one (lsof -nP -iTCP -sTCP:LISTEN)"
            }
            Self::ObsidianUnavailable => {
                "start Obsidian and enable Settings > General > Command line interface, or set [obsidian] cli_path to the CLI (/Applications/Obsidian.app/Contents/MacOS/obsidian)"
            }
            Self::ObsidianPathRefused => {
                let Some(refused) = error.chain().find_map(|cause| {
                    cause.downcast_ref::<crate::domain::functions::obsidian::PathRefused>()
                }) else {
                    return Hint::Derived(None);
                };
                return Hint::derived(refused.hint());
            }
            Self::AgentPolicyDenied => {
                "ask a person whether this call may be made, then rerun it with --confirm; to allow it for every agent run, widen [agent] or [api.<name>.agent] in config.toml"
            }
            _ => return Hint::None,
        };
        Hint::Fixed(hint)
    }

    /// Walk the error chain and pick the code of the first typed error.
    pub fn classify(error: &anyhow::Error) -> Self {
        for cause in error.chain() {
            if cause.downcast_ref::<S3Invalid>().is_some() {
                return Self::S3Invalid;
            }
            if let Some(error) = cause.downcast_ref::<S3Error>() {
                return match error {
                    S3Error::Invalid(_) => Self::S3Invalid,
                    S3Error::Rejected(_)
                    | S3Error::ReadRejected(_)
                    | S3Error::Changed
                    | S3Error::Redirected { .. } => Self::S3Rejected,
                    S3Error::Failed => Self::S3Failed,
                };
            }
            if let Some(error) = cause.downcast_ref::<crate::domain::types::database::DbError>() {
                use crate::domain::types::database::DbError;
                return match error {
                    DbError::Invalid(_) => Self::DbInvalid,
                    DbError::Unreachable(_) | DbError::TunnelUnavailable(_) => Self::DbUnreachable,
                    DbError::Rejected(_) => Self::DbRejected,
                    DbError::Failed(_) | DbError::Io { .. } => Self::DbFailed,
                };
            }
            if let Some(error) = cause.downcast_ref::<crate::domain::types::dataset::DataError>() {
                use crate::domain::types::dataset::DataError;
                return match error {
                    DataError::Invalid(_) | DataError::Sql => Self::DataInvalid,
                    DataError::S3Rejected(_)
                    | DataError::S3OtherRegion { .. }
                    | DataError::S3Redirected { .. } => Self::DataRejected,
                    _ => Self::DataFailed,
                };
            }
            match cause.downcast_ref::<CliExecutorError>() {
                Some(CliExecutorError::ProfileNotFound(_)) => return Self::ProfileNotFound,
                Some(CliExecutorError::TerminalRequired) => return Self::TerminalRequired,
                Some(CliExecutorError::ExecFailed { .. }) => return Self::ExecFailed,
                Some(CliExecutorError::KindUnsupported { .. }) => return Self::KindUnsupported,
                _ => {}
            }
            if let Some(error) = cause.downcast_ref::<OAuthError>() {
                return match error {
                    OAuthError::LoginRequired { .. } => Self::OAuthLoginRequired,
                    OAuthError::Rejected(_) => Self::OAuthRejected,
                    OAuthError::Failed(_) => Self::OAuthFailed,
                    OAuthError::Secret { failure, .. } => Self::from_secret_failure(*failure),
                    OAuthError::Config(_) => Self::ConfigInvalid,
                    OAuthError::UnexpectedState(_) => Self::Internal,
                };
            }
            if cause
                .downcast_ref::<crate::shell::cli::commands::agent_install::SkillWriteFailed>()
                .is_some()
            {
                return Self::AgentInstallFailed;
            }
            if cause
                .downcast_ref::<crate::shell::cli::commands::mcp::McpListenFailed>()
                .is_some()
            {
                return Self::McpListenFailed;
            }
            if cause
                .downcast_ref::<crate::domain::functions::obsidian::PathRefused>()
                .is_some()
            {
                return Self::ObsidianPathRefused;
            }
            if let Some(error) =
                cause.downcast_ref::<crate::adapters::obsidian_cli::ObsidianCliError>()
            {
                use crate::adapters::obsidian_cli::ObsidianCliError;
                return match error {
                    ObsidianCliError::NotRunnable { .. } | ObsidianCliError::NoAnswer { .. } => {
                        Self::ObsidianUnavailable
                    }
                    ObsidianCliError::Failed { .. } => Self::ObsidianFailed,
                };
            }
            if let Some(error) = cause.downcast_ref::<ApiError>() {
                return match error {
                    ApiError::NotFound(_) => Self::ApiNotFound,
                    ApiError::TargetRequired => Self::ApiTargetRequired,
                    ApiError::ArgumentInvalid(_) => Self::ApiArgumentInvalid,
                    ApiError::HttpStatus { .. } => Self::ApiHttpError,
                    ApiError::RequestFailed(_) => Self::ApiRequestFailed,
                    ApiError::Jq(_) => Self::JqError,
                    ApiError::OutputFailed { .. } => Self::ApiOutputFailed,
                    ApiError::SigningTargetRequired { .. } => Self::ApiSigningTargetRequired,
                    ApiError::SpecRequired { .. } => Self::ApiSpecRequired,
                    ApiError::SpecUnavailable { .. } | ApiError::SpecNeedsCredential { .. } => {
                        Self::ApiSpecUnavailable
                    }
                    ApiError::SpecInvalid { .. } => Self::ApiSpecInvalid,
                    ApiError::OperationNotFound { .. } => Self::ApiOperationNotFound,
                    ApiError::OperationInput {
                        error: OperationRequestError::Missing { .. },
                        ..
                    } => Self::ApiParameterMissing,
                    ApiError::OperationInput { .. } => Self::ApiArgumentInvalid,
                    ApiError::IntrospectionRefused { .. } => Self::ApiIntrospectionRefused,
                    ApiError::GraphQl(_) => Self::ApiGraphqlError,
                };
            }
            if let Some(error) = cause.downcast_ref::<SecretError>() {
                return match error.cause.as_ref().map(|cause| Self::classify(cause)) {
                    // The AssumeRole path classified itself already: a wrong
                    // TOTP is exit 3 and a refused role is exit 4, and this
                    // reference is malformed in neither case. A chain that
                    // classifies as nothing says nothing, so the resolver's
                    // own failure still decides rather than `INTERNAL`.
                    Some(Self::Internal) | None => Self::from_secret_failure(error.failure),
                    Some(code) => code,
                };
            }
            if cause.downcast_ref::<TokenStoreError>().is_some() {
                return Self::TokenStoreError;
            }
            if let Some(error) = cause.downcast_ref::<ExecutorError>() {
                return match error {
                    ExecutorError::MfaRequired { .. } => Self::MfaTokenUnavailable,
                    ExecutorError::MfaFailed(_) => Self::MfaProviderFailed,
                    ExecutorError::InvalidProfile(_) => Self::ProfileInvalid,
                    ExecutorError::StsFailed { kind, .. } => Self::from_sts_kind(kind),
                    ExecutorError::UnexpectedState(_) => Self::Internal,
                };
            }
            if cause.downcast_ref::<SessionCacheError>().is_some() {
                return Self::SessionCacheError;
            }
            if cause.downcast_ref::<FederationError>().is_some() {
                return Self::FederationFailed;
            }
            if cause.downcast_ref::<WriteError>().is_some() {
                return Self::ConfigWriteFailed;
            }
            if cause.downcast_ref::<PresetInputRejected>().is_some() {
                return Self::ArgumentInvalid;
            }
            if cause.downcast_ref::<InputError>().is_some() {
                return Self::ConfigInvalid;
            }
            if let Some(error) = cause.downcast_ref::<PresetError>() {
                return match error {
                    PresetError::NotFound(_) => Self::PresetNotFound,
                    PresetError::AuthNamedLikeAwsProfile(_) => Self::ConfigInvalid,
                    PresetError::UnknownInput { .. }
                    | PresetError::MissingInputs { .. }
                    | PresetError::LiteralSecret { .. }
                    | PresetError::ApiTaken { .. }
                    | PresetError::AuthIncompatible { .. } => Self::ArgumentInvalid,
                };
            }
            if cause.downcast_ref::<toml::de::Error>().is_some() {
                return Self::ConfigInvalid;
            }
            if let Some(
                CoreError::Configuration(_)
                | CoreError::TomlParse(_)
                | CoreError::KeyOfAnotherSection { .. },
            ) = cause.downcast_ref::<CoreError>()
            {
                return Self::ConfigInvalid;
            }
            if cause.downcast_ref::<BrowserError>().is_some() {
                return Self::BrowserFailed;
            }
            if cause
                .downcast_ref::<crate::shell::agent_policy::AgentPolicyDenied>()
                .is_some()
            {
                return Self::AgentPolicyDenied;
            }
        }
        Self::Internal
    }

    /// What the store said happened, as one code. `kurama db` hands the
    /// `SecretError` straight to `main` and the OAuth workflow carries the
    /// same `failure` through, so both paths land here and cannot disagree.
    fn from_secret_failure(failure: SecretFailure) -> Self {
        match failure {
            SecretFailure::NeedsAPerson => Self::SecretUnavailable,
            SecretFailure::Invalid => Self::SecretInvalid,
            SecretFailure::Rejected { .. } => Self::SecretRejected,
            SecretFailure::Unreachable => Self::SecretFailed,
        }
    }

    fn from_sts_kind(kind: &StsErrorKind) -> Self {
        match kind {
            StsErrorKind::AccessDenied => Self::StsAccessDenied,
            StsErrorKind::InvalidMfaToken => Self::StsInvalidMfaToken,
            StsErrorKind::InvalidCredentials => Self::StsInvalidCredentials,
            StsErrorKind::RoleNotFound => Self::StsRoleNotFound,
            StsErrorKind::MfaRequired => Self::StsMfaRequired,
            StsErrorKind::ServiceError => Self::StsServiceError,
        }
    }

    /// A JSON client keeps stderr to one error document, including usage
    /// errors; `hint` is the subcommand's own, when it has one.
    pub fn json_error(self, message: &str, hint: Option<String>) -> serde_json::Value {
        super::client_error::document(self, message, hint, None, None)
    }

    /// Include the same typed cause chain and actionable hint as text errors,
    /// and what the run was doing when a wait ended.
    pub fn json_error_with_context(self, error: &anyhow::Error) -> serde_json::Value {
        let scan = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<DataError>())
            .and_then(DataError::scan_context);
        let database = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<crate::domain::types::database::DbError>());
        super::client_error::document(
            self,
            &format!("{error:#}"),
            self.hint(error),
            scan,
            database,
        )
    }

    /// Describe clap usage failures without echoing SQL or URL credentials.
    pub fn usage_message(error: &clap::Error, subcommand: &str) -> String {
        super::client_error::usage_message(error, subcommand)
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What to do about a `kurama s3` failure; each variant has one fixed text.
fn s3_hint(cause: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    if let Some(invalid) = cause.downcast_ref::<S3Invalid>() {
        return Some(invalid.hint());
    }
    Some(match cause.downcast_ref::<S3Error>()? {
        S3Error::Invalid(invalid) => invalid.hint(),
        S3Error::Rejected(_) => {
            "check the bucket name and that the AWS profile's role may list it: s3:ListBucket on the bucket, s3:ListAllMyBuckets for --buckets"
        }
        S3Error::ReadRejected(_) => {
            "check the key and that the AWS profile's role may read it: s3:GetObject on the object (without s3:ListBucket a missing key reads as AccessDenied)"
        }
        S3Error::Changed => {
            "run --head for the current ETag, or drop --if-match to read the object as it is now"
        }
        S3Error::Redirected { .. } => {
            "pass --region with the bucket's region; `aws s3api head-bucket` reports it as BucketRegion"
        }
        S3Error::Failed => {
            "check the network; raise request_timeout_secs in the [s3.*] when S3 answers slowly"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn data_error_causes_appear_once_in_text_and_json() {
        use crate::domain::types::dataset::{DataError, InvalidInput};
        use crate::ports::HttpError;

        for (error, code, message, hint) in [
            (
                anyhow::Error::from(DataError::Invalid(InvalidInput::UnknownTable)),
                ErrorCode::DataInvalid,
                "unknown source table; use --tables",
                Some("run the same data input with --tables, then choose a listed table"),
            ),
            (
                anyhow::Error::from(DataError::Invalid(InvalidInput::RequestFile {
                    path: "query.sql".into(),
                    source: std::io::Error::other("file unreadable"),
                })),
                ErrorCode::DataInvalid,
                "cannot read data request or SQL file query.sql: file unreadable",
                Some("check the --request or --file path and its read permissions"),
            ),
            (
                anyhow::Error::from(DataError::Io(std::io::Error::other("disk failure"))),
                ErrorCode::DataFailed,
                "data file operation failed: disk failure",
                None,
            ),
            (
                anyhow::Error::from(ApiError::RequestFailed(HttpError::Timeout { seconds: 1 })),
                ErrorCode::ApiRequestFailed,
                "request failed: request timed out after 1s",
                None,
            ),
        ] {
            let error = error.context("operation context");
            let expected = format!("operation context: {message}");
            assert_eq!(ErrorCode::classify(&error), code);
            assert_eq!(format!("{error:#}"), expected);
            let document = code.json_error_with_context(&error);
            assert_eq!(document["error"]["message"], expected);
            assert_eq!(document["error"]["exit_code"], code.exit_code());
            assert_eq!(code.hint(&error).as_deref(), hint);
            assert_eq!(
                document["error"]["hint"],
                serde_json::to_value(hint).unwrap()
            );
        }
    }

    #[test]
    fn classifies_typed_errors_through_anyhow_context() {
        let error: anyhow::Error = Err::<(), _>(CliExecutorError::ProfileNotFound("x".into()))
            .context("outer")
            .unwrap_err();
        assert_eq!(ErrorCode::classify(&error), ErrorCode::ProfileNotFound);
        assert_eq!(ErrorCode::ProfileNotFound.exit_code(), 2);

        let error: anyhow::Error = Err::<(), _>(ExecutorError::StsFailed {
            kind: StsErrorKind::AccessDenied,
            message: "denied".into(),
        })
        .context("AssumeRole workflow failed")
        .unwrap_err();
        assert_eq!(ErrorCode::classify(&error), ErrorCode::StsAccessDenied);
        assert_eq!(ErrorCode::StsAccessDenied.exit_code(), 4);

        let error: anyhow::Error = ExecutorError::MfaRequired {
            serial: "arn".into(),
        }
        .into();
        assert_eq!(ErrorCode::classify(&error), ErrorCode::MfaTokenUnavailable);
        assert!(ErrorCode::MfaTokenUnavailable.hint(&error).is_some());
    }

    /// The AssumeRole path classifies itself. Reporting a refused role or a
    /// wrong TOTP as `SECRET_INVALID` tells a person their reference is
    /// malformed and an agent that exit 2 cannot be retried, and both are
    /// wrong.
    #[test]
    fn a_reference_whose_role_cannot_be_assumed_keeps_the_code_of_what_failed() {
        let refused: anyhow::Error = ExecutorError::StsFailed {
            kind: StsErrorKind::AccessDenied,
            message: "not authorized to perform sts:AssumeRole".into(),
        }
        .into();
        let error: anyhow::Error = SecretError::unclassified(
            SecretFailure::Unreachable,
            "aws-secrets://dev/app/db#password: the role of the AWS profile dev \
             could not be assumed",
            refused,
        )
        .into();
        assert_eq!(ErrorCode::classify(&error), ErrorCode::StsAccessDenied);

        // A failure the chain classifies as nothing still has a code of its
        // own, and it is the one that says nothing was read.
        let opaque: anyhow::Error = SecretError::unclassified(
            SecretFailure::Unreachable,
            "dev could not be assumed",
            anyhow::anyhow!("socket"),
        )
        .into();
        assert_eq!(ErrorCode::classify(&opaque), ErrorCode::SecretFailed);
        assert_eq!(ErrorCode::SecretFailed.exit_code(), 1);
    }

    /// The hint names the store that refused, whichever path carried it, and
    /// names none when it cannot tell. A fallback listing every action kurama
    /// knows reads like an answer and is one no assertion can tell from the
    /// real one.
    #[test]
    fn the_refusal_hint_names_the_store_on_both_paths_and_invents_none() {
        use crate::domain::types::AwsSecretStore;

        let db: anyhow::Error = SecretError::rejected(
            AwsSecretStore::SecretsManager,
            false,
            "GetSecretValue AccessDeniedException",
        )
        .into();
        assert_eq!(
            ErrorCode::SecretRejected.hint(&db).as_deref(),
            Some(
                "the role of the AWS profile in the reference needs \
                 secretsmanager:GetSecretValue, and kms:Decrypt when the secret uses a \
                 customer-managed key"
            )
        );

        // The pure OAuth workflow keeps only the `SecretFailure`, so this is
        // the carrier `[auth.*]` arrives in.
        let source: anyhow::Error = OAuthError::Secret {
            failure: SecretFailure::Rejected {
                store: AwsSecretStore::ParameterStore,
                not_found: false,
            },
            message: "GetParameter AccessDeniedException".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::SecretRejected.hint(&source).as_deref(),
            Some(
                "the role of the AWS profile in the reference needs ssm:GetParameter, \
                 and kms:Decrypt for a SecureString"
            )
        );

        // A store that says the name does not exist points at the name first,
        // on the OAuth carrier too, and still names the permission.
        let missing: anyhow::Error = OAuthError::Secret {
            failure: SecretFailure::Rejected {
                store: AwsSecretStore::ParameterStore,
                not_found: true,
            },
            message: "GetParameter ParameterNotFound".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::SecretRejected.hint(&missing).as_deref(),
            Some(
                "check the name in the reference; the role also needs ssm:GetParameter, \
                 and kms:Decrypt for a SecureString"
            )
        );

        let unknown: anyhow::Error = anyhow::anyhow!("something else refused");
        assert_eq!(ErrorCode::SecretRejected.hint(&unknown), None);
    }

    #[test]
    fn oauth_and_api_errors_map_to_their_codes_and_the_login_hint_names_the_profile() {
        let error: anyhow::Error = Err::<(), _>(OAuthError::LoginRequired {
            profile: "github".into(),
            message: "no token".into(),
        })
        .context("Failed to get a token for 'github'")
        .unwrap_err();
        assert_eq!(ErrorCode::classify(&error), ErrorCode::OAuthLoginRequired);
        assert_eq!(ErrorCode::OAuthLoginRequired.exit_code(), 3);
        assert_eq!(
            ErrorCode::OAuthLoginRequired.hint(&error).as_deref(),
            Some("run `kurama login github` in a terminal")
        );
        for (error, code, exit) in [
            (
                anyhow::Error::from(OAuthError::Rejected("invalid_client".into())),
                ErrorCode::OAuthRejected,
                4,
            ),
            (
                OAuthError::Failed("timeout".into()).into(),
                ErrorCode::OAuthFailed,
                1,
            ),
            (
                OAuthError::Secret {
                    failure: SecretFailure::NeedsAPerson,
                    message: "op".into(),
                }
                .into(),
                ErrorCode::SecretUnavailable,
                3,
            ),
            (
                SecretError::invalid("aws-ssm://dev/app: no such key").into(),
                ErrorCode::SecretInvalid,
                2,
            ),
            (
                SecretError::rejected(
                    crate::domain::types::AwsSecretStore::SecretsManager,
                    false,
                    "GetSecretValue AccessDeniedException",
                )
                .into(),
                ErrorCode::SecretRejected,
                4,
            ),
            (
                SecretError::unreachable("GetParameter: dispatch failure").into(),
                ErrorCode::SecretFailed,
                1,
            ),
            (
                TokenStoreError::Backend("locked".into()).into(),
                ErrorCode::TokenStoreError,
                3,
            ),
            (
                ApiError::NotFound("x".into()).into(),
                ErrorCode::ApiNotFound,
                2,
            ),
            (
                ApiError::TargetRequired.into(),
                ErrorCode::ApiTargetRequired,
                2,
            ),
            (
                ApiError::HttpStatus {
                    status: 404,
                    reason: "Not Found".into(),
                    excerpt: String::new(),
                }
                .into(),
                ErrorCode::ApiHttpError,
                4,
            ),
            (
                ApiError::GraphQl("Entity not found (at issue)".into()).into(),
                ErrorCode::ApiGraphqlError,
                4,
            ),
            (
                crate::shell::cli::commands::agent_install::SkillWriteFailed {
                    path: "/x/kurama/SKILL.md".into(),
                    source: std::io::ErrorKind::PermissionDenied.into(),
                }
                .into(),
                ErrorCode::AgentInstallFailed,
                1,
            ),
            (
                crate::shell::cli::commands::mcp::McpListenFailed {
                    address: "127.0.0.1:8807".parse().unwrap(),
                    source: std::io::ErrorKind::AddrInUse.into(),
                }
                .into(),
                ErrorCode::McpListenFailed,
                1,
            ),
            (
                crate::domain::functions::obsidian::PathRefused::Outside {
                    path: "Private/a.md".into(),
                    allowed: vec!["Wiki/".into()],
                }
                .into(),
                ErrorCode::ObsidianPathRefused,
                2,
            ),
            (
                crate::adapters::obsidian_cli::ObsidianCliError::NoAnswer { seconds: 20 }.into(),
                ErrorCode::ObsidianUnavailable,
                3,
            ),
            (
                crate::adapters::obsidian_cli::ObsidianCliError::Failed {
                    message: "Vault not found.".into(),
                }
                .into(),
                ErrorCode::ObsidianFailed,
                1,
            ),
            (ApiError::Jq("bad".into()).into(), ErrorCode::JqError, 1),
            (
                ApiError::OutputFailed {
                    path: "/x/body.json".into(),
                    source: std::io::ErrorKind::PermissionDenied.into(),
                }
                .into(),
                ErrorCode::ApiOutputFailed,
                1,
            ),
            (
                ApiError::SigningTargetRequired {
                    api: "apigw".into(),
                    message: "no".into(),
                }
                .into(),
                ErrorCode::ApiSigningTargetRequired,
                2,
            ),
            (
                CliExecutorError::KindUnsupported {
                    name: "github".into(),
                    kind: "oauth",
                    verb: "console",
                }
                .into(),
                ErrorCode::KindUnsupported,
                2,
            ),
            (
                ApiError::SpecRequired {
                    api: "gh".into(),
                    needed: "--ops".into(),
                }
                .into(),
                ErrorCode::ApiSpecRequired,
                2,
            ),
            (
                ApiError::SpecUnavailable {
                    api: "gh".into(),
                    key: "openapi",
                    location: "https://x".into(),
                    message: "refused".into(),
                }
                .into(),
                ErrorCode::ApiSpecUnavailable,
                1,
            ),
            (
                ApiError::SpecInvalid {
                    api: "gh".into(),
                    key: "openapi",
                    location: "https://x".into(),
                    message: "no paths".into(),
                }
                .into(),
                ErrorCode::ApiSpecInvalid,
                2,
            ),
            (
                ApiError::OperationNotFound {
                    api: "gh".into(),
                    target: "issues/lst".into(),
                    candidates: vec![],
                }
                .into(),
                ErrorCode::ApiOperationNotFound,
                2,
            ),
            (
                ApiError::OperationInput {
                    api: "gh".into(),
                    error: OperationRequestError::Missing {
                        operation: "issues/create".into(),
                        missing: vec!["owner".into()],
                    },
                }
                .into(),
                ErrorCode::ApiParameterMissing,
                2,
            ),
            (
                ApiError::OperationInput {
                    api: "gh".into(),
                    error: OperationRequestError::UnknownParameter {
                        name: "x".into(),
                        known: vec![],
                    },
                }
                .into(),
                ErrorCode::ApiArgumentInvalid,
                2,
            ),
        ] {
            assert_eq!(ErrorCode::classify(&error), code, "{error}");
            assert_eq!(code.exit_code(), exit, "{code}");
        }
    }

    #[test]
    fn the_signing_target_hint_names_the_api_section() {
        let error: anyhow::Error = ApiError::SigningTargetRequired {
            api: "apigw".into(),
            message: "the SigV4 service and region to sign for cannot be told from host \"api.example.com\"".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::ApiSigningTargetRequired.hint(&error).as_deref(),
            Some(
                "set the missing SigV4 setting under [api.apigw] in config.toml, or pass its --service / --region option"
            )
        );
    }

    #[test]
    fn spec_and_operation_hints_name_the_api() {
        let error: anyhow::Error = ApiError::OperationNotFound {
            api: "github".into(),
            target: "issues/lst".into(),
            candidates: vec!["issues/list".into(), "issues/list-for-repo".into()],
        }
        .into();
        assert_eq!(
            error.to_string(),
            "operation \"issues/lst\" is not in the API description; did you mean issues/list, issues/list-for-repo?"
        );
        assert_eq!(
            ErrorCode::ApiOperationNotFound.hint(&error).as_deref(),
            Some("run `kurama api github --ops [QUERY]` to list the operations")
        );
        let error: anyhow::Error = ApiError::SpecRequired {
            api: "github".into(),
            needed: "--ops".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::ApiSpecRequired.hint(&error).as_deref(),
            Some("add openapi = \"<URL or file>\" under [api.github] in config.toml")
        );
        let error: anyhow::Error = ApiError::OperationInput {
            api: "github".into(),
            error: OperationRequestError::Missing {
                operation: "issues/create".into(),
                missing: vec!["owner".into(), "body".into()],
            },
        }
        .into();
        assert_eq!(
            ErrorCode::ApiParameterMissing.hint(&error).as_deref(),
            Some("run `kurama api github --describe <OP>` to see the parameters and the body")
        );
        let from_file: anyhow::Error = ApiError::SpecUnavailable {
            api: "internal".into(),
            key: "openapi",
            location: "/home/me/specs/x.yaml".into(),
            message: "No such file".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::ApiSpecUnavailable.hint(&from_file).as_deref(),
            Some("check the openapi file under [api.internal]")
        );
        let from_url: anyhow::Error = ApiError::SpecUnavailable {
            api: "gh".into(),
            key: "openapi",
            location: "https://x/openapi.json".into(),
            message: "refused".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::ApiSpecUnavailable.hint(&from_url).as_deref(),
            Some(
                "check the openapi URL under [api.gh]; `kurama api gh --refresh-spec` fetches it again"
            )
        );
        // The hints name the key the description is configured under.
        let discovery: anyhow::Error = ApiError::SpecUnavailable {
            api: "sheets".into(),
            key: "discovery",
            location: "/home/me/sheets.json".into(),
            message: "No such file".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::ApiSpecUnavailable.hint(&discovery).as_deref(),
            Some("check the discovery file under [api.sheets]")
        );
        let discovery: anyhow::Error = ApiError::SpecInvalid {
            api: "sheets".into(),
            key: "discovery",
            location: "/home/me/sheets.json".into(),
            message: "not a Google Discovery Document".into(),
        }
        .into();
        assert_eq!(
            ErrorCode::ApiSpecInvalid.hint(&discovery).as_deref(),
            Some(
                "the discovery document under [api.sheets] must be a Google Discovery Document (discovery#restDescription) as JSON"
            )
        );
    }

    #[test]
    fn unknown_errors_are_internal() {
        let error = anyhow::anyhow!("something else");
        assert_eq!(ErrorCode::classify(&error), ErrorCode::Internal);
        assert_eq!(ErrorCode::Internal.exit_code(), 1);
        assert_eq!(ErrorCode::Internal.hint(&error), None);
    }

    /// Every exit a code produces is one `EXITS` describes.
    #[test]
    fn every_exit_code_is_described() {
        use strum::VariantArray;
        for code in ErrorCode::VARIANTS {
            assert!(
                ErrorCode::EXITS
                    .iter()
                    .any(|(exit, _)| *exit == code.exit_code()),
                "{code} exits {} and EXITS does not describe it",
                code.exit_code()
            );
        }
    }

    #[test]
    fn codes_are_screaming_snake_case() {
        for code in [
            ErrorCode::ProfileNotFound,
            ErrorCode::StsAccessDenied,
            ErrorCode::Internal,
        ] {
            assert!(
                code.as_str()
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_')
            );
        }
    }

    #[test]
    fn a_key_of_another_section_adds_where_it_is_read_to_the_hint() {
        let error = anyhow::Error::from(CoreError::KeyOfAnotherSection {
            message: "config.toml line 3: unknown field `format`".into(),
            key: "format".into(),
            sections: vec!["[[data.*.sources]]".into(), "[auth.*]".into()],
        });
        assert_eq!(ErrorCode::classify(&error), ErrorCode::ConfigInvalid);
        assert_eq!(ErrorCode::ConfigInvalid.exit_code(), 2);
        let hint = ErrorCode::ConfigInvalid.hint(&error).unwrap();
        assert!(hint.starts_with("fix "), "{hint}");
        assert!(
            hint.ends_with(
                "; this kurama reads `format` only in [[data.*.sources]], [auth.*], so the key is misplaced or this binary is older than the configuration"
            ),
            "{hint}"
        );
        let plain = anyhow::Error::from(CoreError::config("unknown field `mfa`"));
        assert!(
            ErrorCode::ConfigInvalid
                .hint(&plain)
                .unwrap()
                .ends_with("`kurama agent` lists the keys")
        );
    }

    #[test]
    fn the_audit_log_names_a_policy_refusal_by_its_error_code() {
        assert_eq!(
            ErrorCode::AgentPolicyDenied.as_str(),
            crate::domain::functions::audit::POLICY_REFUSAL_CODE
        );
    }
}
