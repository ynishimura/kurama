//! The API description `kurama api` explores: one `ApiSpec` normalized from
//! an OpenAPI 3.0 / 3.1 or Swagger 2.0 document (`functions::openapi`) or a
//! Google Discovery Document (`functions::discovery`), a
//! list of `Operation`s with their parameters, request/response bodies and scopes, and
//! a `Schema` reduced to what the CLI and the explorer show: the type, the
//! enum, an example and the properties a request body skeleton needs.

use serde_json::Value;

/// The kind of document an `[api.*]` description is, named by the key that
/// configures it; each is normalized into the same `ApiSpec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecFormat {
    /// `openapi`: OpenAPI 3.x or Swagger 2.0, JSON or YAML.
    OpenApi,
    /// `discovery`: a Google Discovery Document.
    Discovery,
    /// `graphql`: the schema of a GraphQL endpoint, from introspection or
    /// from a saved introspection result (`graphql_schema`).
    GraphQl(GraphQlEndpoint),
}

/// Where a GraphQL API takes its operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQlEndpoint {
    /// The endpoint's path under `base_url` (`/graphql`).
    pub path: String,
}

impl SpecFormat {
    /// The `[api.*]` key that names a description of this format.
    pub fn key(&self) -> &'static str {
        match self {
            Self::OpenApi => "openapi",
            Self::Discovery => "discovery",
            Self::GraphQl(_) => "graphql",
        }
    }
}

/// A normalized API description.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiSpec {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    /// The first server URL of the document; informational, the profile's
    /// `base_url` is what requests go to.
    pub server: Option<String>,
    /// In document order: paths as written, methods in `METHODS` order.
    pub operations: Vec<Operation>,
    /// What the normalizer could not represent (external `$ref`, `formData`
    /// or `cookie` parameters), one line each.
    pub warnings: Vec<String>,
}

/// One HTTP operation of the description.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// `operationId`, or `METHOD /path` when the document has none.
    pub id: String,
    pub has_operation_id: bool,
    /// Upper case.
    pub method: String,
    /// The path template, `/repos/{owner}/{repo}/issues`.
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// OAuth scopes the operation's security requirements name; the
    /// document's top-level requirements when the operation has none.
    pub scopes: Vec<String>,
    /// Path, query and header parameters, path-level ones included.
    pub parameters: Vec<Parameter>,
    pub request_body: Option<RequestBody>,
    /// The selected success response, when its body has a schema.
    pub response: Option<Response>,
    pub deprecated: bool,
    pub external_docs: Option<String>,
    /// Inputs kurama cannot send (`formData` and `cookie` parameters); the
    /// operation is listed but a call is refused.
    pub unsupported: Vec<String>,
    /// The root field a GraphQL operation calls; `None` for an HTTP
    /// operation.
    pub graphql: Option<GraphQlField>,
}

/// A root field of a GraphQL schema: what the query document of
/// `query.<name>` / `mutation.<name>` is written from.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphQlField {
    /// `query` or `mutation`.
    pub root: String,
    pub name: String,
    /// Every argument, in schema order, with its type as GraphQL writes it
    /// (`first`, `Int`; `input`, `IssueCreateInput!`).
    pub arguments: Vec<(String, String)>,
    /// The scalar and enum fields of the return type, the selection sent
    /// when none is given; `None` when the return type is a scalar or an
    /// enum itself and takes no selection.
    pub leaf_fields: Option<Vec<String>>,
}

impl Operation {
    /// `GET /repos/{owner}/{repo}/issues`.
    pub fn label(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    pub fn parameter(&self, name: &str) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.name == name).or_else(|| {
            self.parameters.iter().find(|p| {
                p.location == ParameterLocation::Header && p.name.eq_ignore_ascii_case(name)
            })
        })
    }

    /// The names of the required parameters and `body` when the request
    /// body is required.
    pub fn required_inputs(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .parameters
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name.clone())
            .collect();
        if self.request_body.as_ref().is_some_and(|b| b.required) {
            names.push("body".to_string());
        }
        names
    }
}

/// Where a parameter goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
}

impl ParameterLocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
    pub schema: Schema,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestBody {
    /// `application/json` when the operation accepts it, otherwise the
    /// first media type it lists.
    pub content_type: String,
    /// The other media types the operation accepts, whose schemas are not
    /// read.
    pub other_content_types: Vec<String>,
    pub required: bool,
    pub schema: Schema,
    pub description: Option<String>,
}

/// The first known response shape: 200, 201, another 2xx, then default.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: String,
    pub content_type: String,
    /// The other media types of the response, whose schemas are not read.
    pub other_content_types: Vec<String>,
    pub schema: Schema,
    pub description: Option<String>,
}

/// A JSON schema reduced to what is shown, validated and put into a body
/// skeleton. `$ref`s inside the document are resolved (to a bounded depth);
/// an external one stays in `unresolved_ref`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Schema {
    /// `string`, `integer`, `number`, `boolean`, `array`, `object`, or
    /// empty when the document does not say.
    pub type_name: String,
    pub enum_values: Vec<Value>,
    pub default: Option<Value>,
    pub example: Option<Value>,
    pub items: Option<Box<Schema>>,
    pub properties: Vec<Property>,
    pub unresolved_ref: Option<String>,
    /// What the reduction of this schema object left out.
    pub limitations: Vec<SchemaLimitation>,
}

/// Something the reduction of one schema object left out, so a contract
/// can say so instead of presenting the reduced schema as the whole.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaLimitation {
    /// `oneOf` / `anyOf`: only the first of `alternatives` is kept.
    FirstAlternativeOnly {
        keyword: &'static str,
        alternatives: usize,
    },
    /// A `$ref` back to one already being followed ends in a bare object.
    Cycle { reference: String },
    /// Nesting past the depth bound is not read.
    DepthBound,
    /// A 3.1 `type` array: the entries other than the type kept.
    OtherTypes { types: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub required: bool,
    pub schema: Schema,
}

impl Schema {
    /// The complete response shape, including optional properties. Scalar
    /// leaves name their type; example/default values are not response types.
    pub fn shape(&self) -> Value {
        // Destructured without `..` on purpose: a field added to `Schema`
        // has to be answered for by every projection below, or this stops
        // compiling. `shape` once forgot `unresolved_ref` and printed an
        // external `$ref` as `any` while the other two named it.
        let Schema {
            type_name,
            enum_values: _,
            default: _,
            example: _,
            items,
            properties,
            unresolved_ref,
            limitations: _,
        } = self;
        if let Some(reference) = unresolved_ref {
            return Value::String(format!("<$ref {reference}>"));
        }
        match type_name.as_str() {
            "array" => Value::Array(
                items
                    .as_ref()
                    .map(|items| vec![items.shape()])
                    .unwrap_or_default(),
            ),
            "object" => property_shapes(properties),
            _ if !properties.is_empty() => property_shapes(properties),
            "" => Value::String("any".into()),
            scalar => Value::String(scalar.to_string()),
        }
    }

    /// The type as the CLI and the explorer print it: `string`,
    /// `array<integer>`, `object`, `any`, or the enum values.
    pub fn display_type(&self) -> String {
        let Schema {
            type_name,
            enum_values,
            default: _,
            example: _,
            items,
            properties: _,
            unresolved_ref,
            limitations: _,
        } = self;
        if !enum_values.is_empty() {
            return enum_values
                .iter()
                .map(scalar_text)
                .collect::<Vec<_>>()
                .join("|");
        }
        if let Some(reference) = unresolved_ref {
            return format!("$ref {reference}");
        }
        match type_name.as_str() {
            "" => "any".to_string(),
            "array" => format!(
                "array<{}>",
                items
                    .as_ref()
                    .map(|items| items.display_type())
                    .unwrap_or_else(|| "any".to_string())
            ),
            other => other.to_string(),
        }
    }

    /// A value of this shape: the example, the default, the first enum
    /// value, or an empty value of the type, recursively for arrays and for
    /// the required properties of an object. Optional properties are left
    /// out: a document's examples would otherwise be sent as real values.
    pub fn skeleton(&self) -> Value {
        let Schema {
            type_name,
            enum_values,
            default,
            example,
            items,
            properties,
            unresolved_ref,
            limitations: _,
        } = self;
        if let Some(example) = example {
            return example.clone();
        }
        if let Some(default) = default {
            return default.clone();
        }
        if let Some(first) = enum_values.first() {
            return first.clone();
        }
        if let Some(reference) = unresolved_ref {
            return Value::String(format!("<$ref {reference}>"));
        }
        match type_name.as_str() {
            "string" => Value::String(String::new()),
            "integer" | "number" => Value::from(0),
            "boolean" => Value::Bool(false),
            "array" => Value::Array(
                items
                    .as_ref()
                    .map(|items| vec![items.skeleton()])
                    .unwrap_or_default(),
            ),
            "object" => required_properties(properties),
            _ if !properties.is_empty() => required_properties(properties),
            _ => Value::Null,
        }
    }

    /// The properties the skeleton leaves out, as paths (`labels`,
    /// `reviewer.email`, `assignees[].role`): the optional properties of
    /// every object the skeleton reaches.
    pub fn optional_properties(&self) -> Vec<String> {
        let mut paths = Vec::new();
        self.collect_optional("", &mut paths);
        paths
    }

    fn collect_optional(&self, prefix: &str, paths: &mut Vec<String>) {
        let Schema {
            type_name,
            enum_values,
            default,
            example,
            items,
            properties,
            unresolved_ref,
            limitations: _,
        } = self;
        // A seeded value (example, default, enum, `$ref`) is the skeleton
        // whole; the skeleton walks into arrays and properties only.
        if example.is_some()
            || default.is_some()
            || !enum_values.is_empty()
            || unresolved_ref.is_some()
        {
            return;
        }
        if type_name == "array" {
            if let Some(items) = items {
                items.collect_optional(&format!("{prefix}[]"), paths);
            }
            return;
        }
        for property in properties {
            let path = if prefix.is_empty() {
                property.name.clone()
            } else {
                format!("{prefix}.{}", property.name)
            };
            if property.required {
                property.schema.collect_optional(&path, paths);
            } else {
                paths.push(path);
            }
        }
    }
}

/// Every property, for `shape`.
fn property_shapes(properties: &[Property]) -> Value {
    Value::Object(
        properties
            .iter()
            .map(|property| (property.name.clone(), property.schema.shape()))
            .collect(),
    )
}

/// The required properties only, for `skeleton`.
fn required_properties(properties: &[Property]) -> Value {
    Value::Object(
        properties
            .iter()
            .filter(|property| property.required)
            .map(|property| (property.name.clone(), property.schema.skeleton()))
            .collect(),
    )
}

/// A scalar as text: strings without quotes, other values as JSON.
pub fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn string() -> Schema {
        Schema {
            type_name: "string".into(),
            ..Schema::default()
        }
    }

    #[test]
    fn display_type_names_enums_arrays_and_unresolved_refs() {
        assert_eq!(string().display_type(), "string");
        assert_eq!(Schema::default().display_type(), "any");
        let state = Schema {
            enum_values: vec![json!("open"), json!("closed"), json!(1)],
            ..string()
        };
        assert_eq!(state.display_type(), "open|closed|1");
        let labels = Schema {
            type_name: "array".into(),
            items: Some(Box::new(string())),
            ..Schema::default()
        };
        assert_eq!(labels.display_type(), "array<string>");
        let external = Schema {
            unresolved_ref: Some("https://example.com/schema.json#/A".into()),
            ..Schema::default()
        };
        assert_eq!(
            external.display_type(),
            "$ref https://example.com/schema.json#/A"
        );
        assert_eq!(
            external.shape(),
            json!("<$ref https://example.com/schema.json#/A>")
        );
    }

    #[test]
    fn skeleton_prefers_example_then_default_then_enum_then_the_empty_value() {
        assert_eq!(string().skeleton(), json!(""));
        assert_eq!(
            Schema {
                example: Some(json!("octocat")),
                default: Some(json!("x")),
                ..string()
            }
            .skeleton(),
            json!("octocat")
        );
        assert_eq!(
            Schema {
                default: Some(json!(30)),
                type_name: "integer".into(),
                ..Schema::default()
            }
            .skeleton(),
            json!(30)
        );
        assert_eq!(
            Schema {
                enum_values: vec![json!("open"), json!("closed")],
                ..string()
            }
            .skeleton(),
            json!("open")
        );
        let issue = Schema {
            type_name: "object".into(),
            properties: vec![
                Property {
                    name: "title".into(),
                    required: true,
                    schema: string(),
                },
                Property {
                    name: "labels".into(),
                    required: false,
                    schema: Schema {
                        type_name: "array".into(),
                        items: Some(Box::new(string())),
                        ..Schema::default()
                    },
                },
                Property {
                    name: "draft".into(),
                    required: false,
                    schema: Schema {
                        type_name: "boolean".into(),
                        ..Schema::default()
                    },
                },
                Property {
                    name: "milestone".into(),
                    required: true,
                    schema: Schema {
                        unresolved_ref: Some("other.yaml#/M".into()),
                        ..Schema::default()
                    },
                },
                Property {
                    name: "reviewer".into(),
                    required: true,
                    schema: Schema {
                        type_name: "object".into(),
                        properties: vec![
                            Property {
                                name: "login".into(),
                                required: true,
                                schema: string(),
                            },
                            Property {
                                name: "email".into(),
                                required: false,
                                schema: string(),
                            },
                        ],
                        ..Schema::default()
                    },
                },
                Property {
                    name: "assignees".into(),
                    required: true,
                    schema: Schema {
                        type_name: "array".into(),
                        items: Some(Box::new(Schema {
                            type_name: "object".into(),
                            properties: vec![
                                Property {
                                    name: "login".into(),
                                    required: true,
                                    schema: string(),
                                },
                                Property {
                                    name: "role".into(),
                                    required: false,
                                    schema: string(),
                                },
                            ],
                            ..Schema::default()
                        })),
                        ..Schema::default()
                    },
                },
                Property {
                    name: "extra".into(),
                    required: false,
                    schema: Schema::default(),
                },
            ],
            ..Schema::default()
        };
        assert_eq!(
            issue.skeleton(),
            json!({
                "title": "",
                "milestone": "<$ref other.yaml#/M>",
                "reviewer": {"login": ""},
                "assignees": [{"login": ""}]
            }),
            "optional properties (labels, draft, extra, email, role) are left out at every level"
        );
        assert_eq!(
            issue.optional_properties(),
            [
                "labels",
                "draft",
                "reviewer.email",
                "assignees[].role",
                "extra"
            ],
            "the paths the skeleton leaves out, at every level it reaches"
        );
        assert_eq!(
            Schema {
                type_name: "array".into(),
                items: Some(Box::new(Schema {
                    type_name: "boolean".into(),
                    ..Schema::default()
                })),
                ..Schema::default()
            }
            .skeleton(),
            json!([false])
        );
    }

    #[test]
    fn required_inputs_include_the_body_when_it_is_required() {
        let operation = Operation {
            id: "issues/create".into(),
            has_operation_id: true,
            method: "POST".into(),
            path: "/repos/{owner}/{repo}/issues".into(),
            summary: None,
            description: None,
            tags: vec![],
            scopes: vec![],
            parameters: vec![
                Parameter {
                    name: "owner".into(),
                    location: ParameterLocation::Path,
                    required: true,
                    schema: string(),
                    description: None,
                },
                Parameter {
                    name: "page".into(),
                    location: ParameterLocation::Query,
                    required: false,
                    schema: string(),
                    description: None,
                },
            ],
            response: None,
            request_body: Some(RequestBody {
                content_type: "application/json".into(),
                other_content_types: vec![],
                required: true,
                schema: string(),
                description: None,
            }),
            deprecated: false,
            external_docs: None,
            unsupported: vec![],
            graphql: None,
        };
        assert_eq!(operation.required_inputs(), ["owner", "body"]);
        assert_eq!(operation.label(), "POST /repos/{owner}/{repo}/issues");
        assert!(operation.parameter("page").is_some());
        assert!(operation.parameter("repo").is_none());
    }
}
