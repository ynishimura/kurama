//! Check the published database contract against its typed parser and output.
use super::*;
use crate::domain::types::database::{DbKind, DbMeta, DbRequest};

fn args_for(operation: &str) -> Value {
    match operation {
        "query" => json!({"sql": "SELECT 1"}),
        "describe" | "preview" => json!({"table": "orders"}),
        "execute" => {
            json!({"statements": [{"sql": "DELETE FROM orders WHERE id = ?", "params": ["1"]}]})
        }
        _ => json!({}),
    }
}

#[test]
fn db_capabilities_advertise_only_operations_the_parser_accepts() {
    let doc = capabilities();
    assert_eq!(doc["capabilities"]["tui"], json!(true));
    for operation in doc["request_operations"].as_array().unwrap() {
        let operation = operation.as_str().unwrap();
        let request: DbRequest =
            serde_json::from_value(json!({"operation": operation, "args": args_for(operation)}))
                .unwrap_or_else(|error| panic!("{operation}: {error}"));
        assert!(request.validate().is_ok(), "{operation}");
    }
    let schema_operations: Vec<_> = doc["request_schema"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| variant["properties"]["operation"]["const"].clone())
        .collect();
    assert_eq!(doc["request_operations"], json!(schema_operations));
    // Every published example is a request this binary accepts.
    for example in doc["examples"].as_array().unwrap() {
        let request: DbRequest = serde_json::from_value(example.clone())
            .unwrap_or_else(|error| panic!("{example}: {error}"));
        assert!(request.validate().is_ok(), "{example}");
    }
    // Every default names a field a request can override.
    let args = serde_json::to_value(schemars::schema_for!(
        crate::domain::types::database::DbArgs
    ))
    .unwrap();
    for key in doc["defaults"].as_object().unwrap().keys() {
        assert!(!args["properties"][key].is_null(), "unusable default {key}");
    }
    // The one operation that changes anything says so, and only it.
    for operation in DbOperation::REQUESTS {
        assert_eq!(
            operation.writes(),
            operation.as_str() == "execute",
            "{}",
            operation.as_str()
        );
    }
    // The paging chapter names the operations that really page.
    assert_eq!(doc["paging"]["operations"], json!(["schemas", "tables"]));
    for operation in DbOperation::REQUESTS {
        assert_eq!(
            doc["paging"]["operations"]
                .as_array()
                .unwrap()
                .contains(&json!(operation.as_str())),
            operation.is_listing(),
            "{}",
            operation.as_str()
        );
    }
}

#[test]
fn db_result_schema_comes_from_the_same_type_as_the_output() {
    let output = DbOutput {
        schema_version: 1,
        kind: DbKind::Db,
        operation: DbOperation::Tables,
        target: "ad-hoc".into(),
        meta: DbMeta::planned(
            DbOperation::Tables,
            DbLimits::default(),
            crate::domain::types::database::DbEngineInfo {
                name: "sqlite".into(),
                server_version: None,
            },
            "/tmp/app.sqlite3".into(),
            None,
        ),
        dry_run: Some(true),
        result: None,
        statements: None,
        elapsed_ms: None,
    };
    let value = serde_json::to_value(output).unwrap();
    let schema = serde_json::to_value(schemars::schema_for!(DbOutput)).unwrap();
    for key in schema["required"].as_array().unwrap() {
        assert!(
            value.get(key.as_str().unwrap()).is_some(),
            "missing required key {key}"
        );
    }
    // A plan claims no version, no rows and no host.
    assert!(value["meta"]["engine"].get("server_version").is_none());
    assert!(value["meta"].get("host").is_none());
    assert!(value.get("rows").is_none());
    assert_eq!(value["operation"], "tables");
    assert_eq!(value["meta"]["consistency"], "single_statement");
}

#[test]
fn db_status_reads_configuration_only() {
    let config = crate::adapters::config::Config::parse(
        "[db.local]\nengine='sqlite'\npath='/tmp/app.sqlite3'\nallow_write=true\n",
    )
    .unwrap();
    let rows = status_rows(&config, None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "local");
    assert_eq!(rows[0].database, "/tmp/app.sqlite3");
    assert!(rows[0].allow_write);
    assert!(status_rows(&config, Some("other")).is_empty());
}

/// The contract's engine list is the enum's, and the enum is matched
/// exhaustively here: an engine added to `DbEngine` reaches the page an agent
/// reads instead of being invisible to whoever writes the configuration.
#[test]
fn db_the_contract_publishes_every_engine_the_configuration_accepts() {
    use crate::adapters::config::DbEngine;
    let published = capabilities();
    let engines: Vec<&str> = published["engines"]
        .as_array()
        .expect("the engine list")
        .iter()
        .map(|value| value.as_str().expect("an engine name"))
        .collect();
    for engine in DbEngine::ALL {
        // No catch-all: a variant added here has to be given a name above.
        let name = match engine {
            DbEngine::Sqlite => "sqlite",
            DbEngine::Postgresql => "postgresql",
            DbEngine::Mysql => "mysql",
        };
        assert_eq!(engine.as_str(), name);
        assert!(engines.contains(&name), "{name} is not published");
    }
    assert_eq!(engines.len(), DbEngine::ALL.len());
}

/// A write is one transaction of many statements, so it cannot claim what a
/// read claims: `single_statement` was a constant, and it was wrong for every
/// `execute`.
#[test]
fn db_consistency_says_what_the_operation_really_guarantees() {
    use crate::domain::types::database::{DbConsistency, DbOperation};
    for operation in DbOperation::REQUESTS {
        let expected = if operation.writes() {
            DbConsistency::SingleTransaction
        } else {
            DbConsistency::SingleStatement
        };
        assert_eq!(DbConsistency::of(operation), expected, "{operation:?}");
    }
    let written = serde_json::to_value(DbConsistency::of(DbOperation::Execute)).unwrap();
    assert_eq!(written, "single_transaction");
}
