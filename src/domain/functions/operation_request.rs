//! An operation of the description plus `-P name=value` parameters and a
//! body, turned into the request `kurama api` sends.

use crate::domain::functions::api_request::join_base_path;
use crate::domain::functions::graphql_call::build_graphql_request;
use crate::domain::types::api_spec::{Operation, ParameterLocation, Schema, scalar_text};
use crate::domain::types::http::HttpRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationRequestError {
    /// The operation needs inputs kurama cannot send.
    Unsupported {
        operation: String,
        inputs: Vec<String>,
    },
    UnknownParameter {
        name: String,
        known: Vec<String>,
    },
    InvalidValue {
        name: String,
        message: String,
    },
    /// Required parameters (and `body`) without a value.
    Missing {
        operation: String,
        missing: Vec<String>,
    },
    /// `-d` of a GraphQL operation is not a JSON object of variables.
    InvalidBody(String),
    /// A GraphQL operation's selection: none to make by default, or one
    /// given to a field that takes none.
    Selection(String),
}

impl std::fmt::Display for OperationRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { operation, inputs } => write!(
                f,
                "operation {operation} needs {} which kurama does not send",
                inputs.join(", ")
            ),
            Self::UnknownParameter { name, known } => {
                if known.is_empty() {
                    write!(f, "unknown parameter {name:?}: the operation has none")
                } else {
                    write!(
                        f,
                        "unknown parameter {name:?}; the operation takes {}",
                        known.join(", ")
                    )
                }
            }
            Self::InvalidValue { name, message } => write!(f, "parameter {name}: {message}"),
            Self::Missing { operation, missing } => write!(
                f,
                "operation {operation} needs {}",
                missing
                    .iter()
                    .map(|name| {
                        if name == "body" {
                            "a request body (-d)".to_string()
                        } else {
                            format!("-P {name}=<value>")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::InvalidBody(message) => write!(f, "-d: {message}"),
            Self::Selection(message) => write!(f, "{message}"),
        }
    }
}

/// The request for `operation` under `base_url`: path parameters filled
/// into the template (percent-encoded), query parameters appended, header
/// parameters set, the body with the operation's media type. `params` are
/// `-P name=value` pairs in order; a name may repeat for a query parameter.
/// A GraphQL operation is its query document with the default selection.
pub fn build_operation_request(
    base_url: &str,
    operation: &Operation,
    params: &[(String, String)],
    body: Option<Vec<u8>>,
) -> Result<HttpRequest, OperationRequestError> {
    if let Some(field) = &operation.graphql {
        return build_graphql_request(base_url, operation, field, params, body, None);
    }
    if !operation.unsupported.is_empty() {
        return Err(OperationRequestError::Unsupported {
            operation: operation.id.clone(),
            inputs: operation.unsupported.clone(),
        });
    }
    let mut path = operation.path.clone();
    let mut query: Vec<(String, String)> = Vec::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut given: Vec<&str> = Vec::new();
    for (name, value) in params {
        let parameter =
            operation
                .parameter(name)
                .ok_or_else(|| OperationRequestError::UnknownParameter {
                    name: name.clone(),
                    known: operation
                        .parameters
                        .iter()
                        .map(|p| p.name.clone())
                        .collect(),
                })?;
        validate_value(&parameter.schema, value).map_err(|message| {
            OperationRequestError::InvalidValue {
                name: parameter.name.clone(),
                message,
            }
        })?;
        given.push(parameter.name.as_str());
        match parameter.location {
            ParameterLocation::Path => {
                path = path
                    .replace(
                        &format!("{{{}}}", parameter.name),
                        &encode_path_segment(value, false),
                    )
                    .replace(
                        &format!("{{+{}}}", parameter.name),
                        &encode_path_segment(value, true),
                    );
            }
            ParameterLocation::Query => query.push((parameter.name.clone(), value.clone())),
            ParameterLocation::Header => headers.push((parameter.name.clone(), value.clone())),
        }
    }
    let missing: Vec<String> = operation
        .required_inputs()
        .into_iter()
        .filter(|name| {
            if name == "body" {
                body.is_none()
            } else {
                !given.contains(&name.as_str())
            }
        })
        .collect();
    if !missing.is_empty() {
        return Err(OperationRequestError::Missing {
            operation: operation.id.clone(),
            missing,
        });
    }
    let mut url = join_base_path(base_url, &path);
    if !query.is_empty() {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (name, value) in &query {
            serializer.append_pair(name, value);
        }
        url.push('?');
        url.push_str(&serializer.finish());
    }
    let mut request = HttpRequest::new(operation.method.clone(), url);
    for (name, value) in headers {
        request = request.with_header(name, value);
    }
    if let Some(body) = body {
        if let Some(request_body) = &operation.request_body {
            request = request.with_header("Content-Type", request_body.content_type.clone());
        }
        request = request.with_body(body);
    }
    Ok(request)
}

/// Whether `value` (as typed on the command line) fits the parameter's
/// type and enum.
pub fn validate_value(schema: &Schema, value: &str) -> Result<(), String> {
    if !schema.enum_values.is_empty() {
        let allowed: Vec<String> = schema.enum_values.iter().map(scalar_text).collect();
        if !allowed.iter().any(|candidate| candidate == value) {
            return Err(format!("{value:?} is not one of {}", allowed.join(", ")));
        }
        return Ok(());
    }
    match schema.type_name.as_str() {
        "integer" if value.parse::<i64>().is_err() => Err(format!("{value:?} is not an integer")),
        "number" if value.parse::<f64>().is_err() => Err(format!("{value:?} is not a number")),
        "boolean" if value != "true" && value != "false" => {
            Err(format!("{value:?} is not true or false"))
        }
        _ => Ok(()),
    }
}

/// A path parameter value with everything but unreserved characters
/// percent-encoded; with `reserved` (`{+name}`) the reserved characters too
/// are kept as they are.
fn encode_path_segment(value: &str, reserved: bool) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            // RFC 6570 reserved expansion (`{+name}`) keeps the reserved set.
            b':' | b'/' | b'?' | b'#' | b'[' | b']' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'('
            | b')' | b'*' | b'+' | b',' | b';' | b'='
                if reserved =>
            {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::api_spec::{Parameter, Property, RequestBody};
    use serde_json::json;

    fn schema(type_name: &str) -> Schema {
        Schema {
            type_name: type_name.into(),
            ..Schema::default()
        }
    }

    fn parameter(
        name: &str,
        location: ParameterLocation,
        required: bool,
        schema: Schema,
    ) -> Parameter {
        Parameter {
            name: name.into(),
            location,
            required,
            schema,
            description: None,
        }
    }

    fn create_issue() -> Operation {
        Operation {
            id: "issues/create".into(),
            has_operation_id: true,
            method: "POST".into(),
            path: "/repos/{owner}/{repo}/issues".into(),
            summary: Some("Create an issue".into()),
            description: None,
            tags: vec!["issues".into()],
            scopes: vec!["repo".into()],
            parameters: vec![
                parameter("owner", ParameterLocation::Path, true, schema("string")),
                parameter("repo", ParameterLocation::Path, true, schema("string")),
                parameter(
                    "state",
                    ParameterLocation::Query,
                    false,
                    Schema {
                        enum_values: vec![json!("open"), json!("closed")],
                        ..schema("string")
                    },
                ),
                parameter(
                    "per_page",
                    ParameterLocation::Query,
                    false,
                    schema("integer"),
                ),
                parameter(
                    "X-GitHub-Api-Version",
                    ParameterLocation::Header,
                    false,
                    schema("string"),
                ),
            ],
            response: None,
            request_body: Some(RequestBody {
                content_type: "application/json".into(),
                other_content_types: vec![],
                required: true,
                schema: Schema {
                    properties: vec![Property {
                        name: "title".into(),
                        required: true,
                        schema: schema("string"),
                    }],
                    ..schema("object")
                },
                description: None,
            }),
            deprecated: false,
            external_docs: None,
            unsupported: vec![],
            graphql: None,
        }
    }

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn parameters_go_to_the_path_the_query_and_the_headers() {
        let request = build_operation_request(
            "https://api.github.com/",
            &create_issue(),
            &params(&[
                ("owner", "octo cat"),
                ("repo", "hello/world"),
                ("state", "open"),
                ("per_page", "5"),
                ("x-github-api-version", "2022-11-28"),
            ]),
            Some(b"{\"title\":\"x\"}".to_vec()),
        )
        .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.url,
            "https://api.github.com/repos/octo%20cat/hello%2Fworld/issues?state=open&per_page=5"
        );
        assert_eq!(
            request.header("X-GitHub-Api-Version"),
            Some("2022-11-28"),
            "header names match case-insensitively and keep the document's spelling"
        );
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(request.body.as_deref(), Some(&b"{\"title\":\"x\"}"[..]));
    }

    /// RFC 6570 reserved expansion (`{+name}`, Google Discovery): `/`, `!`
    /// and the other reserved characters go as they are; a space or `%`
    /// is still encoded.
    #[test]
    fn a_reserved_expansion_keeps_reserved_characters() {
        let operation = Operation {
            path: "/v1/{+resourceName}/values/{range}".into(),
            parameters: vec![
                parameter(
                    "resourceName",
                    ParameterLocation::Path,
                    true,
                    schema("string"),
                ),
                parameter("range", ParameterLocation::Path, true, schema("string")),
            ],
            request_body: None,
            ..create_issue()
        };
        let request = build_operation_request(
            "https://people.googleapis.com",
            &operation,
            &params(&[
                ("resourceName", "people/me!A1:B2 x%"),
                ("range", "Sheet1!A1"),
            ]),
            None,
        )
        .unwrap();
        assert_eq!(
            request.url,
            "https://people.googleapis.com/v1/people/me!A1:B2%20x%25/values/Sheet1%21A1"
        );
    }

    #[test]
    fn missing_required_inputs_are_listed_and_nothing_is_built() {
        let error = build_operation_request(
            "https://api.github.com",
            &create_issue(),
            &params(&[("owner", "o")]),
            None,
        )
        .unwrap_err();
        assert_eq!(
            error,
            OperationRequestError::Missing {
                operation: "issues/create".into(),
                missing: vec!["repo".into(), "body".into()],
            }
        );
        assert_eq!(
            error.to_string(),
            "operation issues/create needs -P repo=<value>, a request body (-d)"
        );
    }

    #[test]
    fn unknown_parameters_and_bad_values_are_refused() {
        let unknown = build_operation_request(
            "https://x",
            &create_issue(),
            &params(&[("owner", "o"), ("repo", "r"), ("labels", "a")]),
            Some(vec![]),
        )
        .unwrap_err();
        assert!(
            matches!(&unknown, OperationRequestError::UnknownParameter { name, known } if name == "labels" && known.len() == 5),
            "{unknown:?}"
        );
        assert!(unknown.to_string().contains("owner, repo, state"));
        for (name, value, expected) in [
            ("state", "closd", "\"closd\" is not one of open, closed"),
            ("per_page", "many", "\"many\" is not an integer"),
        ] {
            let error = build_operation_request(
                "https://x",
                &create_issue(),
                &params(&[("owner", "o"), ("repo", "r"), (name, value)]),
                Some(vec![]),
            )
            .unwrap_err();
            assert_eq!(
                error,
                OperationRequestError::InvalidValue {
                    name: name.into(),
                    message: expected.into()
                }
            );
        }
        assert!(validate_value(&schema("boolean"), "yes").is_err());
        assert!(validate_value(&schema("boolean"), "true").is_ok());
        assert!(validate_value(&schema("number"), "1.5").is_ok());
        assert!(validate_value(&schema("number"), "x").is_err());
        assert!(validate_value(&schema(""), "anything").is_ok());
    }

    #[test]
    fn unsupported_operations_are_refused_before_anything_else() {
        let mut upload = create_issue();
        upload.unsupported = vec!["formData parameter 'file'".into()];
        let error = build_operation_request("https://x", &upload, &[], None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "operation issues/create needs formData parameter 'file' which kurama does not send"
        );
    }

    #[test]
    fn a_repeated_query_parameter_is_appended() {
        let mut operation = create_issue();
        operation.request_body = None;
        operation.parameters.push(parameter(
            "labels",
            ParameterLocation::Query,
            false,
            schema("array"),
        ));
        let request = build_operation_request(
            "https://x/api/",
            &operation,
            &params(&[
                ("owner", "o"),
                ("repo", "r"),
                ("labels", "a"),
                ("labels", "b"),
            ]),
            None,
        )
        .unwrap();
        assert_eq!(
            request.url,
            "https://x/api/repos/o/r/issues?labels=a&labels=b"
        );
        assert!(request.header("content-type").is_none());
    }
}
