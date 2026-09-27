//! The CLI and the agent contract: every argument explains itself and says how
//! it completes, and every documented command parses.

use std::collections::{BTreeMap, BTreeSet};

use crate::support::*;

/// The arguments of `command` and its subcommands that have no help.
fn arguments_without_help(command: &clap::Command, path: &str) -> Vec<String> {
    let mut missing: Vec<String> = command
        .get_arguments()
        .filter(|arg| arg.get_help().is_none() && arg.get_long_help().is_none())
        .map(|arg| format!("{path}: {}", arg.get_id()))
        .collect();
    for subcommand in command.get_subcommands() {
        missing.extend(arguments_without_help(
            subcommand,
            &format!("{path} {}", subcommand.get_name()),
        ));
    }
    missing
}

#[test]
fn every_cli_argument_explains_itself() {
    let mut command = kurama::build_command();
    command.build();
    let missing = arguments_without_help(&command, "kurama");
    assert!(
        missing.is_empty(),
        "arguments without help:\n{}",
        missing.join("\n")
    );
}

/// Whether the dynamic completion engine can offer anything for this
/// argument's value. `ValueHint::Unknown` is the default, and
/// `clap_complete`'s engine returns nothing for it, so an argument that never
/// says how it completes silently stops completing when the static script is
/// replaced by the dynamic one.
fn says_how_to_complete_its_values(arg: &clap::Arg) -> bool {
    use clap_complete::engine::{ArgValueCandidates, ArgValueCompleter};
    arg.get_value_hint() != clap::ValueHint::Unknown
        || !arg.get_possible_values().is_empty()
        || arg.get::<ArgValueCompleter>().is_some()
        || arg.get::<ArgValueCandidates>().is_some()
}

/// Every argument that takes a value says how its value completes: a
/// `ValueHint`, a set of possible values, or a completer. `ValueHint::Other`
/// is how an argument says its value cannot be completed, so "no completion"
/// stays a decision someone wrote down rather than a default nobody noticed.
/// The value-taking arguments of `command` and its subcommands that never
/// say how their value completes.
fn arguments_that_do_not_complete(command: &clap::Command, path: &str) -> Vec<String> {
    let mut silent: Vec<String> = command
        .get_arguments()
        .filter(|arg| arg.get_action().takes_values() && !says_how_to_complete_its_values(arg))
        .map(|arg| format!("{path}: {}", arg.get_id()))
        .collect();
    for subcommand in command.get_subcommands() {
        silent.extend(arguments_that_do_not_complete(
            subcommand,
            &format!("{path} {}", subcommand.get_name()),
        ));
    }
    silent
}

#[test]
fn every_value_taking_argument_says_how_to_complete_it() {
    let mut command = kurama::build_command();
    command.build();
    let silent = arguments_that_do_not_complete(&command, "kurama");
    assert!(
        silent.is_empty(),
        "these arguments take a value but never say how it completes; add a \
         `value_hint`, `value_parser([..])` or a completer, or say \
         `value_hint(ValueHint::Other)` when the value cannot be completed:\n{}",
        silent.join("\n")
    );
}

fn fixture_command(arg: clap::Arg) -> clap::Command {
    let mut command = clap::Command::new("k").subcommand(clap::Command::new("sub").arg(arg));
    command.build();
    command
}

#[test]
fn an_argument_without_help_is_found_in_a_subcommand() {
    let command = fixture_command(clap::Arg::new("name").long("name"));
    let missing = arguments_without_help(&command, "k");
    assert_detected(
        "ARCH-011",
        missing == ["k sub: name"],
        &format!("{missing:?}"),
    );
}

#[test]
fn an_argument_with_help_or_long_help_is_allowed() {
    for arg in [
        clap::Arg::new("name").long("name").help("the name"),
        clap::Arg::new("name")
            .long("name")
            .long_help("the name, at length"),
    ] {
        let command = fixture_command(arg);
        let missing = arguments_without_help(&command, "k");
        assert_allowed("ARCH-011", missing.is_empty(), &format!("{missing:?}"));
    }
}

#[test]
fn a_value_that_never_says_how_it_completes_is_found() {
    let command = fixture_command(
        clap::Arg::new("name")
            .long("name")
            .help("h")
            .action(clap::ArgAction::Set),
    );
    let silent = arguments_that_do_not_complete(&command, "k");
    assert_detected(
        "ARCH-015",
        silent == ["k sub: name"],
        &format!("{silent:?}"),
    );
}

#[test]
fn a_value_hint_possible_values_or_a_flag_is_allowed() {
    for arg in [
        clap::Arg::new("name")
            .long("name")
            .value_hint(clap::ValueHint::Other),
        clap::Arg::new("name").long("name").value_parser(["a", "b"]),
        clap::Arg::new("name")
            .long("name")
            .action(clap::ArgAction::SetTrue),
    ] {
        let command = fixture_command(arg);
        let silent = arguments_that_do_not_complete(&command, "k");
        assert_allowed("ARCH-015", silent.is_empty(), &format!("{silent:?}"));
    }
}

/// The guide is split into one file per section so two branches editing
/// different parts do not touch the same file, and `agent.rs` concatenates
/// them in a fixed order. A section nobody added to that list would be a
/// chapter the binary silently stops printing.
#[test]
fn every_guide_section_is_printed() {
    let agent = std::fs::read_to_string(root().join("src/shell/cli/commands/agent.rs")).unwrap();
    let listed: Vec<&str> = agent
        .lines()
        .filter_map(|line| line.trim().strip_prefix("include_str!(\""))
        .filter_map(|rest| rest.split('"').next())
        .filter_map(|path| path.strip_prefix("../../../../docs/agents/kurama/"))
        .collect();
    let on_disk: Vec<String> = agent_guide_sections()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let missing: Vec<&String> = on_disk
        .iter()
        .filter(|name| !listed.contains(&name.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "add these to AGENT_GUIDE in src/shell/cli/commands/agent.rs:\n{}",
        missing
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    // And nothing listed twice, which would print a chapter twice.
    let mut seen = BTreeSet::new();
    for name in &listed {
        assert!(seen.insert(*name), "{name} is included more than once");
    }
    assert_eq!(seen.len(), on_disk.len());
}

#[test]
fn documented_commands_are_accepted_by_the_parser() {
    let mut documents: Vec<(String, String)> = ["docs/development/data.md", "README.md"]
        .into_iter()
        .map(|doc| {
            (
                doc.to_string(),
                std::fs::read_to_string(root().join(doc)).unwrap(),
            )
        })
        .collect();
    documents.extend(
        agent_guide_sections()
            .into_iter()
            .map(|(name, text)| (format!("docs/agents/kurama/{name}"), text)),
    );
    // The guide counts as one document: a section such as `exit-codes.md`
    // carries no command, while the page as a whole must.
    let group = |doc: &str| -> String {
        match doc.starts_with("docs/agents/kurama/") {
            true => "docs/agents/kurama/".to_string(),
            false => doc.to_string(),
        }
    };
    let mut checked: BTreeMap<String, usize> =
        documents.iter().map(|(doc, _)| (group(doc), 0)).collect();
    for (doc, text) in documents {
        let mut in_shell = false;
        let mut pending = String::new();
        let mut samples = 0;
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if let Some(language) = line.strip_prefix("```") {
                in_shell = matches!(language, "sh" | "bash" | "zsh");
                assert!(
                    pending.is_empty(),
                    "{doc}:{}: unfinished command",
                    number + 1
                );
                continue;
            }
            if !in_shell {
                continue;
            }
            let line = line.trim_start_matches("$ ");
            if pending.is_empty() && line != "kurama" && !line.starts_with("kurama ") {
                continue;
            }
            if let Some(part) = line.strip_suffix('\\') {
                pending.push_str(part);
                pending.push(' ');
                continue;
            }
            pending.push_str(line);
            let args = shell_words::split(&pending)
                .unwrap_or_else(|error| panic!("{doc}:{}: {error}: {pending}", number + 1));
            let result = kurama::build_command().try_get_matches_from(&args);
            if let Err(error) = result {
                assert!(
                    matches!(
                        error.kind(),
                        clap::error::ErrorKind::DisplayHelp
                            | clap::error::ErrorKind::DisplayVersion
                    ),
                    "{doc}:{}: documented command `{pending}` is rejected:\n{error}",
                    number + 1,
                );
            }
            samples += 1;
            pending.clear();
        }
        *checked.get_mut(&group(&doc)).expect("counted above") += samples;
    }
    for (doc, samples) in checked {
        assert!(samples > 0, "{doc}: no shell commands checked");
    }
}

/// Flags the contract page deliberately does not name, with the reason.
/// Everything else a subcommand takes has to appear there by long or short
/// name: the page is what an agent reads instead of `--help`.
const FLAGS_OFF_THE_CONTRACT: [(&str, &str, &str); 1] = [(
    "*",
    "--help",
    "clap's own; it prints the help the binary already has",
)];

/// The flags of a JSON client are documented by the capability document
/// `kurama agent --kind <KIND> --json` prints, not by the page's prose. The
/// document is built from the request types and the contract's own fields,
/// so a flag missing from both never reaches an agent.
const CLIENT_CONTRACT_SOURCES: [(&str, [&str; 2]); 2] = [
    (
        "data",
        [
            "src/domain/types/dataset.rs",
            "src/shell/cli/commands/data_contract.rs",
        ],
    ),
    (
        "db",
        [
            "src/domain/types/database.rs",
            "src/shell/cli/commands/db_contract.rs",
        ],
    ),
];

#[test]
fn the_agent_contract_names_every_flag() {
    let guide = agent_guide();
    let capabilities: BTreeMap<&str, String> = CLIENT_CONTRACT_SOURCES
        .iter()
        .map(|(kind, paths)| {
            (
                *kind,
                paths
                    .iter()
                    .map(|path| std::fs::read_to_string(root().join(path)).unwrap())
                    .collect(),
            )
        })
        .collect();
    let mut command = kurama::build_command();
    command.build();
    let mut missing = Vec::new();
    for subcommand in command.get_subcommands() {
        let name = subcommand.get_name();
        if name == "help" {
            continue;
        }
        for arg in subcommand.get_arguments() {
            let Some(long) = arg.get_long() else {
                continue;
            };
            let long = format!("--{long}");
            if FLAGS_OFF_THE_CONTRACT
                .iter()
                .any(|(on, flag, _)| (*on == "*" || *on == name) && *flag == long)
            {
                continue;
            }
            let documented = if let Some(document) = capabilities.get(name) {
                document.contains(&arg.get_long().unwrap().replace('-', "_"))
            } else {
                guide.contains(&long)
                    || arg
                        .get_short()
                        // `-H "Name: value"`: the flag opens a code span, the
                        // value closes it.
                        .is_some_and(|short| guide.contains(&format!("`-{short}")))
            };
            if !documented {
                missing.push(format!("kurama {name}: {long}"));
            }
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "these flags are on neither the contract page nor, for a JSON client, its \
         capability document; name them or add them to \
         `FLAGS_OFF_THE_CONTRACT` with the reason:\n{}",
        missing.join("\n")
    );
}

#[test]
fn the_agent_contract_names_real_error_codes() {
    let guide = agent_guide();
    let source = std::fs::read_to_string(root().join("src/shell/cli/error_code.rs")).unwrap();
    let mut unknown: Vec<&str> = guide
        .match_indices("error[")
        .filter_map(|(start, _)| {
            let rest = &guide[start + "error[".len()..];
            rest.find(']').map(|end| &rest[..end])
        })
        // `error[CODE]` is how the page writes the shape of the line.
        .filter(|code| *code != "CODE")
        .filter(|code| !source.contains(&format!("\"{code}\"")))
        .collect();
    unknown.sort_unstable();
    unknown.dedup();
    assert!(
        unknown.is_empty(),
        "the contract page names error codes the binary cannot print: {unknown:?}"
    );
}
