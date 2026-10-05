//! `kurama obsidian search|read|files`: the `[obsidian]` vault read through the official Obsidian CLI, inside `allow_paths` only, and never written.
//!
//! A path is checked before the CLI runs, and every answer is filtered to the
//! allowed folders again after it, so what the CLI returns for a folder it was
//! not asked about never reaches stdout. With `--json` stdout is one
//! document: `{"matches": [...]}`, `{"path", "content", "truncated"}` or
//! `{"files": [...]}`.

use std::io::Write;
use std::time::Duration;

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches, Command, ValueHint};
use serde_json::json;

use crate::adapters::config::Config;
use crate::adapters::config::obsidian::ObsidianConfig;
use crate::adapters::error::CoreError;
use crate::adapters::obsidian_cli::{ObsidianCliError, run_obsidian_cli};
use crate::domain::functions::obsidian::{
    Match, allowed_note, cut_note, files_argv, folders_to_read, listed_files, read_argv,
    search_argv, search_matches,
};

/// The files a search returns when `--limit` is not given.
const DEFAULT_LIMIT: usize = 20;

/// What `kurama obsidian` was asked for.
#[derive(Debug, Clone)]
pub struct ObsidianCommand {
    pub action: ObsidianAction,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub enum ObsidianAction {
    Search {
        query: String,
        folder: Option<String>,
        limit: usize,
    },
    Read {
        path: String,
    },
    Files {
        folder: Option<String>,
    },
}

impl ObsidianCommand {
    pub fn parse(matches: &ArgMatches) -> Self {
        let (name, sub) = matches
            .subcommand()
            .expect("clap requires an obsidian subcommand");
        let text = |id: &str| sub.get_one::<String>(id).cloned();
        let action = match name {
            "search" => ObsidianAction::Search {
                query: text("query").expect("clap requires QUERY"),
                folder: text("path"),
                limit: sub
                    .get_one::<u64>("limit")
                    .map_or(DEFAULT_LIMIT, |limit| *limit as usize),
            },
            "read" => ObsidianAction::Read {
                path: text("path").expect("clap requires PATH"),
            },
            _ => ObsidianAction::Files {
                folder: text("folder"),
            },
        };
        Self {
            action,
            json: sub.get_flag("json"),
        }
    }

    /// What the audit entry names: the note read, the folder searched or
    /// listed, or `*` for every allowed folder. Never the query.
    pub fn audit_target(&self) -> &str {
        match &self.action {
            ObsidianAction::Read { path } => path,
            ObsidianAction::Search { folder, .. } | ObsidianAction::Files { folder } => {
                folder.as_deref().unwrap_or("*")
            }
        }
    }
}

fn json_arg() -> Arg {
    Arg::new("json")
        .long("json")
        .action(ArgAction::SetTrue)
        .help("Print one JSON document instead of text")
}

pub fn command() -> Command {
    Command::new("obsidian")
        .about("Search and read the [obsidian] vault through the Obsidian CLI, inside allow_paths only")
        .long_about(
            "Search and read the [obsidian] vault through the official Obsidian CLI\n\
             (Obsidian 1.12.7+, Settings > General > Command line interface).\n\n\
             Only the folders [obsidian] allow_paths names are searched, listed and read: a\n\
             path that is absolute, holds `..` or lies outside them is refused before the\n\
             CLI runs, and an answer is filtered to them again. Nothing is ever written,\n\
             and no CLI command other than search:context, read and files is run.",
        )
        .subcommand_required(true)
        .subcommand(
            Command::new("search")
                .about("Search the allowed folders; each matching line with its note and line number")
                .arg(
                    Arg::new("query")
                        .value_name("QUERY")
                        .required(true)
                        .value_hint(ValueHint::Other)
                        .help("Text to search for"),
                )
                .arg(
                    Arg::new("path")
                        .long("path")
                        .value_name("FOLDER")
                        .value_hint(ValueHint::Other)
                        .help("Search this folder only, an allowed one or inside one"),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_hint(ValueHint::Other)
                        .value_parser(clap::value_parser!(u64).range(1..))
                        .help("Notes per folder searched (default 20)"),
                )
                .arg(json_arg()),
        )
        .subcommand(
            Command::new("read")
                .about("Print one note, cut at [obsidian] max_read_bytes")
                .arg(
                    Arg::new("path")
                        .value_name("PATH")
                        .required(true)
                        .value_hint(ValueHint::Other)
                        .help("The note's path from the vault root, such as Wiki/kurama.md"),
                )
                .arg(json_arg()),
        )
        .subcommand(
            Command::new("files")
                .about("List the notes of one allowed folder, or of every one")
                .arg(
                    Arg::new("folder")
                        .value_name("FOLDER")
                        .value_hint(ValueHint::Other)
                        .help("An allowed folder or one inside it; every allowed folder when absent"),
                )
                .arg(json_arg()),
        )
}

pub async fn run(command: ObsidianCommand, config: &Config) -> Result<()> {
    let obsidian = config.obsidian.as_ref().ok_or_else(|| {
        CoreError::config("[obsidian] is not configured: add vault and allow_paths")
    })?;
    let allowed = &obsidian.allow_paths;
    match command.action {
        ObsidianAction::Search {
            query,
            folder,
            limit,
        } => {
            let mut matches: Vec<Match> = Vec::new();
            for folder in folders_to_read(folder.as_deref(), allowed)? {
                let answer = call(
                    obsidian,
                    search_argv(&obsidian.vault, &query, &folder, limit),
                    None,
                )
                .await?;
                matches.extend(
                    search_matches(&answer, allowed)
                        .map_err(|source| ObsidianCliError::NotJson { source })?,
                );
            }
            if command.json {
                println!("{}", json!({ "matches": matches }));
            } else {
                for found in &matches {
                    println!("{}:{}: {}", found.path, found.line, found.text);
                }
            }
        }
        ObsidianAction::Read { path } => {
            let path = allowed_note(&path, allowed)?;
            let answer = call(obsidian, read_argv(&obsidian.vault, &path), Some(&path)).await?;
            let (content, truncated) = cut_note(&answer, obsidian.max_read_bytes);
            if command.json {
                println!(
                    "{}",
                    json!({ "path": path, "content": content, "truncated": truncated })
                );
            } else {
                print!("{content}");
                if truncated {
                    // The cut ends mid-line: flushed first, and the note
                    // starts a line of its own, so it follows the text.
                    std::io::stdout().flush()?;
                    let newline = if content.ends_with('\n') { "" } else { "\n" };
                    crate::console::write_line(&format!(
                        "{newline}# cut at {} bytes ([obsidian] max_read_bytes)",
                        obsidian.max_read_bytes
                    ));
                }
            }
        }
        ObsidianAction::Files { folder } => {
            let mut files = Vec::new();
            for folder in folders_to_read(folder.as_deref(), allowed)? {
                let answer = call(obsidian, files_argv(&obsidian.vault, &folder), None).await?;
                files.extend(listed_files(&answer, allowed));
            }
            if command.json {
                println!("{}", json!({ "files": files }));
            } else {
                for file in &files {
                    println!("{file}");
                }
            }
        }
    }
    Ok(())
}

async fn call(obsidian: &ObsidianConfig, argv: Vec<String>, read: Option<&str>) -> Result<String> {
    Ok(run_obsidian_cli(
        &obsidian.cli_path,
        &argv,
        read,
        Duration::from_secs(obsidian.timeout),
    )
    .await?)
}
