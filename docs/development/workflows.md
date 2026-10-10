# Workflows

The full procedures AGENTS.md summarizes: implement, fix, TUI, refactor.

## Implement or change a feature

Before starting, when another branch is already running:
`cargo xtask conflicts <this issue> <the other issue>`.

1. `cargo xtask map <feature>`: read `entry`, then the listed files only.
2. Search before adding: `cargo xtask search <verb or noun>` lists existing
   public symbols; the feature's scenarios show existing behavior.
3. Write the failing test first: a unit test in the same file, and a scenario
   in `tests/scenarios/` whenever user-visible behavior changes.
4. Implement. `cargo check --all-targets --features test-fakes` lists every
   caller to update.
5. `cargo test --locked --features test-fakes -- <filters>`, then
   `cargo xtask verify affected`. On a feature branch the three steps of
   `cargo xtask branch-check` are the gate; `cargo xtask check` runs on `main`
   when a release tag is pushed from it.
6. Update `.agent/features/` when you add files, tests, scenarios or a
   dependency on another feature's code (`depends_on`); the architecture test
   and `doctor` name the missing entries.
7. Land it through a pull request: `main` takes no direct push (a ruleset
   refuses it). Push the branch (the pre-push hook runs `branch-check`),
   `gh pr create --base main` with `Closes #N` in the body, and stop there:
   a person merges it. `main` asks for an approval, the PR checks and a
   branch up to date with `main` (`gh pr update-branch`); an agent does not
   bypass the approval unless its maintainer allowed
   `gh pr merge --merge --admin` in their own `.claude/settings.local.json`.
   After the merge, `git switch main && git pull`. `main` takes merge
   commits only: a squash or a rebase would leave the branch's own commits
   out of `main`, and `worktree remove` could no longer tell it was merged.

## Fix a bug or investigate an error

1. Reproduce it as a scenario: copy the closest one in `tests/scenarios/`,
   set args and fakes (`StsFake::Error`, `OnePassword::NotSignedIn`,
   `with_config`, `then_run` for multi-process state), and assert the correct
   behavior so the test fails now.
2. Locate the source from the code: `rg -n "<CODE>" src tests`; from a
   message: `rg -n "<message text>" src/`. For a live run,
   `KURAMA_LOG_FORMAT=json RUST_LOG=kurama=debug kurama ...` prints one JSON
   object per event with `target`, `filename` and `line_number`.
3. Follow the CLI flow above from `dispatch.rs` to the adapter.
4. Fix, keep the scenario, run `cargo xtask verify affected`.

## Change the TUI

Follow `docs/development/tui-testing.md`: the loop, the fixtures, the snapshot
rules and the step-by-step checklist live there. Compiling and passing unit
tests does not show what a user sees; the change is done only when
`cargo xtask tui-check` passes and the screens in `target/agent/tui-report.md`
have been read.

## Work on several issues at once

See parallel-work.md.

## Refactor

Run `cargo xtask verify all` before and after: the reports under `target/agent/`
must show the same STS, federation, `op` and browser call sequences, exported
variables, exit codes and `error[...]` / `hint:` lines (diff the two reports).
A refactor is done when it removes something it names. The tests an abstraction
has to pass before it is written are in `docs/development/module-structure.md`.
