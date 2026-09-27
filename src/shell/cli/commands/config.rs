//! `kurama config <check|path|list|show|add|set|unset|remove>`: the group, what each subcommand was asked, and which file runs it.
//!
//! Every subcommand reads config.toml itself instead of through bootstrap:
//! `check`, `list` and `show` have to work on a file that does not load, and
//! `path` on one that does not parse.

use anyhow::Result;
use clap::{ArgMatches, Command};

use super::config_edit::EditRequest;

/// One `kurama config` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigCommand {
    Check {
        json: bool,
    },
    Path {
        json: bool,
    },
    List {
        section: Option<String>,
        json: bool,
    },
    Show {
        path: Option<String>,
        json: bool,
    },
    Add {
        /// A file, or `-` for stdin.
        file: String,
        dry_run: bool,
        json: bool,
    },
    /// `set`, `unset` or `remove`.
    Edit {
        request: EditRequest,
        dry_run: bool,
        json: bool,
    },
}

pub fn command() -> Command {
    Command::new("config")
        .about(
            "Show, add to, change and check config.toml without a secret store, a keychain or the network",
        )
        .subcommand_required(true)
        .subcommand(super::config_check::command())
        .subcommand(super::config_show::path_command())
        .subcommand(super::config_show::list_command())
        .subcommand(super::config_show::show_command())
        .subcommand(super::config_add::command())
        .subcommand(super::config_edit::set_command())
        .subcommand(super::config_edit::unset_command())
        .subcommand(super::config_edit::remove_command())
}

impl ConfigCommand {
    pub fn parse(config: &ArgMatches) -> Self {
        let (name, matches) = config
            .subcommand()
            .expect("clap requires a config subcommand");
        let json = matches.get_flag("json");
        let text = |id: &str| matches.get_one::<String>(id).cloned();
        let texts = |id: &str| -> Vec<String> {
            matches
                .get_many::<String>(id)
                .map(|values| values.cloned().collect())
                .unwrap_or_default()
        };
        let edit = |request| Self::Edit {
            request,
            dry_run: matches.get_flag("dry-run"),
            json,
        };
        match name {
            "check" => Self::Check { json },
            "path" => Self::Path { json },
            "list" => Self::List {
                section: text("section"),
                json,
            },
            "show" => Self::Show {
                path: text("path"),
                json,
            },
            "add" => Self::Add {
                file: text("file").expect("clap requires --file"),
                dry_run: matches.get_flag("dry-run"),
                json,
            },
            "set" => edit(match text("file") {
                Some(file) => EditRequest::SetFile { file },
                None => EditRequest::Set {
                    path: text("path").expect("clap requires PATH without --file"),
                    value: text("value").expect("clap requires VALUE without --file"),
                },
            }),
            "unset" => edit(EditRequest::Unset {
                keys: texts("keys"),
            }),
            "remove" => edit(EditRequest::Remove {
                sections: texts("sections"),
            }),
            other => unreachable!("clap has no config subcommand {other}"),
        }
    }

    /// Whether stdout carries one JSON document.
    pub fn json(&self) -> bool {
        match self {
            Self::Check { json }
            | Self::Path { json }
            | Self::List { json, .. }
            | Self::Show { json, .. }
            | Self::Add { json, .. }
            | Self::Edit { json, .. } => *json,
        }
    }

    pub async fn run(self) -> Result<()> {
        match self {
            Self::Check { json } => super::config_check::run(json).await,
            Self::Path { json } => super::config_show::run_path(json),
            Self::List { section, json } => {
                super::config_show::run_list(section.as_deref(), json).await
            }
            Self::Show { path, json } => super::config_show::run_show(path.as_deref(), json).await,
            Self::Add {
                file,
                dry_run,
                json,
            } => super::config_add::run(&file, dry_run, json).await,
            Self::Edit {
                request,
                dry_run,
                json,
            } => super::config_edit::run(&request, dry_run, json).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ConfigCommand {
        let matches = crate::build_command()
            .try_get_matches_from(std::iter::once("kurama").chain(args.iter().copied()))
            .unwrap();
        let (_, config) = matches.subcommand().unwrap();
        ConfigCommand::parse(config)
    }

    #[test]
    fn each_subcommand_parses_to_what_it_was_asked() {
        assert_eq!(
            parse(&["config", "check", "--json"]),
            ConfigCommand::Check { json: true }
        );
        assert_eq!(
            parse(&["config", "path"]),
            ConfigCommand::Path { json: false }
        );
        assert_eq!(
            parse(&["config", "list", "auth", "--json"]),
            ConfigCommand::List {
                section: Some("auth".into()),
                json: true
            }
        );
        assert_eq!(
            parse(&["config", "show"]),
            ConfigCommand::Show {
                path: None,
                json: false
            }
        );
        assert_eq!(
            parse(&["config", "add", "--file", "-", "--dry-run"]),
            ConfigCommand::Add {
                file: "-".into(),
                dry_run: true,
                json: false
            }
        );
        assert_eq!(
            parse(&[
                "config",
                "set",
                "aws.session_cache.duration",
                "-1",
                "--json"
            ]),
            ConfigCommand::Edit {
                request: EditRequest::Set {
                    path: "aws.session_cache.duration".into(),
                    value: "-1".into()
                },
                dry_run: false,
                json: true
            }
        );
        assert_eq!(
            parse(&["config", "set", "--file", "-", "--dry-run"]),
            ConfigCommand::Edit {
                request: EditRequest::SetFile { file: "-".into() },
                dry_run: true,
                json: false
            }
        );
        assert_eq!(
            parse(&["config", "unset", "core.log_level", "api.x.description"]),
            ConfigCommand::Edit {
                request: EditRequest::Unset {
                    keys: vec!["core.log_level".into(), "api.x.description".into()]
                },
                dry_run: false,
                json: false
            }
        );
        assert_eq!(
            parse(&["config", "remove", "api.x", "auth.x", "--dry-run", "--json"]),
            ConfigCommand::Edit {
                request: EditRequest::Remove {
                    sections: vec!["api.x".into(), "auth.x".into()]
                },
                dry_run: true,
                json: true
            }
        );
        let refused = |args: &[&str]| {
            crate::build_command()
                .try_get_matches_from(std::iter::once("kurama").chain(args.iter().copied()))
                .is_err()
        };
        assert!(refused(&["config", "set", "core.log_level"]));
        assert!(refused(&[
            "config",
            "set",
            "core.log_level",
            "1",
            "--file",
            "-"
        ]));
        assert!(refused(&["config", "set"]));
        assert!(refused(&["config", "unset"]));
        assert!(refused(&["config", "remove"]));
    }
}
