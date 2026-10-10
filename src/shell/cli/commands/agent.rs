//! `kurama agent`: the contract for agents and scripts, embedded from
//! `docs/agents/kurama/` so it always describes the installed binary;
//! `--skill` prints the Agent Skill (`docs/agents/SKILL.md`) that points at
//! it. Neither reads configuration: an agent runs `agent` to write or fix
//! `config.toml`.

/// The sections of the guide, in the order `kurama agent` prints them. One
/// file per section, so two branches adding to different parts of the page
/// touch different files and only this list is shared; `concat!` keeps the
/// whole thing a compile-time constant, byte for byte what the files hold.
/// A new section is one more line here and one more file, and
/// `every_guide_section_is_printed` fails for a file left out.
pub const AGENT_GUIDE: &str = concat!(
    include_str!("../../../../docs/agents/kurama/intro.md"),
    include_str!("../../../../docs/agents/kurama/commands.md"),
    include_str!("../../../../docs/agents/kurama/shell-completion.md"),
    include_str!("../../../../docs/agents/kurama/status.md"),
    include_str!("../../../../docs/agents/kurama/api.md"),
    include_str!("../../../../docs/agents/kurama/data.md"),
    include_str!("../../../../docs/agents/kurama/db.md"),
    include_str!("../../../../docs/agents/kurama/s3.md"),
    include_str!("../../../../docs/agents/kurama/mcp.md"),
    include_str!("../../../../docs/agents/kurama/obsidian.md"),
    include_str!("../../../../docs/agents/kurama/setup.md"),
    include_str!("../../../../docs/agents/kurama/recipes.md"),
    include_str!("../../../../docs/agents/kurama/exit-codes.md"),
    include_str!("../../../../docs/agents/kurama/rules.md"),
);

/// `docs/agents/SKILL.md`: what `kurama agent --skill` prints.
pub const AGENT_SKILL: &str = include_str!("../../../../docs/agents/SKILL.md");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::cli::args::build_command;

    /// The subcommands `text` names as `kurama <name>` in inline code, such
    /// as `` `kurama status --json` ``.
    fn commands_named(text: &str) -> Vec<String> {
        text.split("`kurama ")
            .skip(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|c| !matches!(c, '`' | ' ' | '\n'))
                    .collect::<String>()
            })
            .filter(|word| !word.is_empty() && !word.starts_with(['-', '<']))
            .collect()
    }

    #[test]
    fn guide_names_only_commands_the_binary_has() {
        let command = build_command();
        // A hidden command is not for an agent either: it is for xtask.
        let known: Vec<&str> = command
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
            .map(|c| c.get_name())
            .collect();
        let named = commands_named(AGENT_GUIDE);
        // Both directions. A hand-written list of what the page must name
        // goes stale the way the page does -- `unset` was missing from the
        // page for as long as it was missing from the list -- so the list
        // comes from the binary, and only what an agent cannot use is left
        // out, by name and with the reason.
        let not_for_agents = [
            // clap's own; it prints the same help the binary already has.
            "help",
            // Shell setup for a person's rc file, covered in prose instead.
            "init",
            "completions",
            // Opens a browser and prints no URL: there is nothing for a
            // script to consume. The Rules section still says it needs `op`.
            "console",
        ];
        let mut missing: Vec<&str> = known
            .iter()
            .copied()
            .filter(|name| !not_for_agents.contains(name))
            .filter(|name| !named.iter().any(|named| named == name))
            .collect();
        missing.sort_unstable();
        assert!(
            missing.is_empty(),
            "the agent guide does not name these commands: {missing:?}; \
             add them, or add them to `not_for_agents` with the reason"
        );
        let mut unknown: Vec<String> = named
            .into_iter()
            .filter(|name| !known.contains(&name.as_str()))
            .collect();
        unknown.sort();
        unknown.dedup();
        assert!(
            unknown.is_empty(),
            "the agent guide names commands the binary lacks: {unknown:?}"
        );
    }

    /// The README, so its configuration sample is checked by the same parser
    /// that reads a real `config.toml`.
    const README: &str = include_str!("../../../../README.md");

    /// `<...>` stands for a value a person fills in; TOML needs a real one.
    fn without_placeholders(sample: &str) -> String {
        let mut out = String::with_capacity(sample.len());
        let mut rest = sample;
        while let Some(start) = rest.find('<') {
            match rest[start..].find('>') {
                Some(end) => {
                    out.push_str(&rest[..start]);
                    out.push('x');
                    rest = &rest[start + end + 1..];
                }
                None => break,
            }
        }
        out.push_str(rest);
        out
    }

    /// Fenced blocks of one language, fence markers and indentation removed.
    fn fenced_blocks<'a>(text: &'a str, language: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut current: Option<(usize, Vec<&'a str>)> = None;
        for line in text.lines() {
            let trimmed = line.trim_start();
            match &mut current {
                Some((indent, lines)) => {
                    if trimmed.starts_with("```") {
                        blocks.push(
                            lines
                                .iter()
                                .map(|line| line.get(*indent..).unwrap_or(line.trim_start()))
                                .collect::<Vec<_>>()
                                .join("\n"),
                        );
                        current = None;
                    } else {
                        lines.push(line);
                    }
                }
                None if trimmed == format!("```{language}") => {
                    current = Some((line.len() - trimmed.len(), Vec::new()));
                }
                None => {}
            }
        }
        blocks
    }

    /// An agent copies these samples, so a key the parser no longer accepts
    /// or a sample that is not the shape it claims has to fail here.
    #[test]
    fn the_configuration_samples_parse_as_configuration() {
        let samples: Vec<String> = [AGENT_GUIDE, README]
            .iter()
            .flat_map(|text| fenced_blocks(text, "toml"))
            .collect();
        assert!(
            samples.len() >= 2,
            "no toml samples found: {}",
            samples.len()
        );
        for sample in samples {
            // `<name>` reads as a placeholder to a person and as a syntax
            // error to TOML, so give every one of them a real name.
            let sample = without_placeholders(&sample);
            crate::adapters::config::Config::parse(&sample).unwrap_or_else(|error| {
                panic!("a documented config.toml is invalid: {error}\n{sample}")
            });
        }
    }

    #[test]
    fn the_json_samples_are_json() {
        let samples = fenced_blocks(AGENT_GUIDE, "json");
        assert!(!samples.is_empty(), "no json samples found");
        for sample in samples {
            serde_json::from_str::<serde_json::Value>(&sample).unwrap_or_else(|error| {
                panic!("a documented JSON sample is not JSON: {error}\n{sample}")
            });
        }
    }

    /// The minimal OpenAPI description the setup chapter tells an agent to
    /// write for an API that publishes none: copied as it is, it has to
    /// list its operations with no warning, one with a path parameter it
    /// does not declare.
    #[test]
    fn the_minimal_openapi_sample_lists_its_operations() {
        let samples = fenced_blocks(AGENT_GUIDE, "yaml");
        assert_eq!(samples.len(), 1, "one minimal OpenAPI sample");
        assert_eq!(
            format!("{}\n", samples[0]),
            include_str!("../../../../tests/fixtures/openapi/minimal.yaml"),
            "the scenarios run the documented sample"
        );
        let spec = crate::adapters::openapi::parse_spec(
            &crate::domain::types::api_spec::SpecFormat::OpenApi,
            samples[0].as_bytes(),
        )
        .unwrap_or_else(|error| panic!("the sample does not load: {error}\n{}", samples[0]));
        assert!(spec.warnings.is_empty(), "{:?}", spec.warnings);
        let operations: Vec<(&str, &str)> = spec
            .operations
            .iter()
            .map(|op| (op.id.as_str(), op.path.as_str()))
            .collect();
        assert_eq!(
            operations,
            [
                ("users/myself", "/api/v2/users/myself"),
                ("issues/list", "/api/v2/issues"),
                ("issues/get", "/api/v2/issues/{issueIdOrKey}"),
            ]
        );
        assert_eq!(spec.operations[2].required_inputs(), ["issueIdOrKey"]);
    }

    #[test]
    fn guide_describes_the_setup_and_names_itself() {
        assert!(AGENT_GUIDE.starts_with("# kurama for agents and scripts\n"));
        assert!(AGENT_GUIDE.contains("\n## Setup"));
        assert!(AGENT_GUIDE.contains("`kurama agent`"));
    }

    #[test]
    fn skill_has_frontmatter_and_points_at_the_guide() {
        assert!(AGENT_SKILL.starts_with("---\nname: kurama\ndescription: "));
        let body = &AGENT_SKILL["---\n".len()..];
        let (frontmatter, rest) = body.split_once("\n---\n").expect("frontmatter closes");
        let keys: Vec<(&str, &str)> = frontmatter
            .lines()
            .filter_map(|line| line.split_once(": "))
            .collect();
        assert!(
            keys.contains(&("name", "kurama")),
            "the frontmatter must keep `name: kurama`"
        );
        assert!(
            keys.iter()
                .any(|(key, value)| *key == "description" && !value.is_empty()),
            "the frontmatter must keep a `description` saying when to use kurama"
        );
        assert!(rest.contains("`kurama agent`"));
        assert!(rest.contains("op://"));
    }

    #[test]
    fn commands_named_reads_inline_code() {
        let text = "Run `kurama status --json`, then `kurama exec <p> -- ls`.\n\
                    `kurama` alone is the TUI; `kurama --version` prints it.\n\
                    kurama turns a source into credentials (prose, not a command).\n";
        assert_eq!(commands_named(text), ["status", "exec"]);
    }
}
