//! A GraphQL operation target (`query.viewer`, `mutation.issueCreate`)
//! turned into the request `kurama api` sends: the query document written
//! from the root field, `-d` and `-P` as its variables, POSTed to the
//! endpoint; and the one line that says what a GraphQL answer's `errors`
//! hold.

use serde_json::{Map, Value, json};

use super::api_request::join_base_path;
use super::openapi::sanitize_text;
use super::operation_request::{OperationRequestError, validate_value};
use crate::domain::types::api_spec::{GraphQlField, Operation, Schema};
use crate::domain::types::http::HttpRequest;

/// The request that calls `field`: `-d` is the variables object and each
/// `-P name=value` one variable over it, typed by the argument's schema
/// (`first=3` is the number 3, `input={...}` the JSON object). `select` is
/// the selection set without its braces; without it the return type's
/// scalar and enum fields are selected.
pub fn build_graphql_request(
    base_url: &str,
    operation: &Operation,
    field: &GraphQlField,
    params: &[(String, String)],
    body: Option<Vec<u8>>,
    select: Option<&str>,
) -> Result<HttpRequest, OperationRequestError> {
    let arguments: &[_] = operation
        .request_body
        .as_ref()
        .map_or(&[], |body| body.schema.properties.as_slice());
    let unknown = |name: &str| OperationRequestError::UnknownParameter {
        name: name.to_string(),
        known: arguments.iter().map(|a| a.name.clone()).collect(),
    };
    let mut variables = match body {
        Some(body) => match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(variables)) => variables,
            Ok(_) => {
                return Err(OperationRequestError::InvalidBody(
                    "the variables of a GraphQL operation are one JSON object".into(),
                ));
            }
            Err(error) => {
                return Err(OperationRequestError::InvalidBody(format!(
                    "the variables are not JSON: {error}"
                )));
            }
        },
        None => Map::new(),
    };
    if let Some(name) = variables
        .keys()
        .find(|name| !arguments.iter().any(|a| &a.name == *name))
    {
        return Err(unknown(name));
    }
    for (name, value) in params {
        let argument = arguments
            .iter()
            .find(|a| &a.name == name)
            .ok_or_else(|| unknown(name))?;
        let value = variable_value(&argument.schema, value).map_err(|message| {
            OperationRequestError::InvalidValue {
                name: name.clone(),
                message,
            }
        })?;
        variables.insert(name.clone(), value);
    }
    let missing: Vec<String> = arguments
        .iter()
        .filter(|a| a.required && !variables.contains_key(&a.name))
        .map(|a| a.name.clone())
        .collect();
    if !missing.is_empty() {
        return Err(OperationRequestError::Missing {
            operation: operation.id.clone(),
            missing,
        });
    }
    let selection = selection(operation, field, select)?;
    let document = query_document(field, &variables, selection.as_deref());
    let body = json!({"query": document, "variables": variables});
    Ok(HttpRequest::new(
        operation.method.clone(),
        join_base_path(base_url, &operation.path),
    )
    .with_header("Content-Type", "application/json")
    .with_body(body.to_string().into_bytes()))
}

/// A `-P` value as the JSON the argument's type takes.
fn variable_value(schema: &Schema, value: &str) -> Result<Value, String> {
    validate_value(schema, value)?;
    match schema.type_name.as_str() {
        "integer" | "number" | "boolean" | "object" | "array" => serde_json::from_str(value)
            .map_err(|error| format!("{value:?} is not a JSON {}: {error}", schema.type_name)),
        _ => Ok(Value::String(value.to_string())),
    }
}

/// What the document selects on the field: `select`, or the leaf fields of
/// the return type; nothing for a field that returns a scalar.
fn selection(
    operation: &Operation,
    field: &GraphQlField,
    select: Option<&str>,
) -> Result<Option<String>, OperationRequestError> {
    match (&field.leaf_fields, select) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(OperationRequestError::Selection(format!(
            "operation {} returns a scalar, which takes no --select",
            operation.id
        ))),
        (Some(_), Some(select)) => Ok(Some(select.trim().to_string())),
        (Some(leaves), None) if !leaves.is_empty() => Ok(Some(leaves.join(" "))),
        (Some(_), None) => {
            let fields: Vec<&str> = operation
                .response
                .iter()
                .flat_map(|response| &response.schema.properties)
                .map(|property| property.name.as_str())
                .collect();
            Err(OperationRequestError::Selection(format!(
                "operation {} returns no scalar field to select by default; name what to select with --select (its fields: {})",
                operation.id,
                fields.join(", ")
            )))
        }
    }
}

/// `query($first: Int) { issues(first: $first) { nodes { id } } }`: one
/// variable for each argument given, in the schema's order.
fn query_document(
    field: &GraphQlField,
    variables: &Map<String, Value>,
    selection: Option<&str>,
) -> String {
    let given: Vec<&(String, String)> = field
        .arguments
        .iter()
        .filter(|(name, _)| variables.contains_key(name))
        .collect();
    let mut document = field.root.clone();
    if !given.is_empty() {
        let declared: Vec<String> = given
            .iter()
            .map(|(name, kind)| format!("${name}: {kind}"))
            .collect();
        document.push_str(&format!("({})", declared.join(", ")));
    }
    document.push_str(&format!(" {{ {}", field.name));
    if !given.is_empty() {
        let passed: Vec<String> = given
            .iter()
            .map(|(name, _)| format!("{name}: ${name}"))
            .collect();
        document.push_str(&format!("({})", passed.join(", ")));
    }
    if let Some(selection) = selection {
        document.push_str(&format!(" {{ {selection} }}"));
    }
    document.push_str(" }");
    document
}

/// The first of a GraphQL answer's `errors` as one line -- its `message`,
/// the `path` it is at, and how many more there are; `None` when the body
/// is not JSON or carries no error.
pub fn graphql_error_line(body: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let errors = document.get("errors")?.as_array()?;
    let first = errors.first()?;
    let one_line = |text: &str| sanitize_text(&text.replace(['\n', '\r'], " "));
    let mut line = first
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| one_line(&first.to_string()), one_line);
    let path: Vec<String> = first
        .get("path")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|segment| match segment {
            Value::String(name) => one_line(name),
            other => other.to_string(),
        })
        .collect();
    if !path.is_empty() {
        line.push_str(&format!(" (at {})", path.join(".")));
    }
    if errors.len() > 1 {
        line.push_str(&format!("; and {} more", errors.len() - 1));
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::functions::graphql::normalize_graphql;
    use crate::domain::types::api_spec::ApiSpec;

    fn linear() -> ApiSpec {
        let document: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openapi/graphql-linear.json"
        ))
        .unwrap();
        normalize_graphql(&document, "/graphql").unwrap()
    }

    fn call(
        id: &str,
        params: &[(&str, &str)],
        body: Option<&str>,
        select: Option<&str>,
    ) -> Result<HttpRequest, OperationRequestError> {
        let spec = linear();
        let operation = spec.operations.iter().find(|op| op.id == id).unwrap();
        let params: Vec<(String, String)> = params
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        build_graphql_request(
            "https://api.linear.app/",
            operation,
            operation.graphql.as_ref().unwrap(),
            &params,
            body.map(|body| body.as_bytes().to_vec()),
            select,
        )
    }

    fn sent(request: &HttpRequest) -> Value {
        serde_json::from_slice(request.body.as_ref().unwrap()).unwrap()
    }

    #[test]
    fn a_field_without_arguments_selects_its_leaf_fields() {
        let request = call("query.viewer", &[], None, None).unwrap();
        assert_eq!(
            (request.method.as_str(), request.url.as_str()),
            ("POST", "https://api.linear.app/graphql")
        );
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(
            sent(&request),
            json!({"query": "query { viewer { id createdAt name email admin } }", "variables": {}})
        );
    }

    /// The explorer builds every request through `build_operation_request`:
    /// a GraphQL operation is its document there too, never the variables
    /// sent as they are.
    #[test]
    fn an_operation_request_of_a_graphql_operation_is_its_document() {
        let spec = linear();
        let issue = spec
            .operations
            .iter()
            .find(|op| op.id == "query.issue")
            .unwrap();
        let request = crate::domain::functions::operation_request::build_operation_request(
            "https://api.linear.app",
            issue,
            &[],
            Some(br#"{"id":"x"}"#.to_vec()),
        )
        .unwrap();
        assert_eq!(
            sent(&request)["query"],
            "query($id: String!) { issue(id: $id) { id createdAt title priority labelIds identifier } }"
        );
    }

    #[test]
    fn parameters_are_variables_typed_by_their_argument() {
        let request = call(
            "query.issues",
            &[("includeArchived", "true"), ("first", "3")],
            None,
            Some(" nodes { id title } "),
        )
        .unwrap();
        assert_eq!(
            sent(&request),
            json!({
                "query": "query($first: Int, $includeArchived: Boolean) { issues(first: $first, includeArchived: $includeArchived) { nodes { id title } } }",
                "variables": {"includeArchived": true, "first": 3}
            })
        );
        let error = call(
            "query.issues",
            &[("first", "three")],
            None,
            Some("nodes { id }"),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "parameter first: \"three\" is not an integer"
        );
    }

    #[test]
    fn the_body_is_the_variables_and_a_parameter_replaces_one_of_them() {
        let request = call(
            "mutation.issueCreate",
            &[("input", r#"{"teamId":"p","title":"from -P"}"#)],
            Some(r#"{"input":{"teamId":"d"}}"#),
            None,
        )
        .unwrap();
        assert_eq!(
            sent(&request),
            json!({
                "query": "mutation($input: IssueCreateInput!) { issueCreate(input: $input) { lastSyncId success } }",
                "variables": {"input": {"teamId": "p", "title": "from -P"}}
            })
        );
        let from_body = call(
            "mutation.issueCreate",
            &[],
            Some(r#"{"input":{"teamId":"d"}}"#),
            None,
        )
        .unwrap();
        assert_eq!(
            sent(&from_body)["variables"],
            json!({"input": {"teamId": "d"}})
        );
        assert_eq!(
            call("mutation.issueCreate", &[("input", "{")], None, None)
                .unwrap_err()
                .to_string()
                .split(':')
                .next(),
            Some("parameter input")
        );
    }

    #[test]
    fn a_missing_required_argument_is_named_before_anything_is_built() {
        assert_eq!(
            call("query.issue", &[], None, None).unwrap_err(),
            OperationRequestError::Missing {
                operation: "query.issue".into(),
                missing: vec!["id".into()],
            }
        );
        assert_eq!(
            call("mutation.issueCreate", &[], Some("{}"), None).unwrap_err(),
            OperationRequestError::Missing {
                operation: "mutation.issueCreate".into(),
                missing: vec!["input".into()],
            }
        );
    }

    #[test]
    fn unknown_names_and_bodies_that_are_not_an_object_are_refused() {
        assert_eq!(
            call("query.issue", &[("id", "x"), ("nope", "1")], None, None).unwrap_err(),
            OperationRequestError::UnknownParameter {
                name: "nope".into(),
                known: vec!["id".into()],
            }
        );
        assert_eq!(
            call("query.issue", &[], Some(r#"{"id":"x","extra":1}"#), None).unwrap_err(),
            OperationRequestError::UnknownParameter {
                name: "extra".into(),
                known: vec!["id".into()],
            }
        );
        for body in ["[1]", "not json"] {
            assert!(
                matches!(
                    call("query.issue", &[], Some(body), None).unwrap_err(),
                    OperationRequestError::InvalidBody(_)
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn a_type_without_leaf_fields_needs_a_selection_and_a_scalar_takes_none() {
        let error = call("query.issues", &[], None, None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "operation query.issues returns no scalar field to select by default; name what to select with --select (its fields: nodes)"
        );
        let scalar = json!({"__schema": {"queryType": {"name": "Q"}, "types": [
            {"kind": "OBJECT", "name": "Q", "fields": [{"name": "count", "args": [],
                "type": {"kind": "SCALAR", "name": "Int"}}]},
            {"kind": "SCALAR", "name": "Int"}
        ]}});
        let spec = normalize_graphql(&scalar, "/g").unwrap();
        let operation = &spec.operations[0];
        let field = operation.graphql.as_ref().unwrap();
        let request =
            build_graphql_request("https://x", operation, field, &[], None, None).unwrap();
        assert_eq!(sent(&request)["query"], "query { count }");
        assert!(matches!(
            build_graphql_request("https://x", operation, field, &[], None, Some("id")),
            Err(OperationRequestError::Selection(_))
        ));
    }

    #[test]
    fn the_first_error_is_one_line_with_its_path_and_the_count_of_the_rest() {
        let body = br#"{"data":null,"errors":[
            {"message":"Entity not\nfound","path":["issue",0,"title"]},
            {"message":"second"}]}"#;
        assert_eq!(
            graphql_error_line(body).as_deref(),
            Some("Entity not found (at issue.0.title); and 1 more")
        );
        assert_eq!(
            graphql_error_line(br#"{"errors":[{"extensions":{}}]}"#).as_deref(),
            Some(r#"{"extensions":{}}"#)
        );
        for body in [
            &br#"{"data":{"viewer":{}}}"#[..],
            b"{\"errors\":[]}",
            b"not json",
        ] {
            assert_eq!(graphql_error_line(body), None);
        }
    }
}
