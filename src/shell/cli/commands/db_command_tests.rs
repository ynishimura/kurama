//! Database CLI parsing: which operation a call names, what it refuses to mix,
//! and what it never puts in a message.
use super::*;
use crate::shell::cli::args::build_command;

fn parse(args: &[&str]) -> Result<DbCommand, clap::Error> {
    build_command()
        .try_get_matches_from(
            std::iter::once("kurama")
                .chain(std::iter::once("db"))
                .chain(args.iter().copied()),
        )
        .map(|matches| {
            DbCommand::parse(matches.subcommand_matches("db").expect("the db subcommand"))
        })
}

/// The failure of a call, without asking a request to be printable: a request
/// holds SQL and parameters, which never reach a message.
fn failure(result: Result<DbRequest, DbError>) -> DbError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("the request was accepted"),
    }
}

fn request_of(args: &[&str]) -> DbRequest {
    parse(args)
        .expect("accepted arguments")
        .read_request()
        .expect("a valid request")
}

#[test]
fn db_each_flag_names_the_operation_it_runs() {
    assert!(matches!(
        request_of(&["app", "--schemas"]),
        DbRequest::Schemas(_)
    ));
    assert!(matches!(
        request_of(&["app", "--tables"]),
        DbRequest::Tables(_)
    ));
    let describe = request_of(&["app", "--describe", "public.orders"]);
    assert!(matches!(describe, DbRequest::Describe(_)));
    assert_eq!(describe.args().schema.as_deref(), Some("public"));
    assert_eq!(describe.args().table.as_deref(), Some("orders"));
    let preview = request_of(&["app", "--preview", "orders", "--columns", "id"]);
    assert!(matches!(preview, DbRequest::Preview(_)));
    assert_eq!(preview.args().schema, None);
    assert_eq!(preview.args().columns, ["id"]);
    let query = request_of(&["app", "--query", "SELECT 1", "--param", "a", "--param", "b"]);
    assert!(matches!(query, DbRequest::Query(_)));
    assert_eq!(
        query.args().params,
        [Some("a".to_owned()), Some("b".to_owned())]
    );
    // A dry run names the cheapest catalog read, because nothing will run.
    assert!(matches!(
        request_of(&["app", "--dry-run"]),
        DbRequest::Schemas(_)
    ));
}

#[test]
fn db_needs_a_database_and_an_operation() {
    assert!(parse(&[]).is_err(), "DATABASE is required");
    let error = failure(parse(&["app"]).expect("clap accepts it").read_request());
    assert!(matches!(
        error,
        DbError::Invalid(InvalidDb::OperationRequired)
    ));
    assert!(InvalidDb::OperationRequired.hint().is_some());
}

#[test]
fn db_operations_and_a_request_document_are_exclusive() {
    for conflicting in [
        &["app", "--tables", "--schemas"][..],
        &["app", "--query", "SELECT 1", "--tables"],
        &["app", "--describe", "orders", "--preview", "orders"],
        &["app", "--query", "SELECT 1", "--file", "/q.sql"],
        &["app", "--request", "-", "--tables"],
        &["app", "--request", "-", "--param", "a"],
        &["app", "--request", "-", "--max-rows", "10"],
        // A write is not a listing, a preview or a qualified name; these used
        // to be dropped on the floor.
        &[
            "app",
            "--execute",
            "DELETE FROM t",
            "--rollback",
            "--limit",
            "10",
        ],
        &[
            "app",
            "--execute",
            "DELETE FROM t",
            "--rollback",
            "--cursor",
            "abc",
        ],
        &[
            "app",
            "--execute",
            "DELETE FROM t",
            "--rollback",
            "--schema",
            "main",
        ],
        &[
            "app",
            "--execute",
            "DELETE FROM t",
            "--rollback",
            "--columns",
            "id",
        ],
    ] {
        assert!(parse(conflicting).is_err(), "{conflicting:?} was accepted");
    }
    // The one call it does belong to still takes it.
    assert!(
        parse(&[
            "app",
            "--execute",
            "DELETE FROM t",
            "--rollback",
            "--max-affected-rows",
            "1"
        ])
        .is_ok()
    );
}

#[test]
fn db_numeric_options_stay_inside_their_range() {
    for invalid in [
        &["app", "--tables", "--limit", "0"][..],
        &["app", "--tables", "--limit", "1001"],
        &["app", "--tables", "--max-rows", "0"],
        &["app", "--tables", "--max-result-bytes", "1"],
        &["app", "--tables", "--timeout", "0"],
        // A deadline is a point in time; past this the addition overflows.
        &["app", "--tables", "--timeout", "86401"],
        &["app", "--tables", "--timeout", "18446744073709551615"],
    ] {
        assert!(parse(invalid).is_err(), "{invalid:?} was accepted");
    }
    let request = request_of(&["app", "--tables", "--limit", "1000", "--max-rows", "1"]);
    assert_eq!(request.list_limit(), 1000);
    assert_eq!(request.args().max_rows, Some(1));
}

#[test]
fn db_a_path_is_a_sqlite_file_and_a_name_is_a_configured_database() {
    for path in [
        "./fixtures/app.sqlite3",
        "/tmp/app.db",
        "app.SQLITE",
        "dir/app",
    ] {
        assert!(is_sqlite_file(path), "{path}");
    }
    for name in ["app", "orders-dev", "app_pg"] {
        assert!(!is_sqlite_file(name), "{name}");
    }
}

#[test]
fn db_a_request_document_carries_the_operation_instead_of_the_flags() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("request.json");
    std::fs::write(
        &path,
        br#"{"operation":"query","args":{"sql":"SELECT 1","params":["a",null]}}"#,
    )
    .expect("a request file");
    let request = request_of(&["app", "--request", path.to_str().unwrap()]);
    assert!(matches!(request, DbRequest::Query(_)));
    assert_eq!(request.args().params, [Some("a".to_owned()), None]);

    let missing = failure(
        parse(&["app", "--request", "/nope/request.json"])
            .unwrap()
            .read_request(),
    );
    assert!(matches!(
        missing,
        DbError::Invalid(InvalidDb::RequestFile { .. })
    ));

    std::fs::write(&path, b"{\"operation\":\"vacuum\"}").expect("an unknown operation");
    let invalid = failure(
        parse(&["app", "--request", path.to_str().unwrap()])
            .unwrap()
            .read_request(),
    );
    assert!(matches!(
        invalid,
        DbError::Invalid(InvalidDb::InvalidRequest)
    ));
}

#[test]
fn db_a_missing_sql_file_is_a_usage_failure_and_a_plan_never_reads_one() {
    let command = parse(&["app", "--file", "/nope/query.sql"]).unwrap();
    assert!(matches!(
        failure(command.read_request()),
        DbError::Invalid(InvalidDb::RequestFile { .. })
    ));
    let planned = parse(&["app", "--file", "/nope/query.sql", "--dry-run"]).unwrap();
    assert!(planned.read_request().is_ok(), "a plan opened the file");
}

#[test]
fn db_debug_never_prints_the_statement_or_its_parameters() {
    let command = parse(&[
        "app",
        "--query",
        "SELECT * FROM users WHERE token = ?",
        "--param",
        "SECRET_TOKEN",
    ])
    .unwrap();
    let printed = format!("{command:?}");
    assert!(!printed.contains("SECRET_TOKEN"), "{printed}");
    assert!(!printed.contains("SELECT"), "{printed}");
}

#[test]
fn db_jq_implies_json_so_stdout_stays_one_document() {
    let command = parse(&["app", "--tables", "--jq", ".rows"]).unwrap();
    assert!(command.json);
    assert_eq!(command.jq.as_deref(), Some(".rows"));
    assert!(!parse(&["app", "--tables"]).unwrap().json);
}

/// A guard belongs to the call it guards. `--max-affected-rows` on a read
/// bounded nothing and said nothing, so a misspelled operation ran an
/// unbounded write with the flag written out beside it.
#[test]
fn db_a_write_bound_is_refused_on_a_call_that_changes_nothing() {
    for read in [
        &["app", "--tables", "--max-affected-rows", "1"][..],
        &["app", "--query", "SELECT 1", "--max-affected-rows", "1"],
        &["app", "--describe", "orders", "--max-affected-rows", "1"],
    ] {
        let command = parse(read).expect("the arguments parse");
        assert!(
            matches!(
                failure(command.read_request()),
                DbError::Invalid(InvalidDb::ExecuteOptionsOnly)
            ),
            "{read:?} was accepted"
        );
    }
    // The one call it does belong to still takes it.
    let write = parse(&[
        "app",
        "--execute",
        "DELETE FROM t",
        "--rollback",
        "--max-affected-rows",
        "1",
    ])
    .expect("the arguments parse");
    assert!(write.read_request().is_ok());
}

/// Only a call that names nothing to run opens the explorer; anything that
/// names an operation, a request, JSON, a plan or a transaction is the CLI.
#[test]
fn db_only_a_bare_database_name_opens_the_explorer() {
    let bare = |args: &[&str]| parse(args).expect("accepted").names_nothing_to_run();
    assert!(bare(&["app"]));
    for args in [
        &["app", "--tables"][..],
        &["app", "--request", "-"],
        &["app", "--json"],
        &["app", "--jq", ".meta"],
        &["app", "--dry-run"],
        &["app", "--rollback"],
        &["app", "--commit"],
        &["app", "--max-affected-rows", "1"],
    ] {
        assert!(!bare(args), "{args:?}");
    }
}
