# Contributing to kurama

Thank you for taking the time. kurama handles credentials, so the bar for a
change is that it is shown to work by the binary itself, not only by unit
tests. This page is the path from a clone to a pull request.

## Before you start

- **Platform.** kurama targets macOS and zsh, and that is where the gate is
  run. Linux (glibc, x86_64 and aarch64) is a supported build target without
  the keychain-backed caches (MFA sessions, OAuth tokens, the 1Password
  service account entry), but no Linux run of the gate is recorded yet (see
  the [manual CI record](docs/development/setup.md#manual-ci-record)); a
  report of what fails there is welcome. Windows and musl are not supported.
- **Talk first for anything large.** Open an issue with the template before a
  change that adds a command, a configuration key or a dependency.
- **Security issues** do not go in a public issue: see [SECURITY.md](SECURITY.md).

## Setup

You need:

- [rustup](https://rustup.rs) with Rust 1.95.0 or newer, and the `rustfmt`
  and `clippy` components;
- [mise](https://mise.jdx.dev);
- a C and C++ compiler (the Xcode Command Line Tools on macOS, `gcc` and `g++`
  on Linux), because DuckDB is compiled from source;
- `git`, `curl` and network access for the first build, which downloads
  DuckDB's `httpfs` extension and checks its SHA-256 (set
  `KURAMA_HTTPFS_ARCHIVE` to a downloaded archive to build offline);
- `zsh`, for the shell integration tests (`cargo xtask doctor` says when they
  will be skipped).

Then, once per clone:

```bash
mise trust
mise run setup          # pinned Lefthook, cargo-machete, cargo-sweep; installs the pre-push hook
cargo xtask doctor      # toolchain, metadata and the fake environment are ready
```

The first `cargo xtask` compiles for a couple of minutes, and the first full
build compiles DuckDB from source, which takes longer. Nothing here needs AWS,
1Password or a real keychain: the tests run the real binary against fake STS,
OAuth, API and 1Password endpoints (`--features test-fakes`).

## Making a change

[AGENTS.md](AGENTS.md) is the full working agreement, for people and coding
agents alike. The short version:

1. **Find the feature.** `cargo xtask map <feature>` lists its files, tests
   and runtime scenarios. Read those, not the whole tree.
2. **Search before adding.** `cargo xtask search <word>` lists the public
   symbols that already exist.
3. **Write the failing test first.** A unit test next to the code, and a
   runtime scenario (`tests/cases/<feature>/<id>.toml`, or
   `tests/scenarios/<feature>.rs` for what data cannot say) whenever
   user-visible behavior changes. Run `cargo xtask generate-cases` after
   adding a case file.
4. **Implement,** then run
   `cargo test --locked --features test-fakes -- <filters>` and
   `cargo xtask verify affected`.
5. **Update the feature map** in `.agent/features/` when you add files,
   scenarios or a dependency on another feature; `cargo xtask doctor` and the
   architecture tests name what is missing.
6. **Record user-visible changes** under `Unreleased` in
   [CHANGELOG.md](CHANGELOG.md).

Some commands in AGENTS.md are for the maintainer and need access to this
repository's GitHub project board: `cargo xtask ready`, `claim`, `board` and
`worktree add`. A contributor does not need them; a plain branch is enough.
Neither do you need `cargo xtask install-signed` or its signing certificate
to contribute: that only keeps keychain grants across rebuilds of the binary
you use every day.

Every failure the binary reports is one `error[CODE]: message` line, an
optional `hint:` line and an exit code (1 tool, 2 usage, 3 human action,
4 remote rejection). A new failure kind gets a code, a hint when a person has
to act, and a scenario.

## The gate

`cargo xtask check` is the gate: formatting, `clippy -D warnings`, the
dead-code audit, every test and every runtime scenario. Run it before you open
a pull request and paste `target/agent/verification-report.md` into the pull
request description.

The pre-push hook runs `cargo xtask branch-check` on a feature branch. Two of
its steps may not apply to you:

- **Real-environment evidence.** A feature that talks to a real service
  (AWS, 1Password, a database) keeps evidence of a run against that service
  in `.agent/real/`. If your change touches such a feature, `branch-check`
  stops at `real-verification`, because only an environment with those
  services can refresh the evidence. Say so in the pull request; a maintainer
  runs `cargo xtask verify-real <feature>` before merging.
- **Mutation testing** of the changed lines can take hours on a large change.

In either case run `cargo xtask check` instead and push with
`LEFTHOOK=0 git push`.

What each layer of the gate catches, where it runs and how it was seen to
fail is in [docs/development/prevention-layers.md](docs/development/prevention-layers.md).

## CI

CI runs in two parts. Every pull request, forks included, and every push to
`main` runs `.github/workflows/pr.yml`: `cargo fmt --check`, clippy with
`-D warnings` and the unit tests (the library and xtask), on Ubuntu. The
`main` ruleset requires both of its jobs, so they have to pass before a
merge. They are a subset of the local gate, not a replacement: the scenarios,
the TUI checks and the architecture rules run only there and in the full
gate, and the `verification-report.md` you paste is still what a reviewer
reads first.

The full gate (`.github/workflows/ci.yml`) runs **only on manual dispatch**
(`workflow_dispatch`). After review, a maintainer re-runs it on GitHub before
merging.
A dispatch can only run a branch of this repository, so a pull request from a
fork is copied to a `ci/pr-<N>` branch first, once the maintainer has read
the diff (the workflow runs the pull request's code, `build.rs` included):

```bash
gh pr checkout <N> --branch ci/pr-<N>
git push origin ci/pr-<N>
gh workflow run ci.yml --ref ci/pr-<N> -f platform=all
gh run watch
git push origin --delete ci/pr-<N>
```

A pull request from a branch of this repository is dispatched on its own
branch the same way. The workflow holds no secrets and a read-only token, so
there is nothing for a pull request's code to take, and nothing it runs
reaches AWS, a real database or a real keychain: those checks
(`cargo xtask verify-real`, `verify --layer local|throwaway|real`,
`KURAMA_TEST_DB=1`) are run by a maintainer where that environment is.

## Pull requests

- Target `main`. It takes changes only through a pull request, merged with a
  merge commit: squash and rebase merges are off, so the branch's own commits
  stay in `main`'s history. A pull request needs a maintainer's approval, the
  PR checks, and a branch up to date with `main`.
- Keep one concern per pull request; a refactor goes on its own.
- Fill in the template: what changed, the impact from
  `cargo xtask impact`, the verification report, and what the fakes could not
  reach.
- By contributing you agree that your contribution is licensed under the
  [MIT License](LICENSE).
