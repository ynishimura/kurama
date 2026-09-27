//! Check the published data contract against its typed parser and output.
use super::*;
use crate::domain::types::dataset::{DataArgs, DataKind, DataMeta};

#[test]
fn data_capabilities_advertise_only_accepted_request_operations() {
    let doc = capabilities();
    for operation in doc["request_operations"].as_array().unwrap() {
        let args = if operation == "query" {
            json!({"sql":"SELECT 1"})
        } else {
            json!({})
        };
        let request: DataRequest =
            serde_json::from_value(json!({"operation":operation,"args":args})).unwrap();
        assert!(request.validate().is_ok(), "{operation}");
    }
    let schema_operations: Vec<_> = doc["request_schema"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| variant["properties"]["operation"]["const"].clone())
        .collect();
    assert_eq!(doc["request_operations"], json!(schema_operations));
    assert!(
        !doc["request_operations"]
            .as_array()
            .unwrap()
            .contains(&json!("export"))
    );
    assert!(
        doc["result_operations"]
            .as_array()
            .unwrap()
            .contains(&json!("export"))
    );
    assert!(
        doc["operation_modifiers"]["export"]
            .as_str()
            .unwrap()
            .contains("args.export")
    );
    let args = serde_json::to_value(schemars::schema_for!(DataArgs)).unwrap();
    for key in doc["defaults"].as_object().unwrap().keys() {
        assert!(!args["properties"][key].is_null(), "unusable default {key}");
    }
}

#[test]
fn data_result_schema_comes_from_the_same_type_as_the_output() {
    assert_eq!(
        result_schema(),
        serde_json::to_value(schemars::schema_for!(DataOutput)).unwrap()
    );
    let output = DataOutput {
        schema_version: 1,
        kind: DataKind::Data,
        operation: DataOperation::Tables,
        target: "ad-hoc".into(),
        meta: DataMeta::planned(vec![], DataLimits::default(), None, None),
        dry_run: Some(true),
        payload: None,
        elapsed_ms: None,
    };
    let value = serde_json::to_value(output).unwrap();
    for key in result_schema()["required"].as_array().unwrap() {
        assert!(
            value.get(key.as_str().unwrap()).is_some(),
            "missing required key {key}"
        );
    }
    assert!(value.get("engine").is_none());
    assert!(value["meta"].get("inputs").is_none());
    assert_eq!(value["operation"], "tables");
}
