//! `kurama config add --file FILE|- [--dry-run] [--json]`: new sections appended to config.toml through the config writer, checked whole before anything is written.
//!
//! The input is TOML with its own headers (`[auth.x]`, `[api.x]`, ...), and
//! may hold sections that name each other. A section the file already has
//! refuses the whole input: nothing is merged and no `[auth.*]` is reused.
//! The file's bytes stay as they are and the input goes after them. A
//! `--dry-run` prints the lines it would append and writes nothing. No secret
//! is resolved and nothing is contacted: a saved section says nothing about
//! whether the provider accepts it.

use anyhow::Result;
use clap::{Arg, ArgAction, Command, ValueHint};
use serde::Serialize;

use crate::adapters::config::edit::Change;
use crate::adapters::config::input::InputError;
use crate::adapters::config::writer::ConfigFile;
use crate::console::progress;
use crate::shell::cli::client::{RequestReadFailure, read_request_text};

pub fn command() -> Command {
    Command::new("add")
        .about("Append new sections to config.toml, checked whole before anything is written")
        .long_about(
            "Append new sections to config.toml. The input is TOML with its own section\n\
             headers ([auth.<name>], [api.<name>], [db.<name>], ...) and may add several\n\
             sections that name each other. A section the file already has refuses the\n\
             whole input. client_secret, token and password take a secret reference\n\
             (op://, aws-secrets://, aws-ssm://), never the value. The file plus the input\n\
             is checked as `kurama config check` would, then written in one step; the\n\
             file's own bytes, comments and order are kept. No secret is resolved and\n\
             nothing is contacted.",
        )
        .arg(
            Arg::new("file")
                .long("file")
                .value_name("FILE|-")
                .value_hint(ValueHint::FilePath)
                .required(true)
                .help("The TOML to add; - reads stdin"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Check and print the lines that would be appended; write nothing"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print one JSON document"),
        )
}

/// The `--json` document of a command that saves config.toml.
#[derive(Debug, Serialize)]
pub struct SaveReport {
    pub path: String,
    pub changed: bool,
    /// False for `--dry-run`.
    pub applied: bool,
    pub changes: Vec<Change>,
    pub warnings: Vec<String>,
}

/// The TOML a `--file FILE|-` names.
pub fn read_input(input: &str) -> Result<String, InputError> {
    read_request_text(input).map_err(|failure| InputError::Unreadable {
        input: input.to_owned(),
        source: match failure {
            RequestReadFailure::Stdin(error) | RequestReadFailure::File(error) => error,
        },
    })
}

pub async fn run(input: &str, dry_run: bool, json: bool) -> Result<()> {
    let fragment = read_input(input)?;
    let file = ConfigFile::open()?;
    let appended = file.append(&fragment)?;
    let validated = file.validate(&appended).await?;
    let warnings = validated.warnings.clone();
    if !dry_run {
        file.save(validated)?;
    }
    let report = SaveReport {
        path: file.path().display().to_string(),
        changed: true,
        applied: !dry_run,
        changes: appended
            .units
            .iter()
            .map(|section| Change::of(section.clone(), "add"))
            .collect(),
        warnings,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if dry_run {
        for line in appended.added.lines() {
            println!("+{line}");
        }
    }
    for warning in &report.warnings {
        progress!("# warning: {warning}");
    }
    let sections: Vec<String> = appended
        .units
        .iter()
        .map(|unit| format!("[{unit}]"))
        .collect();
    if dry_run {
        progress!(
            "# dry run: {} would be added to {}; nothing was written",
            sections.join(", "),
            report.path
        );
    } else {
        progress!("# added {} to {}", sections.join(", "), report.path);
    }
    Ok(())
}
