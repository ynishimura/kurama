//! `kurama config set|unset|remove [--dry-run] [--json]`: keys and sections of config.toml changed or removed in place through the config writer, the whole result checked once before anything is written.
//!
//! `set PATH VALUE` writes one key inside a section the file has (a fixed
//! table it omits is created); `set --file FILE|-` replaces whole sections;
//! `unset KEY...` removes keys, so their defaults apply; `remove SECTION...`
//! removes sections, and refuses an `[auth.*]` an API it keeps still uses.
//! Only what is named changes: every other value, comment and the order stay
//! as saved. The edits of one command are checked together, once, so an API
//! and its auth can be removed in one run. A `--dry-run` prints the lines
//! that would change, literal secrets redacted, and writes nothing. No
//! secret is resolved and nothing is contacted.

use anyhow::Result;
use clap::{Arg, ArgAction, Command, ValueHint};
use clap_complete::engine::ArgValueCandidates;

use super::config_add::{SaveReport, read_input};
use crate::adapters::config::edit::Change;
use crate::adapters::config::writer::ConfigFile;
use crate::console::progress;
use crate::shell::cli::completion;

/// What one `config set|unset|remove` asks to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditRequest {
    Set {
        path: String,
        value: String,
    },
    /// A file, or `-` for stdin.
    SetFile {
        file: String,
    },
    Unset {
        keys: Vec<String>,
    },
    Remove {
        sections: Vec<String>,
    },
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .action(ArgAction::SetTrue)
        .help("Check and print the lines that would change, secrets redacted; write nothing")
}

fn json_arg() -> Arg {
    Arg::new("json")
        .long("json")
        .action(ArgAction::SetTrue)
        .help("Print one JSON document")
}

pub fn set_command() -> Command {
    Command::new("set")
        .about("Set one key inside a section of config.toml, or replace whole sections with --file")
        .long_about(
            "Set one key inside a section of config.toml, or replace whole sections.\n\
             PATH is a dotted key inside a section the file has (api.github.base_url,\n\
             aws.session_cache.duration); a fixed table the file omits (core, aws,\n\
             onepassword, openapi) is created, a new [auth.*] / [api.*] / ... is added\n\
             with `kurama config add`. VALUE is TOML: '\"debug\"', 3600, '[\"a\", \"b\"]';\n\
             an array is replaced whole. --file takes TOML with its own section headers\n\
             and replaces each of those sections whole: keys it leaves out are removed,\n\
             nothing is merged. client_secret, token and password take a secret\n\
             reference only. The result is checked as `kurama config check` would before\n\
             it is written; only what is named changes.",
        )
        .arg(
            Arg::new("path")
                .value_name("PATH")
                .required_unless_present("file")
                .help("A dotted key inside a section: core.log_level, api.github.base_url")
                .add(ArgValueCandidates::new(completion::config_keys)),
        )
        .arg(
            Arg::new("value")
                .value_name("VALUE")
                .required_unless_present("file")
                .allow_hyphen_values(true)
                .value_hint(ValueHint::Other)
                .help("A TOML value: '\"debug\"', 3600, true, '[\"a\", \"b\"]'"),
        )
        .arg(
            Arg::new("file")
                .long("file")
                .value_name("FILE|-")
                .value_hint(ValueHint::FilePath)
                .conflicts_with_all(["path", "value"])
                .help("TOML sections that replace the file's sections of the same names; - reads stdin"),
        )
        .arg(dry_run_arg())
        .arg(json_arg())
}

pub fn unset_command() -> Command {
    Command::new("unset")
        .about("Remove keys from config.toml, so their defaults apply")
        .arg(
            Arg::new("keys")
                .value_name("KEY")
                .required(true)
                .num_args(1..)
                .help("Dotted keys inside sections: core.log_level, api.github.description")
                .add(ArgValueCandidates::new(completion::config_keys)),
        )
        .arg(dry_run_arg())
        .arg(json_arg())
}

pub fn remove_command() -> Command {
    Command::new("remove")
        .about("Remove whole sections from config.toml; an [auth.*] an API still uses is refused")
        .arg(
            Arg::new("sections")
                .value_name("SECTION")
                .required(true)
                .num_args(1..)
                .help("Sections: api.github, auth.github, openapi")
                .add(ArgValueCandidates::new(completion::config_sections)),
        )
        .arg(dry_run_arg())
        .arg(json_arg())
}

pub async fn run(request: &EditRequest, dry_run: bool, json: bool) -> Result<()> {
    let file = ConfigFile::open()?;
    let mut edit = file.edit()?;
    match request {
        EditRequest::Set { path, value } => edit.set(path, value)?,
        EditRequest::SetFile { file } => edit.replace(&read_input(file)?)?,
        EditRequest::Unset { keys } => edit.unset(keys)?,
        EditRequest::Remove { sections } => edit.remove(sections)?,
    }
    let validated = file.validate_edit(&edit).await?;
    let warnings = validated.warnings.clone();
    let changed = edit.changed();
    if changed && !dry_run {
        file.save(validated)?;
    }
    // An edit that changes nothing is not applied and lists no change.
    let report = SaveReport {
        path: file.path().display().to_string(),
        changed,
        applied: changed && !dry_run,
        changes: if changed {
            edit.changes.clone()
        } else {
            Vec::new()
        },
        warnings,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if dry_run {
        for line in edit.diff() {
            println!("{line}");
        }
    }
    for warning in &report.warnings {
        progress!("# warning: {warning}");
    }
    let done: Vec<String> = report.changes.iter().map(describe).collect();
    let done = done.join(", ");
    if dry_run {
        progress!("# dry run: {done} in {}; nothing was written", report.path);
    } else if !changed {
        progress!("# {} already holds that; nothing was written", report.path);
    } else {
        progress!("# {done} in {}", report.path);
    }
    Ok(())
}

/// One change as the summary line names it.
fn describe(change: &Change) -> String {
    match (&change.key, change.action) {
        (Some(key), action) => format!("{action} {key}"),
        (None, "replace") => format!("replaced [{}] whole", change.section),
        (None, "add") => format!("added [{}]", change.section),
        (None, _) => format!("removed [{}]", change.section),
    }
}
