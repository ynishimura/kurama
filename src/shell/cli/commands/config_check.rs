//! `kurama config check [--json]`: config.toml checked section by section without a secret store, a keychain, STS, OAuth or the network, every problem listed on stdout.
//!
//! stdout carries the report whatever it found: the file in use and what
//! chose it, `~/.aws/config`, the state of each `[api.*] openapi`, and one
//! diagnostic per problem, each with the code and hint the command that hits
//! it would print. The verdict is the exit: when any diagnostic is an
//! `error`, the run fails as every configuration error does -- one
//! `CONFIG_INVALID` line (a JSON error document under `--json`) on stderr and
//! exit 2, whatever codes the diagnostics carry. A `warning` (a cached
//! description kurama would fetch again) leaves the run successful.

use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use serde::Serialize;

use crate::adapters::config::check::check;
use crate::adapters::config::references::check_aws;
use crate::adapters::config::{Config, SpecSource};
use crate::adapters::error::CoreError;
use crate::adapters::openapi::{SpecCache, default_cache_dir};
use crate::adapters::profile::loader::AwsConfigLoader;
use crate::shell::api_error::ApiError;
use crate::shell::cli::error_code::ErrorCode;
use crate::shell::cli::executor::CliExecutorError;
use crate::shell::spec_loader;

/// The version of the report's shape; a field removed or renamed bumps it.
const SCHEMA_VERSION: u64 = 1;

/// `config check`.
pub fn command() -> Command {
    Command::new("check")
        .about("Check config.toml and list every problem; no secret store, keychain, STS, OAuth or network")
        .long_about(
            "Check config.toml and list every problem on stdout: syntax, unknown keys,\n\
             references between [auth.*], [api.*] and ~/.aws/config, exclusive keys, values,\n\
             and each [api.*] openapi (a file is read and normalized; a URL is not fetched,\n\
             only its cached copy is read). Each [auth.*], [api.*], [data.*], [db.*] and\n\
             [s3.*] section is read on its own, so one run reports one problem per section;\n\
             a syntax error is reported alone. No secret is resolved.\n\n\
             Exit 0 when nothing is an error; otherwise CONFIG_INVALID (exit 2) on stderr,\n\
             with the problems on stdout.",
        )
        .arg(
            Arg::new("json")
                .long("json")
                .help("Print the report as one JSON document")
                .action(ArgAction::SetTrue),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    /// A command that uses the section fails on it. Stricter than `status`,
    /// which lists a section without following every reference: an
    /// `aws_profile` that the AWS config lacks, or a cached description that
    /// does not parse, is an error here because the command that uses it
    /// would fail on it.
    Error,
    /// Worth knowing; kurama recovers on its own (a cache entry it cannot
    /// read back is fetched again).
    Warning,
}

#[derive(Debug, Serialize)]
struct Diagnostic {
    severity: Severity,
    code: &'static str,
    /// `api.github`, `aws.session_cache`; null for the file as a whole.
    section: Option<String>,
    line: Option<usize>,
    message: String,
    hint: Option<String>,
}

impl Diagnostic {
    /// The code and hint are the ones the failing command would print.
    fn of(
        severity: Severity,
        section: Option<String>,
        line: Option<usize>,
        error: anyhow::Error,
    ) -> Self {
        let code = ErrorCode::classify(&error);
        Self {
            severity,
            code: code.as_str(),
            section,
            line,
            message: format!("{error:#}").replace(['\r', '\n'], " "),
            hint: code.hint(&error),
        }
    }
}

/// The config file in use, as every `config` subcommand reports it.
#[derive(Debug, Serialize)]
pub(super) struct ConfigFileState {
    path: String,
    /// `KURAMA_CONFIG_PATH` when that variable named the file, else `default`.
    source: &'static str,
    exists: bool,
}

impl ConfigFileState {
    pub(super) fn of(path: &std::path::Path, named: bool, exists: bool) -> Self {
        Self {
            path: path.display().to_string(),
            source: if named {
                "KURAMA_CONFIG_PATH"
            } else {
                "default"
            },
            exists,
        }
    }
}

#[derive(Debug, Serialize)]
struct AwsConfigFileState {
    path: String,
    /// `AWS_CONFIG_FILE` when that variable named the file, else `default`.
    source: &'static str,
    exists: bool,
}

/// The state of one `[api.*] openapi`.
#[derive(Debug, Serialize)]
struct SpecState {
    api: String,
    location: String,
    /// `read` (a file, normalized), `cached` (a URL whose cached copy
    /// normalized; not revalidated), `not_cached` (a URL never fetched:
    /// unverified), `invalid` (a diagnostic says why).
    state: &'static str,
    /// The file read: the description, or the cache entry of a URL.
    path: Option<String>,
    fetched_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct Verification {
    /// Nothing is fetched, so nothing remote is verified.
    network: &'static str,
    /// No secret, token or role credential is read.
    credentials: &'static str,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u64,
    valid: bool,
    config: ConfigFileState,
    aws_config: AwsConfigFileState,
    verification: Verification,
    openapi: Vec<SpecState>,
    diagnostics: Vec<Diagnostic>,
}

/// Check, print the report, and fail when it holds an error.
pub async fn run(json: bool) -> Result<()> {
    let report = inspect().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render(&report));
    }
    let errors = count(&report, Severity::Error);
    if errors > 0 {
        return Err(CoreError::config(format!(
            "{} has {errors} {}; stdout lists each",
            report.config.path,
            plural(errors, "error")
        ))
        .into());
    }
    Ok(())
}

async fn inspect() -> Result<Report> {
    let (path, named) = Config::config_source()?;
    let mut diagnostics = Vec::new();
    let mut exists = false;
    let mut config = None;
    let mut lines = None;
    match Config::read_source(&path, named).await {
        Err(error) => diagnostics.push(Diagnostic::of(Severity::Error, None, None, error)),
        Ok(content) => {
            exists = content.is_some();
            let mut checked = check(content.as_deref().unwrap_or_default());
            for problem in std::mem::take(&mut checked.problems) {
                diagnostics.push(Diagnostic::of(
                    Severity::Error,
                    problem.section,
                    problem.line,
                    problem.error.into(),
                ));
            }
            config = checked.config.take();
            lines = Some(checked);
        }
    }
    let (aws_path, aws_named) = AwsConfigLoader::new()?.config_source();
    let aws_exists = aws_path.exists();
    let mut openapi = Vec::new();
    if let (Some(config), Some(checked)) = (&config, &lines) {
        check_aws_references(config, &mut diagnostics, |section| checked.line(section)).await;
        openapi = check_openapi(config, &mut diagnostics, |section| checked.line(section))?;
    }
    diagnostics.sort_by_key(|diagnostic| diagnostic.line.unwrap_or(usize::MAX));
    Ok(Report {
        schema_version: SCHEMA_VERSION,
        valid: !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error),
        config: ConfigFileState::of(&path, named, exists),
        aws_config: AwsConfigFileState {
            path: aws_path.display().to_string(),
            source: if aws_named {
                "AWS_CONFIG_FILE"
            } else {
                "default"
            },
            exists: aws_exists,
        },
        verification: Verification {
            network: "not_contacted",
            credentials: "not_resolved",
        },
        openapi,
        diagnostics,
    })
}

/// `[auth.*]` names that are AWS profiles too, as `status` reads them (with no
/// AWS config the one profile is `default`), and, when the AWS config exists,
/// `aws_profile` values that name none of its profiles. A section that
/// already has a diagnostic is left to it, so each section reports one.
async fn check_aws_references(
    config: &Config,
    diagnostics: &mut Vec<Diagnostic>,
    line: impl Fn(&str) -> Option<usize>,
) {
    let checked = match check_aws(config).await {
        Ok(checked) => checked,
        Err(error) => {
            diagnostics.push(Diagnostic::of(Severity::Error, None, None, error));
            return;
        }
    };
    let diagnosed = |diagnostics: &[Diagnostic], section: &str| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.section.as_deref() == Some(section))
    };
    for (section, error) in checked.collisions {
        if diagnosed(diagnostics, &section) {
            continue;
        }
        diagnostics.push(Diagnostic::of(
            Severity::Error,
            Some(section.clone()),
            line(&section),
            error.into(),
        ));
    }
    for (section, profile) in checked.unknown.unwrap_or_default() {
        if diagnosed(diagnostics, &section) {
            continue;
        }
        let error = anyhow::Error::from(CliExecutorError::ProfileNotFound(profile))
            .context(format!("[{section}] aws_profile"));
        diagnostics.push(Diagnostic::of(
            Severity::Error,
            Some(section.clone()),
            line(&section),
            error,
        ));
    }
}

/// The state of each valid `[api.*]` that names an `openapi`: a file is read
/// and normalized as `kurama api` would; a URL is not fetched, only its cached
/// copy is read.
fn check_openapi(
    config: &Config,
    diagnostics: &mut Vec<Diagnostic>,
    line: impl Fn(&str) -> Option<usize>,
) -> Result<Vec<SpecState>> {
    let cache = SpecCache::new(default_cache_dir()?);
    let mut states = Vec::new();
    for api in config.api_profiles() {
        let name = &api.name;
        let Some(source) = api.spec.as_ref().map(|spec| &spec.source) else {
            continue;
        };
        let section = format!("api.{name}");
        let mut problem = |severity, error: anyhow::Error| {
            diagnostics.push(Diagnostic::of(
                severity,
                Some(section.clone()),
                line(&section),
                error,
            ));
        };
        let state = match source {
            SpecSource::File(location) => match spec_loader::load_file(&api, location) {
                Ok(loaded) => spec_state(name, location, "read", Some(loaded.origin.to_string())),
                Err(error) => {
                    problem(Severity::Error, error);
                    spec_state(name, location, "invalid", None)
                }
            },
            SpecSource::Url(url) => {
                let (metadata_path, body_path) = cache.paths(url);
                let cached_path = Some(metadata_path.display().to_string());
                let body_file = body_path.display().to_string();
                match cache.read(url) {
                    Ok(None) => spec_state(name, url, "not_cached", None),
                    // `kurama api` uses a cached body as it is, fresh or
                    // revalidated by a 304, so one that does not parse fails
                    // it: an error. Metadata it cannot read is fetched again.
                    Ok(Some(cached)) => {
                        match spec_loader::parse_spec(&api, &body_file, &cached.body) {
                            Ok(_) => SpecState {
                                fetched_at: Some(cached.fetched_at.to_rfc3339()),
                                ..spec_state(name, url, "cached", cached_path)
                            },
                            Err(error) => {
                                problem(Severity::Error, error.into());
                                spec_state(name, url, "invalid", Some(body_file))
                            }
                        }
                    }
                    Err(message) => {
                        let error = ApiError::SpecInvalid {
                            api: name.clone(),
                            key: api.spec_key(),
                            location: url.clone(),
                            message,
                        };
                        problem(Severity::Warning, error.into());
                        spec_state(name, url, "invalid", cached_path)
                    }
                }
            }
        };
        states.push(state);
    }
    Ok(states)
}

fn spec_state(api: &str, location: &str, state: &'static str, path: Option<String>) -> SpecState {
    SpecState {
        api: api.to_owned(),
        location: location.to_owned(),
        state,
        path,
        fetched_at: None,
    }
}

fn count(report: &Report, severity: Severity) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == severity)
        .count()
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

/// The report for a person: the files, each description, then each problem
/// with its hint, and the totals.
fn render(report: &Report) -> String {
    let mut out = format!(
        "config: {} ({}{})\naws config: {} ({}{})\n",
        report.config.path,
        report.config.source,
        if report.config.exists { "" } else { ", absent" },
        report.aws_config.path,
        report.aws_config.source,
        if report.aws_config.exists {
            ""
        } else {
            ", absent: aws_profile references are not checked"
        },
    );
    for spec in &report.openapi {
        let state = match spec.state {
            "not_cached" => "not cached; unverified, the URL is not fetched",
            other => other,
        };
        out.push_str(&format!(
            "openapi {}: {} ({state})\n",
            spec.api, spec.location
        ));
    }
    for diagnostic in &report.diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        // A message serde wrote already names its line.
        let place = match diagnostic.line {
            Some(line) if !diagnostic.message.contains(&format!("line {line}:")) => {
                format!(" line {line}")
            }
            _ => String::new(),
        };
        out.push_str(&format!(
            "{severity}[{}]{place}: {}\n",
            diagnostic.code, diagnostic.message
        ));
        if let Some(hint) = &diagnostic.hint {
            out.push_str(&format!("  hint: {hint}\n"));
        }
    }
    let errors = count(report, Severity::Error);
    let warnings = count(report, Severity::Warning);
    out.push_str(&format!(
        "{errors} {}, {warnings} {}\n",
        plural(errors, "error"),
        plural(warnings, "warning")
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(diagnostics: Vec<Diagnostic>) -> Report {
        Report {
            schema_version: SCHEMA_VERSION,
            valid: diagnostics.is_empty(),
            config: ConfigFileState {
                path: "/c/config.toml".into(),
                source: "KURAMA_CONFIG_PATH",
                exists: true,
            },
            aws_config: AwsConfigFileState {
                path: "/h/.aws/config".into(),
                source: "default",
                exists: false,
            },
            verification: Verification {
                network: "not_contacted",
                credentials: "not_resolved",
            },
            openapi: vec![spec_state(
                "pets",
                "https://x/openapi.json",
                "not_cached",
                None,
            )],
            diagnostics,
        }
    }

    #[test]
    fn the_text_report_names_the_files_each_description_and_each_problem() {
        let text = render(&report(vec![
            Diagnostic::of(
                Severity::Error,
                Some("api.a".into()),
                Some(4),
                CoreError::config("[api.a] boom").into(),
            ),
            Diagnostic::of(
                Severity::Error,
                Some("api.b".into()),
                Some(9),
                CoreError::config("config.toml line 9: unknown field `x`").into(),
            ),
        ]));
        assert_eq!(
            text.lines().collect::<Vec<_>>()[..4],
            [
                "config: /c/config.toml (KURAMA_CONFIG_PATH)",
                "aws config: /h/.aws/config (default, absent: aws_profile references are not checked)",
                "openapi pets: https://x/openapi.json (not cached; unverified, the URL is not fetched)",
                "error[CONFIG_INVALID] line 4: Configuration error: [api.a] boom",
            ]
        );
        assert!(
            text.lines().nth(4).unwrap().starts_with("  hint: fix "),
            "{text}"
        );
        assert_eq!(
            text.lines().nth(5).unwrap(),
            "error[CONFIG_INVALID]: Configuration error: config.toml line 9: unknown field `x`",
            "a message that names its line is not prefixed with it again"
        );
        assert!(text.ends_with("2 errors, 0 warnings\n"), "{text}");
    }

    #[test]
    fn a_diagnostic_carries_the_code_the_failing_command_would() {
        let diagnostic = Diagnostic::of(
            Severity::Error,
            None,
            None,
            anyhow::Error::from(CliExecutorError::ProfileNotFound("gone".into()))
                .context("[api.a] aws_profile"),
        );
        assert_eq!(diagnostic.code, "PROFILE_NOT_FOUND");
        assert_eq!(
            diagnostic.message,
            "[api.a] aws_profile: Profile not found: gone"
        );
        assert!(diagnostic.hint.is_some());
    }
}
