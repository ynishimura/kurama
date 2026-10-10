# Agent contract

How `kurama agent` assembles its page from `docs/agents/kurama/`.

Agent contract: `kurama agent` prints the sections of
`docs/agents/kurama/`, one file per chapter, concatenated in order by
`AGENT_GUIDE` in `src/shell/cli/commands/agent.rs` (`concat!` of
`include_str!`, so the page is still a compile-time constant and always
matches the binary); `--skill` prints `docs/agents/SKILL.md`, the Agent
Skill that tells an agent to read it first. The page carries the Setup
chapter for `[auth.*]` and `[api.*]`; its unit test fails when the page
names a command the binary lacks, and
`every_guide_section_is_printed` fails for a section file that list
leaves out.
