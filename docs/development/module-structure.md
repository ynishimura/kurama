# Module Structure Guidelines

Conventions for organizing Rust modules in kurama. The current tree is not
copied here: run `cargo xtask map` for the file list of each feature, or
`fd -e rs . src` for the full tree.

## Files

- One port implementation, one command, or one workflow per file, under about
  500 lines. Split when a second responsibility appears, not at a line count.
- A module becomes a directory (`foo/mod.rs`) when it needs sub-modules
  (builder, error type, validator) or when its tests move to a separate file.
- Small related types stay together; do not create a file for a single enum.
- Each `mod.rs` re-exports its public types and carries a `//!` doc comment
  saying what the module is for.

## Tests

- Unit tests live in the same file under `#[cfg(test)] mod tests` so they can
  reach private items.
- When unit tests outgrow the file, move them to `<name>_tests.rs` beside it
  and include them with `#[cfg(test)] mod <name>_tests;` (see
  `src/shell/executor_session_tests.rs`).
- `tests/*.rs` are integration targets and use the public API only:
  `config_test.rs`, `env_script_test.rs`, `shell_integration_test.rs`,
  `architecture.rs` (layer rules and feature-map validation) and
  `tests/scenarios/` (runtime verification through `tests/support/mod.rs`).
- Every new `.rs` file must be listed in a feature in `.agent/features/`;
  `tests/architecture/` fails otherwise.

## Layers

Dependencies point inward: `shell` -> `adapters` -> `ports` -> `domain`, and
`workflows` sits beside `domain` (pure). `domain` and `workflows` must not
import `adapters`, `shell`, `tokio`, `std::fs`, `std::env`,
`std::process`, `reqwest`, the AWS SDK, `keyring`, `tracing` or the clock
(`Utc::now`). `ports` must not import `adapters` or `shell`, and `adapters`
must not import `crate::shell`. All three dependency rules are tested.
The offline OpenAPI loader in `adapters/openapi` owns local-file reads,
home/cache paths and parse/normalization; `shell/spec_loader` owns HTTP
orchestration and API-specific errors.

## Refactoring

Run `cargo xtask verify all` before and after. The reports under
`target/agent/` must show the same STS, federation, `op` and browser call
sequences, exported variables, exit codes and `error[...]` / `hint:` lines
(the report lists them per scenario; diff the two reports).

A refactor is done when it removes something it names: a duplication, a
special case, lines. Before adding structure, check it against these tests:

- A trait or a generic parameter needs at least two implementations that
  exist today and differ in most of their body. Two variants of an enum and
  a `match` are the default.
- A parameter that only selects which output a function produces (an
  `Option`, a `bool`) means two functions.
- A method that only forwards to a field of a crate-private struct is not
  written; the field is `pub(crate)`.
- A value the configuration validates is converted once, where the file is
  read (`Config::parse`); no later step re-validates it or returns a
  `Result` for it.
- Tests move with the code they exercise, and `#[path]` is for
  `<name>_tests.rs` only (`tests/architecture/` checks both).
