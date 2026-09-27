//! A Google Discovery Document (`discovery#restDescription`), already parsed
//! into a `serde_json::Value`, normalized into an `ApiSpec`: the document is
//! rewritten as the OpenAPI 3 document it describes and handed to
//! `functions::openapi`, so both formats reach `ApiSpec` through one reader.
//!
//! Every method under `resources.*.methods.*` (at any depth, and the
//! top-level `methods`) is one operation: its `id` is the operation id, its
//! `httpMethod` and `path` the method and path (`/` + the path, which is
//! relative to `rootUrl + servicePath`, the `base_url` to configure), its
//! `parameters` the path and query parameters in `parameterOrder` first,
//! `request.$ref` / `response.$ref` the JSON bodies and `scopes` the scopes.
//! The document-level `parameters` (`fields`, `alt`, ...) apply to every
//! method and are not copied into each. A `{+name}` path template is kept as
//! it is: the request builder expands it without escaping `/`.

use serde_json::{Map, Value, json};

use super::openapi::normalize_spec;
use crate::domain::types::api_spec::ApiSpec;

const KIND: &str = "discovery#restDescription";

pub fn normalize_discovery(document: &Value) -> Result<ApiSpec, String> {
    let (openapi, duplicates) = discovery_to_openapi(document)?;
    let mut spec = normalize_spec(&openapi)?;
    spec.warnings.extend(duplicates);
    Ok(spec)
}

/// The OpenAPI 3 document a Discovery Document describes, and one warning
/// for each method whose path and HTTP method an earlier one already took.
fn discovery_to_openapi(document: &Value) -> Result<(Value, Vec<String>), String> {
    let root = document
        .as_object()
        .ok_or_else(|| "the document is not a JSON object".to_string())?;
    if root.get("kind").and_then(Value::as_str) != Some(KIND) {
        return Err(format!(
            "not a Google Discovery Document: `kind` is not {KIND:?}"
        ));
    }
    let mut methods = Vec::new();
    collect_methods(root, &mut methods);
    let mut paths = Map::new();
    let mut duplicates = Vec::new();
    for method in methods {
        let (Some(path), Some(http_method)) = (
            method.get("path").and_then(Value::as_str),
            method.get("httpMethod").and_then(Value::as_str),
        ) else {
            continue;
        };
        let path = format!("/{}", path.trim_start_matches('/'));
        let http_method = http_method.to_ascii_lowercase();
        let item = paths
            .entry(path.clone())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("every path item is an object");
        if item.contains_key(&http_method) {
            let id = method.get("id").and_then(Value::as_str).unwrap_or_default();
            duplicates.push(format!(
                "{id}: {} {path} is already another method's; it is not listed",
                http_method.to_ascii_uppercase()
            ));
            continue;
        }
        item.insert(http_method, operation(method));
    }
    let text = |key: &str| root.get(key).cloned().unwrap_or(Value::Null);
    let mut openapi = json!({
        "openapi": "3.0.3",
        "info": {"title": text("title"), "version": text("version")},
        "paths": paths,
        "components": {"schemas": rewrite_refs(&text("schemas"))},
    });
    if let Some(description) = root.get("description") {
        openapi["info"]["description"] = description.clone();
    }
    if let Some(root_url) = root.get("rootUrl").and_then(Value::as_str) {
        let service_path = root
            .get("servicePath")
            .and_then(Value::as_str)
            .unwrap_or_default();
        openapi["servers"] = json!([{"url": format!("{root_url}{service_path}")}]);
    }
    Ok((openapi, duplicates))
}

/// The methods of `resource` and of every resource under it, in document
/// order.
fn collect_methods<'d>(resource: &'d Map<String, Value>, out: &mut Vec<&'d Map<String, Value>>) {
    if let Some(methods) = resource.get("methods").and_then(Value::as_object) {
        out.extend(methods.values().filter_map(Value::as_object));
    }
    if let Some(resources) = resource.get("resources").and_then(Value::as_object) {
        for child in resources.values().filter_map(Value::as_object) {
            collect_methods(child, out);
        }
    }
}

/// One Discovery method as an OpenAPI operation object.
fn operation(method: &Map<String, Value>) -> Value {
    let mut operation = Map::new();
    for (from, to) in [
        ("id", "operationId"),
        ("description", "description"),
        ("deprecated", "deprecated"),
    ] {
        if let Some(value) = method.get(from) {
            operation.insert(to.to_string(), value.clone());
        }
    }
    if let Some(summary) = method
        .get("description")
        .and_then(Value::as_str)
        .and_then(first_sentence)
    {
        operation.insert("summary".into(), Value::String(summary));
    }
    let scopes = method.get("scopes").cloned().unwrap_or_else(|| json!([]));
    operation.insert("security".into(), json!([{ "oauth2": scopes }]));
    operation.insert("parameters".into(), Value::Array(parameters(method)));
    if let Some(reference) = schema_ref(method, "request") {
        operation.insert(
            "requestBody".into(),
            json!({"required": true, "content": {"application/json": {"schema": reference}}}),
        );
    }
    let response = match schema_ref(method, "response") {
        Some(reference) => json!({"content": {"application/json": {"schema": reference}}}),
        None => json!({}),
    };
    operation.insert("responses".into(), json!({ "200": response }));
    Value::Object(operation)
}

/// The first sentence of a description, for the `--ops` summary a
/// Discovery method does not have.
fn first_sentence(description: &str) -> Option<String> {
    let line = description.lines().next()?.trim();
    let sentence = match line.find(". ") {
        Some(end) => &line[..=end],
        None => line,
    };
    (!sentence.is_empty()).then(|| sentence.to_string())
}

/// `{"$ref": "#/components/schemas/<name>"}` for the method's `request` or
/// `response`.
fn schema_ref(method: &Map<String, Value>, key: &str) -> Option<Value> {
    let name = method.get(key)?.get("$ref")?.as_str()?;
    Some(json!({ "$ref": format!("#/components/schemas/{name}") }))
}

/// The method's parameters as OpenAPI parameter objects: those
/// `parameterOrder` names first, in its order, then the rest in document
/// order.
fn parameters(method: &Map<String, Value>) -> Vec<Value> {
    let Some(parameters) = method.get("parameters").and_then(Value::as_object) else {
        return Vec::new();
    };
    let order: Vec<&str> = method
        .get("parameterOrder")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let ordered = order
        .iter()
        .filter_map(|name| parameters.get_key_value(*name))
        .chain(
            parameters
                .iter()
                .filter(|(name, _)| !order.contains(&name.as_str())),
        );
    ordered
        .map(|(name, parameter)| {
            let mut schema = Map::new();
            for key in ["type", "format", "enum", "default", "pattern"] {
                if let Some(value) = parameter.get(key) {
                    schema.insert(key.to_string(), value.clone());
                }
            }
            let mut object = json!({
                "name": name,
                "in": parameter.get("location").cloned().unwrap_or(Value::Null),
                "required": parameter.get("required").and_then(Value::as_bool).unwrap_or(false),
                "schema": schema,
            });
            if let Some(description) = parameter.get("description") {
                object["description"] = description.clone();
            }
            object
        })
        .collect()
}

/// A Discovery schema with its bare `$ref: "Name"`s pointing into
/// `#/components/schemas/`.
fn rewrite_refs(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| match (key.as_str(), value) {
                    ("$ref", Value::String(name)) if !name.starts_with('#') => (
                        key.clone(),
                        Value::String(format!("#/components/schemas/{name}")),
                    ),
                    _ => (key.clone(), rewrite_refs(value)),
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(rewrite_refs).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::api_spec::ParameterLocation;

    fn sheets() -> ApiSpec {
        let document: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openapi/discovery-sheets-v4.json"
        ))
        .unwrap();
        normalize_discovery(&document).unwrap()
    }

    #[test]
    fn every_method_at_any_depth_is_an_operation_named_by_its_id() {
        let spec = sheets();
        assert_eq!(spec.title, "Google Sheets API");
        assert_eq!(spec.version, "v4");
        assert_eq!(
            spec.server.as_deref(),
            Some("https://sheets.googleapis.com/")
        );
        let listed: Vec<(&str, &str, &str)> = spec
            .operations
            .iter()
            .map(|op| (op.id.as_str(), op.method.as_str(), op.path.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                (
                    "sheets.spreadsheets.get",
                    "GET",
                    "/v4/spreadsheets/{spreadsheetId}"
                ),
                (
                    "sheets.spreadsheets.batchUpdate",
                    "POST",
                    "/v4/spreadsheets/{spreadsheetId}:batchUpdate"
                ),
                (
                    "sheets.spreadsheets.values.get",
                    "GET",
                    "/v4/spreadsheets/{spreadsheetId}/values/{range}"
                ),
            ]
        );
        assert!(spec.operations.iter().all(|op| op.has_operation_id));
        assert_eq!(
            spec.operations[0].summary.as_deref(),
            Some("Returns the spreadsheet at the given ID."),
            "the first sentence of the description"
        );
        assert!(spec.warnings.is_empty(), "{:?}", spec.warnings);
    }

    #[test]
    fn a_method_carries_its_parameters_scopes_and_bodies() {
        let spec = sheets();
        let get = &spec.operations[0];
        assert_eq!(
            get.parameters
                .iter()
                .map(|p| (p.name.as_str(), p.location, p.required))
                .collect::<Vec<_>>()[..2],
            [
                ("spreadsheetId", ParameterLocation::Path, true),
                ("ranges", ParameterLocation::Query, false),
            ],
            "parameterOrder first"
        );
        assert!(
            get.parameters.iter().all(|p| p.name != "fields"),
            "document-level parameters are not copied into each method"
        );
        assert!(
            get.scopes
                .contains(&"https://www.googleapis.com/auth/spreadsheets.readonly".to_string())
        );
        let response = get.response.as_ref().unwrap();
        assert_eq!(response.content_type, "application/json");
        assert_eq!(
            response.schema.shape(),
            json!({"spreadsheetId": "string", "spreadsheetUrl": "string",
                   "properties": {"title": "string", "locale": "string", "timeZone": "string"}})
        );
        assert!(get.request_body.is_none());
        let update = &spec.operations[1];
        let body = update.request_body.as_ref().unwrap();
        assert!(body.required);
        assert_eq!(
            body.schema.optional_properties(),
            ["includeSpreadsheetInResponse", "responseRanges"]
        );
    }

    #[test]
    fn a_reserved_expansion_stays_in_the_path_template() {
        let document: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openapi/discovery-people-v1.json"
        ))
        .unwrap();
        let spec = normalize_discovery(&document).unwrap();
        assert_eq!(spec.operations[0].id, "people.people.get");
        assert_eq!(spec.operations[0].path, "/v1/{+resourceName}");
        assert_eq!(spec.operations[0].parameters[0].name, "resourceName");
    }

    #[test]
    fn a_document_of_another_kind_is_refused() {
        let error = normalize_discovery(&json!({"openapi": "3.0.0", "paths": {}})).unwrap_err();
        assert!(error.contains("not a Google Discovery Document"), "{error}");
        assert!(normalize_discovery(&json!([])).is_err());
    }

    #[test]
    fn a_second_method_on_the_same_path_and_method_is_a_warning() {
        let method = |id: &str| json!({"id": id, "path": "v1/x", "httpMethod": "GET"});
        let document = json!({
            "kind": KIND,
            "title": "X",
            "methods": {"a": method("x.a"), "b": method("x.b")},
        });
        let spec = normalize_discovery(&document).unwrap();
        assert_eq!(spec.operations.len(), 1);
        assert_eq!(spec.operations[0].id, "x.a");
        assert_eq!(
            spec.warnings,
            ["x.b: GET /v1/x is already another method's; it is not listed"]
        );
    }
}
