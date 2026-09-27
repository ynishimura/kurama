//! `kurama inventory` (hidden): what this binary has, as one JSON document read off the clap definition and the config types.
//!
//! The commands and their options are walked from [`build_command`]; the
//! config keys, kinds and enum values come from `adapters::config::inventory`,
//! which probes the types; the secret schemes are the table
//! `SecretRef::parse` dispatches through; the client kinds are
//! `ClientKind::ALL`. The one thing written by hand is `source_of`, the file
//! each command is defined in.

use clap::Command;
use serde::Serialize;
use serde_json::{Value, json};

use crate::adapters::config::inventory as config_inventory;
use crate::domain::types::SecretRef;
use crate::shell::cli::args::build_command;
use crate::shell::cli::arguments::{ArgumentFacts, visible_arguments};
use crate::shell::cli::client::ClientKind;

const SCHEMES_SOURCE: &str = "src/domain/types/secret_ref.rs: SCHEMES";
const CLIENT_KINDS_SOURCE: &str = "src/shell/cli/client.rs: ClientKind::ALL";
const CLAP_SOURCE: &str = "src/shell/cli/args.rs: build_command";

/// One command, with the subcommands it has under `commands`.
#[derive(Debug, Serialize)]
struct CommandItem {
    /// `api`, or `data describe` for a nested one.
    path: String,
    hidden: bool,
    about: String,
    source: String,
    arguments: Vec<ArgumentItem>,
}

#[derive(Debug, Serialize)]
struct ArgumentItem {
    id: String,
    positional: bool,
    long: Option<String>,
    short: Option<char>,
    value_name: Option<String>,
    required: bool,
    takes_value: bool,
    /// What `value_parser([..])` enumerates; empty otherwise.
    values: Vec<String>,
    /// clap's `ValueHint`, as its variant name.
    value_hint: String,
    /// Whether a completer generates candidates for it.
    completer: bool,
}

/// The whole inventory.
pub fn document() -> Value {
    let root = build_command();
    let mut commands = Vec::new();
    for subcommand in root.get_subcommands() {
        collect(subcommand, "", &mut commands);
    }
    json!({
        "schema_version": 1,
        "binary": {
            "name": root.get_name(),
            "version": root.get_version(),
        },
        "commands": commands,
        "config": {
            "source": config_inventory::SOURCE,
            "keys": config_inventory::config_keys(),
        },
        "secret_schemes": {
            "source": SCHEMES_SOURCE,
            "values": SecretRef::schemes().collect::<Vec<_>>(),
        },
        "client_kinds": {
            "source": CLIENT_KINDS_SOURCE,
            "values": ClientKind::values(),
        },
    })
}

fn collect(command: &Command, parent: &str, out: &mut Vec<CommandItem>) {
    let path = if parent.is_empty() {
        command.get_name().to_owned()
    } else {
        format!("{parent} {}", command.get_name())
    };
    out.push(CommandItem {
        path: path.clone(),
        hidden: command.is_hide_set(),
        about: command
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default(),
        source: source_of(&path),
        arguments: visible_arguments(command).map(argument).collect(),
    });
    for subcommand in command.get_subcommands() {
        collect(subcommand, &path, out);
    }
}

/// The file that defines a command: the commands built next to their own
/// handlers name that file and function, everything else is in the clap
/// definition.
fn source_of(path: &str) -> String {
    let file = match path {
        "config check" => "config_check.rs: command",
        "config path" => "config_show.rs: path_command",
        "config list" => "config_show.rs: list_command",
        "config show" => "config_show.rs: show_command",
        "config add" => "config_add.rs: command",
        "config set" => "config_edit.rs: set_command",
        "config unset" => "config_edit.rs: unset_command",
        "config remove" => "config_edit.rs: remove_command",
        _ => match path.split(' ').next().unwrap_or_default() {
            "api" => "api_command.rs: command",
            "data" => "data_command.rs: command",
            "db" => "db_command.rs: command",
            "config" => "config.rs: command",
            "preset" => "preset.rs: command",
            _ => return CLAP_SOURCE.into(),
        },
    };
    format!("src/shell/cli/commands/{file}")
}

fn argument(arg: &clap::Arg) -> ArgumentItem {
    let facts = ArgumentFacts::of(arg);
    ArgumentItem {
        id: facts.id.to_owned(),
        positional: facts.positional,
        long: facts.long.map(str::to_owned),
        short: facts.short,
        value_name: facts.value_name,
        required: facts.required,
        takes_value: facts.takes_value,
        values: facts.values,
        value_hint: facts.value_hint,
        completer: facts.completer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(document: &Value) -> Vec<&Value> {
        document["commands"].as_array().unwrap().iter().collect()
    }

    fn command<'a>(document: &'a Value, path: &str) -> &'a Value {
        commands(document)
            .into_iter()
            .find(|command| command["path"] == path)
            .unwrap_or_else(|| panic!("{path} is not in the inventory"))
    }

    fn argument<'a>(command: &'a Value, id: &str) -> &'a Value {
        command["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|arg| arg["id"] == id)
            .unwrap_or_else(|| panic!("{id} is not an argument of {}", command["path"]))
    }

    #[test]
    fn every_command_of_the_binary_is_listed_with_its_arguments() {
        let document = document();
        let api = command(&document, "api");
        let target = argument(api, "api");
        assert_eq!(target["positional"], true);
        assert_eq!(target["required"], true);
        assert_eq!(target["completer"], true);
        assert_eq!(
            api["source"],
            "src/shell/cli/commands/api_command.rs: command"
        );
        assert_eq!(api["hidden"], false);

        let agent = command(&document, "agent");
        let kind = argument(agent, "kind");
        assert_eq!(kind["long"], "kind");
        assert_eq!(kind["values"], json!(["data", "db", "s3"]));
        assert_eq!(kind["takes_value"], true);
        let skill = argument(agent, "skill");
        assert_eq!(skill["takes_value"], false);

        let init = command(&document, "init");
        assert_eq!(argument(init, "shell")["values"], json!(["zsh"]));

        let db = command(&document, "db");
        assert!(db["source"].as_str().unwrap().contains("db_command.rs"));

        // The binary's own subcommands and the inventory's list are the same
        // set, so a command added to clap is here without anyone listing it.
        let mut listed: Vec<String> = commands(&document)
            .iter()
            .filter(|command| !command["path"].as_str().unwrap().contains(' '))
            .map(|command| command["path"].as_str().unwrap().to_owned())
            .collect();
        listed.sort();
        let mut known: Vec<String> = build_command()
            .get_subcommands()
            .map(|command| command.get_name().to_owned())
            .collect();
        known.sort();
        assert_eq!(listed, known);
    }

    #[test]
    fn config_and_preset_commands_name_the_file_that_defines_them() {
        let document = document();
        let source = |path: &str| command(&document, path)["source"].clone();
        assert_eq!(source("env"), CLAP_SOURCE);
        assert_eq!(
            source("data"),
            "src/shell/cli/commands/data_command.rs: command"
        );
        assert_eq!(
            source("config"),
            "src/shell/cli/commands/config.rs: command"
        );
        assert_eq!(
            source("config check"),
            "src/shell/cli/commands/config_check.rs: command"
        );
        assert_eq!(
            source("config path"),
            "src/shell/cli/commands/config_show.rs: path_command"
        );
        assert_eq!(
            source("config list"),
            "src/shell/cli/commands/config_show.rs: list_command"
        );
        assert_eq!(
            source("config show"),
            "src/shell/cli/commands/config_show.rs: show_command"
        );
        assert_eq!(
            source("config add"),
            "src/shell/cli/commands/config_add.rs: command"
        );
        assert_eq!(
            source("config set"),
            "src/shell/cli/commands/config_edit.rs: set_command"
        );
        assert_eq!(
            source("config unset"),
            "src/shell/cli/commands/config_edit.rs: unset_command"
        );
        assert_eq!(
            source("config remove"),
            "src/shell/cli/commands/config_edit.rs: remove_command"
        );
        for path in ["preset", "preset show", "preset add"] {
            assert_eq!(source(path), "src/shell/cli/commands/preset.rs: command");
        }
    }

    #[test]
    fn the_inventory_lists_itself_as_hidden() {
        let document = document();
        assert_eq!(command(&document, "inventory")["hidden"], true);
    }

    #[test]
    fn config_keys_enum_values_and_client_kinds_are_listed() {
        let document = document();
        let keys = document["config"]["keys"].as_array().unwrap();
        let find = |section: &str, key: &str| {
            keys.iter()
                .find(|entry| entry["section"] == section && entry["key"] == key)
                .unwrap_or_else(|| panic!("{section} {key} is not in the inventory"))
        };
        assert_eq!(find("[api.*]", "base_url")["kind"], "string");
        assert_eq!(
            find("[auth.*]", "kind")["values"],
            json!(["oauth", "token"])
        );
        assert_eq!(find("[auth.*]", "token")["kind"], "secret");
        assert_eq!(document["config"]["source"], config_inventory::SOURCE);
        assert_eq!(
            document["client_kinds"]["values"],
            json!(["data", "db", "s3"])
        );
        assert_eq!(
            document["secret_schemes"]["values"],
            json!(["op", "aws-secrets", "aws-ssm"])
        );
    }
}
