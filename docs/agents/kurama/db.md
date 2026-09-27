## `kurama db`

Start with `kurama agent --kind db --json` for the versioned capability
document and request schema. `kurama status --kind db --json` lists the
configured databases with `connection_status: "unknown"`; it opens nothing,
reads no credential and does not check that a file exists.

```bash
kurama db app --tables --json
kurama db app --describe public.orders --json
kurama db app --preview orders --columns id --columns status --max-rows 20
kurama db app --query 'SELECT status, count(*) FROM orders GROUP BY status' --jq '.rows'
kurama db app --query 'SELECT * FROM orders WHERE id = ?' --param 42 --json
kurama db app --request ./query.json --json
kurama db ./fixtures/app.sqlite3 --preview orders
kurama db app --dry-run --json
```

`kurama db <DATABASE>` with no operation opens an explorer for a person, and
only on a terminal: without one (an agent, a pipe, `TERM=dumb`) it is
`DB_INVALID` and exit 2, as `capabilities.tui` in the capability document
says. An agent names an operation.

`DATABASE` is a `[db.<name>]` section -- `engine = "sqlite"`, `"postgresql"`
or `"mysql"` -- or a path to a SQLite file: a value with a `/` or a `.db` /
`.sqlite` / `.sqlite3` extension is opened read-only with no configuration and
reports `target: "ad-hoc"`. One operation is
required and they are exclusive: `--schemas`, `--tables`, `--describe TABLE`,
`--preview TABLE`, `--query SQL` or `--file FILE`. `--request FILE` carries
the same operation as strict JSON
(`{"operation":"query","args":{"sql":"SELECT 1"}}`) and replaces every
operation argument; only `--request -` reads stdin, which is how a statement
stays out of argv and shell history. `--dry-run` prints the connection and
the planned operation without connecting, and without the statement or its
parameters.

**One call runs one statement**, so `COMMIT; DROP ...` cannot leave the read.
On a server the engine refuses the second one when the statement is prepared
(`DB_REJECTED`); on SQLite kurama asks SQLite to count the statements first
and refuses before anything runs (`DB_INVALID`), because SQLite would
otherwise run them all. `--query` refuses a statement that returns no
columns. A read cannot write: a SQLite database is opened
read-only, so `INSERT`, `CREATE`, `DROP`, `VACUUM`, `ATTACH` of a new file and
`PRAGMA journal_mode` are refused by SQLite and nothing appears on disk; a
server runs the statement in a read-only transaction, and MySQL also sets the
session read-only, which is what refuses DDL. A statement on a server can turn
its own guard off, and there is no second statement in that connection to
profit from it -- the boundary that holds is the permissions of the database
user.

**Writing is `--execute`, and it is refused twice before a connection is
opened.** The `[db.*]` section needs `allow_write = true` -- a SQLite file
named as a path never has it -- and the call needs exactly one of `--rollback`
and `--commit`; neither is accepted on a read. Nothing prompts: writing
`--commit` is the decision, and a script that wants a confirmation asks for
one itself. In a run whose environment sets `KURAMA_AGENT`, `--commit` also
needs `--confirm`, which says a person agreed: without it the call is
`AGENT_POLICY_DENIED` (exit 3) before a connection is opened, whatever
`allow_write` says; `--rollback` needs nothing more. A request writes several statements at once:

```json
{"operation":"execute","args":{"statements":[
  {"sql":"DELETE FROM aws_account_addresses WHERE aws_account_id = ?","params":["123456789012"]},
  {"sql":"DELETE FROM aws_accounts WHERE account_id = ?","params":["123456789012"]}],
 "max_affected_rows":10}}
```

They run in one transaction, in order, at most 50 of them; one failure rolls
back all. `--rollback` runs every statement and keeps nothing, which is how a
change is measured before it is made -- the locks are still taken.
`max_affected_rows` rolls back and fails when a statement reaches further than
that, so a forgotten `WHERE` is caught before the commit rather than after it.
The `[db.*]` section sets it for every write it allows, and the call may only
be stricter; left out in both, a write is unbounded.
`statements[]` reports `index`, `rows_affected` and the rows a `RETURNING` or
`SELECT` produced; the SQL and its parameters are never reported, and the
index is how to refer to them. `meta.transaction.outcome` is `committed`,
`rolled_back`, or `unknown` when the commit was sent and its answer never
arrived -- then read the rows to see what the database kept, rather than
sending it again.

`--param VALUE` binds the next placeholder, in order, and a parameter is a
value and never SQL; a request writes `"params": ["a", null]` for a null. The
placeholder is `$1`, `$2` in PostgreSQL and `?` in MySQL and SQLite. A
parameter arrives as text in PostgreSQL, so comparing it with a column of
another type needs a cast in the statement (`$1::int`, `$1::date`); without
one PostgreSQL answers `42883 operator does not exist: integer = text`, and
the `hint:` line names the cast. MySQL compares a string with a number or a
date as it is. SQLite compares a string parameter with a column that declares a type, but
against a column declared without one, write `CAST(? AS INTEGER)`: otherwise
the comparison matches nothing instead of failing.

Results keep ordered `columns` and positional `rows`, duplicate column names
included. Every cell is a JSON string or null, exactly as the database stored
it, so an integer, a REAL and a DECIMAL keep their digits.
`columns[].data_type` is the declared type and is null when the column or the
expression has none; `columns[].encoding` is `base64` when at least one cell
of that column came back binary. SQLite types values and not columns, so one
column may hold both text and integer cells, and a column it declares
`NUMERIC` or `DECIMAL` was stored as a float by SQLite before kurama read it.
A PostgreSQL `numeric` and a MySQL `DECIMAL` keep every digit, and PostgreSQL
`NaN` and `Infinity` come back as those words. A type kurama has no reader for
is `DB_FAILED` naming the column and the type, never a blank cell: cast it to
text in the statement. PostgreSQL `interval`, `inet`, `money` and `point` are
not read.

`--tables` lists tables and views alike; its `type` column says which.
`--describe` reports each column's own type as `declared_type`, which is not
the `data_type` of the envelope's own columns. A SQLite path that does not
exist is `DB_INVALID`, not `DB_UNREACHABLE`: no retry makes a file appear,
while `DB_UNREACHABLE` is a server that could not be reached and retrying can
work.

`--schemas` and `--tables` page: `--limit N` (default 100, at most 1000) and
`meta.next_cursor`, which `--cursor` continues. A cursor belongs to the
database and the listing that produced it; another one is `DB_INVALID`.
`--describe` and `--preview` and `--query` have no cursor: narrow the
statement or raise the bounds.

Default limits: 30 seconds, 1,000 result rows and 8 MiB of serialized rows,
each overridable with `--timeout`, `--max-rows` and `--max-result-bytes` or
in the section. A preview that stopped at `max_rows` is a full window and
exits 0. A `--query` cut by `max_rows` or `max_result_bytes` prints the
partial result on stdout, one error document on stderr, and exits 1: do not
treat it as complete. An `execute` whose `RETURNING` rows were cut the same way
reports the same: the change was made, and what came back is not all of it.
`--timeout` is at most 86,400 seconds; a larger one is `DB_INVALID`.
`meta.consistency` is `single_statement` for a read and `single_transaction`
for an `execute`, which is one transaction of several statements.

A database only a bastion can reach adds `[db.<name>.tunnel]` with
`kind = "ssm"`, an `aws_profile` and the bastion as `instance_name` (its
`Name` tag) or `instance_id`. The AWS profile is assumed through the same path
as `kurama env`, and the order is deliberate: the Session Manager plugin is
checked first, then AWS is asked for a session, and only then is the database
password read -- a database nothing can reach is never worth a prompt. The
plugin must be 1.2.536.0 or newer, because older releases take the session
token on their command line where every process can read it. A tunnel makes
the connection target a local port, so `tls` must be `"verify-ca"` or
`"disable"`. `meta.tunnel` names the kind, the instance and the region, never
the local port.

`username` and `password` are references to where the secret is kept:
`op://<vault>/<item>/<field>`, `aws-secrets://<aws-profile>/<secret-id>[#<key>]`
or `aws-ssm://<aws-profile>/<parameter-name>`, with `?region=` where the AWS
profile's region is not the secret's. `username` may also be written out;
`password` may not, because a password in the file is a password in every
backup of it. An RDS managed secret is one JSON document, so
`...#username` and `...#password` on the same secret are one
`GetSecretValue`; two `op://` fields of one item are one `op item get` the same
way. The AWS forms use the same AssumeRole path as `kurama env`,
shared with the tunnel and the IAM token, so one profile is assumed once.
`SECRET_REJECTED` (exit 4) names the IAM action the role needs,
`SECRET_INVALID` (exit 2) is a reference or a value that cannot be used, and
`SECRET_FAILED` (exit 1) is a store that could not be reached. The Setup
chapter has the full grammar.

A database user that authenticates with IAM has `[db.<name>.iam]` with an
`aws_profile` in place of `password`; a section with both is refused. The AWS
profile is assumed through the same path as `kurama env`, and its role signs a
token the database accepts for fifteen minutes. The token is signed locally
and goes to the database only. The role needs `rds-db:connect` on
`arn:aws:rds-db:<region>:<account>:dbuser:<DbiResourceId>/<username>` (RDS,
Aurora) or `dsql:DbConnect` / `dsql:DbConnectAdmin` on the cluster (Aurora
DSQL); nothing asks AWS, so a missing permission is an authentication failure
from the database, never an AccessDenied. Its region is `iam.region`, then the region the
host name carries, then the AWS profile's. For RDS and Aurora the token names
one host, port and username -- the configured ones, also behind a tunnel --
and `tls` must not be `"disable"`. A host `<id>.dsql.<region>.on.aws` is
Aurora DSQL: `engine = "postgresql"` with an `[iam]` section and nothing
else, `database = "postgres"`, and the username `admin` signs
`DbConnectAdmin` while every other role signs `DbConnect`. DSQL has no
`statement_timeout`, `pg_backend_pid` or `pg_cancel_backend`, so nothing can
stop a statement there: a read that outlives `--timeout` or is interrupted is
abandoned and fails as not confirmed to have stopped, and DSQL's own limit on
a transaction is what ends it. On every engine an `execute` that was asked to
stop runs no further statement and sends no COMMIT: its failure says the
change was rolled back and nothing was kept, which is what happened to it even
where no stop reached the statement.

```toml
[db.orders]
engine = "postgresql"
host = "orders.abc123.ap-northeast-1.rds.amazonaws.com"
database = "app"
username = "iam_reader"

[db.orders.iam]
aws_profile = "dev"
```

A timeout or an interrupt stops the statement from a second connection
(`pg_cancel_backend`, `KILL QUERY`), because dropping the client future leaves
the server running it; the failure says whether the database confirmed the
stop. A read that answers only after its stop was asked fails all the same:
MySQL answers a stopped `SLEEP()` with a row. The server's own limit is set
one second past `--timeout`, as a backstop for a client that went away.

Errors: `DB_UNREACHABLE` (exit 1) means the connection never opened, so
nothing was sent and `retry` is `read_only_retry`. `DB_INVALID` (exit 2) is the
call itself -- an unknown database, a
missing or conflicting operation, a file that is not a SQLite database, more
than one statement, a statement with no columns, a cursor from another
listing. `DB_FAILED` (exit 1) means the read did not finish: a timeout, an
interrupt, a limit, a lock that never cleared. `DB_REJECTED` (exit 4) is the
database's own refusal and carries `error.server` with its code and message,
and `next_actions` points at `tables` for an unknown table and `describe` for
an unknown column. `retry: "read_only_retry"` appears only where the type
says nothing was written. The statement and its parameters never appear in an
error, a log or an artifact.

