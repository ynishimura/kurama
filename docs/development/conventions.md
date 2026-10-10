# Conventions, invariants and Definition of Done

The full text, with the reasons, of what AGENTS.md lists in one line each.

## Invariants

1. The keychain is a cache, never a requirement. macOS grants access per
   binary signature, so a rebuilt kurama cannot read entries an earlier build
   wrote. A token that cannot be stored is still returned and used, an
   unreadable stored token counts as none, and an unreadable MFA session is
   fetched again; `cargo xtask install-signed` is what makes the grant stick.
2. Role credentials and tokens never touch a file. Both go to stdout
   (`env`: export script or JSON; `token`: the bearer token), to the
   wrapper's temp file named by `KURAMA_ENV_SCRIPT` (mode 600), or into the
   environment of the command `kurama exec` replaces itself with; an OAuth
   token otherwise lives in the keychain, and the credential of a
   `kind = "token"` source lives nowhere -- it is read from its reference
   each time, so there is nothing to cache and nothing for `logout` to
   remove. The harness checks every scenario for
   role credentials and tokens on disk and lists the files written.
3. stdout carries only data: the export script, a JSON document, the `status`
   table, the API response, or the output of the command run by `exec`.
   Progress, prompts, logs and errors go to stderr. No ANSI sequences when
   piped. A response body `kurama api` prints without `--json` / `--jq` is
   the server's bytes in a pipe; on a terminal a JSON body is re-indented
   (`indent_json`), which changes its whitespace and nothing else.
4. Secrets never appear in `Debug`, `Display` or logs. Credential and token
   types derive `Zeroize` and redact `Debug`; `Authorization` headers are
   masked in `--dry-run` and `-v`.
5. No unexpected STS, federation, token endpoint, API, 1Password or browser
   calls. Scenarios pin the exact call sequence, the signing key, MFA fields,
   policy ARNs, grant types and bearer tokens.
6. Every failure ends as one `error[CODE]: message` line, an optional `hint:`
   line, and exit code 1 (tool), 2 (usage), 3 (human action), 4 (the remote
   side rejected: AWS, an authorization server, an HTTP status outside 2xx).
   `data --json` instead emits one structured stderr error document, with
   the same exit categories, typed causes and hints; the harness rejects
   unclassified `INTERNAL` data errors and extra JSON-mode logs.
   The message carries no newline: what to do about it goes in the hint. The
   harness checks this for every scenario, whatever else it asserts, and
   rejects repeated error causes in both text and JSON messages.
   Without a terminal nothing prompts and nothing waits for a person: a grant
   that needs one exits 3 with `hint: run \`kurama login <source>\``, and the
   1Password CLI is killed at `[onepassword] timeout` rather than left waiting
   for a biometric answer.
7. `kurama init zsh` and `agent` never read configuration or
   the AWS environment: the first runs at every shell start, and the
   agent contract is what an agent reads to write or fix the configuration.
   Whether each source can be used now is `kurama status --ready`, which
   reads what `status` reads (config.toml, `~/.aws/config`, the session
   cache, the token store) and nothing more.

## Conventions

- A scenario's source lives in the file of the feature it is mainly about
  (`tests/scenarios/<feature>.rs`, or `tests/cases/<feature>/` for a TOML
  case), and every other feature that the scenario verifies still lists it
  in its own `scenarios`. `errors` and `shell-integration` claim scenarios that are
  mainly about another feature, so they have no file of their own. A fixture
  more than one feature's scenarios read lives in `tests/support/configs.rs`,
  not in one of their files.
- Scenario names: `<feature>_<behavior>`. The `ucNN_` prefix on older
  scenarios numbers use cases of the original product spec, which is not in
  this repository. Scenarios that need the file session cache carry
  `#[cfg_attr(not(feature = "test-fakes"), ignore)]`.
- A fake answers with what it was given, never a constant. A fake that
  returns the same value for two different inputs cannot tell them apart, so
  every assertion on it passes for the wrong reason: two review rounds on
  this repository found the same scenario tautological twice for exactly
  that reason. `tests/fakes/op` echoes the token it received into the secret
  it prints, so a scenario can say which source the token came from.
- Before trusting a new assertion, break the code it covers and watch it
  fail. `cargo xtask mutate` does this mechanically for the current diff.
- Names say verb + object (`generate_export_script`, `classify_sts_error`).
  Avoid `handle`, `process`, `util`, `helper`.
- One port implementation or one command per file, under about 500 lines of
  production code. Tests live in the same file under `#[cfg(test)]`; when a
  file grows past the limit, move the tests to `<name>_tests.rs` first, and
  split production code only along a seam that already exists (a type and
  the functions on it), never through a `#[path]` child module.
- Every production file starts with a `//!` line that says what it is for
  (`tests/architecture/` checks it; `cargo xtask map` prints it).
- Errors use `thiserror`. Keep typed errors in the `anyhow` chain with `?`
  or `.context(...)`, never as text (`anyhow!("{e}")`,
  `map_err(|e| X(e.to_string()))`), or `ErrorCode::classify` cannot see
  them; `tests/architecture/` checks `src/shell`, `src/workflows` and
  `src/domain`, and adapters wrap foreign errors with a `#[source]` field.
  A new failure kind gets a code, an exit code, a hint if a person must
  act, and a scenario (`tests/architecture/` enforces the scenario).
- Dead code is a build error, `pub` items included: `src/lib.rs` publishes
  `adapters`, `domain`, `ports` and `workflows` for the integration tests and
  doctests, which hides their `pub` items from rustc's lint, so the static
  checks compile the library once more with `--cfg dead_code_audit`, which
  makes those modules crate-private. An item only tests use is
  `#[cfg(test)]`. Unused dependencies fail `cargo xtask check` when
  `cargo-machete` is installed.
- `Cargo.lock` is tracked. Add dependencies with `cargo add`; never edit
  versions by hand. Rust 1.95.0 is the minimum.
- Edition 2024, which is what makes rustdoc compile every doctest into one
  binary instead of linking the 150MB rlib once per example: a documented
  sample costs a fraction of a second, so write it. It is also why an
  environment write in a test goes through `adapters::utils::test_env`, where
  the one `unsafe` and the argument for it live.
- An argument that takes a value says how it completes: a `ValueHint`, a
  `value_parser([..])` or a completer. `ValueHint::Other` is how it says the
  value cannot be completed, so silence is a decision someone wrote down and
  not the default nobody noticed (`tests/architecture/`).
- Several projections of one type destructure it without `..`, so a new
  field stops the build instead of quietly missing from one of them
  (`Operation` in `spec_output.rs`, `Schema` in `api_spec.rs`).
- A choice made per variant lists every variant, with no `_ =>`: a new
  error variant gets the code and hint someone chose in
  `ErrorCode::classify` / `hint_of`, and a new subcommand its output kind and
  audit entry in `CliCommand::contract` (ARCH-037). An invariant violation
  maps to `INTERNAL` by name. A variant list is `strum::VariantArray`, never
  written by hand (`ClientKind::VARIANTS`, `DbEngine::VARIANTS`, the TUI
  tabs, whose digit keys follow `Tab::VARIANTS.len()`).
- Mutually exclusive state is one enum, not two fields that can disagree
  (`JqPanel`). The bounds external input is cut to live in
  `domain/types/limits.rs`, where a test destructures them exhaustively and
  drives the same function production cuts with (`InputListLimits::split`).
- A sentence in a development document that promises behavior ("once",
  "before", "only") is listed in a `| Claim | Held by |` table with the test
  that fails when it stops being true; `tests/architecture/` checks the
  test exists, and the claim is listed only after the test was seen to fail
  against the regression.
- A fact two output modes report is derived once, in `src/domain/functions`,
  and both call it: `parquet_advice` is what keeps the `--describe` text line
  and `meta.next_actions` from disagreeing. A rule a document publishes is
  derived the same way: `DataRequest::spends_byte_budget` decides, and
  `tests/architecture/` checks the table in `docs/development/data.md`
  against it.
- A query kurama wrote itself goes through `result::collect_complete`: the
  display limits are what a person asked to see, and must not silently
  shorten an internal answer. A number that could not be read is `None`,
  never `0`.
- `Observed` is what a run produced; `Seeded` is what the harness put in
  place. A check built only from `Seeded` tests the harness's own arithmetic
  and passes whatever the binary does.
- A filter is an input that can select nothing, and `cargo test` exits 0 when
  it matches no test. It can also select too little, which looks the same: the
  feature map derives its filters from the files a feature claims so that
  neither is possible by hand. So an exit code is never what says a run verified
  something: `verify` and `check` compare the scenarios they asked for against
  the reports the run wrote, and a name no test carries stops the run instead
  of emptying it. The scenario names have to be converted between the two
  spellings the repository uses -- the feature map's and the test binary's
  module-qualified one -- in one place (`scenario_name`), because they drifted
  apart once and every gate kept passing.
- Run `cargo fmt --all` before finishing.

## Definition of Done

```text
[ ] Existing implementation searched (map, search, scenarios); no duplicate capability added
[ ] cargo check --all-targets --features test-fakes clean: every caller updated
[ ] Tests written first; cargo test --locked --features test-fakes -- <filters> passes
[ ] User-visible change has a scenario; cargo xtask verify affected passes
[ ] Scenario checks confirm: no secrets on disk, no error logs, no extra STS/op/browser calls, clean stdout
[ ] TUI change: cargo xtask tui-check passes, target/agent/tui-report.md screens read, snapshots accepted only after review
[ ] New failure paths have an error code, exit code, hint and scenario
[ ] Changed SQL/FFI/limit guards have regression tests whose detection is checked with targeted mutations
[ ] Every new scenario and harness check is shown to fail against a deliberate regression before it is trusted
[ ] The regressions `cargo xtask mutate` cannot write are put in by hand and watched to fail: the value of a `const`, an argument swapped at a call site, one arm of a `match` whose other arms still read plausibly
[ ] A check built only from `v.seeded` values is rewritten to compare against `v.observed`: it tests the harness, not the binary
[ ] New CLI variants, DuckDB entry points and limit fields update the matching exhaustive checks and boundary tests
[ ] cargo xtask check passes (fmt, clippy -D warnings, all tests, architecture rules, machete)
[ ] .agent/features/, AGENTS.md, README.md and README.ja.md updated when structure or behavior changed (ARCH-045 names each English section README.ja.md has not caught up with, and the `<!-- en: ... -->` marker to set once it is translated)
[ ] PR description contains target/agent/verification-report.md
```

## Runtime verification boundaries


What the scenarios cover is what `cargo xtask verify-matrix` classifies; what
was run against the real services is the evidence under `.agent/real/`; what
only a person can run is `tests/cases/needs-human.toml`. Read them before
claiming a feature is complete.
