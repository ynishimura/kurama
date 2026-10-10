//! What the subcommands that answer with one JSON document share: which one a
//! run invoked, which of them report failures as a JSON error document, the
//! code its usage failures carry, reading a typed request,
//! projecting the result envelope with `--jq`, and the text form of its rows.
use super::error_code::ErrorCode;
use clap::{Arg, ArgAction, Command, ValueHint};
use serde_json::Value;
use std::ffi::OsString;
use std::io::Read;
use strum::VariantArray;

/// A subcommand that answers an agent with one JSON document on stdout and one
/// JSON error document on stderr. `ClientKind::VARIANTS` is every kind, in
/// the order `--kind` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::VariantArray)]
pub enum ClientKind {
    Data,
    Db,
    S3,
}

impl ClientKind {
    /// The subcommand this kind names; also its `--kind` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Db => "db",
            Self::S3 => "s3",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::VARIANTS
            .iter()
            .copied()
            .find(|kind| kind.as_str() == name)
    }

    /// The code a usage failure of this subcommand carries.
    pub fn usage_code(self) -> ErrorCode {
        match self {
            Self::Data => ErrorCode::DataInvalid,
            Self::Db => ErrorCode::DbInvalid,
            Self::S3 => ErrorCode::S3Invalid,
        }
    }

    /// The `--kind` values, for the argument definition.
    pub fn values() -> Vec<&'static str> {
        Self::VARIANTS.iter().map(|kind| kind.as_str()).collect()
    }
}

/// A subcommand whose failures, when it runs with `--json` or `--jq`, are one
/// JSON error document on stderr. A [`ClientKind`] is one, and more: it has a
/// request contract and answers its usage failures with its own code even
/// without `--json`. The others have only the error contract, so they are a
/// separate type that `agent --kind` cannot list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonErrorKind {
    Client(ClientKind),
    ErrorOnly(ErrorOnlyKind),
}

/// A subcommand with only the JSON error contract. `ErrorOnlyKind::VARIANTS`
/// is every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::VariantArray)]
pub enum ErrorOnlyKind {
    Api,
    Status,
    Env,
    Token,
    Config,
    Preset,
    Obsidian,
}

impl ErrorOnlyKind {
    /// The subcommand this kind names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Status => "status",
            Self::Env => "env",
            Self::Token => "token",
            Self::Config => "config",
            Self::Preset => "preset",
            Self::Obsidian => "obsidian",
        }
    }
}

impl JsonErrorKind {
    /// Every kind: the clients, then the error-only kinds.
    pub fn all() -> impl Iterator<Item = Self> {
        ClientKind::VARIANTS
            .iter()
            .copied()
            .map(Self::Client)
            .chain(ErrorOnlyKind::VARIANTS.iter().copied().map(Self::ErrorOnly))
    }

    /// The subcommand this kind names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client(kind) => kind.as_str(),
            Self::ErrorOnly(kind) => kind.as_str(),
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::all().find(|kind| kind.as_str() == name)
    }

    /// The code a usage failure of this subcommand carries. `status`, `env`,
    /// `token` and `config` have no argument code of their own, so they share the
    /// generic one.
    pub fn usage_code(self) -> ErrorCode {
        match self {
            Self::Client(kind) => kind.usage_code(),
            Self::ErrorOnly(ErrorOnlyKind::Api) => ErrorCode::ApiArgumentInvalid,
            Self::ErrorOnly(
                ErrorOnlyKind::Status
                | ErrorOnlyKind::Env
                | ErrorOnlyKind::Token
                | ErrorOnlyKind::Config
                | ErrorOnlyKind::Preset
                | ErrorOnlyKind::Obsidian,
            ) => ErrorCode::ArgumentInvalid,
        }
    }

    /// The hint of a usage failure: the help of the subcommand that failed.
    /// A client's usage document keeps its own recovery text.
    pub fn usage_hint(self) -> Option<String> {
        match self {
            // `s3` takes no request document, so its help is what to read.
            Self::Client(ClientKind::Data | ClientKind::Db) => None,
            // `config` is a group: its help lists the subcommands, and each
            // subcommand's help its arguments.
            Self::ErrorOnly(ErrorOnlyKind::Config) => Some(
                "run `kurama config --help` to see its subcommands, and `kurama config <subcommand> --help` for their arguments"
                    .to_owned(),
            ),
            Self::Client(ClientKind::S3)
            | Self::ErrorOnly(
                ErrorOnlyKind::Api
                | ErrorOnlyKind::Status
                | ErrorOnlyKind::Env
                | ErrorOnlyKind::Token
                | ErrorOnlyKind::Preset
                | ErrorOnlyKind::Obsidian,
            ) => Some(format!(
                "run `kurama {} --help` to see the arguments it takes",
                self.as_str()
            )),
        }
    }

    /// Whether a usage failure is this subcommand's to report when no JSON
    /// was asked for. Only a client does; the others keep clap's text.
    pub fn reports_text_usage(self) -> bool {
        match self {
            Self::Client(_) => true,
            Self::ErrorOnly(_) => false,
        }
    }
}

/// The subcommand an argv invokes, and whether it asked for JSON output.
/// Read from the raw arguments because a usage failure has no parsed matches.
pub struct ClientInvocation {
    pub kind: JsonErrorKind,
    pub json: bool,
}

impl ClientInvocation {
    /// Whether a usage failure of this run is reported by kurama rather than
    /// by clap: always for a client, and with JSON for the others.
    pub fn reports_usage(&self) -> bool {
        self.json || self.kind.reports_text_usage()
    }
}

/// The subcommand `args` invokes, when it is one with a JSON error contract.
pub fn classify_invocation(args: &[OsString]) -> Option<ClientInvocation> {
    let kind = args
        .get(1)
        .and_then(|name| name.to_str())
        .and_then(JsonErrorKind::parse)?;
    let json = args.iter().any(|arg| {
        arg == "--json" || arg == "--jq" || arg.to_str().is_some_and(|s| s.starts_with("--jq="))
    });
    Some(ClientInvocation { kind, json })
}

/// Reading a typed JSON request failed. The client classifies it: only it knows
/// what a missing request file means for its own contract.
pub enum RequestReadFailure {
    Stdin(std::io::Error),
    File(std::io::Error),
}

/// Read a typed JSON request: a file, or stdin for `-`, so an input that
/// carries a credential never has to appear in argv.
pub fn read_request_text(path: &str) -> Result<String, RequestReadFailure> {
    if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(RequestReadFailure::Stdin)?;
        Ok(text)
    } else {
        std::fs::read_to_string(path).map_err(RequestReadFailure::File)
    }
}

/// `--request`, `--json`, `--jq` and `--dry-run`, in that order: the flags
/// every client reads the same way. What a request file and a plan mean is
/// the subcommand's, so it writes those two help lines.
pub fn envelope_args(command: Command, request_help: &str, dry_run_help: &str) -> Command {
    command
        .arg(
            Arg::new("request")
                .long("request")
                .value_name("JSON_FILE|-")
                .value_hint(ValueHint::FilePath)
                .help(request_help.to_owned()),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print one JSON result and structured errors"),
        )
        .arg(
            Arg::new("jq")
                .long("jq")
                .value_name("FILTER")
                .value_hint(ValueHint::Other)
                .help(
                    "Project the result envelope to one JSON value (implies --json), e.g. '.rows'",
                ),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help(dry_run_help.to_owned()),
        )
}

/// A request or SQL file the run was told to read and could not open: the
/// caller named it, so it is a usage failure; any other read error is I/O.
pub fn is_unopenable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    )
}

/// A `--jq` filter that did not produce exactly one JSON value. Each client
/// reports it with its own code.
#[derive(Debug)]
pub struct InvalidProjection;

/// Project a typed result envelope to exactly one JSON value.
pub fn project_document<T: serde::Serialize>(
    document: &T,
    filter: &str,
) -> Result<Value, InvalidProjection> {
    let value = serde_json::to_value(document).expect("typed output serializes");
    project(&value, filter)
}

/// The envelope as one line of JSON.
pub fn json_line<T: serde::Serialize>(document: &T) -> String {
    serde_json::to_string(document).expect("typed output serializes")
}

/// A header line and one line per row, tab-separated, every name and cell
/// through `safe_text`.
pub fn tab_separated<'a>(names: impl Iterator<Item = &'a str>, rows: &[Vec<Value>]) -> String {
    let mut text = names.map(safe_text).collect::<Vec<_>>().join("\t");
    text.push('\n');
    for row in rows {
        text.push_str(
            &row.iter()
                .map(|cell| match cell.as_str() {
                    Some(cell) => safe_text(cell),
                    None => safe_text(&cell.to_string()),
                })
                .collect::<Vec<_>>()
                .join("\t"),
        );
        text.push('\n');
    }
    text
}

/// Names and cells come from a database or a file, so a control character in
/// one never reaches the terminal as a control character.
pub fn safe_text(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Project a result envelope to exactly one JSON value.
pub fn project(value: &Value, filter: &str) -> Result<Value, InvalidProjection> {
    // Wrapping in an array preserves JSON strings and lets us enforce one result.
    let lines = crate::adapters::jq::apply_filter(&format!("[\n{filter}\n]"), value, None)
        .map_err(|_| InvalidProjection)?;
    let [line] = lines.as_slice() else {
        return Err(InvalidProjection);
    };
    let mut values: Vec<Value> = serde_json::from_str(line).map_err(|_| InvalidProjection)?;
    if values.len() != 1 {
        return Err(InvalidProjection);
    }
    Ok(values.pop().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_document_projects_and_prints_as_one_json_line() {
        let document = json!({"meta": {"rows": 2}, "rows": [[1], [2]]});
        assert_eq!(project_document(&document, ".meta.rows").unwrap(), json!(2));
        assert!(project_document(&document, ".rows[]").is_err());
        assert_eq!(
            json_line(&document),
            r#"{"meta":{"rows":2},"rows":[[1],[2]]}"#
        );
    }

    #[test]
    fn only_a_file_that_cannot_be_opened_is_the_callers_to_fix() {
        use std::io::{Error, ErrorKind};
        assert!(is_unopenable(&Error::from(ErrorKind::NotFound)));
        assert!(is_unopenable(&Error::from(ErrorKind::PermissionDenied)));
        assert!(!is_unopenable(&Error::from(ErrorKind::InvalidData)));
    }

    #[test]
    fn every_kind_names_its_subcommand_and_parses_back() {
        for kind in ClientKind::VARIANTS {
            assert_eq!(ClientKind::parse(kind.as_str()), Some(*kind));
            assert!(
                crate::build_command()
                    .find_subcommand(kind.as_str())
                    .is_some()
            );
        }
        assert_eq!(ClientKind::values(), ["data", "db", "s3"]);
        assert_eq!(ClientKind::parse("aws"), None);
    }

    /// `agent --kind` lists the kinds with a request contract; the error
    /// contract covers those and the error-only kinds, which it never lists.
    #[test]
    fn agent_kinds_are_the_clients_and_the_error_contract_is_wider() {
        for kind in ClientKind::VARIANTS {
            assert_eq!(
                JsonErrorKind::parse(kind.as_str()),
                Some(JsonErrorKind::Client(*kind))
            );
        }
        for kind in ErrorOnlyKind::VARIANTS
            .iter()
            .copied()
            .map(JsonErrorKind::ErrorOnly)
        {
            assert!(!ClientKind::values().contains(&kind.as_str()));
            assert_eq!(ClientKind::parse(kind.as_str()), None);
            assert_eq!(JsonErrorKind::parse(kind.as_str()), Some(kind));
            assert!(!kind.reports_text_usage());
            assert_eq!(kind.usage_code().exit_code(), 2);
            // `config` takes `--json` on its subcommands.
            let command = crate::build_command();
            let command = command
                .find_subcommand(kind.as_str())
                .expect("the subcommand exists");
            let json = std::iter::once(command)
                .chain(command.get_subcommands())
                .any(|command| command.get_arguments().any(|arg| arg.get_id() == "json"));
            assert!(json, "{} has --json", kind.as_str());
        }
        assert_eq!(
            ErrorOnlyKind::VARIANTS
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>(),
            [
                "api", "status", "env", "token", "config", "preset", "obsidian"
            ]
        );
        assert!(JsonErrorKind::Client(ClientKind::Data).reports_text_usage());
        assert_eq!(
            JsonErrorKind::ErrorOnly(ErrorOnlyKind::Api).usage_code(),
            ErrorCode::ApiArgumentInvalid
        );
        let s3 = JsonErrorKind::Client(ClientKind::S3);
        assert_eq!(s3.usage_code(), ErrorCode::S3Invalid);
        // `s3` has no request document to point at, so its usage failure
        // keeps the help hint.
        assert_eq!(
            s3.usage_hint().as_deref(),
            Some("run `kurama s3 --help` to see the arguments it takes")
        );
        for kind in [
            ErrorOnlyKind::Status,
            ErrorOnlyKind::Env,
            ErrorOnlyKind::Token,
            ErrorOnlyKind::Config,
            ErrorOnlyKind::Preset,
            ErrorOnlyKind::Obsidian,
        ] {
            assert_eq!(
                JsonErrorKind::ErrorOnly(kind).usage_code(),
                ErrorCode::ArgumentInvalid
            );
        }
        assert_eq!(
            JsonErrorKind::ErrorOnly(ErrorOnlyKind::Token)
                .usage_hint()
                .as_deref(),
            Some("run `kurama token --help` to see the arguments it takes")
        );
        assert_eq!(JsonErrorKind::Client(ClientKind::Db).usage_hint(), None);
    }

    /// The subcommands that take `--json` and have no JSON error contract,
    /// each with the reason. A subcommand that takes `--json` is either a
    /// `JsonErrorKind` or listed here.
    const JSON_WITHOUT_ERROR_DOCUMENT: &[(&str, &str)] = &[
        (
            "agent",
            "prints a constant contract read off the binary; it reads no configuration and has nothing to fail at but its arguments, which clap reports",
        ),
        (
            "audit",
            "reads the local audit log only; a failure is the text error line, and no agent contract promises it a document",
        ),
    ];

    #[test]
    fn every_subcommand_with_json_has_an_error_contract_or_a_reason() {
        fn takes_json(command: &Command) -> bool {
            command.get_arguments().any(|arg| arg.get_id() == "json")
                || command.get_subcommands().any(takes_json)
        }
        let root = crate::build_command();
        let mut unexplained = Vec::new();
        let mut listed = Vec::new();
        for command in root.get_subcommands().filter(|command| takes_json(command)) {
            let name = command.get_name();
            let reason = JSON_WITHOUT_ERROR_DOCUMENT
                .iter()
                .find(|(listed, _)| *listed == name);
            match (JsonErrorKind::parse(name), reason) {
                (Some(_), None) | (None, Some(_)) => {}
                (Some(_), Some(_)) => listed.push(name.to_owned()),
                (None, None) => unexplained.push(name.to_owned()),
            }
        }
        assert!(
            unexplained.is_empty(),
            "takes --json with no JsonErrorKind and no reason: {unexplained:?}"
        );
        assert!(
            listed.is_empty(),
            "has a JsonErrorKind and is still listed without one: {listed:?}"
        );
        for (name, _) in JSON_WITHOUT_ERROR_DOCUMENT {
            let command = root
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("{name} is a subcommand"));
            assert!(takes_json(command), "{name} no longer takes --json");
        }
    }

    #[test]
    fn only_a_json_client_subcommand_is_classified() {
        let argv = |args: &[&str]| -> Vec<OsString> {
            std::iter::once("kurama")
                .chain(args.iter().copied())
                .map(OsString::from)
                .collect()
        };
        let invocation = classify_invocation(&argv(&["data", "--jq=.rows"])).unwrap();
        assert_eq!(invocation.kind, JsonErrorKind::Client(ClientKind::Data));
        assert!(invocation.json);
        assert!(
            !classify_invocation(&argv(&["data", "./x.csv"]))
                .unwrap()
                .json
        );
        assert!(
            classify_invocation(&argv(&["data", "--json"]))
                .unwrap()
                .json
        );
        assert_eq!(
            classify_invocation(&argv(&["db", "app", "--json"]))
                .expect("a db invocation")
                .kind,
            JsonErrorKind::Client(ClientKind::Db)
        );
        let status = classify_invocation(&argv(&["status", "--json"])).unwrap();
        assert_eq!(status.kind, JsonErrorKind::ErrorOnly(ErrorOnlyKind::Status));
        assert!(status.reports_usage());
        let api = classify_invocation(&argv(&["api", "github", "/x"])).unwrap();
        assert!(!api.json);
        assert!(!api.reports_usage());
        assert!(
            classify_invocation(&argv(&["data", "x.csv"]))
                .unwrap()
                .reports_usage()
        );
        assert!(classify_invocation(&argv(&["exec", "p", "--", "x", "--json"])).is_none());
        assert!(classify_invocation(&argv(&[])).is_none());
    }

    #[test]
    fn jq_projection_keeps_json_values_and_accepts_trailing_comments() {
        let input = json!({"rows": [["001", "日本語"], [null, true]]});
        assert_eq!(
            project(&input, ".rows # only the rows").unwrap(),
            input["rows"]
        );
        assert_eq!(project(&input, ".rows[0][0]").unwrap(), "001");
        assert_eq!(project(&input, ".missing").unwrap(), Value::Null);
        assert!(project(&input, "empty").is_err());
        assert!(project(&input, ".rows[]").is_err());
    }
}
