//! `kurama agent --json`: the catalog of every subcommand an agent can run, read off `build_command()`, with the exit codes from `ErrorCode` and the JSON error contract from `JsonErrorKind`.
//!
//! Nothing here lists a command, an argument or a code by hand: a subcommand
//! or an argument added to the clap definition is in the catalog without
//! anyone touching this file. The walk follows the definition's order, so the
//! same binary prints the same bytes every time. Every string comes from the
//! definition, and each goes through `safe_text` all the same, so a control
//! character in a help line cannot reach the reader as one.

use clap::{Arg, Command};
use serde_json::{Map, Value, json};

use crate::shell::cli::args::build_command;
use crate::shell::cli::arguments::{ArgumentFacts, visible_arguments};
use crate::shell::cli::client::{ClientKind, JsonErrorKind, safe_text};
use crate::shell::cli::error_code::ErrorCode;
use strum::VariantArray;

/// The version of the catalog's shape; a field removed or renamed bumps it.
const SCHEMA_VERSION: u64 = 1;

/// The whole catalog, as one JSON document.
pub fn catalog() -> Value {
    let root = build_command();
    json!({
        "schema_version": SCHEMA_VERSION,
        "binary": {
            "name": root.get_name(),
            "version": root.get_version(),
        },
        "commands": subcommands(&root, true),
        "exit_codes": exit_codes(),
    })
}

/// The visible subcommands of `command`, keyed by name in definition order.
/// `top` marks the level where a subcommand name is what `JsonErrorKind` and
/// `ClientKind` parse.
fn subcommands(command: &Command, top: bool) -> Value {
    let mut out = Map::new();
    for subcommand in command.get_subcommands().filter(|c| !c.is_hide_set()) {
        let name = subcommand.get_name();
        let mut entry = Map::new();
        entry.insert("about".into(), text(subcommand.get_about()));
        if top {
            // The subcommands whose failures are one JSON error document on
            // stderr when run with `--json` or `--jq`.
            entry.insert(
                "json_errors".into(),
                JsonErrorKind::parse(name).is_some().into(),
            );
            // A usage failure kurama reports itself, with its code, and
            // whether it does so without `--json` too; null when clap's text
            // (no code) is all a usage failure prints.
            entry.insert(
                "usage_error".into(),
                JsonErrorKind::parse(name)
                    .map(|kind| {
                        json!({
                            "code": kind.usage_code().as_str(),
                            "without_json": kind.reports_text_usage(),
                        })
                    })
                    .into(),
            );
            // The ones `kurama agent --kind <name> --json` describes.
            entry.insert(
                "request_contract".into(),
                ClientKind::parse(name).is_some().into(),
            );
        }
        entry.insert(
            "arguments".into(),
            Value::Array(
                visible_arguments(subcommand)
                    .map(|arg| argument(subcommand, arg))
                    .collect(),
            ),
        );
        entry.insert("groups".into(), groups(subcommand));
        entry.insert("subcommands".into(), subcommands(subcommand, false));
        out.insert(name.to_owned(), Value::Object(entry));
    }
    Value::Object(out)
}

fn argument(command: &Command, arg: &Arg) -> Value {
    let facts = ArgumentFacts::of(arg);
    json!({
        "id": facts.id,
        "positional": facts.positional,
        "long": facts.long.map(|long| format!("--{long}")),
        "short": facts.short.map(|short| format!("-{short}")),
        "value_name": facts.value_name.as_deref().map(safe_text),
        "takes_value": facts.takes_value,
        "value_optional": facts.value_optional,
        "required": facts.required,
        "multiple": facts.multiple,
        "values": facts.values.iter().map(|value| safe_text(value)).collect::<Vec<_>>(),
        "default": facts.default.iter().map(|value| safe_text(value)).collect::<Vec<_>>(),
        "value_hint": facts.value_hint,
        "completer": facts.completer,
        "help": text(arg.get_help()),
        "conflicts_with": conflicts(command, arg),
        "requires": declared_ids(command, arg, "requires"),
        "required_unless_present": declared_ids(command, arg, "r_unless"),
    })
}

/// The ids `arg` cannot be given with, whichever side declared it: clap
/// reports only the side that did.
fn conflicts(command: &Command, arg: &Arg) -> Vec<String> {
    visible_arguments(command)
        .filter(|other| other.get_id() != arg.get_id())
        .filter(|other| {
            command
                .get_arg_conflicts_with(arg)
                .iter()
                .any(|c| c.get_id() == other.get_id())
                || command
                    .get_arg_conflicts_with(other)
                    .iter()
                    .any(|c| c.get_id() == arg.get_id())
        })
        .map(|other| other.get_id().to_string())
        .collect()
}

/// The ids `arg` names in `field`, kept only when they are an argument or a
/// group of `command`, so a misread can drop an id but never invent one.
fn declared_ids(command: &Command, arg: &Arg, field: &str) -> Vec<String> {
    debug_ids(arg, field)
        .into_iter()
        .filter(|id| {
            command.get_arguments().any(|other| other.get_id() == id)
                || command.get_groups().any(|group| group.get_id() == id)
        })
        .collect()
}

/// The ids an argument names in one of its builder fields that clap 4.6 has
/// no getter for (`requires`, `r_unless`). clap's `Debug` of an `Arg` prints
/// every field as `name: [...]` with an id as a quoted string, the help text
/// before these fields; the last match is taken, so a help line that happens
/// to contain `requires: [` cannot shadow the field. The unit tests pin what
/// it reads, and `every_declared_id_resolves` walks the whole tree, so a
/// change in that format fails there instead of emptying the catalog.
fn debug_ids(arg: &Arg, field: &str) -> Vec<String> {
    let debug = format!("{arg:?}");
    let marker = format!(", {field}: [");
    let Some(start) = debug.rfind(&marker) else {
        return Vec::new();
    };
    let rest = &debug[start + marker.len()..];
    let list = &rest[..rest.find(']').unwrap_or(rest.len())];
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The argument groups: without `multiple`, at most one of `args` is given.
fn groups(command: &Command) -> Value {
    Value::Array(
        command
            .get_groups()
            .map(|group| {
                json!({
                    "id": group.get_id().as_str(),
                    "args": group.get_args().map(|id| id.as_str()).collect::<Vec<_>>(),
                    "multiple": group.clone().is_multiple(),
                    "required": group.is_required_set(),
                })
            })
            .collect(),
    )
}

/// Every exit code with what it means and the `error[CODE]`s that end with
/// it, in declaration order.
fn exit_codes() -> Value {
    Value::Array(
        ErrorCode::EXITS
            .iter()
            .map(|(exit, meaning)| {
                json!({
                    "exit": exit,
                    "meaning": meaning,
                    "error_codes": ErrorCode::VARIANTS
                        .iter()
                        .filter(|code| code.exit_code() == *exit)
                        .map(|code| code.as_str())
                        .collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn text(styled: Option<&clap::builder::StyledStr>) -> Value {
    styled
        .map(|text| Value::String(safe_text(&text.to_string())))
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argument_of<'a>(catalog: &'a Value, path: &str, id: &str) -> &'a Value {
        catalog
            .pointer(path)
            .and_then(|args| args.as_array())
            .and_then(|args| args.iter().find(|arg| arg["id"] == id))
            .unwrap_or_else(|| panic!("{id} is not an argument at {path}"))
    }

    /// The top level is the binary's visible subcommands, in its order, so a
    /// subcommand added to clap is in the catalog without anyone listing it.
    #[test]
    fn every_visible_subcommand_is_in_the_catalog_in_definition_order() {
        let catalog = catalog();
        let listed: Vec<&str> = catalog["commands"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let command = build_command();
        let known: Vec<&str> = command
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
            .map(|c| c.get_name())
            .collect();
        assert_eq!(listed, known);
        assert!(
            !listed.contains(&"inventory"),
            "a hidden command is not listed"
        );
        assert_eq!(catalog["schema_version"], 1);
        assert_eq!(catalog["binary"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn arguments_carry_what_the_definition_says() {
        let catalog = catalog();
        let kind = argument_of(&catalog, "/commands/agent/arguments", "kind");
        assert_eq!(kind["long"], "--kind");
        assert_eq!(kind["takes_value"], true);
        assert_eq!(kind["values"], json!(["data", "db", "s3"]));
        assert_eq!(kind["requires"], json!(["json"]));
        assert_eq!(kind["conflicts_with"], json!(["skill"]));
        // `json` declares its conflict; `skill` declares none, and still
        // reads as conflicting with both.
        let skill = argument_of(&catalog, "/commands/agent/arguments", "skill");
        assert_eq!(skill["conflicts_with"], json!(["kind", "json"]));
        assert_eq!(skill["takes_value"], false);

        let profile = argument_of(&catalog, "/commands/logout/arguments", "profile");
        assert_eq!(profile["positional"], true);
        assert_eq!(profile["required_unless_present"], json!(["all"]));
        assert_eq!(profile["requires"], json!([]));

        let param = argument_of(&catalog, "/commands/api/arguments", "param");
        assert_eq!(param["short"], "-P");
        assert_eq!(param["multiple"], true);
        let schema = argument_of(&catalog, "/commands/api/arguments", "schema");
        assert_eq!(schema["takes_value"], true);
        assert_eq!(schema["value_optional"], true);
        let target = argument_of(&catalog, "/commands/api/arguments", "api");
        assert_eq!(target["completer"], true);
        let timeout = argument_of(&catalog, "/commands/api/arguments", "timeout");
        assert_eq!(timeout["value_optional"], false);
        assert_eq!(timeout["default"], json!(["60"]));
        assert_eq!(timeout["value_hint"], "Other");

        let groups = catalog["commands"]["db"]["groups"].as_array().unwrap();
        assert!(
            groups
                .iter()
                .any(|group| group["id"] == "operation" && group["multiple"] == false)
        );
    }

    /// The marks come from `JsonErrorKind` and `ClientKind`, so both lists
    /// are exactly what those types parse.
    #[test]
    fn json_error_and_request_contract_marks_follow_the_kinds() {
        let catalog = catalog();
        for (name, command) in catalog["commands"].as_object().unwrap() {
            assert_eq!(
                command["json_errors"],
                JsonErrorKind::parse(name).is_some(),
                "{name}"
            );
            assert_eq!(
                command["request_contract"],
                ClientKind::parse(name).is_some(),
                "{name}"
            );
        }
        assert_eq!(catalog["commands"]["api"]["json_errors"], true);
        assert_eq!(catalog["commands"]["exec"]["json_errors"], false);
        assert_eq!(catalog["commands"]["db"]["request_contract"], true);
        assert_eq!(
            catalog["commands"]["api"]["usage_error"],
            json!({"code": "API_ARGUMENT_INVALID", "without_json": false})
        );
        assert_eq!(
            catalog["commands"]["db"]["usage_error"],
            json!({"code": "DB_INVALID", "without_json": true})
        );
        assert_eq!(catalog["commands"]["exec"]["usage_error"], Value::Null);
    }

    #[test]
    fn exit_codes_group_every_error_code_once() {
        let catalog = catalog();
        let exits = catalog["exit_codes"].as_array().unwrap();
        assert_eq!(
            exits
                .iter()
                .map(|exit| exit["exit"].clone())
                .collect::<Vec<_>>(),
            json!([0, 1, 2, 3, 4]).as_array().unwrap().clone()
        );
        let listed: usize = exits
            .iter()
            .map(|exit| exit["error_codes"].as_array().unwrap().len())
            .sum();
        assert_eq!(listed, ErrorCode::VARIANTS.len());
        let usage = exits[2]["error_codes"].as_array().unwrap();
        assert!(usage.contains(&json!("CONFIG_INVALID")));
        assert!(exits[0]["error_codes"].as_array().unwrap().is_empty());
    }

    #[test]
    fn declared_ids_reads_only_the_named_field_and_keeps_what_resolves() {
        let command = Command::new("x")
            .arg(
                Arg::new("a")
                    .long("a")
                    .help("a note, requires: [\"c\"]")
                    .requires("b")
                    .required_unless_present("c"),
            )
            .arg(Arg::new("b").long("b"))
            .arg(Arg::new("c").long("c"));
        let a = command.get_arguments().next().unwrap();
        assert_eq!(declared_ids(&command, a, "requires"), ["b"]);
        assert_eq!(declared_ids(&command, a, "r_unless"), ["c"]);
        let b = command.get_arguments().nth(1).unwrap();
        assert!(declared_ids(&command, b, "requires").is_empty());
        // An id the command does not have is dropped, never reported.
        let lone = Command::new("y").arg(Arg::new("a").long("a").requires("gone"));
        let a = lone.get_arguments().next().unwrap();
        assert_eq!(debug_ids(a, "requires"), ["gone"]);
        assert!(declared_ids(&lone, a, "requires").is_empty());
    }

    /// Every id read out of clap's `Debug` names an argument or a group of
    /// its command, in the whole tree, hidden commands included: a change in
    /// the format that misreads an id fails here.
    #[test]
    fn every_declared_id_resolves() {
        fn walk(command: &Command, found: &mut usize) {
            for arg in command.get_arguments() {
                for field in ["requires", "r_unless"] {
                    let read = debug_ids(arg, field);
                    *found += read.len();
                    assert_eq!(
                        declared_ids(command, arg, field),
                        read,
                        "{} {} {field}",
                        command.get_name(),
                        arg.get_id()
                    );
                }
            }
            for subcommand in command.get_subcommands() {
                walk(subcommand, found);
            }
        }
        let mut found = 0;
        walk(&build_command(), &mut found);
        assert!(found >= 2, "agent --kind and logout PROFILE declare ids");
    }
}
