//! A GraphQL schema, as the result of the standard introspection query
//! (`{"data": {"__schema": ...}}`, or the `__schema` object alone), already
//! parsed into a `serde_json::Value`, normalized into an `ApiSpec`.
//!
//! Every field of the query and mutation root types is one operation named
//! `query.<field>` / `mutation.<field>` (subscriptions are not listed), sent
//! as `POST <endpoint>`. Its arguments are the variables object, described as
//! a JSON request body (`input: IssueCreateInput!` is a required `input`
//! property whose required fields are its non-null input fields); its return
//! type is the response. Object types are expanded to `limits::GRAPHQL`, a
//! type already being expanded ends in a bare object, and every string the
//! schema says is made terminal-safe. Each operation carries its
//! `GraphQlField`, which `graphql_call` writes the query document from.

use serde_json::{Map, Value, json};

use super::openapi::sanitize_text;
use crate::domain::types::api_spec::{
    ApiSpec, GraphQlField, Operation, Property, RequestBody, Response, Schema, SchemaLimitation,
};
use crate::domain::types::limits::GRAPHQL;

/// The standard introspection query, with type references nested deep
/// enough for `[[String!]!]!`.
pub const INTROSPECTION_QUERY: &str = "query IntrospectionQuery { __schema { queryType { name } mutationType { name } subscriptionType { name } description types { ...FullType } } } \
fragment FullType on __Type { kind name description fields(includeDeprecated: true) { name description args { ...InputValue } type { ...TypeRef } isDeprecated deprecationReason } inputFields { ...InputValue } interfaces { ...TypeRef } enumValues(includeDeprecated: true) { name description isDeprecated deprecationReason } possibleTypes { ...TypeRef } } \
fragment InputValue on __InputValue { name description type { ...TypeRef } defaultValue } \
fragment TypeRef on __Type { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } } } } } }";

/// The body of the introspection request.
pub fn introspection_request_body() -> Vec<u8> {
    json!({ "query": INTROSPECTION_QUERY })
        .to_string()
        .into_bytes()
}

/// The server's messages when an introspection answer is only `errors`
/// (introspection disabled or not allowed), one line; `None` for an answer
/// that carries a schema, or that is not a GraphQL answer at all.
pub fn introspection_refusal(document: &Value) -> Option<String> {
    if schema_of(document).is_some() {
        return None;
    }
    let messages: Vec<String> = document
        .get("errors")?
        .as_array()?
        .iter()
        .map(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| error.to_string(), sanitize_text)
        })
        .collect();
    (!messages.is_empty()).then(|| messages.join("; "))
}

fn schema_of(document: &Value) -> Option<&Map<String, Value>> {
    document
        .get("data")
        .and_then(|data| data.get("__schema"))
        .or_else(|| document.get("__schema"))
        .and_then(Value::as_object)
}

/// The operations of the schema, each sent as `POST <endpoint>` (the
/// endpoint's path under `base_url`).
pub fn normalize_graphql(document: &Value, endpoint: &str) -> Result<ApiSpec, String> {
    let schema = schema_of(document)
        .ok_or_else(|| "not a GraphQL introspection result: there is no `__schema`".to_string())?;
    let types: Map<String, Value> = schema
        .get("types")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|kind| Some((kind.get("name")?.as_str()?.to_string(), kind.clone())))
        .collect();
    let reader = Types { types: &types };
    let mut operations = Vec::new();
    for (root, key) in [("query", "queryType"), ("mutation", "mutationType")] {
        let Some(root_type) = schema
            .get(key)
            .and_then(|root| root.get("name"))
            .and_then(Value::as_str)
            .and_then(|name| types.get(name))
        else {
            continue;
        };
        for field in fields(root_type, "fields") {
            operations.push(reader.operation(root, field, endpoint));
        }
    }
    Ok(ApiSpec {
        title: "GraphQL schema".to_string(),
        version: String::new(),
        description: schema
            .get("description")
            .and_then(Value::as_str)
            .map(paragraphs),
        server: None,
        operations,
        warnings: Vec::new(),
    })
}

/// The objects of `kind[key]` (`fields`, `inputFields`, `args`).
fn fields<'a>(kind: &'a Value, key: &str) -> impl Iterator<Item = &'a Map<String, Value>> {
    kind.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

fn text(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(sanitize_text)
}

/// A description with its line breaks kept and every other control removed.
fn paragraphs(text: &str) -> String {
    text.split('\n')
        .map(sanitize_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `IssueCreateInput!`, `[String!]`: a type reference as GraphQL writes it.
fn type_text(reference: &Value) -> String {
    let inner = || reference.get("ofType").map(type_text).unwrap_or_default();
    match reference.get("kind").and_then(Value::as_str) {
        Some("NON_NULL") => format!("{}!", inner()),
        Some("LIST") => format!("[{}]", inner()),
        _ => sanitize_text(
            reference
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
    }
}

/// The named types of the schema.
struct Types<'t> {
    types: &'t Map<String, Value>,
}

impl Types<'_> {
    fn operation(&self, root: &str, field: &Map<String, Value>, endpoint: &str) -> Operation {
        let name = text(field, "name").unwrap_or_default();
        let description = field.get("description").and_then(Value::as_str);
        let arguments: Vec<&Map<String, Value>> = field
            .get("args")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .collect();
        let request_body = (!arguments.is_empty()).then(|| {
            let properties: Vec<Property> = arguments
                .iter()
                .map(|argument| self.input_property(argument, 0, &mut Vec::new()))
                .collect();
            let signature: Vec<String> = arguments
                .iter()
                .map(|argument| {
                    format!(
                        "{}: {}",
                        text(argument, "name").unwrap_or_default(),
                        argument.get("type").map(type_text).unwrap_or_default()
                    )
                })
                .collect();
            RequestBody {
                content_type: "application/json".to_string(),
                other_content_types: Vec::new(),
                required: properties.iter().any(|property| property.required),
                schema: Schema {
                    type_name: "object".to_string(),
                    properties,
                    ..Schema::default()
                },
                description: Some(format!("GraphQL variables: {}", signature.join(", "))),
            }
        });
        let return_type = field.get("type").cloned().unwrap_or(Value::Null);
        Operation {
            id: format!("{root}.{name}"),
            has_operation_id: true,
            method: "POST".to_string(),
            path: sanitize_text(endpoint),
            summary: description
                .and_then(|text| text.lines().next())
                .map(sanitize_text)
                .filter(|line| !line.is_empty()),
            description: description.map(paragraphs),
            tags: vec![root.to_string()],
            scopes: Vec::new(),
            parameters: Vec::new(),
            request_body,
            response: Some(Response {
                status: "200".to_string(),
                content_type: "application/json".to_string(),
                other_content_types: Vec::new(),
                schema: self.output_schema(&return_type, 0, &mut Vec::new()),
                description: Some(type_text(&return_type)),
            }),
            deprecated: field
                .get("isDeprecated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            external_docs: None,
            unsupported: Vec::new(),
            graphql: Some(GraphQlField {
                root: root.to_string(),
                name,
                arguments: arguments
                    .iter()
                    .map(|argument| {
                        (
                            text(argument, "name").unwrap_or_default(),
                            argument.get("type").map(type_text).unwrap_or_default(),
                        )
                    })
                    .collect(),
                leaf_fields: self.leaf_fields(&return_type),
            }),
        }
    }

    /// The fields of the type `reference` names whose own type is a scalar
    /// or an enum; `None` when that type is a scalar or an enum itself.
    fn leaf_fields(&self, reference: &Value) -> Option<Vec<String>> {
        let kind = self.types.get(named_type(reference))?;
        if is_leaf(kind) {
            return None;
        }
        Some(
            fields(kind, "fields")
                .filter(|field| {
                    field
                        .get("type")
                        .and_then(|reference| self.types.get(named_type(reference)))
                        .is_some_and(is_leaf)
                })
                .filter_map(|field| text(field, "name"))
                .collect(),
        )
    }

    /// An argument or input field: required when non-null without a default.
    fn input_property(
        &self,
        value: &Map<String, Value>,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Property {
        let reference = value.get("type").cloned().unwrap_or(Value::Null);
        let has_default = value
            .get("defaultValue")
            .is_some_and(|value| !value.is_null());
        Property {
            name: text(value, "name").unwrap_or_default(),
            required: is_non_null(&reference) && !has_default,
            schema: self.schema(&reference, depth, stack, Direction::Input),
        }
    }

    fn output_schema(&self, reference: &Value, depth: usize, stack: &mut Vec<String>) -> Schema {
        self.schema(reference, depth, stack, Direction::Output)
    }

    /// The schema of a type reference: `NON_NULL` unwrapped, `LIST` an
    /// array, a named type by its kind.
    fn schema(
        &self,
        reference: &Value,
        depth: usize,
        stack: &mut Vec<String>,
        direction: Direction,
    ) -> Schema {
        match reference.get("kind").and_then(Value::as_str) {
            Some("NON_NULL") => self.schema(&reference["ofType"], depth, stack, direction),
            Some("LIST") => Schema {
                type_name: "array".to_string(),
                items: Some(Box::new(self.schema(
                    &reference["ofType"],
                    depth,
                    stack,
                    direction,
                ))),
                ..Schema::default()
            },
            _ => {
                let name = reference
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.named(name, depth, stack, direction)
            }
        }
    }

    fn named(
        &self,
        name: &str,
        depth: usize,
        stack: &mut Vec<String>,
        direction: Direction,
    ) -> Schema {
        let Some(kind) = self.types.get(name) else {
            return Schema::default();
        };
        match kind.get("kind").and_then(Value::as_str) {
            Some("SCALAR") => Schema {
                type_name: scalar_type(name).to_string(),
                ..Schema::default()
            },
            Some("ENUM") => Schema {
                type_name: "string".to_string(),
                enum_values: fields(kind, "enumValues")
                    .filter_map(|value| text(value, "name"))
                    .map(Value::String)
                    .collect(),
                ..Schema::default()
            },
            Some("OBJECT" | "INTERFACE" | "INPUT_OBJECT") => {
                let bound = match direction {
                    Direction::Input => GRAPHQL.input_depth,
                    Direction::Output => GRAPHQL.response_depth,
                };
                let bare = |limitation| Schema {
                    type_name: "object".to_string(),
                    limitations: vec![limitation],
                    ..Schema::default()
                };
                if stack.iter().any(|seen| seen == name) {
                    return bare(SchemaLimitation::Cycle {
                        reference: sanitize_text(name),
                    });
                }
                if depth >= bound {
                    return bare(SchemaLimitation::DepthBound);
                }
                stack.push(name.to_string());
                let properties = match direction {
                    Direction::Input => fields(kind, "inputFields")
                        .map(|field| self.input_property(field, depth + 1, stack))
                        .collect(),
                    Direction::Output => fields(kind, "fields")
                        .map(|field| {
                            let reference = field.get("type").cloned().unwrap_or(Value::Null);
                            Property {
                                name: text(field, "name").unwrap_or_default(),
                                required: is_non_null(&reference),
                                schema: self.output_schema(&reference, depth + 1, stack),
                            }
                        })
                        .collect(),
                };
                stack.pop();
                Schema {
                    type_name: "object".to_string(),
                    properties,
                    ..Schema::default()
                }
            }
            // A union's members have fields of their own; which one comes
            // back is the server's choice.
            _ => Schema {
                type_name: "object".to_string(),
                ..Schema::default()
            },
        }
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Input,
    Output,
}

/// Whether a named type is a scalar or an enum: a field that takes no
/// selection of its own.
fn is_leaf(kind: &Value) -> bool {
    matches!(
        kind.get("kind").and_then(Value::as_str),
        Some("SCALAR" | "ENUM")
    )
}

/// The type a reference names under its `NON_NULL` and `LIST` wrappers.
fn named_type(reference: &Value) -> &str {
    match reference.get("ofType") {
        Some(inner) if !inner.is_null() => named_type(inner),
        _ => reference
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    }
}

fn is_non_null(reference: &Value) -> bool {
    reference.get("kind").and_then(Value::as_str) == Some("NON_NULL")
}

/// The JSON type of a scalar; a custom scalar (`DateTime`, `JSON`) says
/// nothing about its JSON form.
fn scalar_type(name: &str) -> &'static str {
    match name {
        "Int" => "integer",
        "Float" => "number",
        "Boolean" => "boolean",
        "String" | "ID" => "string",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear() -> ApiSpec {
        let document: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openapi/graphql-linear.json"
        ))
        .unwrap();
        normalize_graphql(&document, "/graphql").unwrap()
    }

    fn operation<'a>(spec: &'a ApiSpec, id: &str) -> &'a Operation {
        spec.operations.iter().find(|op| op.id == id).unwrap()
    }

    #[test]
    fn root_fields_of_query_and_mutation_are_operations_on_the_endpoint() {
        let spec = linear();
        let ids: Vec<&str> = spec.operations.iter().map(|op| op.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "query.attachmentIssue",
                "query.issue",
                "query.viewer",
                "query.issues",
                "mutation.issueCreate"
            ]
        );
        for op in &spec.operations {
            assert_eq!((op.method.as_str(), op.path.as_str()), ("POST", "/graphql"));
            assert!(op.unsupported.is_empty(), "{}", op.id);
        }
        assert!(operation(&spec, "query.attachmentIssue").deprecated);
        assert!(!operation(&spec, "query.viewer").deprecated);
        assert_eq!(operation(&spec, "mutation.issueCreate").tags, ["mutation"]);
    }

    #[test]
    fn each_operation_names_its_field_the_argument_types_and_the_leaf_fields() {
        let spec = linear();
        let field = |id: &str| operation(&spec, id).graphql.clone().unwrap();
        let issues = field("query.issues");
        assert_eq!(
            (issues.root.as_str(), issues.name.as_str()),
            ("query", "issues")
        );
        assert_eq!(
            issues.arguments,
            [
                ("first".to_string(), "Int".to_string()),
                ("includeArchived".to_string(), "Boolean".to_string())
            ]
        );
        // IssueConnection has `nodes` only, an object list: nothing to select
        // by default.
        assert_eq!(issues.leaf_fields, Some(vec![]));
        assert_eq!(
            field("query.viewer").leaf_fields,
            Some(
                ["id", "createdAt", "name", "email", "admin"]
                    .map(String::from)
                    .to_vec()
            )
        );
        let create = field("mutation.issueCreate");
        assert_eq!(create.root, "mutation");
        assert_eq!(
            create.arguments,
            [("input".to_string(), "IssueCreateInput!".to_string())]
        );
        assert_eq!(
            create.leaf_fields,
            Some(["lastSyncId", "success"].map(String::from).to_vec())
        );
        let scalar = json!({"__schema": {"queryType": {"name": "Q"}, "types": [
            {"kind": "OBJECT", "name": "Q", "fields": [{"name": "count", "args": [],
                "type": {"kind": "NON_NULL", "ofType": {"kind": "SCALAR", "name": "Int"}}}]},
            {"kind": "SCALAR", "name": "Int"}
        ]}});
        let spec = normalize_graphql(&scalar, "/g").unwrap();
        assert_eq!(
            spec.operations[0].graphql.as_ref().unwrap().leaf_fields,
            None
        );
    }

    #[test]
    fn arguments_are_the_variables_and_an_input_object_names_its_required_fields() {
        let spec = linear();
        let create = operation(&spec, "mutation.issueCreate");
        let body = create.request_body.as_ref().unwrap();
        assert!(body.required);
        assert_eq!(
            body.description.as_deref(),
            Some("GraphQL variables: input: IssueCreateInput!")
        );
        assert_eq!(body.schema.skeleton(), json!({"input": {"teamId": ""}}));
        assert!(
            body.schema
                .optional_properties()
                .contains(&"input.title".to_string())
        );
        assert!(operation(&spec, "query.viewer").request_body.is_none());
        let issue = operation(&spec, "query.issue")
            .request_body
            .as_ref()
            .unwrap();
        assert_eq!(issue.schema.skeleton(), json!({"id": ""}));
    }

    #[test]
    fn the_return_type_is_the_response_to_the_depth_bound() {
        let spec = linear();
        let viewer = operation(&spec, "query.viewer").response.as_ref().unwrap();
        assert_eq!(viewer.description.as_deref(), Some("User!"));
        assert_eq!(viewer.schema.shape()["name"], json!("string"));
        assert_eq!(viewer.schema.shape()["admin"], json!("boolean"));
        assert_eq!(viewer.schema.shape()["createdAt"], json!("any"));
        let issue = operation(&spec, "query.issue").response.as_ref().unwrap();
        // Issue (level 1) -> assignee: User (level 2) -> nothing deeper here;
        // IssuePayload (1) -> issue: Issue (2) -> assignee: User is bare.
        assert_eq!(issue.schema.shape()["assignee"]["name"], json!("string"));
        assert_eq!(issue.schema.shape()["labelIds"], json!(["string"]));
        let payload = operation(&spec, "mutation.issueCreate")
            .response
            .as_ref()
            .unwrap();
        let assignee = &payload.schema.properties[1].schema.properties[5];
        assert_eq!(assignee.name, "assignee");
        assert_eq!(assignee.schema.limitations, [SchemaLimitation::DepthBound]);
        assert!(assignee.schema.properties.is_empty());
    }

    #[test]
    fn a_type_already_expanded_ends_in_a_cycle() {
        let types = json!({"__schema": {"queryType": {"name": "Query"}, "types": [
            {"kind": "OBJECT", "name": "Query", "fields": [
                {"name": "node", "args": [], "type": {"kind": "OBJECT", "name": "Node"}}]},
            {"kind": "OBJECT", "name": "Node", "fields": [
                {"name": "parent", "args": [], "type": {"kind": "OBJECT", "name": "Node"}}]}
        ]}});
        let spec = normalize_graphql(&types, "/gql").unwrap();
        let node = &spec.operations[0].response.as_ref().unwrap().schema;
        assert_eq!(
            node.properties[0].schema.limitations,
            [SchemaLimitation::Cycle {
                reference: "Node".into()
            }]
        );
    }

    #[test]
    fn an_answer_with_errors_only_is_a_refusal() {
        let refused: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openapi/graphql-refused.json"
        ))
        .unwrap();
        assert_eq!(
            introspection_refusal(&refused).as_deref(),
            Some(
                "GraphQL introspection is not allowed, but the query contained __schema or __type"
            )
        );
        assert!(normalize_graphql(&refused, "/graphql").is_err());
        let linear: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openapi/graphql-linear.json"
        ))
        .unwrap();
        assert_eq!(introspection_refusal(&linear), None);
        assert_eq!(introspection_refusal(&json!({"openapi": "3.0.0"})), None);
        let error = normalize_graphql(&json!({"openapi": "3.0.0"}), "/graphql").unwrap_err();
        assert!(error.contains("no `__schema`"), "{error}");
    }

    #[test]
    fn the_schema_object_alone_is_accepted_and_strings_are_terminal_safe() {
        let document = json!({"__schema": {"mutationType": {"name": "M"}, "types": [
            {"kind": "OBJECT", "name": "M", "fields": [{
                "name": "go\u{1b}[31m", "description": "Go\u{202e} on\nnow", "args": [],
                "type": {"kind": "SCALAR", "name": "Boolean"}}]},
            {"kind": "SCALAR", "name": "Boolean"}
        ]}});
        let spec = normalize_graphql(&document, "/g").unwrap();
        let op = &spec.operations[0];
        assert_eq!(op.id, "mutation.go[31m");
        assert_eq!(op.summary.as_deref(), Some("Go on"));
        assert_eq!(op.description.as_deref(), Some("Go on\nnow"));
    }

    #[test]
    fn the_introspection_request_is_one_query() {
        let body: Value = serde_json::from_slice(&introspection_request_body()).unwrap();
        assert_eq!(body["query"], INTROSPECTION_QUERY);
        assert!(INTROSPECTION_QUERY.starts_with("query IntrospectionQuery { __schema"));
    }
}
