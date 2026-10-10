# Config check and writes

How `config check` reports problems and how every config.toml write is validated and saved.

Config check: `kurama config check [--json]`
(`src/shell/cli/commands/config_check.rs`) reads config.toml through
`src/adapters/config/check.rs`, which parses the syntax once and reads each
top-level table and each `[auth.*]` / `[api.*]` / `[data.*]` / `[db.*]` /
`[s3.*]` entry into `Config` alone, then runs `Config::problems` -- the
same rules `validate` stops at the first of -- over what read. The report
goes to stdout whatever it found; any `error` fails the run as
`CONFIG_INVALID` (exit 2) under the JSON error contract. It resolves no
secret and fetches nothing: a URL `openapi` is read from its cache only.

Config writes: `src/adapters/config/writer.rs` is the one place that saves
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
