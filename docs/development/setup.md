# Development setup (macOS)

How to build, test and try kurama while developing. Installation for daily
use is covered in the [README](../../README.md#installation), and the short
path from a clone to a pull request in
[CONTRIBUTING.md](../../CONTRIBUTING.md).

Most of this page applies to every contributor. Three parts are for the
maintainer: [Manual GitHub Actions](#manual-github-actions) (dispatching
needs write access to the repository), [Several worktrees at
once](#several-worktrees-at-once) (the `worktree` commands move issues on the
repository's project board) and [A stable signature for the installed
binary](#a-stable-signature-for-the-installed-binary) (only for the binary you
use every day, never needed to contribute).

## Toolchain

- Rust via rustup, stable channel. The minimum is `rust-version` in
  `Cargo.toml` (1.95.0); CI runs the gate on the latest stable, as developers
  do, checks that 1.95.0 still builds, and builds with `--locked`, so
  `Cargo.lock` is tracked and dependency changes go through `cargo update` /
  `cargo add` explicitly.
- `cargo-llvm-cov` for coverage (optional): `cargo install cargo-llvm-cov`.
- [mise](https://mise.jdx.dev/) installs the pinned Lefthook, `cargo-machete`
  and `cargo-sweep` versions from `mise.toml` for the local pre-push gate (see below).
- 1Password CLI (`op`) and an AWS profile with MFA, only for manual end-to-end
  checks.

## Everyday commands

```bash
cargo xtask doctor                        # toolchain, metadata, fake environment
cargo xtask map [feature]                 # files, test filter, scenarios per feature
cargo build --locked
cargo test --locked --features test-fakes
cargo test --locked --features test-fakes -- <filters>   # the `tests` value from `map`
cargo xtask verify all                    # runtime scenarios (fake STS, fake op)
cargo xtask tui-check                     # TUI: snapshots, UI contract, PTY scenarios, screens
cargo xtask check                         # fmt --check, clippy -D warnings, machete, then all tests
cargo xtask install-signed                # build, code sign, install ~/.cargo/bin/kurama
cargo fmt --all
cargo llvm-cov --locked --all-features    # coverage
```

`cargo xtask check` is the gate used by the local pre-push hook and manual CI.
`cargo xtask` is a small workspace crate in `xtask/`; it is never installed.

### Local checks before push

After cloning, review `mise.toml` and install the development tools and hooks:

```bash
mise trust
mise run setup
```

The setup task installs Lefthook's `pre-push` hook in this clone. Git hooks are
not copied by `git clone`, so each contributor runs setup once. The hook uses
`mise exec` to make the pinned tools available even without mise shell activation.

The gate has two stages. A `git push` from `dev` or `main` runs
`cargo xtask check` and then `mise run sweep`: formatting, clippy, all tests
(including runtime and TUI scenarios), unused dependencies, and the 12GB
`target/` cap. A push from a feature branch runs `cargo xtask branch-check`
instead -- the static checks, the tests of the affected features,
`cargo xtask verify affected`, then `cargo xtask mutate` on the lines it changed
under `src/` and `xtask/src/` -- so parallel branches do
not each pay for the full gate; what lands on `dev` still passes `check` once,
on the push that puts it there. Two `check` runs queue on one lock rather than
compiling at the same time. A failure stops the push.
Every xtask command that builds or runs the tree (`check`, `branch-check`,
`verify`, `tui-check`, `mutate`) runs the same static checks
first -- whether `tests/scenarios/cases_generated.rs` matches the cases
(`cargo xtask generate-cases` rewrites it), `cargo fmt --all -- --check`,
`cargo clippy --locked --workspace
--all-targets --features test-fakes -- -D warnings`, `cargo machete` when it is
installed -- and stops at the first failure: they take seconds to a couple of
minutes, and a lint found after half an hour of tests is half an hour lost.
Commit your changes and push from the branch's own worktree: the hook checks the
current working tree, including any uncommitted changes. Results are written to
`target/agent/verification-report.md` and `.json`.

To run the same hook without pushing:

```bash
mise exec -- lefthook run pre-push
```

### Manual GitHub Actions

Pushes and pull requests do not start GitHub Actions. For another OS, the minimum
Rust version, or coverage, open **Actions > CI > Run workflow**, select the branch,
and choose `ubuntu`, `macos`, or `all`. Ubuntu alone is the default. Enable the
coverage checkbox only when needed; it runs an additional Ubuntu job.

The same operation is available through the GitHub CLI:

```bash
gh workflow run ci.yml --ref dev -f platform=ubuntu
gh workflow run ci.yml --ref dev -f platform=all -f coverage=true
```

Run the relevant OS checks before a release or after changing
platform-specific code. The local hook tests only the current machine's OS and
Rust toolchain; manual CI runs the gate on the latest stable Rust on the
selected runners and checks that the minimum, 1.95.0, still builds (`cargo
check --locked`). Each check job uploads its verification report and captured
TUI screens. Coverage uses the latest stable too and uploads `lcov.info` as a
run artifact (`coverage-lcov`). A pull request from a fork is dispatched from
a `ci/pr-<N>` copy of its branch: [CONTRIBUTING.md](../../CONTRIBUTING.md#ci)
has the steps.

The cache is keyed on `Cargo.lock` alone and holds `target/` after
`cargo xtask check` has capped it at 12GB; `CARGO_INCREMENTAL=0` keeps the
incremental caches out of it. A cache saved by a run on `ci/pr-<N>` is visible
only to that branch, so a pull request's build never seeds the one `dev`
restores. CI never sets `KURAMA_HTTPFS_ARCHIVE`: `build.rs` downloads httpfs
into `target/`, so a run that restores no cache is the plain online build.

#### Manual CI record

Automatic runs on push or pull request are a separate decision, taken after
the repository is public. These runs are what it is taken on; fill a row in
when the run is made.

| Run | Date | Commit | Result | Duration |
| --- | --- | --- | --- | --- |
| `platform=ubuntu`, cold cache (also the build without `KURAMA_HTTPFS_ARCHIVE`) | not run yet | | | |
| `platform=ubuntu`, warm cache | not run yet | | | |
| `platform=macos`, cold cache (also the build without `KURAMA_HTTPFS_ARCHIVE`) | not run yet | | | |
| `platform=macos`, warm cache | not run yet | | | |

### Runtime scenarios

`tests/scenarios/` runs the built binary against one local fake server
(`wiremock`): STS and the federation endpoint (reached through
`AWS_ENDPOINT_URL_STS` and `KURAMA_FEDERATION_ENDPOINT`), an OAuth
authorization server (`/oauth/token`, `/oauth/device`, OpenID Connect
discovery) and an API (`/api/...`) reached through `{server}` in the kurama
config; plus a fake 1Password CLI (`tests/fakes/op`, also `op read` and `op item get`) and a
fake browser (`tests/fakes/open`, which answers an authorization request with
the redirect) inside a temporary HOME. The real keychain is never touched:
scenarios that need a session cache or a token store use the file backends
compiled in with `--features test-fakes` (`cargo xtask check` and
`cargo xtask verify` pass the flag; without it `cargo test` reports those
scenarios as ignored). Every scenario is also checked for role credentials
written to disk and for error logs in successful runs. Each scenario records
what it observed under `target/agent/scenarios/`, and `cargo xtask check` or
`cargo xtask verify` turns those files into
`target/agent/verification-report.md` for the pull request. A CLI command
that needs a terminal (an OAuth `login`) runs on the pseudo terminal through
`tests/support/tui.rs` (`launch_command`).

### TUI checks

`cargo xtask tui-check` renders every home screen state at 80x24, 100x30,
120x40 and 160x50 (`tests/tui_snapshots/`), checks the UI contract, and runs
the `tui_*` scenarios, which launch the built binary on a pseudo terminal,
press keys, resize it and capture each screen as `screen.txt`, `screen.ansi`
and `screen.png` under `target/agent/tui/<scenario>/<step>/`. The report
`target/agent/tui-report.md` embeds every captured screen. Details and the
agent workflow: [tui-testing.md](tui-testing.md).

### Exit codes

| exit | meaning |
| --- | --- |
| 0 | success |
| 1 | tool error: bug, network, unexpected service response |
| 2 | usage: unknown profile or API, invalid configuration, bad arguments, a verb the kind does not support, no terminal for the TUI |
| 3 | human action required; a `hint:` line on stderr says what |
| 4 | the remote side rejected the request: AWS (AccessDenied, invalid MFA code, ...), an authorization server, an HTTP status outside 2xx |

Every failure prints one `error[CODE]: message` line on stderr. The codes are
defined in `src/shell/cli/error_code.rs`; `rg -n <CODE> src tests` finds the
classification and the scenario that pins it (`tests/architecture/` fails
for a code that no scenario expects).

## Build artifacts and disk space

`libduckdb-sys` compiles DuckDB from source, so one `target/debug` is about
7 GB: 5.5 GB of dependencies, a 1.3 GB DuckDB build, a 95 MB rlib and test
binaries of 100 MB and up. Cargo never deletes the previous generation, every
feature set gets its own, and every worktree carries a separate `target/`: an
untended clone reaches tens of gigabytes.

```bash
cargo xtask sweep                     # bring target/ under 12GB (mise run sweep runs this)
rm -rf target/debug target/release    # start over, keeping target/agent
```

`cargo xtask check` also caps `target/` after its gates; the pre-push hook
runs `mise run sweep` again so a push always ends under 12GB.
The cap drops `target/*/incremental` first: those are gigabytes that belong to
the workspace crates alone and the next compile writes them again. Only if the
folder is still over the cap does it fall back to `cargo sweep --maxsize`,
which evicts in modification order -- and the files with the oldest
modification time are the C++ crates nothing ever changes, DuckDB and
aws-lc-sys, so that fallback is what makes the next gate spend five minutes
compiling. Time-based cleanup is no use here: every artifact the build just
read looks fresh. `target/agent` is left alone either way.

`cargo clean` also deletes `target/agent`, where the verification reports a
pull request description quotes live, so remove `debug` and `release` by hand
instead. Remove a worktree once its branch is merged
(`git worktree remove <path>`) rather than leaving its copy behind.

### Several worktrees at once

Measured on 2026-09-23 (M-series Mac), one worktree per issue with its own
`target/`:

| What | Time | Disk |
| --- | --- | --- |
| `worktree add --install` on a new worktree (xtask and a cold `--release` build) | 8 min 34 s | 1.9 GB |
| First `cargo xtask doctor` there (builds the test targets) | 4 min 40 s | +4.3 GB |
| `cargo xtask scenarios-check` afterwards | 3.5 s | |
| `branch-check` of a change to one `src/` file | 10 min 2 s | 6.8 GB in total |
| of which `mutate` (8 mutants of that file) | 7 min | |
| `branch-check` of an xtask-only change, xtask mutants included | 4 min 38 s | |

So each worktree costs about 5 min and 5 GB before its first gate, 7 GB once
it has run one, and 2 GB more with `--install`. With the main checkout capped
at 12 GB, three worktrees in parallel come to about 35 GB, 40 GB with their
own `kurama-<ISSUE>`. `cargo xtask worktree list` prints the sum.

What was chosen, and why:

- **One `target/` per worktree stays.** A shared `CARGO_TARGET_DIR` made the
  second worktree's cold build 1.6 s instead of 4 min 24 s and saved about 5
  GB, but it is wrong: Cargo names a workspace crate's artifacts without its
  path and judges them fresh by modification time, so the second worktree
  listed and ran a test that only the first worktree's source contained. A
  gate there passes on another branch's code. `branch-check` and `check`
  refuse a `CARGO_TARGET_DIR` outside the worktree for that reason.
- **A mutant runs the tests of the features that own its file.** A branch
  that touches a hub (`domain/types/http.rs`, `adapters/config/`) makes
  `impact` select most of the suite: 116 s a mutant on one such branch, whose
  28 diff mutants came to about an hour. The changed files are grouped by the
  features that own them and each group runs those features' filters and
  scenarios; a mutant only a distant scenario would catch survives and is
  read, rather than every mutant paying for it. In `branch-check` there is
  no baseline -- its tests and scenarios just passed on the same tree, and
  running them again only gave a flaky scenario a second chance to stop the
  step, which it took on that branch -- and a mutant times out at twice what those
  steps took (at least 60 s) instead of a fixed 300 s, which 18 of 93
  mutants spent in full on the same branch. Before it starts, `mutate`
  prints the count and an estimate, and with PATH the count of the branch's
  own lines beside it.
- **A mutant runs in place.** Measured on a
  one-line change to `db_render.rs` (6 mutants): the whole gate went from
  13 min 44 s to 5 min 31 s. cargo-mutants copied the tree without
  `target/`, so each run began with a 258 s cold DuckDB build; kurama is now
  mutated `--in-place` over a clean `src/` (0.8 s). nextest was slower (329 s for the suite), and
  more test threads did not help: the scenarios are CPU-bound.
- **`mutate` stays in `branch-check`.** It is 70% of a `src/` branch's gate,
  each mutant rebuilding kurama and running its feature's tests, but a
  branch's diff is a handful of mutants, and the gate is the one
  place that shows a test passing for the wrong reason before review.
- **The 12 GB cap stays per `target/`.** The sum is what `worktree list`
  shows; one cap for all worktrees would evict another worktree's DuckDB
  build to make room, which costs that worktree five minutes.
- **sccache is not used.** Measured on three fresh worktrees: 300 s
  without it, 392 s to fill its cache, 288 s from a warm cache (269 s with
  `SCCACHE_BASEDIRS` set to the worktree). It hit 380 Rust crates, but 354
  of DuckDB's C/C++ compilations missed even warm, and those are what a new
  worktree waits for. Not worth a tool and a cache directory.

A build that xtask starts and one the shell starts reuse each other's work
only because xtask drops the variables `cargo run` set for it before it runs
cargo (`cargo()` in `xtask/src/main.rs`). ring's build reads
`CARGO_MANIFEST_DIR`, so an inherited one made each side find the other's ring
stale, and through rustls and reqwest the build script of libduckdb-sys: two
to five minutes of DuckDB whenever `cargo xtask doctor` or a gate alternated
with a plain `cargo test`.

`[profile.dev]` in `Cargo.toml` keeps the DWARF out: `debug = false` for every
dependency (`package."*"`), which is 90% of the DuckDB objects' size and is
never read, and `debug = "line-tables-only"` for this crate, which is what a
backtrace into kurama's own code needs. Debug info is what made the folder
outgrow the cap in the first place.

## Two binaries: installed and development

| Binary | Path | Produced by |
| --- | --- | --- |
| Installed (daily use) | `~/.cargo/bin/kurama` | `cargo xtask install-signed` |
| Development | `target/debug/kurama` | `cargo build --locked` / `cargo run` |

They never overwrite each other. The `kurama` shell function comes from
`eval "$(kurama init zsh)"` in `~/.zshrc` and is bound to the binary that
printed it, so the installed binary keeps working while you rebuild.
The completion function uses that same absolute path directly, bypassing the
wrapper and its temporary export file. Tab reads local configuration and
local/cached OpenAPI descriptions only; it does not authenticate or refresh a
URL. Re-evaluate `init zsh` to change the binary used by both functions.
`kurama completions zsh` prints the completion function alone for `fpath`
setups.

### Running the development build

Without shell integration, credentials go to stdout:

```bash
cargo run -- --help
cargo run -- status
cargo run -- env <profile> --json
```

To use the development build with the shell wrapper (credentials exported into
the shell, completions from the new command definition), evaluate its own
`init` output in the shell you are testing in:

```bash
cargo build --locked
eval "$(target/debug/kurama init zsh)"   # this shell now wraps target/debug/kurama
kurama env <profile>
```

Open a new shell to go back to the installed binary. To make the current
checkout the installed binary, run `cargo xtask install-signed` (see
[Keychain prompts](#keychain-prompts) for why not `cargo install`).

### Isolated configuration

- `KURAMA_CONFIG_PATH=<file>` uses another kurama config instead of
  `~/.config/kurama/config.toml`.
- `AWS_CONFIG_FILE=<file>` uses another AWS config file.
- `RUST_LOG=kurama=debug` enables debug logging (stderr).

## Keychain prompts

The MFA session cache lives in the login keychain (service `kurama-session`),
the OAuth token store next to it (service `kurama-token`).
macOS grants access per binary code signature: the entry stores the designated
requirement of the binary that created it. The linker ad-hoc signs every build
with a new hash, so each new build prompts once on its first cache read. Click
**Allow** (or **Always Allow**, which only lasts until the next rebuild).
`kurama logout --all` removes the cached sessions if you want to start from a
clean state.

Answer that prompt in a terminal. A run without one (an agent, a scenario
harness against the real keychain, SSH, cron) cannot show the dialog, so the
grant stays missing and the read fails with `macOS denied this build access to
the keychain entry (error -25293)`; macOS words the underlying status as a
wrong password, which `src/adapters/keychain.rs` replaces with that line.
A failed read of the MFA session is not fatal: it counts as absent and a new
session is fetched. An unreachable OAuth token is not as harmless — the grant
that produced it has to run again, and an authorization code grant needs a
person.

### A stable signature for the installed binary

Sign every installed build with the same self-signed identity and the same
identifier, and the designated requirement becomes

```text
identifier "dev.kurama.cli" and certificate leaf H"<certificate hash>"
```

instead of the `cdhash H"..."` of an ad-hoc build, which is a different value
after every build.

That is not enough on its own. Each entry also carries a partition list, and
for a signature without an Apple team ID, macOS records **Always Allow** there
as `cdhash:<hash>`, which the next build changes. A run without a terminal is
then denied (`error -25293`) even though the requirement matches. So
`install-signed` ends by adding the new build's `cdhash` to the list of every
`kurama-token` and `kurama-session` entry and of the entry
`[onepassword] service_account_keychain` names, keeping the hashes already
there (a `--as NAME` build beside the daily one still reads them). It asks for
the login keychain password once and passes it to
`security set-generic-password-partition-list` on stdin; without a terminal it
only says how many entries still lack the hash.

Create the identity once. From the terminal:

```bash
P=$(openssl rand -hex 16)
openssl req -x509 -newkey rsa:2048 -nodes -days 7300 \
  -keyout /tmp/kurama-dev.key -out /tmp/kurama-dev.crt \
  -subj "/CN=kurama-dev" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning"
openssl pkcs12 -export -inkey /tmp/kurama-dev.key -in /tmp/kurama-dev.crt \
  -out /tmp/kurama-dev.p12 -name kurama-dev -passout "pass:$P" \
  -macalg sha1 -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -legacy
security import /tmp/kurama-dev.p12 -P "$P" -A -T /usr/bin/codesign
rm -f /tmp/kurama-dev.key /tmp/kurama-dev.p12; unset P
```

Two details are not optional. `-legacy`, because macOS cannot read the PKCS#12
container OpenSSL 3 writes by default. And a non-empty password, because an
empty one fails the same import with `MAC verification failed`.

There is no `security add-trusted-cert` step. A trust root would only be needed
to make `security find-identity -v` list the certificate; `codesign` signs with
it either way, adding one needs an authorization dialog, and a trusted root is
a larger change to the system than this needs. The first signing does ask once
for permission to use the key: answer **Always Allow**.

Keychain Access > Certificate Assistant > Create a Certificate does the same
job: name `kurama-dev`, Identity Type Self Signed Root, Certificate Type Code
Signing, and check **Let me override defaults** so the validity period can be
raised from its default of 365 days.

Either way, give it 20 years. The requirement names the certificate, so a
certificate replaced after it expires is a new requirement, and every entry has
to be approved again.

Then build and install in one step:

```bash
cargo xtask install-signed
```

It builds `--release --locked`, signs `target/release/kurama` with the
identity `kurama-dev` and the identifier `dev.kurama.cli`, verifies the
signature, prints the resulting designated requirement and copies the binary
to `~/.cargo/bin/kurama`. `--identity NAME` and `--identifier ID` override the
defaults; changing either one changes the requirement, so pick them once.

Do not run `cargo install --locked --path .` after that: it rebuilds and
copies an ad-hoc signed binary over the signed one, and the prompts come back.
Sign a development build you use repeatedly by hand, with the same two values,
to give it the same requirement:

```bash
codesign --force --sign kurama-dev --identifier dev.kurama.cli target/debug/kurama
```

### Trying a branch against real services

A branch build run from `target/` is ad-hoc signed, so it cannot read the MFA
session or the OAuth tokens, and every command asks 1Password for a new TOTP.
Installing it with `cargo xtask install-signed` fixes that but makes the daily
`kurama` the unmerged branch. Install it under another name instead:

```bash
cargo xtask install-signed --as kurama-dev     # ~/.cargo/bin/kurama-dev
kurama-dev status
```

The signature carries the same identity and identifier, so the designated
requirement is the same and every keychain grant the daily binary has applies
to `kurama-dev` too, and the install adds its `cdhash` to the partition lists
as it does for the daily one; `~/.cargo/bin/kurama` is not touched. A name that cannot
be a command (a path, a leading `-` or `.`) is refused with exit 2. Delete the
file when the branch is merged. The configuration is still the shared
`~/.config/kurama/config.toml`: a key the branch adds there makes the daily
binary refuse the whole file, so point the branch at a copy with
`KURAMA_CONFIG_PATH` (`cargo xtask worktree add` prepares one per worktree).

Entries written by earlier ad-hoc builds still carry the old requirement, so
the first signed binary prompts once for each of them (the MFA session, every
OAuth source). Approve them once — or run `kurama logout --all` first — and
they stay approved across rebuilds from then on.

## Branches and releases

Feature work branches from `dev` and merges back via PR; `dev` is merged to
`main` when a state is worth installing. A `v*` tag on `main` builds the
prebuilt archives, the shell installer and the Homebrew formula
([releasing.md](releasing.md)). Installing from source means building `main`
(or whatever is checked out) with `cargo xtask install-signed`.
