//! The reviewed DuckDB and SQLite entry points, one place that prepares a
//! server statement, and a real-database test per engine.

use std::collections::BTreeSet;

use crate::support::*;

pub(crate) const RUST_ENTRY_POINTS: &[&str] = &[
    "duckdb_create_config",
    "duckdb_set_config",
    "duckdb_destroy_config",
    "duckdb_open_ext",
    "duckdb_connect",
    "duckdb_disconnect",
    "duckdb_close",
    "duckdb_query",
    "duckdb_destroy_result",
    "duckdb_free",
    "duckdb_value_varchar",
    "duckdb_extract_statements",
    "duckdb_destroy_extracted",
    "duckdb_prepare_extracted_statement",
    "duckdb_destroy_prepare",
    "duckdb_prepared_statement_type",
    "duckdb_execute_prepared_streaming",
    "duckdb_fetch_chunk",
    "duckdb_destroy_data_chunk",
    "duckdb_destroy_arrow_options",
    "duckdb_destroy_logical_type",
    "duckdb_column_count",
    "duckdb_column_name",
    "duckdb_get_type_id",
    "duckdb_result_error",
    "duckdb_result_error_type",
    "duckdb_interrupt",
    "duckdb_library_version",
];

pub(crate) const RAW_TYPES_AND_CONSTANTS: &[&str] = &[
    "duckdb_connection",
    "duckdb_database",
    "duckdb_config",
    "duckdb_result",
    "duckdb_prepared_statement",
    "duckdb_extracted_statements",
    "duckdb_data_chunk",
    "duckdb_arrow_options",
    "duckdb_logical_type",
    "duckdb_statement_type_DUCKDB_STATEMENT_TYPE_SELECT",
    "duckdb_error_type_DUCKDB_ERROR_PARSER",
    "duckdb_error_type_DUCKDB_ERROR_BINDER",
    "duckdb_error_type_DUCKDB_ERROR_INTERRUPT",
    "duckdb_error_type_DUCKDB_ERROR_HTTP",
];

pub(crate) const BRIDGE_ENTRY_POINTS: &[&str] = &[
    "duckdb_result_get_arrow_options",
    "duckdb_column_logical_type",
    "duckdb_get_type_id",
    "duckdb_list_type_child_type",
    "duckdb_array_type_child_type",
    "duckdb_struct_type_child_type",
    "duckdb_map_type_key_type",
    "duckdb_map_type_value_type",
    "duckdb_union_type_member_type",
    "duckdb_to_arrow_schema",
    "duckdb_data_chunk_to_arrow",
    "duckdb_error_data_has_error",
    "duckdb_destroy_error_data",
];

pub(crate) const BRIDGE_EXPORTS: &[&str] = &[
    "kurama_arrow_options",
    "kurama_column_type",
    "kurama_child_type",
    "kurama_arrow_schema",
    "kurama_arrow_array",
];

/// The DuckDB C names a Rust source reaches that no reviewed list holds,
/// read as syntax (`syntax.rs`): `libduckdb_sys::duckdb_*`, under the `ffi`
/// alias or not, in a path, a `use` or a macro's tokens -- a function value
/// handed to a RAII macro counts -- and a call of a `kurama_*` bridge
/// function that is not a reviewed export.
fn unreviewed_duckdb_calls(source: &str) -> Vec<(usize, String)> {
    let reading = crate::syntax::read_source(source);
    let mut found = Vec::new();
    for path in &reading.paths {
        for pair in path.segments.windows(2) {
            let name = pair[1].as_str();
            if !name.starts_with("duckdb_") {
                continue;
            }
            let unreviewed = ["ffi", "libduckdb_sys"].contains(&pair[0].as_str())
                && !RUST_ENTRY_POINTS.contains(&name)
                && !RAW_TYPES_AND_CONSTANTS.contains(&name);
            if unreviewed {
                found.push((path.line, path.text()));
            }
        }
    }
    for (name, line) in &reading.calls {
        if name.starts_with("kurama_") && !BRIDGE_EXPORTS.contains(&name.as_str()) {
            found.push((*line, name.clone()));
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn an_unreviewed_duckdb_name_in_any_form_is_found() {
    for source in [
        "fn f() { unsafe { ffi::duckdb_unreviewed(x) }; }",
        "fn f() {\n    unsafe {\n        ffi::\n            duckdb_unreviewed(x)\n    };\n}",
        "fn f() { guard!(ffi::duckdb_unreviewed); }",
        "use libduckdb_sys::duckdb_unreviewed;",
        "use libduckdb_sys as raw;\nfn f() { unsafe { raw::duckdb_unreviewed(x) }; }",
        "fn f() { kurama_unreviewed_export(x); }",
    ] {
        assert_detected(
            "ARCH-009",
            !unreviewed_duckdb_calls(source).is_empty(),
            source,
        );
    }
}

#[test]
fn a_reviewed_duckdb_name_a_comment_or_a_test_is_not_found() {
    for source in [
        "fn f() { unsafe { ffi::duckdb_query(c, q, r) }; guard!(ffi::duckdb_destroy_result); }",
        "struct S(ffi::duckdb_result);",
        "fn f() { kurama_arrow_array(x); }",
        "// ffi::duckdb_unreviewed(x)\nfn f() -> &'static str { \"ffi::duckdb_unreviewed\" }",
        "#[cfg(test)]\nmod tests {\n    fn f() { unsafe { ffi::duckdb_unreviewed(x) }; }\n}",
    ] {
        assert_allowed(
            "ARCH-009",
            unreviewed_duckdb_calls(source).is_empty(),
            source,
        );
    }
}

/// DuckDB's C surface is not uniformly exception-safe. The allocating Arrow
/// calls stay in the noexcept bridge; every other entry point and owned handle
/// below has an explicit error/null and destruction contract at its call site.
#[test]
fn only_reviewed_duckdb_entry_points_are_called() {
    fn identifiers<'a>(source: &'a str, prefix: &str) -> Vec<&'a str> {
        source
            .match_indices(prefix)
            .map(|(start, _)| {
                let symbol = &source[start..];
                &symbol[..symbol
                    .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                    .unwrap_or(symbol.len())]
            })
            .collect()
    }
    let mut unexpected = Vec::new();
    for path in rust_files(&root().join("src/adapters/duckdb")) {
        if production_code(&path).is_none() {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        unexpected.extend(
            unreviewed_duckdb_calls(&source)
                .into_iter()
                .map(|(line, name)| format!("{}:{line}: {name}", path.display())),
        );
    }
    let bridge =
        std::fs::read_to_string(root().join("src/adapters/duckdb/arrow_bridge.cpp")).unwrap();
    for name in identifiers(&bridge, "duckdb_") {
        let called = bridge
            .match_indices(name)
            .any(|(index, _)| bridge[index + name.len()..].trim_start().starts_with('('));
        if called && !BRIDGE_ENTRY_POINTS.contains(&name) {
            unexpected.push(format!("arrow_bridge.cpp: {name}"));
        }
    }
    for function in bridge.split("extern \"C\"").skip(1) {
        let (signature, body) = function
            .split_once('{')
            .expect("bridge function has a body");
        assert!(
            signature.contains("noexcept") && body.contains("catch (...)"),
            "every Arrow bridge entry point catches C++ exceptions: {signature}"
        );
        for name in identifiers(signature, "kurama_") {
            assert!(
                BRIDGE_EXPORTS.contains(&name),
                "unreviewed Arrow bridge export: {name}"
            );
        }
    }
    assert!(
        unexpected.is_empty(),
        "review DuckDB C++ exception safety, null handling and ownership before adding a C entry point:\n{}",
        unexpected.join("\n")
    );
}

pub(crate) const SQLITE_ENTRY_POINTS: &[&str] = &[
    "sqlite3_prepare_v2",
    "sqlite3_finalize",
    "sqlite3_column_count",
    "sqlite3_column_name",
    "sqlite3_column_decltype",
    "sqlite3_errmsg",
];

/// Whether a source reaches SQLite's C API.
fn reaches_sqlite(source: &str) -> bool {
    source.contains("libsqlite3_sys")
}

/// The `sqlite3_*` names of the reviewed file that no reviewed list holds.
fn unreviewed_sqlite_names(source: &str) -> Vec<&str> {
    source
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|name| name.starts_with("sqlite3_"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|name| !SQLITE_ENTRY_POINTS.contains(name) && *name != "sqlite3_stmt")
        .collect()
}

#[test]
fn an_unreviewed_sqlite_call_or_a_second_file_reaching_sqlite_is_found() {
    let source =
        "unsafe { ffi::sqlite3_prepare_v2(db, sql, n, &mut stmt, tail); ffi::sqlite3_step(stmt) }";
    assert_detected(
        "ARCH-034",
        unreviewed_sqlite_names(source) == ["sqlite3_step"],
        source,
    );
    let elsewhere = "use libsqlite3_sys as ffi;";
    assert_detected("ARCH-034", reaches_sqlite(elsewhere), elsewhere);
}

#[test]
fn a_reviewed_sqlite_call_is_allowed() {
    let source = "unsafe { ffi::sqlite3_prepare_v2(db, sql, n, &mut stmt, tail); ffi::sqlite3_finalize(stmt); }\nstruct S(*mut ffi::sqlite3_stmt);";
    assert_allowed(
        "ARCH-034",
        unreviewed_sqlite_names(source).is_empty(),
        source,
    );
    assert_allowed("ARCH-034", !reaches_sqlite("use sqlx::SqlitePool;"), "sqlx");
}

/// SQLite is reached through sqlx everywhere but one file, which asks SQLite
/// itself how many statements a piece of SQL is. Prepare/finalize without a
/// step is the whole reason that file exists; anything else through the C API
/// would be a second way to talk to the database.
#[test]
fn only_reviewed_sqlite_entry_points_are_called() {
    let reviewed = root().join("src/adapters/database/sqlite_statements.rs");
    let mut problems = Vec::new();
    for path in rust_files(&root().join("src")) {
        let source = std::fs::read_to_string(&path).unwrap();
        if path != reviewed && reaches_sqlite(&source) {
            problems.push(format!(
                "{}: call SQLite through sqlx, or move the C call into \
                 src/adapters/database/sqlite_statements.rs",
                path.display()
            ));
        }
    }
    let source = std::fs::read_to_string(&reviewed).unwrap();
    for name in unreviewed_sqlite_names(&source) {
        problems.push(format!(
            "{name} is not in SQLITE_ENTRY_POINTS: review it and add it, or do not call it"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// A server statement is prepared in exactly one place, which is what declares
/// its parameter types.
///
/// Preparing somewhere else, to learn the column names, is how the parameters
/// lost their declaration once already: PostgreSQL then inferred a type from
/// where each one was used and refused the text that arrived as a malformed
/// message. One place cannot disagree with itself.
#[test]
fn a_server_statement_is_prepared_in_one_place() {
    let source = std::fs::read_to_string(root().join("src/adapters/database/server.rs")).unwrap();
    let prepared = source.matches("as Executor>::prepare").count();
    assert_eq!(
        prepared, 2,
        "the two engines prepare once each, inside `ServerSession::prepare`; \
         a third call is a second place that decides the parameter types"
    );
    let body = source
        .split("async fn prepare(")
        .nth(1)
        .expect("ServerSession::prepare");
    let end = body.find("\n    }\n").expect("the end of prepare");
    assert_eq!(
        body[..end].matches("as Executor>::prepare").count(),
        2,
        "both calls belong to `ServerSession::prepare`"
    );
}

/// A database failure never holds a bare `String`.
///
/// `Unreachable(String)` and `TunnelUnavailable(String)` were what let seven
/// places build the same failure and five of them forget that the text is a
/// driver's, an SDK's or a child process's: untrusted, possibly several lines,
/// of no length. The rule of an `error[CODE]:` line is one bounded line, and
/// `Detail` is the only way to put such text into a `DbError`, so the rule is
/// kept by the type instead of by whoever writes the next adapter. The
/// sanitizing then lives in one function rather than in a copy per adapter.
#[test]
fn a_database_failure_carries_no_bare_string() {
    let source =
        std::fs::read_to_string(root().join("src/domain/types/database_error.rs")).unwrap();
    // All three, not only `DbError`: a `DbFailure` and an `InvalidDb` reach
    // the same `error[CODE]:` line, and both already carried a column name and
    // a child process's stdout as bare `String`s.
    let mut bare = Vec::new();
    for name in ["DbError", "DbFailure", "InvalidDb"] {
        let body = source
            .split(&format!("pub enum {name} {{"))
            .nth(1)
            .unwrap_or_else(|| panic!("the {name} enum"))
            .split("\n}")
            .next()
            .expect("its variants");
        for line in body.lines() {
            // Any shape a `String` can take in a variant, not two of them:
            // `(String)`, `{ x: String }`, `Option<String>`, `Vec<String>`
            // and the same folded across lines all say the text was never cut
            // to one line.
            if line.trim_start().starts_with("//") || line.contains("#[error") {
                continue;
            }
            if line.contains("String") {
                bare.push(format!("{name}: {}", line.trim()));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "text a driver, an SDK or a child process wrote reaches an error line \
         through `Detail`, which is what makes it one bounded line:\n{}",
        bare.join("\n")
    );
    // And no adapter keeps its own copy of the one-lining `Detail` does.
    // Turning a control character into a space is that; escaping one so a
    // terminal prints it (`db_render`, `data_render`) is a different job.
    let mut copies = Vec::new();
    for dir in ["src/adapters", "src/shell"] {
        for path in rust_files(&root().join(dir)) {
            let Some(code) = production_code(&path) else {
                continue;
            };
            let allowed = path.ends_with("cli/client_error.rs");
            for (number, line) in code.lines().enumerate() {
                if line.contains("if c.is_control() { ' ' }") && !allowed {
                    copies.push(location(&path, number, line));
                }
            }
        }
    }
    assert!(
        copies.is_empty(),
        "untrusted text is cut to one line in one place: `Detail` for a database \
         failure, and `client_error` for the JSON error document:\n{}",
        copies.join("\n")
    );
}

/// A choice made per engine is made for every engine by name.
///
/// `quoting()` already says why: an engine added to `DbEngine` has to answer
/// which quote it reads, and a `_ =>` arm answers for it with whatever the
/// previous engine needed. MySQL got PostgreSQL's identifier quote exactly
/// that way, and only a real server said so. This is the rule
/// `AGENTS.md` and `docs/development/database.md` publish; six of the seven
/// matches did not keep it.
#[test]
fn a_choice_made_per_engine_names_every_engine() {
    let mut found = Vec::new();
    for dir in ["src/adapters/database", "src/adapters/config"] {
        for path in rust_files(&root().join(dir)) {
            let Some(code) = production_code(&path) else {
                continue;
            };
            let lines: Vec<&str> = code.lines().collect();
            // A `match` whose scrutinee names an engine. Each of its arms
            // names one; anything else -- `_`, a binding like `other` --
            // answers for an engine nobody has added yet, which is what this
            // forbids.
            for reading in crate::syntax::read_matches(&code) {
                if !reading.scrutinee.iter().any(|name| name.contains("engine")) {
                    continue;
                }
                for arm in reading.arms {
                    if !arm.names.iter().any(|name| name == "DbEngine") {
                        found.push(location(&path, arm.line - 1, lines[arm.line - 1]));
                    }
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "a per-engine choice names every engine, so adding one stops the build \
         instead of silently giving it another engine's answer:\n{}",
        found.join("\n")
    );
}

/// Every database engine is exercised against a real one of its own kind.
///
/// A fake cannot say what bytes a server sends, which quote it reads or which
/// number it calls an error; each of those was wrong once and only a real
/// server said so. An engine added without such a test is an engine nobody has
/// seen work.
#[test]
fn every_database_engine_has_a_real_database_test() {
    let config = std::fs::read_to_string(root().join("src/adapters/config/db.rs")).unwrap();
    let engines: Vec<String> = config
        .split("pub enum DbEngine {")
        .nth(1)
        .expect("the DbEngine enum")
        .split('}')
        .next()
        .expect("its variants")
        .lines()
        .filter_map(|line| {
            let name = line.trim().trim_end_matches(',');
            (!name.is_empty() && !name.starts_with("//")).then(|| name.to_lowercase())
        })
        .collect();
    assert!(engines.len() >= 3, "{engines:?}");
    // The list `real_db.rs` declares, not its whole text: every engine name
    // also appears in a doc comment there, so searching the file said MySQL
    // was covered even with `Server::MySql` removed from what runs.
    let real = std::fs::read_to_string(root().join("tests/real_db.rs")).unwrap();
    let declared = real
        .split("const REAL_ENGINES")
        .nth(1)
        .expect("REAL_ENGINES in tests/real_db.rs");
    // Past the type, which carries a `;` of its own, to the list itself.
    let declared = declared
        .split_once('=')
        .expect("REAL_ENGINES has a value")
        .1;
    let declared = declared[..declared.find(']').expect("its end")].to_lowercase();
    let missing: Vec<&String> = engines
        .iter()
        // SQLite is a file, and `sqlite_tests.rs` reads a real one.
        .filter(|engine| engine.as_str() != "sqlite")
        .filter(|engine| !declared.contains(&format!("\"{engine}\"")))
        .collect();
    assert!(
        missing.is_empty(),
        "these engines are named by the configuration and by no test against a \
         real database of that kind (tests/real_db.rs, or sqlite_tests.rs for a \
         file):\n{missing:?}"
    );
}
