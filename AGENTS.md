# AGENTS.md

kurama is a credential switcher and API client (Rust CLI/TUI, macOS, zsh): it
resolves an AWS profile to temporary credentials through STS, with MFA from
1Password, or an `[auth.*]` source to a token -- from an OAuth grant
(`kind = "oauth"`) or from the secret store an issued API key already lives
in (`kind = "token"`) -- exports them into
the current shell, calls `[api.*]` profiles with the token, and explores an
API's OpenAPI description (`--ops`, `--describe`, operation targets, the
explorer TUI). This file is the entry point for any coding agent. Read it, run `cargo xtask map <feature>`
for the feature you touch, and read only the files it lists. Do not read the
whole tree.

## Commands

| Command | Purpose |
| --- | --- |
| `cargo xtask doctor` | Toolchain, metadata, test filters, scenario names and fake environment are ready |
| `cargo xtask map [FEATURE]` | Feature map: entry file, files with their `//!` purpose line, test filters, runtime scenarios |
| `cargo xtask search <PATTERN>` | Public symbols whose name contains PATTERN, with file:line and doc |
| `cargo xtask impact [--base REF]` | Changed files -> features and the features that depend on them -> test filters -> scenarios (JSON) |
| `cargo xtask conflicts <ISSUE>...` | Before starting: whether those issues can be implemented at the same time, from the `Affects:` line each issue body declares (`--json`) |
| `cargo test --locked --features test-fakes -- <filters>` | Unit and integration tests for one feature (`tests` from `map`) |
| `cargo xtask verify <all\|affected\|FEATURE\|PREFIX>` | Run runtime scenarios, write `target/agent/verification-report.{md,json}`; a run that reports fewer scenarios than it asked for fails on the `coverage` gate |
| `cargo xtask tui-check [--update-snapshots]` | After a TUI change: the static checks, render snapshots + UI contract, PTY scenarios; writes `target/agent/tui-report.md` with every captured screen |
| `cargo xtask mutate [PATH ...]` | Mutation testing (cargo-mutants) of the lines the branch changed under `src/` and `xtask/src/`, each kurama mutant running the unit tests and scenarios of the features that own its file and the scenarios of those that own a file it imports, or of whole files with PATH (the run prints both counts first): a mutant that survives names a test that passes for the wrong reason; it prints the mutant count and an estimate before it starts, and times a mutant out at twice the unmutated tests; writes `target/agent/mutation-report.md`. It replaces function bodies and negates conditions, and writes none of the shapes `cargo xtask mutate --help` lists, so a green run is not a claim about those |
| `cargo xtask db-up` / `db-down` | Start (or remove) the PostgreSQL 17/18 and MySQL 8.4 that `KURAMA_TEST_DB=1 cargo test --features test-fakes --test real_db` reads; needs Docker, and the gate never runs it |
| `cargo xtask check` | The CI gate: the static checks, every test; writes the report; then caps `target/` at 12GB |
| `cargo xtask sweep` | Bring `target/` under 12GB: the incremental caches first, the oldest artifacts only if that is not enough (`target/agent` stays). `mise run sweep` runs this |
| `cargo xtask install-signed` | Build, code sign with a stable identity and install `~/.cargo/bin/kurama`, add its `cdhash` to the partition list of every keychain entry kurama reads (asking for the login keychain password once), so no build asks again; `--as NAME` installs the same signed build as `~/.cargo/bin/NAME` and leaves `kurama` alone |
| `cargo xtask scenarios-check <ISSUE>` | After implementing: whether the scenarios the issue declares under its `## Scenarios` checklist exist, which ones this branch adds without declaring them, and which no feature claims (`--base REF`, `--json`) |
| `cargo xtask branch-check` | The branch-side gate: the static checks, the tests of the affected features, `verify affected`, then `mutate` on the lines it changed under `src/` and `xtask/src/`. `pre-push` runs this from a feature branch and `check` from `main`. At most `KURAMA_XTASK_GATE_SLOTS` (default 2) run at once across the clone's worktrees, static checks included; another waits for a slot and names the processes and worktrees holding them |
| `cargo xtask ready` | The issues an agent may start now: open, unblocked, nobody on them, not a `tracker`, not `needs-human`, and not editing what a running branch edits. Each offer names the other offers it cannot run beside, and `--agents N` picks N that can all run at once; a blocked issue is listed with each open blocker and where it stands, tracker or not; `batch` lists the serial offers closed over one feature, with the `worktree add` that takes them together (`--all`, `--json`) |
| `cargo xtask worktree add <ISSUE>... [SLUG] [--install]` / `list` / `remove <ISSUE>` | One worktree per issue, or per batch of issues named after the first (refused whole, naming each refusal, when one is refused), branched from `main`, and the issues move to `In progress`; `add` clones the main checkout's `target/` with `cp -c` (APFS blocks shared until rewritten, so DuckDB is not rebuilt; without one it says so and starts empty) less `target/agent/`, whose reports are the main checkout's, copies the daily config to `.kurama/config.toml` in the worktree and, with `--install`, installs its signed build as `kurama-<ISSUE>`, and prints how to use both; `list` prints every `target/` and their total (which counts a clone's shared blocks in each), `remove <FIRST>` deletes the worktree with its `target/` unless it holds unpushed commits, and puts every `In progress` issue it was added for back to `Backlog` unless the branch was merged into `main` |
| `cargo xtask issue-check <FILE\|-\|ISSUE>` | Before posting: whether an issue body's `Affects:` line resolves and every `## Scenarios` item is shaped like a scenario name; a file or stdin reads nothing from GitHub, and labels and `blocked_by` are not checked |
| `cargo xtask claim <ISSUE>` / `claim --release <ISSUE>` | Say on the board that an issue is taken without making a worktree (one agent working issues one after another in the main checkout), refused for the reasons `worktree add` refuses; `--release` puts an `In progress` issue back |
| `cargo xtask board set-status <STATUS> <ISSUE>...` | Move many issues to one status in one request, for a stocktaking; it writes what it is told and names each issue it could not write |
| `cargo xtask deps` | After adding an import: the imports that cross into a feature `depends_on` does not name, with the line to add, and the declared edges no import crosses (`--json`) |
| `cargo xtask architecture-audit` | The architecture rules of `tests/architecture/rules.toml` (stable `ARCH-NNN` ids) against their tests, fixtures, exemptions and the list in `tests/architecture/main.rs`; fails on a name that resolves to nothing or a test no rule names, and writes `target/agent/architecture-report.{md,json}`, which counts registrations and names what it does not measure. `--run-fixtures` runs every registered fixture and reports the violation detection and allowed-code pass rates; a fixture that misses, refuses, fails for another reason or does not run fails it. `branch-check` runs the static audit, `check` the one with fixtures |
| `cargo xtask verify-real <FEATURE>` / `--check` | Run the real-environment probe a feature declares under `real` in `.agent/features/` (only when the variables it requires are set; `throwaway` access only for an allowlisted environment), or, for a feature whose `real` says `cases = "..."`, its `[real]` cases through `verify --layer real <FEATURE>`, and write `.agent/real/<FEATURE>.json`, to commit with the change; `--check` holds that evidence to the current files, fixtures and a credential scan without running anything. `branch-check` fails a changed feature with a probe and no fresh verified evidence; a `pending` feature is named as unverified. `database` is probed through `cargo xtask db-up` |
| (every checking command) | `check`, `branch-check`, `verify`, `tui-check` and `mutate` first run the static checks -- the generated case tests against `tests/cases/`, `cargo fmt --check`, `cargo clippy -D warnings` over every target with `test-fakes`, the library compiled with `--cfg dead_code_audit`, `cargo machete` when installed -- and stop at the first failure, before any test is built; `COMMANDS` in `xtask/src/main.rs` decides which commands do |
| `cargo xtask inventory` | What the binary has -- every command and option from the clap definition, every config key with its kind and the values an enum key accepts from the config types, the secret schemes, the client kinds -- read from the hidden `kurama inventory` (which, like `agent`, reads no configuration), written to `target/agent/inventory.{json,md}` with the registered scenarios no report was written for; `--from FILE` reads a saved document. It counts the coordinates `verify-matrix` classifies and leaves the classifying to it |
| `cargo xtask verify-matrix` | Every value `kurama inventory` enumerates (a command, an enum config value, a value a CLI option enumerates, a secret scheme, a client kind), each as one coordinate `dimension=value`, against the cases under `tests/cases/` (a case classifies every coordinate its `combination` names), `tests/cases/not-applicable.toml`, `tests/cases/needs-human.toml` and the reports the last `verify` wrote (under `CARGO_TARGET_DIR` when set; a layer's reports count only when its `verification-report-<layer>.json` was written on the current HEAD, clean or dirty as the tree is now, and are otherwise shown as stale); runs nothing itself. Writes `target/agent/verification-matrix.{json,md}` -- one row per case, scenario, declaration and uncovered coordinate, in exactly one of `PASS`, `FAIL`, `UNCLASSIFIED`, `UNEXECUTED`, `UNVERIFIED_REAL` (a case with `[real]` and fake evidence only; a case with no fake report is `UNEXECUTED`), `NEEDS_HUMAN`, `NOT_APPLICABLE`, with the failed checks and a `cargo xtask verify <name>` to reproduce -- and fails on `FAIL`, on any `UNCLASSIFIED` coordinate, and on a declaration that names other than one coordinate or a value the inventory does not enumerate on a dimension it does. A dimension only cases name and that a single case uses is listed as a possible typo, and a value a case names that the inventory does not enumerate (a rejected value, a refinement) is listed, neither gated. `check` and `branch-check` run it after `verify`; `--from FILE` reads a saved inventory |
| `cargo xtask verify --layer local [all\|FEATURE]` | The local layer: `db-up`, the cases under `tests/cases/` that declare `[local]` run against those databases with `KURAMA_CASE_LAYER=local` (the same generated tests, the layer's own `config` / `args` / `expect`), then `db-down` whatever happened (databases that were already up are left up); reports with `"evidence": "local"` under `target/agent/scenarios-local/` and `verification-report-local.{md,json}`. Without Docker it runs nothing and writes `scenarios-local/not-run.txt` with the reason, which `verify-matrix` shows on each such case as `UNEXECUTED` instead of failing |
| `cargo xtask verify --layer throwaway [all\|FEATURE] [--profile NAME] [--yes]` | The throwaway layer: the disposable AWS stacks the cases that declare `[throwaway]` name (`xtask/src/throwaway_stacks.rs`: `iam-api`, `bastion`, `rds-iam`, `dsql`, `s3-data`, each a `*-up.sh` / `*-down.sh` under `tests/api/`, `tests/db/` or `tests/s3/` with its cost and duration from the script header), created in `--profile` (default `kurama-sandbox`) through `kurama exec`, the cases run against their outputs with `KURAMA_CASE_LAYER=throwaway` and `"evidence": "throwaway"` under `target/agent/scenarios-throwaway/`, then every stack the run created deleted and its deletion checked with `describe-stacks`, whatever happened in between; each stack is named after the run (a stack AWS already knows is refused, never taken over), a `cleanup:<stack>` gate per stack says created-and-deleted or left, with the exact command that removes it, and `cleanup.sh` next to the reports lists every `down` with the run's names and configuration from before the first `up`, stays while a stack may be left, and stops the next run until it has been run. Without `--yes` it prints the stacks and the estimate, writes `scenarios-throwaway/not-run.txt` ("approval needed" and the command to run) and fails; a gate never passes `--yes`, because a stack bills the account |
| `cargo xtask verify --layer real [all\|FEATURE]` | The real layer: the cases that declare `[real]` run against the real services with `KURAMA_CASE_LAYER=real` -- no fake endpoint, the person's own PATH, `~/.aws` files and 1Password, the copied daily configuration (`KURAMA_CONFIG_PATH`, else `.kurama/config.toml`) as the base plus the layer's own sections, the sandbox HOME and file-backed stores so nothing reaches the keychain. Each case runs only when what it `requires` (`profile:`, `auth:` / `api:`, `keychain:`, `env:`) is there; the rest go to `scenarios-real/not-run.txt` by name with what to prepare, which `verify-matrix` shows on the row. The service account token comes from the keychain into the process environment and nowhere else; every report is scanned for a credential before the matrix may read it |
| `cargo xtask generate-cases` | Write `tests/scenarios/cases_generated.rs`, one `#[test]` per `tests/cases/<feature>/<id>.toml`, which `tests/scenarios/main.rs` includes; commit it. Rerun it when a case is added, removed or renamed: editing a case's content needs nothing, because each test reads its file through `include_str!`, and rebuilds only the scenario binary. The static checks and `doctor` fail first, before anything compiles, while the file does not match the cases |

The fake STS, federation, OAuth and API endpoints (wiremock), the fake `op`,
browser and `gh` (`tests/fakes/`; `xtask/tests/` runs `worktree` against the
fake `gh` and a scratch repository named by `KURAMA_XTASK_ROOT`), the sandbox HOME, the file-backed session cache
and token store (`--features test-fakes`) and the report writer all live
behind those commands. `cargo xtask --help` lists the xtask commands. `mutate`
needs `cargo-mutants` installed (`cargo install --locked cargo-mutants`), the
way the unused-dependency gate needs `cargo-machete`. Scope `mutate` with a
PATH: each mutant leaves a generation of artifacts in `target/`, and the
bundled DuckDB makes those generations gigabytes.

Run `mise trust` and `mise run setup` once per clone to install the pinned
Lefthook, cargo-machete and cargo-sweep tools and the `pre-push` hook, and to
build `xtask` itself: on a cold `target/` the first `cargo xtask map` spends
about two minutes compiling before it prints a thing, which is long enough
that the instruction above gets skipped. The hook
runs `cargo xtask branch-check` for a push from a feature branch and
`cargo xtask check` for one from `main`, then `mise run sweep`, and
stops a failing push. Two `check` runs queue on one lock in the clone's common
git directory instead of compiling at the same time.
`check` also caps `target/` at 12GB on its own: DuckDB is compiled from source
and Cargo keeps every generation. The cap drops the incremental caches first,
because the artifacts with the oldest modification time are the C++ crates
nothing changes, and evicting those made the next gate spend five minutes
rebuilding DuckDB. `cargo xtask sweep` is the same cap for a manual run.
Push from the
branch's own worktree after committing: the gate checks the current working
tree. GitHub Actions is manual only (`workflow_dispatch`), with a platform
choice and optional coverage. See `docs/development/setup.md` for setup and
manual CI commands.

## Layout

Every file starts with a `//!` line that says what it is for, and
`cargo xtask map <feature>` prints it next to each file. This section
describes the flows, not the files.

- `src/domain`, `src/workflows`: pure. No I/O, no clock, no logging, no
  dependency on adapters or shell. Enforced by `tests/architecture/`.
- `src/ports`: traits and plain data. `src/adapters`: I/O implementations
  (AWS SDK, 1Password CLI, keychain, files). `src/shell`: private module with
  the CLI, the TUI, `Runtime` (dependency container) and the effect
  interpreter; only `build_command`, `run` and `ErrorCode` leave the crate.
- CLI flow: `src/shell/cli/args.rs` (clap) -> `parser.rs` (`CliCommand`) ->
  `bootstrap.rs` (config, logging) -> `dispatch.rs` ->
  `commands/*.rs` -> effects -> `cli/executor.rs` -> `src/shell/executor.rs`
  (the only AssumeRole interpreter; the TUI calls it too) -> `src/adapters/*`.
  Inherited `AWS_*` credentials are cleared at the SDK boundary, in
  `src/adapters/aws/config_builder.rs`.
  `kurama login` on an AWS profile runs `src/workflows/mfa_login` through
  `src/shell/mfa_login_executor.rs`, which shares the GetSessionToken and
  TOTP-window helpers of `src/shell/executor.rs`.
- OAuth flow: `src/shell/cli/commands/source.rs` resolves a `<PROFILE>` to an
  AWS profile or an `[auth.*]` source (one namespace; a shared name is
  `CONFIG_INVALID`). `ApiRuntime` (`src/shell/api_runtime.rs`) runs
  `src/workflows/oauth_token` through `src/shell/oauth_executor.rs` for
  `login` / `token` / `env` / `exec` on an `oauth` source and for `api`;
  `ApiRuntime::call` adds the bearer token and retries once after a 401. A
  `kind = "token"` source (`src/domain/types/token_source.rs`) has no grant
  and no store: `ApiRuntime::ensure_credential` reads its `token` reference,
  `with_credential_header` puts the value in the `header` the source names
  through its `format`, and the request is sent once -- a retry would read
  the same value again. `AuthSource` (`src/domain/types/auth_source.rs`) is
  the enum both kinds resolve to, so each verb matches on it once. `call`
  instead
  signs the request with SigV4 when the `[api.*]` names an `aws_profile`
  (service and region from `--service` / `--region`, the profile, the host,
  then the AWS profile's region; the role credentials come from the same
  AssumeRole executor as `env`). Ports: `HttpClient` (reqwest), `TokenStore`
  (keychain service `kurama-token`), `SecretResolver` (every reference form).
  The files: `cargo xtask map api-client` and `cargo xtask map oauth`.
- Secret references: a configured secret (`[auth.*] client_secret`,
  `[auth.*] token`, `[db.*]
  username` and `password`) is one string, parsed once at deserialization by
  `src/domain/types/secret_ref.rs` -- `op://`, `aws-secrets://`, `aws-ssm://`
  or a literal, and a `<scheme>://` that is none of them is `CONFIG_INVALID`
  with its line rather than a password nobody notices. One `SecretResolver`
  implementation (`src/adapters/secret_resolver.rs`) dispatches on the variant
  to `src/adapters/auth/secret.rs` (`op item get`, or `op read` for a section or an attribute) or `src/adapters/aws/
  secret_store.rs` (`GetSecretValue`, `GetParameter`), and holds each secret it
  read for the process whichever backend answered, so `#username` and
  `#password` of one managed secret are one `GetSecretValue` and two fields of
  one 1Password item are one `op item get` -- one biometric prompt. AWS credentials come through `AssumedRoles`
  (`src/shell/aws_profile_credentials.rs`), the shared cache in front of the
  `kurama env` AssumeRole path, so a tunnel, an IAM token and every reference
  naming one profile assume its role once. A failure carries a typed
  `SecretFailure` the whole way -- through the pure OAuth workflow too -- and
  that is what picks `SECRET_UNAVAILABLE` / `SECRET_INVALID` /
  `SECRET_REJECTED` / `SECRET_FAILED`. Getting the credentials is not one of
  them: `SecretError::credentials` keeps the AssumeRole chain, so a wrong TOTP
  stays `STS_*` and exit 3 instead of becoming a reference that is malformed.
  The files: `cargo xtask map secrets`.
- Every `op` call goes through `src/adapters/auth/op_cli.rs`: it passes a
  1Password service account token (the environment wins, else the keychain
  entry named by `[onepassword] service_account_keychain`, read for `$USER`)
  and, when no terminal is attached, kills the CLI after
  `[onepassword] timeout` seconds, so an unanswered biometric prompt is
  `SECRET_UNAVAILABLE` (exit 3) instead of a hang.
- OpenAPI flow: `[api.*] openapi` (a URL or an absolute file) is loaded by
  `src/shell/spec_loader.rs`: a file is read as is; a URL is fetched through
  `ApiRuntime` (with the API's credential when `openapi_auth`, which the
  config restricts to the origin of `base_url`), cached under
  `~/.cache/kurama/openapi/`, reused for `[openapi] revalidate_after` seconds
  (default 3600; 0 checks every time), then revalidated with `ETag` /
  `Last-Modified`. A 304 restarts the interval by updating only the matching
  generation's metadata. Short per-entry write locks prevent an older 304
  from replacing a newer fetch; reads and completion do not create locks.
  An expired cached copy is used
  with a warning when the server is unreachable. `--refresh-spec` ignores
  the interval; files are always reread. Local-file reads,
  home/cache paths and byte parsing/normalization live in `adapters/openapi`,
  with no HTTP client or credential dependency. The document is
  normalized into an `ApiSpec` (`src/domain/types/api_spec.rs`) by the
  format its key names (`SpecFormat`): `openapi` by `functions/openapi.rs`,
  `discovery` (a Google Discovery Document, loaded the same way) by
  `functions/discovery.rs`, which rewrites it as OpenAPI 3 first,
  `graphql` (an endpoint path, introspected by a POST with the API's
  credential, or a `graphql_schema` file) by `functions/graphql.rs`; the
  `ApiSpec` is what
  `--ops`, `--describe`, operation targets with `-P` and the explorer TUI
  read. The files: `cargo xtask map api-explorer`.
- TUI (`src/shell/tui`): `theme.rs` (colors, styles, borders), `layout.rs`
  (breakpoints compact `< 100` / normal / wide `>= 140` columns, regions,
  modal placement), `components/` (Header, KeyHints, Tabs, panel, ProfileTable,
  DetailPane, KeyValues, OperationTable, Form, LineInput, TextView, Modal,
  ResultTable), `tea/view.rs` (what to show; pure) and `tea/update.rs`
  (state; pure) for the home screen, `explorer/view.rs` and
  `explorer/update.rs` for `kurama api <API>`, `database/view.rs` and
  `database/update.rs` for `kurama db <DB>`, `s3/view.rs` and `s3/update.rs`
  for `kurama s3 <S3>`. A screen never picks a color or a magic width itself;
  `tests/architecture/` keeps these files free of I/O, clocks and logging.
  `tea/runtime.rs` and `explorer/runtime.rs` redraw after an event only; the
  explorer runs its effects (the description, the call through
  `ApiRuntime::call`, `$EDITOR` with the terminal suspended, the clipboard,
  the browser, jq) and shows their results. jq input state and panels live in
  `explorer/jq_input.rs` and `jq_view.rs`; `domain/functions/jq_completion.rs`
  shares pure JsonShape traversal with CLI completion. Previews reuse the
  parsed response and the CLI jq evaluator with a bounded output count. Progress lines and logs go
  through `src/console.rs`, which holds them while the TUI owns the
  terminal. Details in `docs/development/tui-testing.md`.
  The database explorer (`database/runtime.rs`) is the one screen whose
  requests do not hold it: a spawned task owns the connection and the
  tunnel, takes one request at a time over a channel, and the screen reads
  the answer on the next key or tick, so `Esc` can stop a statement. Before
  the screen opens, `database/mod.rs` resolves the secrets, the tunnel and
  the first connection through `src/shell/db_connection.rs`, which the CLI
  shares with it (with the deadline-and-stop wait), so a failure there is
  the CLI's `error[CODE]`. Each request re-arms the stop flag and ends with
  `DbSession::end_request`, which rolls a server's read guard back; the next
  read opens it again.
  `terminal.rs` checks every entry point for interactive streams and
  `TERM != dumb`, is the one file outside the adapters that asks whether a
  stream is a terminal (a `KURAMA_AGENT` run counts as a pipe there: no
  TUI, the pipe's stdout bytes, no rewritten line), and applies `NO_COLOR` to the rendered buffer through
  the pure `theme::remove_colors` before the backend diff. Text and
  non-color attributes remain intact; keep environment reads out of views.
- A child process is started only in the files `tests/architecture/` names
  (`only_named_files_start_a_child_process`): getting stdio, reaping and
  deadlines right is the hard part, and each of those files is tested for
  its own shape. `op` always goes through `src/adapters/auth/op_cli.rs`.
- JSON clients: a subcommand that answers an agent with one JSON document on
  stdout and one JSON error document on stderr is a `ClientKind`
  (`src/shell/cli/client.rs`). That one table decides the `--kind` values of
  `agent` and `status`, which subcommand `main.rs` lets answer its own usage
  failures and with what code, and it carries what those subcommands share:
  reading a `--request` file or stdin, and projecting the envelope with
  `--jq`. The error document itself is `src/shell/cli/client_error.rs`.
  `api`, `status`, `env` and `token` have that error document under
  `--json` / `--jq` but no request contract: they are the other variants of
  `JsonErrorKind` in the same file, so `agent --kind` cannot list them, and
  `main.rs` answers their usage failures only when JSON was asked for.
  `CliCommand::leaves_stderr_to_json_errors` silences their progress lines
  and logs when stderr is not a terminal.
- Data analysis: `dispatch.rs` -> `commands/data.rs`, with typed plans and
  output in `data_plan.rs` / `data_render.rs`, bounded local/S3 enumeration
  in `adapters/data_inputs.rs` / `adapters/aws/s3_data.rs`, and ephemeral
  execution in `adapters/duckdb/`. SQL validation preserves CTE scope and
  declaration order; Arrow C interchange uses reviewed entry points and a
  small exception-catching bridge. Each bucket is read where it lives: a
  region the request names (`--region`, the workspace, `[s3.*]`, a regional
  S3 URL host) is used as it is, and otherwise the AWS profile's region only
  signs the first request, which S3 corrects once through
  `x-amz-bucket-region`; the later HEAD/LIST calls and that bucket's httpfs
  secret follow it, while AssumeRole stays in the profile's region.
  `max_source_bytes` bounds only what an operation reads end to end, so a
  metadata read and a Parquet scan are not refused for size; `--describe` on
  Parquet adds the footer facts (`duckdb/parquet_summary.rs`), an S3 run
  reports the requests and bytes it moved (SDK counters plus the engine's own
  HTTP log, `duckdb/transfer.rs`), a terminal gets the elapsed seconds, and a
  cancelled wait names the columns the statement referenced.
  Files: `cargo xtask map local-analytics`.
- S3 browsing: `dispatch.rs` -> `commands/s3.rs` (arguments in
  `s3_command.rs`), which plans the start (TARGET, the `[s3.*]` bucket and
  prefix, a cursor) before anything is called, assumes the role once and
  drives the pure `domain/functions/s3_scan.rs` over pages that
  `adapters/aws/s3_browse.rs` fetches through `s3_data::BucketClients`, so a
  bucket's region is followed the same way as for `kurama data`. A run stops
  at `--max-objects` entries, possibly inside a page; its cursor names the
  page's continuation token and how many entries of it were seen, so the next
  run asks for that page again and skips them. Files: `cargo xtask map
  s3-explorer`.
- Databases: `dispatch.rs` -> `commands/db.rs` (with no operation on a
  terminal: the explorer, `src/shell/tui/database/`), with the CLI and the typed
  request in `db_command.rs`, the contract and `status` rows in
  `db_contract.rs`, output in `db_render.rs`, and one engine per file under
  `adapters/database/`. `DbSession` is an enum and a `match`, not a port: the
  command is the only caller and nothing mocks a connection. A read is
  guarded three times -- the file is opened read-only, SQLite counts the
  statements before any runs (`sqlite_statements.rs`, the only C call), and a
  statement with no columns is not a read. Dropping the future does not stop
  the engine, so a deadline or a SIGINT sets the flag the progress handler
  reads and then waits for the statement to end. A server canceller sets a
  flag too: the session reads it before each statement of an `execute` and
  before its COMMIT, because a server stops only the statement it is running
  and Aurora DSQL stops none. A read nobody could stop is not waited for.
  A server user authenticates with a password from 1Password or, with
  `[db.<name>.iam]`, a token `adapters/aws/db_iam_token.rs` signs with the
  role of an AWS profile, each time a connection opens; `adapters/sigv4.rs`
  is the only file that signs. Whether a host is Aurora DSQL is decided once,
  when the section is loaded (`ServerDatabase.aurora_dsql`): it gets DSQL's
  token and none of the PostgreSQL settings DSQL refuses.
  A server engine is verified against a real one: `cargo xtask db-up` and
  `KURAMA_TEST_DB=1 ... --test real_db`. A fake cannot say what bytes a server
  sends, which quote it reads or what it calls an error, and each of those was
  wrong until a real server said so; the per-engine choices are now matches
  with no catch-all, preparing happens in one place that declares the
  parameter types, and `tests/architecture/` requires a real-database test
  for every engine the configuration names.
  Files: `cargo xtask map database`; the guards and the bounds in
  `docs/development/database.md`.
- Agent runs: `KURAMA_AGENT` (anything but empty or `0`) marks a run as an
  agent's (`src/shell/agent_policy.rs`). The `[agent]` policy
  (`src/domain/functions/agent_policy.rs`, `[api.<name>.agent]` replacing
  its keys) refuses an `api` request, a `db --commit` and `exec`'s full
  permissions as `AGENT_POLICY_DENIED` before any credential is read, unless
  `--confirm`. `src/shell/cli/mod.rs` opens one audit entry per `api`,
  `exec`, `db` and `data` run (`src/shell/audit.rs`), the commands note the
  method and path (never the query), the status and the SQL fingerprint, and
  the entry is appended to `~/.local/state/kurama/audit.jsonl` when the run
  ends -- or before `exec` replaces kurama. A write failure is a warning.
  The harness holds every audit line to the fields `AuditEntry` names.
  Files: `cargo xtask map agent-policy` and `cargo xtask map audit`.
- Errors: `src/shell/cli/error_code.rs` maps the typed error chain to
  `error[CODE]` and an exit code. `rg -n <CODE> src tests` lands on the
  classification arm and the scenario that pins it; `tests/architecture/`
  fails for a code that no scenario expects.
- Dynamic zsh completion: `main` runs `CompleteEnv` before runtime setup;
  `shell/cli/completion.rs` reparses the completed argv prefix with clap and
  uses `adapters/completion.rs` to read configuration, AWS profiles and local
  or cached OpenAPI descriptions synchronously. Candidate generation is
  pure in `domain/functions/completion_candidates.rs`. The printed zsh
  function calls the generating binary's absolute path without the export
  wrapper; `name=` candidates suppress the trailing space. Completion never
  authenticates, fetches or writes files. The harness marks direct and real-zsh
  completion sessions automatically and applies shared zero-call/file checks.
  Only direct runs expose child exit codes; the completing child's stderr
  goes to `KURAMA_COMPLETION_STDERR` when set and to `/dev/null` otherwise,
  so a person pressing Tab sees nothing while every scenario fails on a
  diagnostic. Real-zsh PTY also checks visible diagnostics and command
  insertion.
  Files: `cargo xtask map shell-integration`.
- Config check: `kurama config check [--json]`
  (`src/shell/cli/commands/config_check.rs`) reads config.toml through
  `src/adapters/config/check.rs`, which parses the syntax once and reads each
  top-level table and each `[auth.*]` / `[api.*]` / `[data.*]` / `[db.*]` /
  `[s3.*]` entry into `Config` alone, then runs `Config::problems` -- the
  same rules `validate` stops at the first of -- over what read. The report
  goes to stdout whatever it found; any `error` fails the run as
  `CONFIG_INVALID` (exit 2) under the JSON error contract. It resolves no
  secret and fetches nothing: a URL `openapi` is read from its cache only.
- Config writes: `src/adapters/config/writer.rs` is the one place that saves
  config.toml. `ConfigFile::open` reads it (absent is empty), the caller
  builds the whole candidate (`ConfigFile::append` for new units, keeping
  the file's bytes), `validate` runs `Config::parse` and the checks against
  `~/.aws/config` that `config check` shares
  (`src/adapters/config/references.rs`), and `ConfigFile::save` locks the
  directory, refuses a file changed since the read (`CONFIG_WRITE_FAILED`)
  and renames a temporary file from the same directory over the target of a
  symbolic link, keeping its mode. `src/adapters/config/saved.rs` reads the
  file for syntax only (units, keys, redaction of literal secrets) for
  `config list` / `config show`, their completion and the writer.
  `config set` / `unset` / `remove` (`commands/config_edit.rs`) edit that
  document through `ConfigFile::edit`, an `Edit` in
  `src/adapters/config/edit.rs` that changes only the key or unit named
  (toml_edit, never a reserialized `Config`), refuses what the file lacks,
  repeats and overlaps, and an `[auth.*]` an API it keeps still uses;
  `ConfigFile::validate_edit` checks the whole result once, as `validate`
  checks an append, before `save`.
- Presets: `kurama preset` (`src/shell/cli/commands/preset.rs`) lists the
  catalog, const data in `src/domain/types/preset.rs`, without reading
  configuration. `preset show` expands one through
  `domain/functions/preset_render.rs`: `plan_preset` decides the names
  (`--as` renames the `[api.*]` only), whether an existing `[auth.*]` is
  reused (its contract must be the preset's; references are compared as
  written, never resolved), the missing inputs, the setup steps and the
  TOML. `plan_against_file` and `check_plan` run it against config.toml
  through the config writer's `append` and `validate`; `show` never calls
  `save`, and `preset add` (`commands/preset_add.rs`) saves what they passed
  and prints `config add`'s `SaveReport`. A preset is
  never read at runtime: what it prints is ordinary configuration.
  Files: `cargo xtask map preset`.
- Agent contract: `kurama agent` prints the sections of
  `docs/agents/kurama/`, one file per chapter, concatenated in order by
  `AGENT_GUIDE` in `src/shell/cli/commands/agent.rs` (`concat!` of
  `include_str!`, so the page is still a compile-time constant and always
  matches the binary); `--skill` prints `docs/agents/SKILL.md`, the Agent
  Skill that tells an agent to read it first. The page carries the Setup
  chapter for `[auth.*]` and `[api.*]`; its unit test fails when the page
  names a command the binary lacks, and
  `every_guide_section_is_printed` fails for a section file that list
  leaves out.
- MCP: `kurama mcp` (`commands/mcp.rs`) reads JSON-RPC lines on stdin and
  answers on stdout, one request at a time; `domain/functions/mcp.rs` maps
  each tool call to one of kurama's own JSON command lines (every agent value
  after `--` or glued with `=`), and `adapters/own_command.rs` runs this
  binary again with `KURAMA_AGENT=1`, stdin null or the request, and a
  deadline. The policy, the audit entry and the error document are the
  command line's, not a copy. Files: `cargo xtask map mcp`.
- `.agent/features/<feature>.toml`: hand-written feature map, one file per
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
- `tests/scenarios/<feature>.rs` + `tests/support/mod.rs`: runtime
  verification against the real binary, one file per feature with
  `main.rs` holding only the module list. The harness checks every scenario
  for role credentials on disk and for error logs in successful runs. Each
  scenario writes `target/agent/scenarios/<name>.json`, with the
  `combination` the scenario verifies (a TOML case declares one; a Rust
  scenario has none) and the `evidence` it ran against (`fake`).
- `tests/cases/<feature>/<id>.toml`: a scenario declared as data
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
- The registries above are split so two branches touch different files.
  Adding a feature adds a file; adding a scenario appends to one feature's
  file. What is still shared is the `mod` list of `tests/scenarios/main.rs`
  and the section list in `agent.rs`. `.gitattributes` gives the first,
  a pure list, `merge=union`; the section list is code and is merged by hand.
  The split separates two branches only while they are in two features: two
  issues in one feature append to the same `.agent/features/<feature>.toml`,
  which is why that file is `merge=union` too. The Commands table of this
  file is the other thing every branch appends to, and the one place a pair
  `conflicts` calls parallel can still need a hand: append at the end.
- `Affects: <feature>, <feature>` on one line of an issue body: the features
  whose files that issue edits, named as in `.agent/features/`.
  `cargo xtask conflicts <ISSUE>...` reads it through `gh` and answers, for
  every pair, whether the two can be implemented at the same time.
  `<feature>/<path>` narrows a declaration to one path, which that feature has
  to claim; a directory entry is enough, so a file the issue is about to write
  counts before it exists. Two issues that meet only in a file nobody can
  split (the clap definition, the dispatch table, `ClientKind`, the error
  classification, the xtask subcommand branch) are told to append at the end
  rather than to take turns. A directory entry does not own a file another
  feature claims by name, so `tests/fakes/` does not make the harness wait for
  whoever edits `tests/fakes/session-manager-plugin`. Adding an
  entry to a registry that is already split per feature
  (`.agent/features/`, `tests/scenarios/`, `docs/agents/kurama/`) is not
  declared: appending
  there is what the split made safe. An issue with no `Affects:` line, or one
  naming a feature the map does not hold, is an error and not a verdict:
  "nothing in common" and "nothing was declared" never read the same.
  The line holds names and commas only: a note written on it is reported
  as a line that cannot be read, so the note goes on the next line.
- `## Scenarios` (older issues: `## シナリオ`) in an issue body: the runtime scenarios that issue is done by,
  one `- [ ] <feature>_<behavior>` checklist item each, so the first test to
  write is read off the issue. `cargo xtask scenarios-check <ISSUE>` answers
  what is still missing, what the branch adds without declaring it, what no
  feature claims and which declaration is not shaped like a scenario name. The
  prose acceptance criteria stay; only what runtime verification shows moves
  into the section. An issue with no section (an xtask command, a document) is
  reported as declaring nothing, never as satisfied.

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
7. `kurama init zsh`, `completions` and `agent` never read configuration or
   the AWS environment: the first two run at every shell start, and the
   agent contract is what an agent reads to write or fix the configuration.
   `kurama agent ready` is the one exception, a separate subcommand: it
   reads what `status` reads (config.toml, `~/.aws/config`, the session
   cache, the token store) and nothing more.

## Workflows

### Implement or change a feature

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
   `gh pr create --base main` with `Closes #N` in the body, then
   `gh pr merge --merge` and `git switch main && git pull`. `main` takes merge
   commits only: a squash or a rebase would leave the branch's own commits
   out of `main`, and `worktree remove` could no longer tell it was merged.

### Fix a bug or investigate an error

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

### Change the TUI

Follow `docs/development/tui-testing.md`: the loop, the fixtures, the snapshot
rules and the step-by-step checklist live there. Compiling and passing unit
tests does not show what a user sees; the change is done only when
`cargo xtask tui-check` passes and the screens in `target/agent/tui-report.md`
have been read.

### Work on several issues at once

One issue (or one batch of small issues of one feature) = one worktree = one
branch = one agent. Nothing here is a lock:
every agent acts as the same GitHub account, so two of them writing
`In progress` write the same value and cannot be told apart. What the board
buys instead is that the second agent, and a person, can see the first.

1. `cargo xtask ready` picks the issue. It answers with what is free, what is
   running, what a running branch holds back, and what it could not judge.
   Each offer names the offers it cannot run beside, which is how several
   agents are started at once.
2. `cargo xtask worktree add <ISSUE>` takes it. It refuses a tracker, an
   issue labelled `needs-human`, one with an open blocker, and one someone is
   already on. Do not set the status by hand. It needs GitHub to see any of
   that, so it refuses when the board cannot be reached; `git worktree add`
   makes the branch offline, without the checks.
   `cargo xtask worktree remove <ISSUE>` deletes the worktree whether or not
   the board answers -- the worktree is gone either way -- and says so when
   the issue was left `In progress`.
3. Work in that worktree only. Never touch another branch's files from it.
   The base is `main`.
   To run the branch against real services, use the `KURAMA_CONFIG_PATH` and
   the `kurama-<ISSUE>` that `add` printed, never the daily `kurama` and its
   config: a key one branch adds makes every other binary refuse the file.
4. Append at the end of the files nobody can split: `error_code.rs`'s
   `classify`, `args.rs`'s clap definition, `dispatch.rs`, `client.rs`'s
   `ClientKind`, `xtask/src/main.rs`'s subcommand branch and its HELP, and the
   Commands table of this file. A conflict there is then one obvious hunk.
5. Do not refactor a hub file while others are running. A refactor goes on its
   own, checked by the report diff of `cargo xtask verify all`.
6. The gate on a branch is `cargo xtask branch-check`. `cargo xtask check` is
   the integration-side one and runs on `main`, before a release tag is
   pushed from it.
7. Turn a review finding into a rule in `tests/architecture/` (with an id in
   its `rules.toml` and its `main.rs` list) or a check in xtask before
   closing it; `docs/development/prevention-layers.md` lists the layers
   that already exist. Only what could not be made a rule stays a matter
   of human review.
8. An issue is written with an `Affects:` line, a `## Scenarios` section when it
   changes what a user sees, and `blocked_by` links for what has to land
   first. A parent issue is labelled `tracker`, and one only a person can
   close is labelled `needs-human`.
9. `cargo xtask worktree remove <ISSUE>` gives the issue back; run it after
   the pull request is merged and `main` pulled, so it sees the merge. An
   `In progress` that has not moved for a day is what `ready` warns about; a
   person decides whether that worktree was abandoned.
10. Small issues of one feature go to one worktree: the `batch` lines of
    `cargo xtask ready` name them, and `cargo xtask worktree add <ISSUE>...`
    takes them together, so the build and the gate are paid once.
    `remove` with the first number gives every one of them back.
11. Where an issue leaves a choice to a person, the agent picks the
    recommended option, implements it, and lists it under "Decisions" in its
    final report; the person approves them together. The agent stops to ask
    only before what cannot be undone, what bills an account, or what makes
    something public.

### Refactor

Run `cargo xtask verify all` before and after: the reports under `target/agent/`
must show the same STS, federation, `op` and browser call sequences, exported
variables, exit codes and `error[...]` / `hint:` lines (diff the two reports).
A refactor is done when it removes something it names. The tests an abstraction
has to pass before it is written are in `docs/development/module-structure.md`.

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
