# AGENTS.md

kurama is a credential switcher and API client (Rust CLI/TUI, macOS, zsh): it
resolves an AWS profile to temporary credentials through STS (MFA from
1Password) or an `[auth.*]` source to a token (an OAuth grant, or a key read
from a secret store), exports them into the shell or `exec`s a command with
them, calls `[api.*]` profiles, explores OpenAPI descriptions, and reads
databases, S3 and local data files.

## Start here

1. Before any Grep or Read, run `cargo xtask map <feature>` for the feature
   the task is about (several if it spans them): the entry file, the `docs`
   that narrate the flow, every file with its `//!` purpose line, the test
   filters and the scenarios. Read what it lists; do not search the tree.
2. `cargo xtask search <word>`: existing public symbols, before adding one.
3. Write the failing test first: a unit test in the same file, and a scenario
   (`tests/cases/<feature>/<id>.toml`, or `tests/scenarios/<feature>.rs` for
   what data cannot say) when user-visible behavior changes.
4. `cargo check --all-targets --features test-fakes` lists every caller.
5. `cargo test --locked --features test-fakes -- <filters from map>`, then
   `cargo xtask verify affected`. The branch gate is `cargo xtask branch-check`.
6. Update `.agent/features/<feature>.toml` for new files, scenarios or a new
   `depends_on` (`cargo xtask deps` names it); `doctor` and
   `tests/architecture/` name what is missing.
7. Land through a PR to `main` with `Closes #N`; a person merges it (merge
   commits only).

Features: `agent-guide`, `agent-policy`, `api-client`, `api-explorer`, `assume-role`, `audit`, `cli-entry`, `config`, `console-federation`, `database`, `errors`, `exec`, `export-output`, `local-analytics`, `mcp`, `mfa-login`, `oauth`, `onepassword-mfa`, `preset`, `profile-loading`, `s3-explorer`, `secrets`, `session-cache`, `shell-integration`, `status`, `tui`, `verification-harness`, `xtask`

Once per clone: `mise trust && mise run setup` (hooks, tools, xtask build).

## Daily commands

| Command | Use |
| --- | --- |
| `cargo xtask map [FEATURE]` / `search <PATTERN>` | where to read |
| `cargo xtask impact [--base REF]` | changed files -> features -> filters -> scenarios |
| `cargo xtask verify <affected\|FEATURE>` | run scenarios, write `target/agent/verification-report.md` |
| `cargo xtask tui-check` | after a TUI change; read `target/agent/tui-report.md` |
| `cargo xtask generate-cases` | after adding, removing or renaming a `tests/cases/` file |
| `cargo xtask branch-check` | the branch gate (pre-push runs it) |
| `cargo xtask ready` / `worktree add <ISSUE>` | pick and take an issue |

Every command, with its options: `cargo xtask --help`, or
`docs/development/commands.md`.

## Layout

- `src/domain`, `src/workflows`: pure -- no I/O, clock, logging, adapters or
  shell. `src/ports`: traits and plain data. `src/adapters`: I/O (AWS SDK,
  1Password CLI, keychain, files). `src/shell`: the CLI, the TUI, `Runtime`
  and the effect interpreter. `tests/architecture/` enforces the layering.
- CLI flow: `args.rs` (clap) -> `parser.rs` -> `bootstrap.rs` ->
  `dispatch.rs` -> `commands/*.rs` -> effects -> `src/shell/executor.rs` ->
  `src/adapters/*`. Each feature's own flow is in the `docs` `map` prints.
- Errors: `src/shell/cli/error_code.rs` maps the typed chain to
  `error[CODE]` and an exit code; `rg -n <CODE> src tests` finds the arm and
  the scenario.

## Invariants

1. The keychain is a cache, never a requirement: a token that cannot be
   stored is still used; an unreadable entry counts as absent.
2. Role credentials and tokens never touch a file: stdout, the wrapper's
   mode-600 temp file, or the `exec`ed command's environment only.
3. stdout carries only data; progress, prompts, logs and errors go to
   stderr; no ANSI sequences when piped.
4. Secrets never appear in `Debug`, `Display`, logs, `--dry-run` or `-v`.
5. No unexpected STS, federation, token endpoint, API, 1Password or browser
   call: scenarios pin the exact sequence.
6. Every failure is one `error[CODE]: message` line (no newline), an
   optional `hint:`, and exit 1 tool / 2 usage / 3 human action / 4 remote
   rejected. Without a terminal nothing prompts or waits.
7. `init zsh`, `completions` and `agent` read no configuration.

## Conventions no test enforces

- A fake answers with what it was given, never a constant; break the code
  and watch a new assertion fail before trusting it.
- Check `Observed` (what the run produced), not `Seeded` (what the harness
  put in place).
- Names are verb + object; no `handle`, `process`, `util`, `helper`.
- One port implementation or command per file, under about 500 lines; tests
  in the same file, moved to `<name>_tests.rs` when it grows.
- Keep typed errors in the `anyhow` chain (`?`, `.context`), never as text.
  A new failure kind gets a code, an exit code, a hint and a scenario.
- Mutually exclusive state is one enum; projections of one type destructure
  it without `..`.
- Dependencies via `cargo add`; `Cargo.lock` is tracked.
- In parallel work, append at the end of the shared files (`error_code.rs`
  `classify`, `args.rs`, `dispatch.rs`, `client.rs`, xtask's subcommand
  branch and HELP, `docs/development/commands.md`).

## Definition of Done

- [ ] Tests written first and passing; `cargo xtask verify affected` passes.
- [ ] A user-visible change has a scenario, seen to fail against a deliberate regression.
- [ ] A new failure path has a code, an exit code, a hint and a scenario.
- [ ] A TUI change passes `cargo xtask tui-check`, its screens read.
- [ ] `.agent/features/`, `README.md` and `README.ja.md` updated where structure or behavior changed.
- [ ] The PR description carries `target/agent/verification-report.md`.

The full checklist: `docs/development/conventions.md`.

## More, when the task needs it

| Need | Read |
| --- | --- |
| Full procedures (bug, TUI, refactor) | `docs/development/workflows.md` |
| Several issues at once, `Affects:`, `## Scenarios` | `docs/development/parallel-work.md` |
| Feature map, scenarios, TOML cases, verification layers | `docs/development/harness.md` |
| Invariants and conventions with their reasons, Definition of Done | `docs/development/conventions.md` |
| What runs against real services | `cargo xtask verify-matrix`, `.agent/real/`, `tests/cases/needs-human.toml` |
