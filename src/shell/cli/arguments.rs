//! What one command-line argument is, read off its clap definition once: the facts `kurama inventory`, `kurama agent --json` and the `api --schema` options each serialize in their own shape.

use clap::{Arg, ArgAction, Command};
use clap_complete::engine::{ArgValueCandidates, ArgValueCompleter};

/// The arguments of `command` a person can pass: clap's own `--help` /
/// `--version` and a hidden argument are not part of any contract.
pub fn visible_arguments(command: &Command) -> impl Iterator<Item = &Arg> {
    command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
}

/// The facts every projection reports, spelled as clap has them (`long`
/// without its dashes, `short` as a char); each projection spells them its
/// own way.
pub struct ArgumentFacts<'a> {
    pub id: &'a str,
    pub positional: bool,
    pub long: Option<&'a str>,
    pub short: Option<char>,
    pub value_name: Option<String>,
    /// It takes a value at all: a flag does not.
    pub takes_value: bool,
    /// It may be given without its value (`num_args(0..=1)`, as
    /// `api --schema [OP]`).
    pub value_optional: bool,
    pub required: bool,
    /// It may be given more than once, or with more than one value.
    pub multiple: bool,
    /// What `value_parser([..])` enumerates, hidden values left out.
    pub values: Vec<String>,
    pub default: Vec<String>,
    /// clap's `ValueHint`, as its variant name.
    pub value_hint: String,
    /// Whether a completer generates candidates for it.
    pub completer: bool,
}

impl<'a> ArgumentFacts<'a> {
    pub fn of(arg: &'a Arg) -> Self {
        let takes_value = !matches!(
            arg.get_action(),
            ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Count
        );
        let num_args = arg.get_num_args();
        Self {
            id: arg.get_id().as_str(),
            positional: arg.is_positional(),
            long: arg.get_long(),
            short: arg.get_short(),
            value_name: arg
                .get_value_names()
                .and_then(|names| names.first())
                .map(ToString::to_string),
            takes_value,
            value_optional: takes_value && num_args.is_some_and(|range| range.min_values() == 0),
            required: arg.is_required_set(),
            multiple: matches!(arg.get_action(), ArgAction::Append | ArgAction::Count)
                || num_args.is_some_and(|range| range.max_values() > 1),
            values: arg
                .get_possible_values()
                .iter()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_owned())
                .collect(),
            default: arg
                .get_default_values()
                .iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect(),
            value_hint: format!("{:?}", arg.get_value_hint()),
            completer: arg.get::<ArgValueCandidates>().is_some()
                || arg.get::<ArgValueCompleter>().is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::cli::args::build_command;

    fn facts_of<'a>(command: &'a Command, id: &str) -> ArgumentFacts<'a> {
        ArgumentFacts::of(
            visible_arguments(command)
                .find(|arg| arg.get_id() == id)
                .unwrap_or_else(|| panic!("{id} is not an argument of {}", command.get_name())),
        )
    }

    #[test]
    fn an_optional_value_is_told_from_a_required_one_and_from_a_flag() {
        let root = build_command();
        let api = root.find_subcommand("api").unwrap();
        let schema = facts_of(api, "schema");
        assert!(schema.takes_value && schema.value_optional && !schema.multiple);
        let timeout = facts_of(api, "timeout");
        assert!(timeout.takes_value && !timeout.value_optional);
        assert_eq!(timeout.default, ["60"]);
        assert_eq!(timeout.value_hint, "Other");
        let param = facts_of(api, "param");
        assert!(param.multiple);
        assert_eq!((param.long, param.short), (Some("param"), Some('P')));
        let dry_run = facts_of(api, "dry-run");
        assert!(!dry_run.takes_value && !dry_run.value_optional);
        let target = facts_of(api, "api");
        assert!(target.positional && target.required && target.completer);
    }

    #[test]
    fn help_version_and_hidden_arguments_are_not_visible() {
        let mut built = Command::new("x")
            .version("1")
            .arg(Arg::new("shown").long("shown"))
            .arg(Arg::new("secret").long("secret").hide(true));
        built.build();
        let ids: Vec<&str> = visible_arguments(&built)
            .map(|arg| arg.get_id().as_str())
            .collect();
        assert_eq!(ids, ["shown"]);
    }
}
