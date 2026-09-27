//! `kurama api`: its clap definition and the state one invocation parses to.

use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command};
use clap_complete::engine::{ArgValueCandidates, ArgValueCompleter};
use serde_json::{Value, json};

use crate::domain::functions::api_body_fit::BodyFit;
use crate::domain::functions::api_pages::{CursorStyle, PageStyle};
use crate::domain::functions::signing_target::SigningHint;
use crate::shell::api_error::ApiError;
use crate::shell::cli::arguments::{ArgumentFacts, visible_arguments};
use crate::shell::cli::completion;

use super::api_spec::ListingFormat;

/// Everything `kurama api` was told on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiCommand {
    pub(crate) api: String,
    pub(crate) target: Option<String>,
    pub(crate) request: ApiRequestOptions,
    pub(crate) output: ApiOutputOptions,
    pub(crate) connection: ApiConnectionOptions,
    /// `--service` / `--region`: the SigV4 target of an `aws_profile` API,
    /// over the profile's own and the host's.
    pub(crate) signing: SigningHint,
    pub(crate) spec: ApiSpecOptions,
    /// `--pages N` (with `--cursor`): fetch up to N pages instead of one.
    pub(crate) pages: Option<ApiPages>,
}

/// `--pages N` and how the next page is found: the `Link` header, or the
/// `--cursor PATH=PARAM` the caller names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiPages {
    pub(crate) limit: u32,
    pub(crate) style: PageStyle,
}

/// Request options shared by plain and OpenAPI operation targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiRequestOptions {
    pub(crate) method: Option<String>,
    /// `-H "Name: value"`, in order.
    pub(crate) headers: Vec<String>,
    /// `-d`: a literal, `@file`, or `@-` for stdin.
    pub(crate) data: Option<String>,
    /// `-P name=value`, in order.
    pub(crate) params: Vec<String>,
    /// `--confirm`: a person agreed to this call, so the `[agent]` policy
    /// does not hold it back.
    pub(crate) confirm: bool,
    /// `--select`: the selection set of a GraphQL operation, without its
    /// braces.
    pub(crate) select: Option<String>,
}

/// Response and diagnostic options, kept separate from request construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiOutputOptions {
    pub(crate) jq: Option<String>,
    pub(crate) json: bool,
    pub(crate) dry_run: bool,
    pub(crate) verbose: bool,
    /// `--output PATH`: the file a 2xx body is written to; `None` is stdout,
    /// which `--output -` names explicitly.
    pub(crate) output: Option<PathBuf>,
    /// `--shape` / `--sample N`: the printed body cut to fit a context.
    pub(crate) fit: Option<BodyFit>,
}

impl ApiOutputOptions {
    /// `--dry-run` with `--json` or `--jq`: the plan document on stdout
    /// instead of the request on stderr.
    pub(crate) fn plan_requested(&self) -> bool {
        self.dry_run && (self.json || self.jq.is_some())
    }

    /// `--output PATH` takes the body itself, so a `--json` envelope or
    /// `--jq` results beside it would have nowhere to go but stdout, and
    /// which of the two holds what would be a guess. A dry run writes no
    /// file: its `--json` / `--jq` are the plan, which names the write.
    pub(crate) fn check_output(&self) -> Result<(), ApiError> {
        if self.output.is_some() && !self.dry_run && (self.json || self.jq.is_some()) {
            return Err(ApiError::ArgumentInvalid(
                "--output PATH takes the response body; drop --json / --jq, or use --output - to print them".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn listing_format(&self) -> ListingFormat<'_> {
        ListingFormat {
            json: self.json,
            jq: self.jq.as_deref(),
        }
    }
}

/// Transport options for API and OpenAPI-description requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApiConnectionOptions {
    pub(crate) insecure: bool,
    pub(crate) timeout_secs: u64,
}

/// OpenAPI listing, description and cache controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiSpecOptions {
    /// `--ops [QUERY]`: the query, empty for every operation.
    pub(crate) ops: Option<String>,
    /// `--describe <OP>`.
    pub(crate) describe: Option<String>,
    /// `--schema [OP]`: `Some(None)` for every operation.
    pub(crate) schema: Option<Option<String>>,
    /// `--skill`.
    pub(crate) skill: bool,
    /// `--refresh-spec`.
    pub(crate) refresh: bool,
}

impl ApiCommand {
    pub(crate) fn parse(api: &ArgMatches) -> Self {
        Self {
            api: api
                .get_one::<String>("api")
                .expect("clap requires API")
                .clone(),
            target: api.get_one::<String>("target").cloned(),
            request: ApiRequestOptions {
                method: api.get_one::<String>("method").cloned(),
                headers: api
                    .get_many::<String>("header")
                    .map(|headers| headers.cloned().collect())
                    .unwrap_or_default(),
                data: api.get_one::<String>("data").cloned(),
                params: api
                    .get_many::<String>("param")
                    .map(|params| params.cloned().collect())
                    .unwrap_or_default(),
                confirm: api.get_flag("confirm"),
                select: api.get_one::<String>("select").cloned(),
            },
            output: ApiOutputOptions {
                jq: api.get_one::<String>("jq").cloned(),
                json: api.get_flag("json"),
                dry_run: api.get_flag("dry-run"),
                verbose: api.get_flag("verbose"),
                output: api
                    .get_one::<String>("output")
                    .filter(|path| *path != "-")
                    .map(PathBuf::from),
                fit: match api.get_one::<u32>("sample") {
                    Some(keep) => Some(BodyFit::Sample(*keep)),
                    None => api.get_flag("shape").then_some(BodyFit::Shape),
                },
            },
            connection: ApiConnectionOptions {
                insecure: api.get_flag("insecure"),
                timeout_secs: *api
                    .get_one::<u64>("timeout")
                    .expect("timeout has a default"),
            },
            signing: SigningHint {
                service: api.get_one::<String>("service").cloned(),
                region: api.get_one::<String>("region").cloned(),
            },
            spec: ApiSpecOptions {
                // `--ops` alone lists everything; `--ops QUERY` filters.
                ops: api
                    .contains_id("ops")
                    .then(|| api.get_one::<String>("ops").cloned().unwrap_or_default()),
                describe: api.get_one::<String>("describe").cloned(),
                schema: api
                    .contains_id("schema")
                    .then(|| api.get_one::<String>("schema").cloned()),
                skill: api.get_flag("skill"),
                refresh: api.get_flag("refresh-spec"),
            },
            pages: api.get_one::<u32>("pages").map(|limit| ApiPages {
                limit: *limit,
                style: match api.get_one::<CursorStyle>("cursor") {
                    Some(cursor) => PageStyle::Cursor(cursor.clone()),
                    None => PageStyle::Link,
                },
            }),
        }
    }
}

pub(crate) fn command() -> Command {
    Command::new("api")
        .about("Call an [api.*] profile with its bearer token or SigV4 signature; explore its OpenAPI description")
        .long_about(
            "Send one HTTP request to an [api.*] profile with its bearer token or SigV4 signature.\n\n\
             TARGET is a path under base_url (/user), a URL on the origin of base_url,\n\
             `METHOD /path`, or an operation of the API's OpenAPI description (an operationId\n\
             such as issues/create, or `METHOD /path/{param}` with -P name=value for its\n\
             parameters); the credential never goes to another host, and with openapi_auth\n\
             the description URL must be on that host too. Without a TARGET the\n\
             explorer opens on a terminal. --ops lists the operations, --describe shows one\n\
             with its parameters, body skeleton, scopes and an example command, and --schema\n\
             prints the versioned JSON contract an agent builds calls from (--skill turns it\n\
             into an Agent Skill, SKILL.md); the\n\
             description comes from `openapi` under [api.<name>] (a URL, cached under\n\
             ~/.cache/kurama/openapi/, or a file) and --refresh-spec fetches it again.\n\n\
             The body goes to stdout, or with --output PATH to a file that is replaced (not\n\
             written through) only after a 2xx body was written whole; a status outside 2xx is error[API_HTTP_ERROR] with exit\n\
             code 4. A bearer-token 401 gets a new token and retries once. Accept defaults to\n\
             application/json, and so does Content-Type when -d is given. -k relaxes\n\
             certificate checks for the API request only, never for the authorization server.\n\n\
             An API with aws_profile is signed with the profile's role credentials (SigV4).\n\
             The service and region come from --service / --region, the profile's service /\n\
             region, the host (API Gateway, Lambda function URLs, OpenSearch, AppSync and\n\
             <service>.<region>.amazonaws.com), then the AWS profile's region. A signed request is sent once;\n\
             rejection is error[API_HTTP_ERROR] without retry.\n\n\
             --pages N fetches up to N pages, one request at a time: the next page is the\n\
             Link rel=\"next\" of each response (RFC 8288), or with --cursor PATH=PARAM the\n\
             value at PATH in the JSON body, sent as the query parameter PARAM of the first\n\
             request. Each page is one line on stdout (the body, the --json envelope, or the\n\
             --jq results of that page), and a last stderr line says why paging stopped:\n\
             limit, last, unsupported or refused (a JSON document with --json / --jq).\n\
             --pages takes a GET only: each page repeats the request.",
        )
        .arg(
            Arg::new("api")
                .add(ArgValueCandidates::new(completion::apis))
                .value_name("API")
                .help("[api.*] profile in config.toml")
                .required(true),
        )
        .arg(
            Arg::new("target")
                .add(ArgValueCandidates::new(completion::operations))
                .value_name("TARGET")
                .help("/path, a URL on the origin of base_url, `METHOD /path`, or an operationId of the description"),
        )
        .arg(
            Arg::new("param")
                .add(ArgValueCompleter::new(completion::parameters))
                .short('P')
                .long("param")
                .value_name("NAME=VALUE")
                .help("Operation parameter; the description says whether it goes in the path, the query or a header. Repeatable")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("ops")
                .long("ops")
                .value_name("QUERY")
                .num_args(0..=1)
                .add(ArgValueCandidates::new(completion::operations))
                .help("List the operations of the description (those matching QUERY); --json for JSON")
                .conflicts_with_all(["target", "describe"]),
        )
        .arg(
            Arg::new("describe")
                .add(ArgValueCandidates::new(completion::operations))
                .long("describe")
                .value_name("OP")
                .help("Describe one operation: parameters, body skeleton, scopes, example command; --json for JSON")
                .conflicts_with("target"),
        )
        .arg(
            Arg::new("schema")
                .add(ArgValueCandidates::new(completion::operations))
                .long("schema")
                .value_name("OP")
                .num_args(0..=1)
                .help("Print the machine-readable contract of the API (of OP): options, parameter and body schemas, limitations; loads the description like --ops and calls no operation")
                .conflicts_with_all(["target", "ops", "describe"]),
        )
        .arg(
            Arg::new("skill")
                .long("skill")
                .help("Print an Agent Skill (SKILL.md) for the API, written from the --schema contract; loads the description like --ops and calls no operation")
                .action(ArgAction::SetTrue)
                .conflicts_with_all(["target", "ops", "describe", "schema", "json", "jq"]),
        )
        .arg(
            Arg::new("refresh-spec")
                .long("refresh-spec")
                .help("Fetch the description again instead of revalidating the cached copy")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("method")
                .short('X')
                .long("method")
                .value_name("METHOD")
                .add(ArgValueCandidates::new(completion::http_methods))
                .help("HTTP method (default GET, or POST with -d)"),
        )
        .arg(
            Arg::new("header")
                .short('H')
                .long("header")
                .value_name("NAME: VALUE")
                .help("Request header; repeatable")
                // Free text: the header set an API accepts is not in the description.
                .value_hint(clap::ValueHint::Other)
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("data")
                .short('d')
                .long("data")
                .value_name("DATA")
                // A literal body, or `@` then a path: completing the path alone
                // would insert it without the `@` the value needs.
                .value_hint(clap::ValueHint::Other)
                .help("Request body: a literal, @file, or @- for stdin"),
        )
        .arg(
            Arg::new("jq")
                .long("jq")
                .add(ArgValueCompleter::new(completion::jq))
                .value_name("FILTER")
                .help("Print the results of a jq filter on the body (on the envelope with --json)"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .help("Print the {status, headers, body} envelope instead of the body")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("PATH")
                .value_hint(clap::ValueHint::FilePath)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .requires("target")
                // clap waives `requires` when an argument that conflicts
                // with TARGET is present.
                .conflicts_with_all(["ops", "describe", "schema", "skill"])
                .help("Write a 2xx response body to PATH byte for byte (- is stdout). PATH is replaced once the whole body is written: a symlink or hard link there is not followed, the new file has mode 600, and a read-only file is refused; not with --json / --jq outside --dry-run"),
        )
        .arg(
            Arg::new("shape")
                .long("shape")
                .requires("target")
                .conflicts_with_all(["ops", "describe", "schema", "skill", "output", "sample"])
                .help("Print the type of each value of the JSON body instead of the value: object keys kept, an array as {\"array\": COUNT, \"of\": SHAPE}, differing shapes as a list; before --json / --jq and for each page; not with --output")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("sample")
                .long("sample")
                .value_name("N")
                .value_hint(clap::ValueHint::Other)
                .value_parser(clap::value_parser!(u32))
                .requires("target")
                .conflicts_with_all(["ops", "describe", "schema", "skill", "output"])
                .help("Print every array of the JSON body cut to its first N elements, ending one that was cut with {\"…\": {\"omitted\": M}}; before --json / --jq and for each page; not with --output"),
        )
        .arg(
            Arg::new("pages")
                .long("pages")
                .value_name("N")
                .value_hint(clap::ValueHint::Other)
                .value_parser(clap::value_parser!(u32).range(1..))
                .requires("target")
                .conflicts_with_all(["ops", "describe", "schema", "skill", "output"])
                .help("Fetch up to N pages, following the Link rel=\"next\" of each response (or --cursor): one line per page on stdout, and why paging stopped (limit, last, unsupported, refused) as the last stderr line; GET only, not with --output"),
        )
        .arg(
            Arg::new("cursor")
                .long("cursor")
                .value_name("PATH=PARAM")
                // A body path of the caller's API: nothing to complete from.
                .value_hint(clap::ValueHint::Other)
                .value_parser(CursorStyle::parse)
                .requires("pages")
                .help("With --pages: the next page is the value at PATH in the JSON body (meta.next_cursor), sent as the query parameter PARAM of the first request; null, \"\" or no PATH on a later page ends paging"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .help("Print the request on stderr (credentials masked) and send nothing; with --json, the plan (request, auth, description source, effects) as JSON on stdout")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("confirm")
                .long("confirm")
                .help("A person agreed to this call: send it even when the [agent] policy of a KURAMA_AGENT run would refuse it")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Print request and response headers on stderr (credentials masked)")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("insecure")
                .short('k')
                .long("insecure")
                .help("Skip TLS certificate verification for the API request")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .value_name("SECONDS")
                .help("Request timeout")
                .value_hint(clap::ValueHint::Other)
                .default_value("60")
                .value_parser(clap::value_parser!(u64).range(1..)),
        )
        .arg(
            Arg::new("service")
                .long("service")
                .value_name("SERVICE")
                // Any AWS service can be signed for; kurama has no list of them.
                .value_hint(clap::ValueHint::Other)
                .help("SigV4 service name, e.g. execute-api (aws_profile APIs; wins over the profile and the host)"),
        )
        .arg(
            Arg::new("region")
                .long("region")
                .value_name("REGION")
                .value_hint(clap::ValueHint::Other)
                .help("SigV4 region, e.g. ap-northeast-1 (aws_profile APIs; wins over the profile and the host)"),
        )
        .arg(
            Arg::new("select")
                .long("select")
                .value_name("FIELDS")
                // A selection set of the caller's schema: nothing to complete from.
                .value_hint(clap::ValueHint::Other)
                .requires("target")
                .conflicts_with_all(["ops", "describe", "schema", "skill"])
                .help("The selection set of a GraphQL operation target, without its braces ('nodes { id title }'); without it, the scalar and enum fields of the return type"),
        )
}

/// The `options` of the `--schema` contract: every argument of `kurama api`
/// as the definition above states it, so the contract cannot drift from
/// what the parser accepts.
pub(crate) fn options_contract() -> Value {
    let command = command();
    Value::Array(
        visible_arguments(&command)
            .map(|arg| {
                let facts = ArgumentFacts::of(arg);
                let name = match facts.long {
                    Some(long) => format!("--{long}"),
                    None => facts
                        .value_name
                        .clone()
                        .unwrap_or_else(|| facts.id.to_owned()),
                };
                json!({
                    "name": name,
                    "short": facts.short.map(|short| format!("-{short}")),
                    "value": if facts.long.is_some() { facts.value_name } else { None },
                    "repeatable": facts.multiple,
                    "help": arg.get_help().map(ToString::to_string),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_options_contract_is_the_clap_definition() {
        let options = options_contract();
        let option = |name: &str| {
            options
                .as_array()
                .unwrap()
                .iter()
                .find(|option| option["name"] == name)
                .unwrap_or_else(|| panic!("{name} in {options}"))
                .clone()
        };
        assert_eq!(
            option("--param"),
            json!({
                "name": "--param",
                "short": "-P",
                "value": "NAME=VALUE",
                "repeatable": true,
                "help": "Operation parameter; the description says whether it goes in the path, the query or a header. Repeatable",
            })
        );
        assert_eq!(option("--dry-run")["value"], Value::Null);
        assert_eq!(option("--dry-run")["repeatable"], false);
        assert_eq!(option("--jq")["value"], "FILTER");
        assert_eq!(option("TARGET")["short"], Value::Null);
        assert_eq!(option("TARGET")["value"], Value::Null);
        assert_eq!(
            options.as_array().unwrap().len(),
            command().get_arguments().count()
        );
    }

    #[test]
    fn schema_alone_means_every_operation_and_conflicts_with_a_target() {
        let parse = |args: &[&str]| {
            command()
                .try_get_matches_from(args)
                .map(|matches| ApiCommand::parse(&matches).spec.schema)
        };
        assert_eq!(parse(&["api", "pets", "--schema"]).unwrap(), Some(None));
        assert_eq!(
            parse(&["api", "pets", "--schema", "pets/get"]).unwrap(),
            Some(Some("pets/get".into()))
        );
        assert_eq!(parse(&["api", "pets"]).unwrap(), None);
        assert!(parse(&["api", "pets", "/x", "--schema"]).is_err());
        assert!(parse(&["api", "pets", "--schema", "--ops"]).is_err());
    }

    #[test]
    fn skill_is_a_flag_that_stands_alone() {
        let parse = |args: &[&str]| {
            command()
                .try_get_matches_from(args)
                .map(|matches| ApiCommand::parse(&matches).spec.skill)
        };
        assert!(parse(&["api", "pets", "--skill"]).unwrap());
        assert!(parse(&["api", "pets", "--skill", "--refresh-spec"]).unwrap());
        assert!(!parse(&["api", "pets", "--ops"]).unwrap());
        for conflicting in [
            &["api", "pets", "/x", "--skill"][..],
            &["api", "pets", "--skill", "--schema"],
            &["api", "pets", "--skill", "--ops"],
            &["api", "pets", "--skill", "--describe", "x"],
            &["api", "pets", "--skill", "--json"],
            &["api", "pets", "--skill", "--jq", "."],
            &["api", "pets", "--skill", "-o", "x"],
        ] {
            assert!(parse(conflicting).is_err(), "{conflicting:?}");
        }
    }

    #[test]
    fn pages_follow_the_link_header_unless_a_cursor_is_named() {
        let parse = |args: &[&str]| {
            command()
                .try_get_matches_from(args)
                .map(|matches| ApiCommand::parse(&matches).pages)
        };
        assert_eq!(parse(&["api", "pets", "/x"]).unwrap(), None);
        assert_eq!(
            parse(&["api", "pets", "/x", "--pages", "3"]).unwrap(),
            Some(ApiPages {
                limit: 3,
                style: PageStyle::Link
            })
        );
        assert_eq!(
            parse(&[
                "api",
                "pets",
                "/x",
                "--pages",
                "2",
                "--cursor",
                "meta.next=cursor"
            ])
            .unwrap(),
            Some(ApiPages {
                limit: 2,
                style: PageStyle::Cursor(CursorStyle::parse("meta.next=cursor").unwrap())
            })
        );
        for refused in [
            &["api", "pets", "/x", "--pages", "0"][..],
            &["api", "pets", "/x", "--cursor", "a=b"],
            &["api", "pets", "/x", "--pages", "2", "--cursor", "a"],
            &["api", "pets", "/x", "--pages", "2", "-o", "out.json"],
            &["api", "pets", "--ops", "--pages", "2"],
            &["api", "pets", "--pages", "2"],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn output_takes_a_non_empty_path_or_dash_for_stdout() {
        let parse = |args: &[&str]| {
            command()
                .try_get_matches_from(args)
                .map(|matches| ApiCommand::parse(&matches).output.output)
        };
        assert_eq!(
            parse(&["api", "pets", "/x", "-o", "body.json"]).unwrap(),
            Some(PathBuf::from("body.json"))
        );
        assert_eq!(
            parse(&["api", "pets", "/x", "--output", "-"]).unwrap(),
            None
        );
        assert!(parse(&["api", "pets", "/x", "--output", ""]).is_err());
        assert!(parse(&["api", "pets", "--ops", "-o", "x"]).is_err());
    }
}
