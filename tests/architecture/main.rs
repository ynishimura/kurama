//! Architecture rules that used to live only in documentation.
//!
//! Each rule has a stable id; `rules.toml` next to this file names its
//! checks, documents, exemptions and fixtures, and `cargo xtask
//! architecture-audit` fails when this list, that registry and the tests
//! stop naming the same rules.
//!
//! ARCH-001 `src/domain` and `src/workflows` are pure: no I/O, no clock, no
//!          logging, no dependency on adapters or the shell.
//! ARCH-002 `src/ports` holds traits and plain data: no adapter or shell
//!          imports.
//! ARCH-003 `.agent/features/` matches the tree: every listed path exists,
//!          every scenario exists in `tests/scenarios/`, every source and test
//!          file belongs to a feature, every scenario is claimed by a feature,
//!          and every `depends_on` entry names another feature.
//! ARCH-004 Every error code is expected by a scenario, except the listed
//!          fallback.
//! ARCH-005 `#[path]` loads `<name>_tests.rs` files only; a production
//!          submodule is a file under its parent's directory.
//! ARCH-006 `src/shell`, `src/workflows` and `src/domain` keep typed errors
//!          typed: no `map_err(|e| X(e.to_string()))`, no `anyhow!("{e}")`.
//! ARCH-007 Every public function of `src/domain/functions` is exercised by a
//!          test in its own file or its `<name>_tests.rs`: tests move with the
//!          code.
//! ARCH-008 Every production file starts with a `//!` line; `cargo xtask map`
//!          prints it as the file's purpose.
//! ARCH-009 DuckDB calls use the reviewed entry points and the
//!          exception-catching Arrow bridge only.
//! ARCH-010 Scenarios that reach S3 require `test-fakes`, so they cannot
//!          contact a real S3 endpoint.
//! ARCH-011 Every clap argument explains itself in command help.
//! ARCH-012 Documented command examples are accepted by the real CLI parser.
//! ARCH-013 Adapters do not depend on the shell.
//! ARCH-014 No error variant prints its own cause twice under `{error:#}`.
//! ARCH-015 Every value-taking CLI argument says how its value completes.
//! ARCH-016 One implementation per concern: media type selection, and each
//!          progress line a person reads to decide something.
//! ARCH-017 The blocking file lock is only taken behind an async entry point.
//! ARCH-018 The agent contract names every flag and only real error codes.
//! ARCH-019 One file signs with SigV4; every other signature goes through it.
//! ARCH-020 A claim a development document lists under "Held by" names
//!          exactly one test function, found by parsing.
//! ARCH-021 (`cargo xtask deps`, tested in xtask) Every import that crosses
//!          into another feature's files is declared in `depends_on`.
//! ARCH-022 xtask: every file starts with a `//!` line, and only `board.rs`
//!          starts `gh`, so the judgement of the other files never talks to
//!          GitHub.
//! ARCH-023 xtask's integration tests start the binary only through
//!          `xtask/tests/support/`, which points HOME, CARGO_HOME and every
//!          kurama override into the test's scratch directory.
//! ARCH-024 The TUI's view, update, layout, theme and components are pure.
//! ARCH-025 A child process is started only in the named files.
//! ARCH-026 Only `console.rs` rewrites a line in place.
//! ARCH-027 xtask starts cargo only through its cargo helper.
//! ARCH-028 Every section file of the agent guide is printed.
//! ARCH-029 The byte-budget table matches the rule it publishes.
//! ARCH-030 Scenarios that store a token require the fake token store.
//! ARCH-031 A hint is never invented when the lookup behind it finds nothing.
//! ARCH-032 Every derived hint is pinned by a scenario that reads it.
//! ARCH-033 An adapter never turns an anyhow chain into text.
//! ARCH-034 SQLite's C API is reached through the reviewed entry points only.
//! ARCH-035 A server statement is prepared in one place.
//! ARCH-036 A database failure never holds a bare `String`.
//! ARCH-037 A choice made per variant names every variant: per database
//!          engine, in `ErrorCode::classify` / `hint_of` and in
//!          `CliCommand::contract`.
//! ARCH-038 Every database engine has a real-database test.
//! ARCH-039 (`cargo xtask architecture-audit`, tested in xtask) This list,
//!          `rules.toml` and the tests name the same rules.
//! ARCH-040 Every allowlist entry exists, once, with a reason, is still
//!          needed, and a file allowed across a boundary names its test.
//! ARCH-041 Every rule that reads the Rust syntax states what the reading
//!          cannot see.
//! ARCH-042 (`cargo xtask verify-real`, tested in xtask) Every feature says
//!          how it is verified against the real thing, and a probe pairs each
//!          of its tests with the mock scenarios that pin the same contract.
//! ARCH-043 A SIGINT listener is registered before the work it stops.
//! ARCH-044 Only `terminal.rs` asks whether a stream is a terminal, outside
//!          the adapters.
//! ARCH-045 `README.ja.md` translates the current `README.md`: the same
//!          headings, code blocks and images, and a digest of each English
//!          section it translated.
//!
//! One file per group of rules; `support` is what they share.

mod allowlists;
mod cli;
mod database;
mod docs;
mod error_code_exemptions;
mod errors;
mod feature_map;
mod layers;
mod readme;
mod sigint_listener;
mod support;
mod syntax;
mod test_index;
mod xtask;
