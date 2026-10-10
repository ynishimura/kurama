# Prevention layers

What catches a mistake an agent makes, where, and how that layer is measured.
A layer is listed here with the mistake it catches and the run that showed it
fail on one; a layer that has only ever passed is not evidence of anything.

## The layers

| Layer | Catches | Where it runs | How it is measured |
| --- | --- | --- | --- |
| Runtime scenarios (`tests/scenarios/`) | A changed call sequence, exit code, `error[...]` / `hint:` line, stdout content, a secret on disk | `branch-check` (affected), `check` (all) | `coverage` gate: every scenario the feature map claims wrote a report |
| Architecture rules (`tests/architecture/`) | A layer importing what it may not, a second implementation of a one-place concern, an untyped error, an unreviewed FFI call, a stale allowlist | `cargo test`, both gates | `cargo xtask architecture-audit`: per rule, its detectors, fixtures and exemptions |
| Rule registry (`tests/architecture/rules.toml`) | A rule, a test or a list entry that exists on one side only; an id used twice | `branch-check`, `check` (`architecture-registry`) | Its own unit tests (ARCH-039) |
| Fixtures of the rules | A detector that stopped detecting, or started refusing allowed code | `check` (`architecture-fixtures`), `architecture-audit --run-fixtures` | Violation detection rate and allowed-code pass rate |
| Mutation testing (`cargo xtask mutate`) | A test that passes for the wrong reason on the lines a branch changed | `branch-check` | Surviving mutants |
| Feature map and imports (`cargo xtask deps`) | An import across features that `depends_on` does not declare, so `impact` would not select the scenarios it affects | `cargo test` of xtask (ARCH-021) | Undeclared edges |
| Real-environment evidence (`cargo xtask verify-real`, `.agent/real/`) | A fake whose contract is wrong: every mock test passes and the real service answers otherwise | `branch-check` (`real-verification`) for a changed feature with a probe; CI (`--check`) | `verified` / `unverified` / `failed` per feature, each contract paired with its mock scenarios |
| Issue declarations (`conflicts`, `scenarios-check`, `issue-check`) | Two branches editing the same files; a declared scenario that does not exist | Before and after a branch | The answer names every pair and every missing scenario |

What no layer here catches is left to review: whether a reason an exemption
gives is right, whether a test proves the claim it holds, and whether a rule
reads what its author meant. The audit report lists what it does not measure
under "Not measured".

## What the syntax rules read

ARCH-001, 002, 006, 009, 013, 019, 024, 025 and 037 read the Rust syntax
(`tests/architecture/syntax.rs`): a nested, aliased or multi-line `use`, a call
split over lines and `Command::new` under any import are seen, and a comment,
a string or test-only code is not a violation. Names are not resolved beyond
the file's own `use` items; the registry states this for each of them and
ARCH-041 holds that statement to the reader's.

## Seen to fail

Each new layer was broken on purpose and the failure read before it was
trusted. Recorded 2026-09-23.

| Layer | What was broken | What failed |
| --- | --- | --- |
| Registry | A check removed from `rules.toml`; an id added only to the list in `main.rs` | `architecture-audit`: "a #[test] that no rule names", "in the list ... but not in the registry" |
| Held by resolution | A claim pointed at the production function `sign_request`; every attribute taken for a test attribute | `every_published_claim_names_a_test_that_exists`; both ARCH-020 fixtures |
| Allowlists | `FILES_THAT_SIGN` pointed at a renamed path; the TUI's own `Command::new("open")` put back | ARCH-040 ("does not exist"), ARCH-025 |
| Syntax rules | `use crate::{adapters::config}` in `src/domain`; a `map_err(|error| X(error.to_string()))` over four lines in `src/shell` | ARCH-001, ARCH-006 (the text checks passed both) |
| Fixture runner | `syntax::names` made to match no segment sequence | Detection rate 13 / 14, the miss attributed to ARCH-001's fixture |
| Fixture runner | A fixture source that does not parse | Reported as "failed for another reason", not as a miss |
| Contract matches (2026-10-11) | A `_ =>` put back after the explicit `DataError` arms of `classify`, and after the last arm of `CliCommand::contract`; a `_ =>` arm in a per-engine match of `server.rs` | ARCH-037 named each line |

Holding the allowlists to their files also found two gaps in rules that had
passed for months: the spawn check could not see five of the seven allowed
files (a `std::process::{...}` group, `tokio::process`), and the TUI home
screen started a browser that inherited stdin with no test.
