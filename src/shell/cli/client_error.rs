//! The structured error contract every JSON client shares, and redacted clap
//! usage diagnostics.
use super::error_code::ErrorCode;
use crate::domain::types::{database::ServerError, dataset::ScanContext, limits::INPUT_LIST};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
struct ErrorDocument {
    #[schemars(range(min = 1, max = 1))]
    schema_version: u8,
    error: ErrorDetails,
}

#[derive(Serialize, JsonSchema)]
struct ErrorDetails {
    code: &'static str,
    category: Category,
    #[schemars(range(min = 1, max = 4))]
    exit_code: u8,
    message: String,
    hint: Option<String>,
    retry: Retry,
    next_actions: Vec<NextAction>,
    /// What the database itself answered, for the failures it caused. Its text
    /// is the server's, cut to one bounded line, and it is never logged.
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<ServerError>,
    /// How long the run lasted, for the failures that end a wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<u64>,
    /// The columns the request named, for the same failures, cut to the same
    /// bound as every other list kurama publishes. Empty when it named none:
    /// the query was reading whatever the source has.
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<String>>,
    /// Columns left out of `columns` by that bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    columns_omitted: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Category {
    Tool,
    Usage,
    Human,
    Remote,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Retry {
    Never,
    AfterHuman,
    /// The failure happened before anything was sent, so repeating the same
    /// read changes nothing that the caller did.
    #[serde(rename = "read_only_retry")]
    ReadOnly,
}

#[derive(Serialize, JsonSchema)]
struct NextAction {
    /// The operation that answers this failure, when one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    message: String,
}

pub(super) fn document(
    code: ErrorCode,
    message: &str,
    hint: Option<String>,
    scan: Option<&ScanContext>,
    database: Option<&crate::domain::types::database::DbError>,
) -> serde_json::Value {
    let exit = code.exit_code();
    let next = hint.clone().unwrap_or_else(|| match code {
        ErrorCode::DataInvalid => "Check the data arguments with `kurama data --help` and the source columns with --describe.",
        ErrorCode::DataFailed => "Check input availability and effective limits with --dry-run, then retry the read explicitly.",
        ErrorCode::DataRejected => "Check S3 permissions and AWS credentials, then retry explicitly.",
        ErrorCode::DbInvalid => "Check the database arguments with `kurama db --help` and the structure with --tables.",
        ErrorCode::DbUnreachable => "Check that the database accepts connections from here; no statement was sent.",
        ErrorCode::DbFailed => "Check the effective limits with --dry-run, then run the call again explicitly; a write that failed kept nothing unless the outcome says unknown.",
        ErrorCode::DbRejected => "Correct the statement or the permissions the database named, then run it again.",
        ErrorCode::ApiHttpError => "The API answered outside 2xx; read its status and body (the stdout envelope with --json), correct the request or the permissions, then retry explicitly.",
        ErrorCode::ArgumentInvalid => "Check the arguments with `kurama <subcommand> --help`, then run it again.",
        ErrorCode::ApiArgumentInvalid => "Check the arguments with `kurama api --help`, and an operation's parameters with --describe.",
        ErrorCode::JqError => "The request was already sent; correct the --jq filter before sending it again.",
        ErrorCode::ApiRequestFailed => "No answer came back; check the network and base_url, then retry explicitly.",
        _ if exit == 3 => "Complete the login or MFA action required by this error, then retry explicitly.",
        _ => "Correct the reported configuration or authentication failure, then retry explicitly.",
    }.into());
    let output = ErrorDocument {
        schema_version: 1,
        error: ErrorDetails {
            code: code.as_str(),
            category: match exit {
                2 => Category::Usage,
                3 => Category::Human,
                4 => Category::Remote,
                _ => Category::Tool,
            },
            exit_code: exit,
            message: one_line(message),
            hint: hint.map(|h| one_line(&h)),
            retry: match database {
                // Only a failure whose type says nothing was sent, or nothing
                // was written, invites the caller to read again.
                Some(error) if error.read_only_retry() => Retry::ReadOnly,
                _ if exit == 3 => Retry::AfterHuman,
                _ => Retry::Never,
            },
            next_actions: vec![NextAction {
                operation: database
                    .and_then(|error| error.server())
                    .and_then(ServerError::next_operation)
                    .map(|operation| operation.as_str().to_owned()),
                message: one_line(&next),
            }],
            server: database.and_then(|error| error.server()).cloned(),
            elapsed_ms: scan.map(|scan| scan.elapsed_ms),
            columns: scan.map(|scan| INPUT_LIST.split(&scan.columns).0.to_vec()),
            columns_omitted: scan.map(|scan| INPUT_LIST.split(&scan.columns).1),
        },
    };
    serde_json::to_value(output).expect("typed data error serializes")
}

pub(crate) fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ErrorDocument)).expect("error schema serializes")
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub(super) fn usage_message(error: &clap::Error, subcommand: &str) -> String {
    use clap::error::{ContextKind, ErrorKind};
    // Context values may contain SQL or a signed URL. Only print option names,
    // never invalid values, suggestions derived from them, or raw clap text.
    let argument = |kind| {
        error.get(kind).and_then(|value| {
            let text = value.to_string();
            let name = text.split_whitespace().next()?.split('=').next()?;
            (name.starts_with("--")
                && name[2..]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'))
            .then(|| name.to_owned())
        })
    };
    let arg = argument(ContextKind::InvalidArg).unwrap_or_else(|| format!("{subcommand} argument"));
    let detail = match error.kind() {
        ErrorKind::ArgumentConflict => match argument(ContextKind::PriorArg) {
            Some(prior) => format!("{arg} cannot be used with {prior}"),
            None => format!("{arg} conflicts with another {subcommand} argument"),
        },
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => {
            format!("invalid value for {arg}; check its type or range")
        }
        ErrorKind::UnknownArgument => format!("unknown argument {arg}"),
        ErrorKind::TooFewValues | ErrorKind::InvalidUtf8 => {
            format!("invalid or missing value for {arg}")
        }
        ErrorKind::TooManyValues => format!("too many values for {arg}"),
        ErrorKind::MissingRequiredArgument => {
            format!("a required {subcommand} argument is missing")
        }
        _ => format!("invalid {subcommand} arguments"),
    };
    format!("{detail}; consult kurama {subcommand} --help")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_usage_names_the_option_without_echoing_secret_values() {
        for value in ["0", "SECRET_IN_SIGNED_URL"] {
            let error = crate::build_command()
                .try_get_matches_from(["kurama", "data", "--threads", value, "--json"])
                .unwrap_err();
            let message = usage_message(&error, "data");
            assert!(message.contains("--threads"), "{message}");
            assert!(message.contains("type or range"), "{message}");
            assert!(message.ends_with("consult kurama data --help"), "{message}");
            assert!(!message.contains("SECRET_IN_SIGNED_URL"));
        }
    }

    #[test]
    fn usage_diagnostics_name_the_subcommand_that_failed() {
        let error = crate::build_command()
            .try_get_matches_from(["kurama", "env"])
            .unwrap_err();
        let message = usage_message(&error, "db");
        assert_eq!(
            message,
            "a required db argument is missing; consult kurama db --help"
        );
    }
}
