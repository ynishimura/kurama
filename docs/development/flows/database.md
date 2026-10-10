# Database flow

How `kurama db` connects, guards a read and stops a statement; the bounds are in ../database.md.

Databases: `dispatch.rs` -> `commands/db.rs` (with no operation on a
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
