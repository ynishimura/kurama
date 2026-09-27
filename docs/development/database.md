# `kurama db`

What the database client guarantees, where each guarantee lives, and what has
not been verified yet. The agent-facing contract is `kurama agent --kind db
--json` and the `kurama db` chapter of `docs/agents/kurama/`; this page is
for whoever changes the code.

## The three guards

A read is guarded three times, and each guard is independent of the others.

1. **The connection cannot write.** On a server the statement runs in a
   read-only transaction (`BEGIN READ ONLY`; MySQL also sets the session
   read-only, which is what refuses DDL, because DDL commits a transaction
   implicitly and would then run read-write). A statement can turn its own
   guard off, so the guard covers exactly one statement and there is no second
   one in the connection to profit from it: the boundary that holds is the
   permissions of the database user. A SQLite database is opened with
   `read_only(true)` and `create_if_missing(false)`
   (`adapters/database/sqlite.rs`). `INSERT`, `CREATE`, `DROP`, `VACUUM`,
   `PRAGMA journal_mode`, `ATTACH` of a new file and `file:...?mode=rwc` are
   refused by SQLite itself, nothing is created on disk, and no statement the
   caller sends can lift it: `PRAGMA query_only = 0` is accepted and changes
   nothing, because the file handle is what refuses.
   A statement with a NUL in it is `DB_INVALID` naming the NUL, not an empty
   identifier: a NUL ends the C string, so SQLite would prepare the part
   before it and count the statements of something nobody wrote.
2. **One call runs one statement.** On a server the statement is prepared
   before it runs, and the server refuses a string that is two statements
   (PostgreSQL `42601`, MySQL `42000`); `sqlx::raw_sql` would run both, so no
   caller SQL goes through it. Preparing also names the columns, which is how
   an empty result still reports them. SQLite executes every statement of a
   string in one call, so `SELECT 1; DROP TABLE t` would run both.
   `adapters/database/sqlite_statements.rs` asks SQLite to prepare the SQL and
   counts the statements it finds, then finalizes them without stepping any:
   nothing runs, and more than one statement is `DB_INVALID`. A hand-written
   lexer is not used, because `;` inside a string, inside a comment and inside
   `CREATE TRIGGER ... BEGIN ... END` is not a separator and SQLite already
   knows that.
3. **A read returns rows.** The same preparation reports the columns of the
   statement. A statement with none is not a read, and `--query` refuses it
   before executing it.

The first guard is the one that matters; the other two are what stops a
statement from becoming two, and a write from arriving as a read.

## The only FFI

`sqlite3_prepare_v2`, `sqlite3_finalize`, `sqlite3_column_count`,
`sqlite3_column_name`, `sqlite3_column_decltype` and `sqlite3_errmsg` are
called in `sqlite_statements.rs` and nowhere else;
`tests/architecture/database.rs::only_reviewed_sqlite_entry_points_are_called` keeps it
that way. `libsqlite3-sys` is a direct dependency pinned to the version
`sqlx-sqlite` uses, so the `links` key keeps one copy of SQLite in the binary.

## Values

A prepared statement answers in the binary format on both servers, so every
type has its own reader (`postgres_decode.rs`, `mysql_decode.rs`), tested
against the bytes the two protocols document. `numeric` is read from its
base-10000 digits rather than through a decimal crate, because `NaN` and
`Infinity` are values no such crate holds. A type with no reader is
`DB_FAILED` naming the column and the type; a blank cell would be a lie.

## Stopping a statement

A server keeps running the statement after the client stops listening, so a
deadline and a SIGINT open a second connection and send `pg_cancel_backend` or
`KILL QUERY`. Only `true` from `pg_cancel_backend` confirms a stop: `false` is
PostgreSQL saying the backend could not be signalled, from a statement that
itself ran. How the statement ended is one value and not two flags that can
disagree (`StatementEnd`): the database confirmed it stopped, nobody confirmed
it, or the statement ran to its end and the change was rolled back. The
engine's own answer wins over the cancel's, because an engine that reported
the stop has confirmed it even when the second connection could not be opened,
and a rollback wins over both, because it says what became of the change
rather than what a cancel was told. MySQL names a `KILL` in SQLSTATE (70100)
but a `max_execution_time` stop only in its own error
number (3024, SQLSTATE `HY000`, which means everything else too), so the
driver's number is read for that one; matching MySQL numbers against SQLSTATE
matched nothing at all. A write is not bounded by `max_execution_time`, which
applies to read-only `SELECT`s: it sets `innodb_lock_wait_timeout`, which is
what a long write really waits for, and the client deadline ends the rest. Dropping the future
does not stop SQLite either: the statement keeps running and the connection
stays busy. A progress handler installed at connect time reads an
`AtomicBool`, and `commands/db.rs` sets it when the deadline passes or a
SIGINT arrives, then keeps awaiting the statement so it really ends. The
engine reports every stop as `interrupted`, so only that loop knows whether it
was a timeout or a signal, and it rewrites the failure accordingly
(`name_why_it_ended`).

Telling the server is not the whole stop. `pg_cancel_backend` and `KILL QUERY`
end the statement that is running and know nothing of the next one, and Aurora
DSQL has neither, so the canceller first sets a flag the session reads before
each statement of an `execute` and once more before its `COMMIT`: a change that
was asked to stop is rolled back, whatever the server could be told, and the
failure says the change was rolled back and nothing was kept. That is what
happened to it; the statement itself ran to its end, and saying a stop was
confirmed would name one nobody made. A real DSQL cluster reported exactly
that on 2026-09-21, before the three ends were one value. A read nobody could
stop is not waited for (`abandons_the_wait`): it leaves nothing
behind, and on DSQL the only other end is DSQL's own transaction limit. A
write is always waited for, because what it kept is the answer.

The deadline is what ends a statement, and the server's own limit
(`statement_timeout`, `max_execution_time`) is a backstop one second later,
for a client that went away. Were the two the same, they would race: the
engine would stop the statement first, no cancel would be sent, and the
failure would read as an interrupt. A read that answers after its stop was
asked is not a result: MySQL answers a `KILL QUERY` or `max_execution_time`
stop of `SLEEP()` and `BENCHMARK()` with a row, as if they had finished, and
kurama cannot tell that row from a complete one, so the read fails the way
the stop was asked. A write that answers was committed before anyone asked,
and stands.

`reported()`, `quoting_of()` and the SQL the two engines disagree about are
free functions with tests in `server.rs`, so the gate covers them: the real
servers are the only thing that can say what an error means, but they need
Docker and a MySQL identifier quoted the PostgreSQL way should not need a
container to be caught.

## The tunnel

`adapters/aws/ssm_tunnel.rs` owns the one child process this feature starts.
The Session Manager websocket protocol is AWS's own plugin's job; what the file
owns is what goes wrong without it:

- **Its output is read for as long as it runs.** The plugin keeps writing
  after the line that names the port. Closing that pipe ends the plugin with a
  broken pipe, and leaving it unread eventually blocks it, so the port is read
  out of a reader the tunnel keeps and drains. A fake plugin that prints
  everything and then sleeps never shows this; the real one died before the
  first connection reached it.
- **The plugin is checked first.** Releases before 1.2.536.0 take the session
  response, token included, on their command line. `require_usable_plugin`
  returns a `PluginReady` that `OpenTunnel::open` demands, so the check cannot
  be skipped, and `commands/db.rs` runs it before AWS or 1Password is asked
  for anything.
- **The bastion has to be online.** A tag filter cannot be combined with any
  other filter, so `PingStatus == Online` is checked here rather than by the
  service. An EC2 instance that is running with a stopped agent accepts no
  session, and is a different failure from a name that matches nothing and
  from a name that matches several.
- **`localPortNumber` is not sent.** The plugin listens on a free port and
  prints `Port N opened`; choosing one here would race with whoever takes it
  between the check and the listen.
- **Nothing outlives the call.** The plugin is killed and reaped and then
  `TerminateSession` is called, on every path: a tunnel that opened and had
  nothing to carry is closed too, and so is a session whose plugin could not
  be spawned at all -- `StartSession` has already created it by then.
- **The plugin's own reason is kept.** Its stderr is drained into the last few
  lines, bounded, and a plugin that ends before the port opens says why in the
  failure. Without it every such failure read the same, and the hint points at
  a bastion `find_bastion` has just found online.
  `tests/real_db.rs::db_tunnel_failures_are_classified_and_close_the_session_they_opened`
  covers both that and the plugin that never becomes ready; it needs no
  database, so it runs in the gate.

## IAM authentication

`[db.<name>.iam]` takes the place of `password`. `adapters/aws/db_iam_token.rs`
signs the token; nothing is sent to AWS for it, so the only AWS call is the
AssumeRole of the profile the section names, through the executor `kurama env`
uses. `commands/db.rs` keeps the roles one call has assumed, so a tunnel and a
token under one AWS profile cost one AssumeRole.

- **Two tokens, told apart by the host.** RDS and Aurora sign `Action=connect`
  and `DBUser` for `host:port` under the service `rds-db`. Aurora DSQL signs
  `DbConnectAdmin` for the role `admin` and `DbConnect` for every other one,
  for the host alone, under `dsql`. A DSQL cluster is reached only under
  `<id>.dsql.<region>.on.aws`, which is what `names_aurora_dsql` reads --
  that shape and no other, once, when the section is loaded:
  `ServerDatabase.aurora_dsql` is what every later decision reads, and a DSQL
  host with another engine or a password is refused there. The unit tests
  compare whole tokens, as text, with what botocore prints with its clock held
  still: read back through a URL parser, a wrong encoding decodes to the right
  value. The address is written by `TokenSigner` and not read back from the
  URL, which drops a default port (443).
- **The token is signed when a connection opens.** It lives fifteen minutes
  and `--timeout` allows a day, so `ServerAccess` holds the signer and not a
  token: the connection that stops a statement signs its own.
- **What the role needs.** `rds-db:connect` on the `dbuser` ARN, or
  `dsql:DbConnect` / `dsql:DbConnectAdmin` on the cluster. AWS is never asked,
  so a missing permission is the database refusing the login.
- **One file signs.** `adapters/sigv4.rs` is the only file that names
  `aws_sigv4` (`tests/architecture/`): the request signature of `kurama api`
  and the presigned URL a token is share the identity, the settings and the
  way an error is described.
- **The token names the database, not the tunnel.** Behind a tunnel the TCP
  target is a local port, and the token is still signed for the configured
  host and port: that is the name the database checks.
- **The region is decided before anything is asked**: `iam.region`, the
  region in the host name, then the AWS profile's. A token nobody can sign is
  not worth a TOTP, and not worth the tunnel's role and session either, so
  `open_target` plans the authentication before it opens the tunnel.
- **`tests/db/rds-iam-up.sh` makes the real thing.** It ran end to end on
  2026-09-21 and the checks of the real layer ran against what it made. One stack: an RDS MySQL
  and an RDS PostgreSQL with IAM authentication on, reachable from the one
  address that ran the script and from the bastion beside them, and the
  `iam_reader` user on both. RDS certificates are signed by Amazon's RDS
  authority, which no system trust store carries, so a section names the
  bundle the script downloads as `ca_file`. `rds-iam-down.sh` removes all of
  it, with no final snapshot.
- **No IAM without TLS.** The token is a password, and MySQL takes it through
  `mysql_clear_password`, which sends it as it is. The configuration refuses
  `tls = "disable"` with `[iam]`, and the plugin is enabled for IAM only.
- **What Aurora DSQL leaves out.** A real cluster answered 0A000 to
  `SET LOCAL statement_timeout`, `pg_backend_pid()` and `pg_cancel_backend()`
  on 2026-09-21, and the first of them failed every call before its statement
  ran. For a DSQL host the driver sends none of the three: the read guard is
  `BEGIN READ ONLY` alone, which DSQL enforces (25006), and the canceller has
  no backend to name, so it opens no second connection and confirms no stop.
  A read that outlives `--timeout` there is abandoned and fails as not
  confirmed to have stopped; DSQL's own limit on a transaction is what ends
  it. An `execute` is waited for, rolled back and reported as a change that
  was rolled back with nothing kept, which is the one true thing to say when
  the statement ran to its end and no stop reached it. The isolation reported
  is `repeatable read`, the one level DSQL has.

### What holds each claim

A sentence in this chapter that says "once", "before" or "only" is a promise
about behavior, and a promise nothing tests was how this chapter came to
publish three that a changed line would have broken unnoticed. Each one names
the test that fails when it stops being true; `tests/architecture/` checks
that the test exists, and each was shown to fail against the regression it
names before it was listed.

| Claim | Held by |
| --- | --- |
| A tunnel and a token under one AWS profile assume its role once | `db_iam_behind_a_tunnel_assumes_the_shared_role_once` |
| The token's region is decided before a role, a tunnel or 1Password is asked | `db_iam_without_a_region_is_refused_before_anything_is_asked` |
| The token is signed locally and no password is read | `db_iam_signs_its_token_locally_and_reads_no_password` |
| A section with a password and `[iam]` is refused | `db_iam_and_a_password_together_are_refused_by_the_configuration` |
| A DSQL host takes PostgreSQL and IAM, nothing else | `db_aurora_dsql_host_with_a_password_is_refused_by_the_configuration` |
| Only `<id>.dsql.<region>.on.aws` is a DSQL cluster | `an_aurora_dsql_endpoint_is_told_from_every_other_database_host` |
| The token is botocore's, byte for byte, port 443 included | `db_iam_token_names_the_port_even_when_a_url_would_drop_it` |
| The cleartext plugin is on for an IAM user only | `db_server_offers_the_cleartext_plugin_to_an_iam_user_only` |
| DSQL is sent no `statement_timeout` and no `pg_backend_pid`, and reports `repeatable read` | `db_server_sends_aurora_dsql_no_setting_it_refuses` |
| A change asked to stop runs no further statement and sends no COMMIT | `db_server_asked_to_stop_sends_no_further_statement_and_no_commit` |
| Only a read nobody could stop is abandoned | `db_only_a_read_nobody_could_stop_is_abandoned` |
| A change rolled back after a stop says nothing was kept, never that the database stopped it | `db_a_change_rolled_back_after_a_stop_nobody_confirmed_says_nothing_was_kept` |
| The server's own limit ends one second after the client deadline | `db_server_limit_ends_after_the_client_deadline` |
| A read that answers after its stop was asked fails; a write that answers stands | `db_a_read_answered_after_its_stop_was_asked_is_not_a_result` |
| Only `true` from `pg_cancel_backend` confirms a stop | `db_postgres_cancel_false_is_not_confirmed` |

## Cells

Every cell is a JSON string or null, never a JSON number: an integer, a REAL
and a DECIMAL keep the digits the database holds. SQLite types values and not
columns, so the storage class is read per cell, `columns[].data_type` is the
declared type (null for an expression or an undeclared column), and
`columns[].encoding` is `base64` when at least one cell of that column came
back binary. A column SQLite declares `NUMERIC` or `DECIMAL` was already
stored as a float by SQLite before kurama read it; the lost digits are lost in
the file, not in the client.

## Writing

A write is refused twice before a connection is opened: the section has to say
`allow_write = true`, and the call has to say `--rollback` or `--commit`. Both
checks are in `commands/db.rs`, before `DbSession::open`, so a database nobody
meant to change is never connected to. A SQLite file named as a path builds a
connection with `allow_write: false`, so no path is ever writable.

`DbSession::open` takes the write flag: it is what decides whether the SQLite
file is opened read-only, so the read guard and the write path cannot be the
same connection by accident. Every statement runs in one transaction, prepared
one at a time so a second statement is still refused, and one failure rolls
back all of them. `max_affected_rows` is checked per statement, inside the
transaction, so the rollback happens before the commit is even considered. It
is a limit like the display bounds: the section that granted `allow_write` sets
it for every write it allows, and a call may only be stricter (`DbLimits`
resolves both in one place and takes the smaller). A read reports no such
bound, because none of them applies to it. A
commit whose answer never arrives is `DbFailure::CommitUnknown` and
`meta.transaction.outcome: "unknown"`: the only honest thing left to say.

## Bounds and completion

`query_timeout_secs` is at most 86,400: a deadline is `Instant + Duration`, and
a number nobody can wait out is an overflow rather than a long wait, so it is
`DB_INVALID` before anything is opened. It lives in `domain/types/limits.rs`
with every other bound external input is cut to.

`max_rows` and `max_result_bytes` cut the stream as it is read. A preview asks
SQLite for one row past its bound, so a full window is reported as
`truncated` and still exits 0: it is a window, not a partial answer. Any other
truncation prints the partial result on stdout, one error document on stderr,
and exits 1 -- including an `execute` whose `RETURNING` rows were cut: the
change was made, and what came back is not all of what it changed.
`--schemas` and `--tables` page instead, with `--limit` and a
`meta.next_cursor` bound to the database and the listing that produced it.

## The script this was for

`tests/db/delete-account.sh` is the shape an account-deletion script over MySQL has to have,
and it is kept here because the thing worth checking is what it does not run:
no `mysql`, no `op`, no `aws`, no `nc`, no `jq`. The JSON is
printed with a shell builtin rather than `cat`, so `kurama` is the only
program on its PATH. Without `--execute` it runs every statement and rolls
back, which is how the change is measured before it is made.

On 2026-09-21 it ran twice: against the Docker MySQL, and through a real SSM
tunnel to a bastion created by `bastion.yaml` with
`KURAMA_BASTION_ENGINE=mysql`. Both times it reported two addresses and one
account, kept nothing on the measuring pass, refused a mistyped confirmation,
and committed exactly those three rows on the second.

## Against a real server

`cargo xtask db-up` starts PostgreSQL 17, PostgreSQL 18 and MySQL 8.4 in
Docker with TLS on and loads `tests/db/*.sql`;
`KURAMA_TEST_DB=1 cargo test --features test-fakes --test real_db` then runs
`tests/real_db.rs` against all three. Without the variable each test says it
did not run and returns, so the gate never needs Docker. `cargo xtask db-down`
removes them.

This is not a nicety. A fake cannot say what bytes a server sends, which quote
it reads or what it calls an error, and the first run found four defects that
every other test had passed:

- MySQL identifiers were quoted the ANSI way, so `--preview` was a syntax
  error on every MySQL table.
- MySQL errors were classified by SQLSTATE while the code matches on MySQL's
  own numbers, so an unknown table pointed at nothing and a cancelled
  statement was reported as a rejection.
- Preparing a statement a second time, to learn its column names, dropped the
  parameter declarations: PostgreSQL then inferred each parameter's type from
  where it was used and refused the text that arrived as a malformed message.
- The contract named `42883` for a parameter that needs a cast, which is only
  true once the declaration is there.

Three of the four are now prevented by shape rather than by care: preparing
happens in one function that always declares the parameters
(`tests/architecture/database.rs::a_server_statement_is_prepared_in_one_place`), the
per-engine choices of quote and error code are matches with no catch-all, so a
new engine has to answer them, and every engine the configuration names must
have a test against a real database of its kind
(`every_database_engine_has_a_real_database_test`).

## What is not verified
- A private development database behind a real bastion has not been read,
  because that bastion's agent has been offline since 2026-09-19. Everything the script does was
  rehearsed against a bastion of the same shape:
  `KURAMA_BASTION_ENGINE=mysql tests/db/bastion-up.sh` puts MySQL 8.4 behind
  one, and `tests/db/delete-account.sh` -- the script the database client was
  built for -- measured and then made the change through a real Session Manager
  tunnel on 2026-09-21, with `kurama` as the only program it ran.
- MySQL's `SLEEP()` is not stopped by `KILL QUERY`: it returns 1 as if it had
  finished. The client deadline is what ends such a statement, and the
  real-database test cancels a statement that really works instead. The
  deadline path itself -- the command's deadline, the cancel, the failure it
  reports -- runs against PostgreSQL 17 (`pg_sleep`) and MySQL 8.4 (`SLEEP`)
  in the local layer of `db_server_query_deadline_exercises_cancellation`.
- The lock path (`DB_FAILED` with `retry: "read_only_retry"`) is reachable
  only when another process holds the write lock; the unit tests cover the
  classification, not a contended file.
- IAM authentication was verified against real servers on 2026-09-21: an
  Aurora DSQL cluster (both actions, every operation, a write, the guards),
  and the RDS PostgreSQL 17.9 and RDS MySQL 8.4.9 of `tests/db/rds-iam.yaml`,
  each directly under `verify-full` and through a real Session Manager tunnel
  under `verify-ca`. The MySQL runs are what prove `mysql_clear_password`,
  which no MySQL in Docker asks for. Not verified: an Aurora (non-DSQL)
  cluster, which takes the same token as RDS, and that a tunnel and a token
  under one profile cost one AssumeRole in a real run -- the one run that
  counted them lost its connection to SSM before the token was signed, so the
  unit test is what holds that.
