# Feature map, scenarios and cases

What `.agent/features/`, `tests/scenarios/` and `tests/cases/` hold, and how the verification layers run a case.

`.agent/features/<feature>.toml`: hand-written feature map, one file per
feature holding a single table named after the file, validated by tests and
`cargo xtask doctor`. The `cargo test` filters are not hand-written: they are
derived from `files`, because a substring over test *names* cannot be checked
against a list of *paths*, and one that selects a single test of a file reads
exactly like one that selects them all. `tests` holds only the filters no
path can imply -- an integration-test binary under `tests/` names its tests
at the top level -- and `doctor` fails a declared filter the paths already
cover. xtask needs none: `xtask/src/<module>.rs` is `<module>::`, `main.rs`
is `main_tests::`, and every test of `xtask/tests/<name>.rs` starts with
`<name>_`, which a test enforces. `depends_on` names the features whose code a feature
runs, and `cargo xtask deps` holds it to the imports. `impact` walks a
changed `src/` file to every file that imports it, stopping at the
registration hubs (`cli-entry`, `errors`), then takes one `depends_on`
step, because a scenario runs the code its feature declares; a changed
file outside `src/` follows `depends_on` transitively. So a change to a
leaf selects its neighbours, and a change to shared code still selects
every scenario that goes through it.

`tests/scenarios/<feature>.rs` + `tests/support/mod.rs`: runtime
verification against the real binary, one file per feature with
`main.rs` holding only the module list. The harness checks every scenario
for role credentials on disk and for error logs in successful runs. Each
scenario writes `target/agent/scenarios/<name>.json`, with the
`combination` the scenario verifies (a TOML case declares one; a Rust
scenario has none) and the `evidence` it ran against (`fake`).

`tests/cases/<feature>/<id>.toml`: a scenario declared as data
(`tests/support/cases.rs`): what the user gives (`[input]`), how the fakes
answer (`[fakes]`), and what is expected (`[expect]`, one key per
`expect_*` of the harness), plus the `combination` it verifies.
`cargo xtask generate-cases` writes one `#[test]` per file into the
committed `tests/scenarios/cases_generated.rs`, which the scenario binary
includes, so a case
lists, filters, reports and mutates like a Rust scenario; the directory is
its feature, so the feature map never lists it. A check a case fails is
named by its key (`expect.exit_code: exit code is 4: observed exit code
Some(2)`). `[expect]` reads the last run and a `[[expect.runs]]` block
with `run = N` run `N` of a case with `then_run`, with the same keys;
what spans every run -- the secrets read, the files written, the env
script -- reads the same wherever it sits. An `api_calls` that names no
`authorization` says the requests carry none, unless every request is
checked for it under `calls`, or a `signing_region` says it is a
signature (`tests/support/expect_detail.rs` holds the request-by-request,
any-order request-count, STS-call, description, error-line, table and
file-pattern checks). `{home}` in an argument or stdin is the sandbox
HOME, which the harness fills in once it exists. A
case says nothing a Rust scenario
could not: an expectation with no `expect_*` gets the method first. Rust
is for what data cannot say -- TUI/PTY, completion sessions, parallel and
interrupted runs, a seeded description cache, corrupt caches.
`[local]` is the same case against the databases
`db-up` starts (its own `config`, `combination` and `expect`);
`cargo xtask verify --layer local` runs it with `KURAMA_CASE_LAYER=local`
and its report says `"evidence": "local"` under `scenarios-local/`.
`[throwaway]` is the same case against AWS resources the run creates and
deletes: `stacks` names them (`iam-api`, `bastion`, `rds-iam`, the
templates under `tests/api/` and `tests/db/`), its `config` reads their
outputs through `{iam_api_url}`-style placeholders and the profile through
`{throwaway_profile}`; `cargo xtask verify --layer throwaway` runs it with
`KURAMA_CASE_LAYER=throwaway` against AWS itself (the person's `~/.aws`
files and `[onepassword]` section, no fake endpoint, the sandbox HOME),
and only after `--yes`.
`[real]` is the same case against the real service, with `requires` naming
what the person's environment has to hold; `cargo xtask verify --layer real`
runs it without the fakes on the copied daily configuration, and a
feature whose `real` says `cases` has those cases as its `verify-real`
probe. What only a person can run is declared in
`tests/cases/needs-human.toml` with what to prepare.
