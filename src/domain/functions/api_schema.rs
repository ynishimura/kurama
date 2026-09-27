//! What `kurama api <API> --schema [OP]` prints: one versioned JSON contract
//! of an `[api.*]` profile -- the description's identity, the options of
//! `kurama api`, and each operation with the full schemas of its parameters
//! and bodies and every reduction the normalizer made, as a limitation.

use serde_json::{Map, Value, json};

use crate::domain::functions::operation_command::example_command;
use crate::domain::types::api_spec::{
    ApiSpec, Operation, Parameter, RequestBody, Response, Schema, SchemaLimitation,
};

/// The version of the document's shape; raised when a key changes meaning
/// or goes away.
pub const SCHEMA_VERSION: u32 = 1;

/// What the contract is made of besides the operations.
pub struct SchemaContract<'a> {
    pub api: &'a str,
    pub base_url: &'a str,
    pub spec: &'a ApiSpec,
    /// Where the description was read from this time.
    pub source: Value,
    /// The options of `kurama api`, as the clap definition states them.
    pub options: Value,
}

/// The whole document, for `operations` (every operation, or the one asked
/// for).
pub fn schema_document(contract: &SchemaContract, operations: &[&Operation]) -> Value {
    let SchemaContract {
        api,
        base_url,
        spec,
        source,
        options,
    } = contract;
    json!({
        "schema_version": SCHEMA_VERSION,
        "api": {"name": api, "base_url": base_url},
        "description": {
            "title": spec.title,
            "version": spec.version,
            "description": spec.description,
            "server": spec.server,
            "source": source,
        },
        "options": options,
        "operations": operations
            .iter()
            .map(|operation| operation_contract(api, operation))
            .collect::<Vec<_>>(),
        "warnings": spec.warnings,
    })
}

fn operation_contract(api: &str, operation: &Operation) -> Value {
    let Operation {
        id,
        has_operation_id: _,
        method,
        path,
        summary,
        description,
        tags,
        scopes,
        parameters,
        request_body,
        response,
        deprecated,
        external_docs,
        unsupported,
        graphql: _,
    } = operation;
    let mut limitations: Vec<Value> = unsupported
        .iter()
        .map(|detail| json!({"kind": "unsupported_parameter", "at": "/parameters", "detail": detail}))
        .collect();
    let parameters: Vec<Value> = parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let Parameter {
                name,
                location,
                required,
                schema,
                description,
            } = parameter;
            collect_limitations(
                schema,
                &format!("/parameters/{index}/schema"),
                &mut limitations,
            );
            json!({
                "name": name,
                "in": location.as_str(),
                "required": required,
                "description": description,
                "schema": schema_json(schema),
            })
        })
        .collect();
    let request_body = request_body.as_ref().map(|body| {
        let RequestBody {
            content_type,
            other_content_types,
            required,
            schema,
            description,
        } = body;
        other_content_types_limitation("/request_body", other_content_types, &mut limitations);
        collect_limitations(schema, "/request_body/schema", &mut limitations);
        json!({
            "content_type": content_type,
            "required": required,
            "description": description,
            "schema": schema_json(schema),
        })
    });
    let response = response.as_ref().map(|response| {
        let Response {
            status,
            content_type,
            other_content_types,
            schema,
            description,
        } = response;
        other_content_types_limitation("/response", other_content_types, &mut limitations);
        collect_limitations(schema, "/response/schema", &mut limitations);
        json!({
            "status": status,
            "content_type": content_type,
            "description": description,
            "schema": schema_json(schema),
        })
    });
    json!({
        "id": id,
        "method": method,
        "path": path,
        "summary": summary,
        "description": description,
        "tags": tags,
        "scopes": scopes,
        "deprecated": deprecated,
        "external_docs": external_docs,
        "parameters": parameters,
        "request_body": request_body,
        "response": response,
        "example_command": example_command(api, operation),
        "limitations": limitations,
    })
}

/// A reduced schema as JSON Schema keywords; a keyword the document did not
/// give is left out.
fn schema_json(schema: &Schema) -> Value {
    let Schema {
        type_name,
        enum_values,
        default,
        example,
        items,
        properties,
        unresolved_ref,
        limitations,
    } = schema;
    let mut out = Map::new();
    if let Some(reference) = unresolved_ref {
        out.insert("$ref".into(), json!(reference));
    }
    // The type array the reduction cut to one type is whole again here, so
    // it is no limitation; the others go to the operation's `limitations`.
    let other_types: Vec<&String> = limitations
        .iter()
        .flat_map(|limitation| match limitation {
            SchemaLimitation::OtherTypes { types } => types.iter().collect(),
            _ => Vec::new(),
        })
        .collect();
    if !other_types.is_empty() {
        let mut types = vec![type_name];
        types.extend(other_types);
        out.insert("type".into(), json!(types));
    } else if !type_name.is_empty() {
        out.insert("type".into(), json!(type_name));
    }
    if !enum_values.is_empty() {
        out.insert("enum".into(), json!(enum_values));
    }
    if let Some(default) = default {
        out.insert("default".into(), default.clone());
    }
    if let Some(example) = example {
        out.insert("example".into(), example.clone());
    }
    if let Some(items) = items {
        out.insert("items".into(), schema_json(items));
    }
    if !properties.is_empty() {
        out.insert(
            "properties".into(),
            Value::Object(
                properties
                    .iter()
                    .map(|property| (property.name.clone(), schema_json(&property.schema)))
                    .collect(),
            ),
        );
        let required: Vec<&str> = properties
            .iter()
            .filter(|property| property.required)
            .map(|property| property.name.as_str())
            .collect();
        if !required.is_empty() {
            out.insert("required".into(), json!(required));
        }
    }
    Value::Object(out)
}

fn other_content_types_limitation(at: &str, types: &[String], out: &mut Vec<Value>) {
    if !types.is_empty() {
        out.push(json!({"kind": "other_content_types", "at": at, "content_types": types}));
    }
}

/// Every reduction under `schema`, located by a JSON pointer into the
/// operation's contract.
fn collect_limitations(schema: &Schema, at: &str, out: &mut Vec<Value>) {
    if let Some(reference) = &schema.unresolved_ref {
        out.push(json!({"kind": "unresolved_ref", "at": at, "ref": reference}));
    }
    for limitation in &schema.limitations {
        out.push(match limitation {
            SchemaLimitation::FirstAlternativeOnly {
                keyword,
                alternatives,
            } => json!({
                "kind": "first_alternative_only",
                "at": at,
                "keyword": keyword,
                "alternatives": alternatives,
            }),
            SchemaLimitation::Cycle { reference } => {
                json!({"kind": "cycle", "at": at, "ref": reference})
            }
            SchemaLimitation::DepthBound => json!({"kind": "depth_bound", "at": at}),
            // Written back into the schema's `type` by `schema_json`.
            SchemaLimitation::OtherTypes { .. } => continue,
        });
    }
    if let Some(items) = &schema.items {
        collect_limitations(items, &format!("{at}/items"), out);
    }
    for property in &schema.properties {
        collect_limitations(
            &property.schema,
            &format!("{at}/properties/{}", pointer_token(&property.name)),
            out,
        );
    }
}

/// A name as one JSON pointer token (RFC 6901).
fn pointer_token(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::functions::openapi::normalize_spec;

    fn contract(spec: &ApiSpec) -> SchemaContract<'_> {
        SchemaContract {
            api: "pets",
            base_url: "https://x/api",
            spec,
            source: json!({"kind": "file"}),
            options: json!([{"name": "--json"}]),
        }
    }

    fn document(spec: &ApiSpec) -> Value {
        let operations: Vec<&Operation> = spec.operations.iter().collect();
        schema_document(&contract(spec), &operations)
    }

    #[test]
    fn the_document_names_its_version_the_api_and_the_description() {
        let spec = normalize_spec(
            &serde_json::from_str(include_str!(
                "../../../tests/fixtures/openapi/petstore.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let value = document(&spec);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(
            value["api"],
            json!({"name": "pets", "base_url": "https://x/api"})
        );
        assert_eq!(value["description"]["title"], "Petstore");
        assert_eq!(value["description"]["version"], "1.2.0");
        assert_eq!(value["description"]["source"], json!({"kind": "file"}));
        assert_eq!(value["options"], json!([{"name": "--json"}]));
        assert_eq!(value["operations"].as_array().unwrap().len(), 5);
        assert_eq!(value["warnings"], json!([]));
        let create = &value["operations"][1];
        assert_eq!(create["id"], "pets/create");
        assert_eq!(
            create["request_body"]["schema"],
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "owner": {"type": "object", "properties": {"login": {"type": "string", "example": "octocat"}}}
                },
                "required": ["name"]
            }),
            "the whole schema, optional properties included"
        );
        assert_eq!(
            create["limitations"],
            json!([{"kind": "other_content_types", "at": "/request_body", "content_types": ["application/xml"]}])
        );
        let list = &value["operations"][0];
        assert_eq!(
            list["parameters"][1],
            json!({"name": "status", "in": "query", "required": false,
                   "description": "Only pets in this state",
                   "schema": {"type": "string", "enum": ["available", "sold"]}})
        );
        assert_eq!(
            list["parameters"][0]["schema"],
            json!({"type": "integer", "default": 20})
        );
        assert_eq!(list["example_command"], "kurama api pets pets/list");
    }

    #[test]
    fn every_reduction_is_a_located_limitation() {
        let spec = normalize_spec(&json!({
            "openapi": "3.1.0",
            "info": {"title": "T", "version": "1"},
            "paths": {"/x": {"post": {
                "operationId": "x",
                "parameters": [{"name": "c", "in": "cookie"}],
                "requestBody": {"content": {"application/json": {"schema": {
                    "type": "object",
                    "properties": {
                        "a/b": {"type": "array", "items": {"oneOf": [{"type": "string"}, {"type": "integer"}]}},
                        "next": {"$ref": "#/components/schemas/Node"},
                        "far": {"$ref": "other.json#/Far"}
                    }
                }}}},
                "responses": {"200": {"description": "ok", "content": {
                    "application/json": {"schema": {"type": ["string", "null"]}},
                    "text/csv": {}
                }}}
            }}},
            "components": {"schemas": {"Node": {"type": "object", "properties": {
                "next": {"$ref": "#/components/schemas/Node"}
            }}}}
        }))
        .unwrap();
        let value = document(&spec);
        let operation = &value["operations"][0];
        assert_eq!(
            operation["limitations"],
            json!([
                {"kind": "unsupported_parameter", "at": "/parameters", "detail": "cookie parameter 'c'"},
                {"kind": "first_alternative_only", "at": "/request_body/schema/properties/a~1b/items", "keyword": "oneOf", "alternatives": 2},
                {"kind": "cycle", "at": "/request_body/schema/properties/next/properties/next", "ref": "#/components/schemas/Node"},
                {"kind": "unresolved_ref", "at": "/request_body/schema/properties/far", "ref": "other.json#/Far"},
                {"kind": "other_content_types", "at": "/response", "content_types": ["text/csv"]}
            ])
        );
        assert_eq!(
            operation["response"]["schema"],
            json!({"type": ["string", "null"]}),
            "a type array is kept whole, not reported"
        );
        assert_eq!(
            operation["request_body"]["schema"]["properties"]["far"],
            json!({"$ref": "other.json#/Far"})
        );
        assert_eq!(value["warnings"].as_array().unwrap().len(), 2);
        for limitation in operation["limitations"].as_array().unwrap() {
            let at = limitation["at"].as_str().unwrap();
            assert!(
                operation.pointer(at).is_some(),
                "{at} points into the operation"
            );
        }
    }

    #[test]
    fn a_depth_bound_is_reported_where_reading_stopped() {
        let schema = Schema {
            type_name: "object".into(),
            limitations: vec![SchemaLimitation::DepthBound],
            ..Schema::default()
        };
        let mut out = Vec::new();
        collect_limitations(&schema, "/response/schema", &mut out);
        assert_eq!(
            out,
            [json!({"kind": "depth_bound", "at": "/response/schema"})]
        );
        assert_eq!(schema_json(&Schema::default()), json!({}));
    }
}
