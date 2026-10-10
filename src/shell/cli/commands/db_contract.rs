//! The offline database contract and the configured databases; no connection,
//! no credential lookup, no file is opened.
use crate::adapters::config::{Config, DbEngine};
use crate::domain::types::database::{
    DbError, DbLimits, DbOperation, DbOutput, DbRequest, InvalidDb,
};
use crate::domain::types::limits;
use serde_json::{Value, json};
use strum::VariantArray;

pub fn capabilities() -> Value {
    json!({
        "schema_version": 1,
        "kind": "db",
        "request_operations": DbOperation::REQUESTS.map(DbOperation::as_str),
        "result_operations": DbOperation::REQUESTS.map(DbOperation::as_str),
        "request_schema": schemars::schema_for!(DbRequest),
        "result_schema": schemars::schema_for!(DbOutput),
        "error_schema": crate::shell::cli::client_error::schema(),
        "defaults": DbLimits::default(),
        "list_default_limit": limits::DB_CALL.list_page,
        "engines": DbEngine::VARIANTS.iter().copied().map(DbEngine::as_str).collect::<Vec<_>>(),
        // `kurama db <DB>` with no operation, on a terminal: a person's
        // explorer over one connection. An agent never gets it: without a
        // terminal the same call is DB_INVALID.
        "capabilities": {"tui": true},
        "side_effects": {
            "schemas": "read the catalog",
            "tables": "read the catalog; it lists tables and views alike, and the type column says which",
            "describe": "read the catalog; its rows carry declared_type, which is the column's own type, not the type of the catalog result",
            "preview": "read rows",
            "query": "read rows",
            "execute --rollback": "runs every statement and keeps nothing; the locks are still taken",
            "execute --commit": "writes to the database",
            "dry_run": "none"
        },
        "write": {
            "allow_write": "execute is refused before a connection is opened unless the [db.*] section has allow_write = true; a SQLite file named as a path is never writable",
            "decision": "execute needs exactly one of --rollback and --commit, and neither is accepted on a read. Nothing prompts: writing --commit is the decision",
            "agent": "in a run whose environment sets KURAMA_AGENT, --commit also needs --confirm, which says a person agreed; without it the call is AGENT_POLICY_DENIED (exit 3) before a connection is opened, whatever allow_write says. --rollback needs nothing more",
            "transaction": "every statement runs in one transaction, in the order given; one failure rolls back all of them, and at most 50 statements make one transaction",
            "max_affected_rows": "when a statement changes more rows than this, the transaction is rolled back and the call fails, so a forgotten WHERE is caught before the commit. The [db.*] section sets it for every write it allows and the call may only be stricter; left out in both, a write is unbounded",
            "result": "statements[] carries index, rows_affected and the rows a RETURNING or SELECT produced; the SQL and its parameters are never reported, an index names them",
            "outcome": "meta.transaction.outcome is committed, rolled_back, or unknown when the commit was sent and its answer never arrived -- then read the rows to see what the database kept"
        },
        "examples": [
            {"operation": "tables", "args": {"limit": 100}},
            {"operation": "query", "args": {"sql": "SELECT status, count(*) FROM orders GROUP BY status", "max_rows": 100}},
            {"operation": "query", "args": {"sql": "SELECT * FROM orders WHERE id = ?", "params": ["42"]}},
            {"operation": "execute", "args": {"statements": [{"sql": "DELETE FROM orders WHERE id = ?", "params": ["42"]}], "max_affected_rows": 1}}
        ],
        "statement_rules": {
            "one_statement": "one call runs exactly one statement, so `COMMIT; DROP ...` cannot leave the read. On a server the engine refuses the second one when the statement is prepared (PostgreSQL 42601, MySQL 42000, DB_REJECTED). On SQLite kurama asks SQLite to count the statements first and refuses before anything runs (DB_INVALID), because SQLite would otherwise run them all",
            "rows_required": "query needs a statement that returns columns; one that returns none is DB_INVALID",
            "read_only": "a read cannot write. SQLite is opened read-only, so INSERT, CREATE, DROP, VACUUM, ATTACH of a new file and PRAGMA journal_mode are refused by the engine. PostgreSQL runs the statement in BEGIN READ ONLY with SET LOCAL statement_timeout; MySQL adds SET SESSION TRANSACTION READ ONLY, which is what refuses DDL, because DDL commits a transaction implicitly. A server statement can turn its own guard off, and there is no second statement in that connection to profit from it: the boundary that holds is the permissions of the database user",
            "params": "--param and args.params bind in order and are values, never SQL. A placeholder is $1, $2 in PostgreSQL and ? in MySQL and SQLite. A parameter arrives as text in PostgreSQL, so comparing it with a column of another type needs a cast in the statement ($1::int, $1::date); without one PostgreSQL answers 42883 operator does not exist: integer = text, and the hint names the cast. MySQL compares a string with a number or a date as it is. SQLite compares a string parameter with a column that declares a type, but against a column declared without one, write CAST(? AS INTEGER) or the comparison matches nothing instead of failing"
        },
        "credentials": {
            "username": "a literal name, or a reference to where it is kept",
            "password": "a reference only, never the password itself: a password in the file is a password in every backup of it",
            "references": {
                "op": "op://<vault>/<item>/<field>, read through the 1Password CLI",
                "aws-secrets": "aws-secrets://<aws-profile>/<secret-id>[?region=<region>][#<json-key>]: Secrets Manager GetSecretValue with the role of that AWS profile. The secret id is a name or a full ARN and may contain /; #<json-key> takes one top-level string of a JSON SecretString, which is the shape of an RDS managed secret",
                "aws-ssm": "aws-ssm://<aws-profile>/<parameter-name>[?region=<region>]: Parameter Store GetParameter with WithDecryption=true; the leading / of the name may be left out"
            },
            "region": "the ARN's region, then ?region=, then the AWS profile's; an ARN and a ?region= that disagree are CONFIG_INVALID",
            "reads": "each secret is read once per process, so ...#username and ...#password on one secret are one GetSecretValue. The AWS profile is assumed through the same path as `kurama env`, shared with the tunnel and the IAM token, so one profile is assumed once",
            "permission": "the role needs secretsmanager:GetSecretValue or ssm:GetParameter, plus kms:Decrypt for a customer-managed key or a SecureString",
            "errors": "SECRET_REJECTED (exit 4) is the store's refusal and the hint names the IAM action; SECRET_INVALID (exit 2) is a reference or a value that cannot be used (an unknown scheme, no such JSON key, a SecretString that is not JSON, SecretBinary); SECRET_FAILED (exit 1) is a store that could not be reached; SECRET_UNAVAILABLE (exit 3) is 1Password needing a person",
            "not_resolved": "status and --dry-run resolve nothing and call neither 1Password nor AWS"
        },
        "tunnel": {
            "section": "[db.<name>.tunnel] with kind = \"ssm\", aws_profile, and the bastion as instance_name (its Name tag) or instance_id; region defaults to the AWS profile's",
            "credentials": "the AWS profile is assumed through the same path as `kurama env`, with its session cache and 1Password TOTP",
            "order": "the Session Manager plugin is checked, then AWS is asked for a session, then the database username and password are read: a database nothing can reach is never worth a prompt",
            "plugin": "session-manager-plugin 1.2.536.0 or newer is required, because older releases take the session token on their command line where every process can read it; an older one is DB_INVALID",
            "tls": "a tunnel makes the connection target a local port, and one name is used both to connect and to check the certificate, so tls must be \"verify-ca\" or \"disable\"",
            "lifetime": "one tunnel per call; meta.tunnel names the kind, the instance and the region, and never the local port, which is gone when the call ends"
        },
        "iam": {
            "section": "[db.<name>.iam] with aws_profile, in place of password, for a database user that authenticates with IAM; a section with both is refused",
            "token": "the AWS profile is assumed through the same path as `kurama env`, and its role signs a token the database accepts for fifteen minutes; the token is signed here, each time a connection opens, and sent to the database only, never to AWS. A tunnel and a token under one AWS profile assume its role once",
            "permission": "the role needs rds-db:connect on arn:aws:rds-db:<region>:<account>:dbuser:<DbiResourceId>/<username> (RDS, Aurora), or dsql:DbConnect / dsql:DbConnectAdmin on the cluster (Aurora DSQL). Nothing asks AWS, so a missing permission is an authentication failure from the database (DB_REJECTED), never an AccessDenied",
            "region": "iam.region, then the region the host name carries (<id>.<region>.rds.amazonaws.com, <id>.dsql.<region>.on.aws), then the AWS profile's; none of them is DB_INVALID before any role is assumed, any tunnel is opened or 1Password is asked",
            "rds": "RDS and Aurora: the token is for one host, port and username, and behind a tunnel it is still signed for the configured host and port, which is the name the database checks. MySQL takes it through mysql_clear_password",
            "aurora_dsql": "a host <id>.dsql.<region>.on.aws is Aurora DSQL: engine = \"postgresql\" and an [iam] section, anything else is refused by the configuration; database = \"postgres\", and the username decides the action -- admin signs DbConnectAdmin, every other role DbConnect. DSQL has no statement_timeout, pg_backend_pid or pg_cancel_backend, so nothing can stop a statement there: a read that outlives --timeout or is interrupted is abandoned and fails as not confirmed to have stopped, while DSQL runs it to its own transaction limit, and an execute is waited for, rolled back and reported as a change that was rolled back with nothing kept. Its isolation is repeatable read",
            "tls": "the token is a password, so tls must not be \"disable\""
        },
        "server_stop": "a timeout or an interrupt stops the statement from a second connection (pg_cancel_backend, KILL QUERY); the failure says how the statement ended -- the database confirmed it stopped, nobody confirmed it, or the change was rolled back -- because dropping the client future alone leaves the server running it. A read nobody could stop is not waited for. An execute is: once a stop was asked it runs no further statement and sends no COMMIT, on every engine, so its failure means nothing was kept",
        "cells": {
            "representation": "every cell is a JSON string or null, exactly as the database stored it; an integer, a REAL and a DECIMAL are never converted to JSON numbers",
            "sqlite_types": "SQLite types values and not columns, so one column may hold text and integer cells; columns[].data_type is the declared type and is null when the column or the expression has none",
            "encoding": "columns[].encoding is base64 when at least one cell of that column came back binary; in a column that mixes binary and text cells the text cells are still literal",
            "numeric_precision": "a column SQLite declares NUMERIC or DECIMAL is stored as a float by SQLite itself, so digits it dropped on the way in are gone before kurama reads it; a PostgreSQL numeric and a MySQL DECIMAL keep every digit, and PostgreSQL NaN and Infinity come back as those words",
            "unsupported": "a type kurama has no reader for is DB_FAILED naming the column and the type, never a blank cell; cast it to text in the statement. PostgreSQL interval, inet, money and point are not read"
        },
        "not_found": {
            "database_file": "a SQLite path that does not exist is DB_INVALID, not DB_UNREACHABLE: no retry will make it appear. DB_UNREACHABLE is for a server that could not be reached, where retrying can work"
        },
        "paging": {
            "operations": ["schemas", "tables"],
            "cursor": "meta.next_cursor continues the same listing on the same database; another target or another operation is DB_INVALID",
            "statements": "preview and query have no cursor: narrow the statement or raise the bounds"
        },
        "completion_rules": {
            "preview": "a preview that stopped at max_rows is a full window and exits 0",
            "query": "a query cut by max_rows or max_result_bytes prints the partial result on stdout, one error document on stderr, and exits 1"
        },
        "output_options": {
            "jq": "--jq FILTER implies --json and projects the envelope to exactly one JSON value"
        },
        "result_envelope": {
            "schema_version": 1,
            "kind": "db",
            "operation": "operation name",
            "target": "the [db.*] name, or ad-hoc for a SQLite file given as a path",
            "meta": "limits, engine, host, database, transaction, consistency, observed_at, returned_rows, returned_result_bytes, next_cursor, next_actions",
            "columns": "name, data_type, encoding",
            "rows": "arrays of strings and nulls",
            "elapsed_ms": "integer"
        }
    })
}

/// One configured database, as far as configuration alone can describe it.
#[derive(Debug, Clone)]
pub struct DbStatusRow {
    pub name: String,
    pub engine: DbEngine,
    /// The file for SQLite, the database name for a server.
    pub database: String,
    /// `host:port` for a server; a file has none.
    pub host: Option<String>,
    pub allow_write: bool,
}

pub fn status_rows(config: &Config, name: Option<&str>) -> Vec<DbStatusRow> {
    config
        .db_connections()
        .filter(|(database, _)| name.is_none_or(|name| *database == name))
        .map(|(name, connection)| DbStatusRow {
            name: name.clone(),
            engine: connection.engine(),
            database: connection.database(),
            host: connection.host(),
            allow_write: connection.allow_write(),
        })
        .collect()
}

pub fn print_status(config: &Config, name: Option<&str>, json: bool) -> anyhow::Result<()> {
    let rows: Vec<_> = status_rows(config, name)
        .into_iter()
        .map(super::status::StatusRow::Db)
        .collect();
    if name.is_some() && rows.is_empty() {
        return Err(DbError::from(InvalidDb::UnknownDatabase).into());
    }
    super::status::print_rows(&rows, chrono::Utc::now(), json);
    Ok(())
}

#[cfg(test)]
#[path = "db_contract_tests.rs"]
mod tests;
