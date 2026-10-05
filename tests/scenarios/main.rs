//! Runtime verification scenarios: the real binary, a fake STS and
//! federation endpoint, a fake 1Password CLI, a fake browser and an isolated
//! HOME. See `tests/support/mod.rs`.
//!
//! Naming: `<feature>_<behavior>`; the `ucNN_` prefix on older scenarios
//! numbers use cases of the original product spec, which is not in this
//! repository. `cargo test --test scenarios <prefix>` runs one scenario;
//! `cargo test --test scenarios -- --list` is the scenario map. Each scenario writes
//! `target/agent/scenarios/<name>.json`. Scenarios that use the session
//! cache need `--features test-fakes` (`cargo xtask verify` passes it).

//! One file per feature; this list and `support` are all they share. The
//! cases declared under `tests/cases/` are the tests `cargo xtask
//! generate-cases` writes into `cases_generated.rs`, one per file, included
//! at the end.

#[path = "../support/mod.rs"]
mod support;

mod agent_guide;
mod api_client;
mod api_explorer;
mod cli_entry;
mod config;
mod database;
mod local_analytics;
mod mcp;
mod oauth;
mod preset;
mod s3_explorer;
mod shell_integration;
mod tui;
mod verification_harness;

include!("cases_generated.rs");
