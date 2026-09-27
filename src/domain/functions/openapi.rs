//! OpenAPI 3.x and Swagger 2.0 documents, already parsed from JSON or YAML
//! into a `serde_json::Value`, normalized into one `ApiSpec`.
//!
//! What is kept: every operation with its path, query and header
//! parameters (path-level ones merged in), its request body with the media
//! type and a reduced schema, its OAuth scopes (operation-level `security`
//! over the document's), tags, summary and description. `$ref`s inside the
//! document (`#/components/...`, `#/definitions/...`, `#/parameters/...`)
//! are resolved to a bounded depth, cycles stop at an empty object, and
//! `oneOf` / `anyOf` keep their first alternative only. A `$ref` that does
//! not resolve (external, or a dangling local one), a `formData` or a
//! `cookie` parameter cannot be represented: the schema or operation says
//! so, and the spec carries a warning line for each. A hand-written minimal
//! document (`openapi` and `paths` only) is read with defaults, not warnings:
//! an undeclared `{name}` of a path is a required string path parameter.

use serde_json::{Map, Value};

use crate::domain::types::api_spec::{
    ApiSpec, Operation, Parameter, ParameterLocation, RequestBody, Response, Schema, scalar_text,
};

/// HTTP methods of a path item, in listing order.
pub const METHODS: [&str; 8] = [
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// Nesting beyond which a schema keeps its type but loses its properties.
use crate::domain::types::limits::SHAPE;

use super::openapi_schema::Document;

pub(super) const MAX_SCHEMA_DEPTH: usize = SHAPE.depth;

const SUPPORTED_VERSIONS: &str = "OpenAPI 3.x and Swagger 2.0";

/// Document text is external input: strip controls and invisible formatting
/// characters before it can reach a terminal, a diagnostic or a completion
/// description.
pub(super) fn sanitize_text(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{feff}'
                )
        })
        .collect()
}

/// Descriptions keep paragraph boundaries; every other control is removed.
fn description(object: &Map<String, Value>) -> Option<String> {
    object
        .get("description")
        .and_then(Value::as_str)
        .map(|text| {
            text.split('\n')
                .map(sanitize_text)
                .collect::<Vec<_>>()
                .join("\n")
        })
}

pub(super) fn sanitize_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(sanitize_text(text)),
        Value::Array(values) => Value::Array(values.iter().map(sanitize_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (sanitize_text(key), sanitize_value(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DocumentFormat {
    /// OpenAPI 3.x: `servers`, `requestBody`, `components`.
    OpenApi3,
    /// Swagger 2.0: `host` / `basePath`, an `in: body` parameter, `definitions`.
    Swagger2,
}

impl DocumentFormat {
    /// The format from the document's version marker.
    fn detect(root: &Map<String, Value>) -> Result<Self, String> {
        // YAML leaves `swagger: 2.0` and `version: 1.0` as numbers.
        let header = |key: &str| root.get(key).map(scalar_text);
        match (header("openapi"), header("swagger")) {
            (Some(version), _) if version.starts_with("3.") => Ok(Self::OpenApi3),
            (Some(version), _) => Err(format!(
                "unsupported OpenAPI version {version:?}; {SUPPORTED_VERSIONS} are supported"
            )),
            (None, Some(version)) if version == "2.0" => Ok(Self::Swagger2),
            (None, Some(version)) => Err(format!(
                "unsupported Swagger version {version:?}; {SUPPORTED_VERSIONS} are supported"
            )),
            (None, None) => Err(
                "not an OpenAPI or Swagger document: neither `openapi` nor `swagger` is set".into(),
            ),
        }
    }

    /// The first server URL (OpenAPI 3), or `scheme://host/basePath`
    /// (Swagger 2).
    fn server(self, root: &Map<String, Value>) -> Option<String> {
        match self {
            Self::OpenApi3 => root
                .get("servers")
                .and_then(Value::as_array)
                .and_then(|servers| servers.first())
                .and_then(|server| server.get("url"))
                .and_then(Value::as_str)
                .map(sanitize_text),
            Self::Swagger2 => {
                let host = root.get("host").and_then(Value::as_str)?;
                let scheme = root
                    .get("schemes")
                    .and_then(Value::as_array)
                    .and_then(|schemes| schemes.first())
                    .and_then(Value::as_str)
                    .unwrap_or("https");
                let base_path = root.get("basePath").and_then(Value::as_str).unwrap_or("");
                Some(sanitize_text(&format!("{scheme}://{host}{base_path}")))
            }
        }
    }
}

pub fn normalize_spec(document: &Value) -> Result<ApiSpec, String> {
    let root = document
        .as_object()
        .ok_or_else(|| "the document is not a JSON object".to_string())?;
    let format = DocumentFormat::detect(root)?;
    let mut normalizer = Normalizer {
        document: Document::new(document),
        format,
    };
    let operations = normalizer.operations(root)?;
    let info = root.get("info").and_then(Value::as_object);
    let text = |key: &str| {
        info.and_then(|info| info.get(key))
            .and_then(Value::as_str)
            .map(sanitize_text)
    };
    Ok(ApiSpec {
        title: text("title").unwrap_or_else(|| "API".to_string()),
        version: info
            .and_then(|info| info.get("version"))
            .map(|value| sanitize_text(&scalar_text(value)))
            .unwrap_or_default(),
        description: info.and_then(description),
        server: format.server(root),
        operations,
        warnings: normalizer.document.warnings,
    })
}

struct Normalizer<'d> {
    document: Document<'d>,
    format: DocumentFormat,
}

/// The `parameters` of an operation, sorted by what kurama does with them.
struct ParameterInputs<'d> {
    parameters: Vec<Parameter>,
    /// The Swagger 2.0 `in: body` parameter, when there is one.
    body: Option<&'d Map<String, Value>>,
    unsupported: Vec<String>,
}

impl<'d> Normalizer<'d> {
    fn operations(&mut self, root: &'d Map<String, Value>) -> Result<Vec<Operation>, String> {
        let paths = root
            .get("paths")
            .and_then(Value::as_object)
            .ok_or_else(|| "the document has no `paths` object".to_string())?;
        let root_scopes = root.get("security").map(scopes_of);
        let root_consumes = root.get("consumes");
        let mut operations = Vec::new();
        for (path, item) in paths {
            let Some(item) = self.document.deref(item).and_then(Value::as_object) else {
                continue;
            };
            let path_parameters: Vec<&'d Value> = item
                .get("parameters")
                .and_then(Value::as_array)
                .map(|parameters| parameters.iter().collect())
                .unwrap_or_default();
            for method in METHODS {
                let Some(operation) = item.get(method).and_then(Value::as_object) else {
                    continue;
                };
                let scopes = match operation.get("security") {
                    Some(security) => scopes_of(security),
                    None => root_scopes.clone().unwrap_or_default(),
                };
                let consumes = operation.get("consumes").or(root_consumes);
                operations.push(self.operation(
                    path,
                    method,
                    operation,
                    &path_parameters,
                    scopes,
                    consumes,
                ));
            }
        }
        Ok(operations)
    }

    fn operation(
        &mut self,
        path: &str,
        method: &str,
        operation: &'d Map<String, Value>,
        path_parameters: &[&'d Value],
        scopes: Vec<String>,
        consumes: Option<&'d Value>,
    ) -> Operation {
        let method = method.to_ascii_uppercase();
        let path = sanitize_text(path);
        let text = |key: &str| {
            operation
                .get(key)
                .and_then(Value::as_str)
                .map(sanitize_text)
        };
        let (id, has_operation_id) = match text("operationId") {
            Some(id) if !id.is_empty() => (id, true),
            _ => (format!("{method} {path}"), false),
        };
        let mut inputs = self.collect_parameters(&id, operation, path_parameters);
        add_undeclared_path_parameters(&path, &mut inputs.parameters);
        let request_body = match self.format {
            DocumentFormat::OpenApi3 => operation
                .get("requestBody")
                .and_then(|body| self.request_body(body)),
            DocumentFormat::Swagger2 => inputs
                .body
                .map(|parameter| self.body_parameter(parameter, consumes)),
        };
        let tags = operation
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(Value::as_str)
                    .map(sanitize_text)
                    .collect()
            })
            .unwrap_or_default();
        Operation {
            id,
            has_operation_id,
            method,
            path,
            summary: text("summary"),
            description: description(operation),
            tags,
            scopes,
            parameters: inputs.parameters,
            request_body,
            response: self.response(operation),
            deprecated: operation
                .get("deprecated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            external_docs: operation
                .get("externalDocs")
                .and_then(|docs| docs.get("url"))
                .and_then(Value::as_str)
                .map(sanitize_text),
            unsupported: inputs.unsupported,
            graphql: None,
        }
    }

    /// Collect path-level and operation-level parameters. The latter are
    /// intentionally returned after the former so the caller can preserve the
    /// existing override rule.
    fn operation_parameters(
        &mut self,
        operation: &'d Map<String, Value>,
        path_parameters: &[&'d Value],
    ) -> Vec<&'d Map<String, Value>> {
        let own = operation
            .get("parameters")
            .and_then(Value::as_array)
            .map(|parameters| parameters.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        path_parameters
            .iter()
            .copied()
            .chain(own)
            .filter_map(|value| self.document.deref(value).and_then(Value::as_object))
            .collect()
    }

    /// Path, query and header parameters as `Parameter`s, a Swagger 2.0
    /// `in: body` parameter handed back for `body_parameter`, and the
    /// `formData` / `cookie` ones reported as unsupported.
    fn collect_parameters(
        &mut self,
        id: &str,
        operation: &'d Map<String, Value>,
        path_parameters: &[&'d Value],
    ) -> ParameterInputs<'d> {
        let mut inputs = ParameterInputs {
            parameters: Vec::new(),
            body: None,
            unsupported: Vec::new(),
        };
        for raw in self.operation_parameters(operation, path_parameters) {
            let location = raw.get("in").and_then(Value::as_str).unwrap_or_default();
            match location {
                "path" | "query" | "header" => {
                    if let Some(parameter) = self.supported_parameter(raw) {
                        Self::insert_parameter(&mut inputs.parameters, parameter);
                    }
                }
                "body" => inputs.body = Some(raw),
                "formData" | "cookie" => {
                    self.add_unsupported(id, raw, location, &mut inputs.unsupported);
                }
                _ => {}
            }
        }
        inputs
    }

    fn supported_parameter(&mut self, raw: &'d Map<String, Value>) -> Option<Parameter> {
        let location = match raw.get("in").and_then(Value::as_str)? {
            "path" => ParameterLocation::Path,
            "query" => ParameterLocation::Query,
            "header" => ParameterLocation::Header,
            _ => return None,
        };
        let name = sanitize_text(raw.get("name").and_then(Value::as_str).unwrap_or_default());
        Some(Parameter {
            name,
            location,
            required: raw
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || location == ParameterLocation::Path,
            schema: self.parameter_schema(raw),
            description: description(raw),
        })
    }

    fn insert_parameter(parameters: &mut Vec<Parameter>, parameter: Parameter) {
        match parameters.iter().position(|existing| {
            existing.name == parameter.name && existing.location == parameter.location
        }) {
            Some(index) => parameters[index] = parameter,
            None => parameters.push(parameter),
        }
    }

    fn add_unsupported(
        &mut self,
        id: &str,
        raw: &Map<String, Value>,
        location: &str,
        unsupported: &mut Vec<String>,
    ) {
        let name = raw.get("name").and_then(Value::as_str).unwrap_or_default();
        let line = sanitize_text(&format!("{location} parameter '{name}'"));
        self.document.warn(format!("{id}: {line} is not supported"));
        unsupported.push(line);
    }

    /// Swagger 2: the `in: body` parameter as the request body, with the
    /// media type from `consumes`.
    fn body_parameter(
        &mut self,
        parameter: &'d Map<String, Value>,
        consumes: Option<&'d Value>,
    ) -> RequestBody {
        let schema = parameter
            .get("schema")
            .map(|schema| self.document.root_schema(schema))
            .unwrap_or_default();
        let types: Vec<&str> = consumes
            .and_then(Value::as_array)
            .map(|types| types.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let content_type = pick_media_type(types.iter().copied())
            .unwrap_or_else(|| "application/json".to_string());
        RequestBody {
            other_content_types: other_media_types(types, &content_type),
            content_type: sanitize_text(&content_type),
            required: parameter
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            schema,
            description: description(parameter),
        }
    }

    /// OpenAPI 3: the parameter's `schema` (or the schema of its first
    /// `content` media type); Swagger 2: the type keys on the parameter.
    fn parameter_schema(&mut self, parameter: &'d Map<String, Value>) -> Schema {
        if let Some(schema) = parameter.get("schema") {
            return self.document.root_schema(schema);
        }
        if let Some(media) = parameter
            .get("content")
            .and_then(Value::as_object)
            .and_then(|content| content.values().next())
            .and_then(|media| media.get("schema"))
        {
            return self.document.root_schema(media);
        }
        // Swagger 2.0 keeps type, enum, items on the parameter itself.
        let mut schema = self.document.schema_fields(parameter, 0, &mut Vec::new());
        if schema.example.is_none() {
            schema.example = parameter.get("x-example").map(sanitize_value);
        }
        schema
    }

    fn request_body(&mut self, body: &'d Value) -> Option<RequestBody> {
        let body = self.document.deref(body)?.as_object()?;
        let content = body.get("content").and_then(Value::as_object);
        let media_type = content
            .and_then(|content| pick_media_type(content.keys().map(String::as_str)))
            .unwrap_or_else(|| "application/json".to_string());
        let schema = content
            .and_then(|content| content.get(&media_type))
            .and_then(|media| media.get("schema"))
            .map(|schema| self.document.root_schema(schema))
            .unwrap_or_default();
        Some(RequestBody {
            other_content_types: other_media_types(
                content
                    .into_iter()
                    .flat_map(|content| content.keys().map(String::as_str)),
                &media_type,
            ),
            content_type: sanitize_text(&media_type),
            required: body
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            schema,
            description: description(body),
        })
    }

    /// Select one response before normalizing its schema. A response with
    /// no body/schema (such as 204) has no shape to describe.
    fn response(&mut self, operation: &'d Map<String, Value>) -> Option<Response> {
        let responses = operation.get("responses")?.as_object()?;
        let (status, response) = responses
            .get_key_value("200")
            .or_else(|| responses.get_key_value("201"))
            .or_else(|| responses.iter().find(|(status, _)| status.starts_with('2')))
            .or_else(|| responses.get_key_value("default"))?;
        let response = self.document.deref(response)?.as_object()?;
        let (content_type, other_content_types, raw_schema) = match self.format {
            DocumentFormat::OpenApi3 => {
                let content = response.get("content")?.as_object()?;
                let media = pick_media_type(content.keys().map(String::as_str))?;
                let schema = content.get(&media).and_then(|body| body.get("schema"))?;
                let others = other_media_types(content.keys().map(String::as_str), &media);
                (media, others, schema)
            }
            DocumentFormat::Swagger2 => {
                let produces = operation
                    .get("produces")
                    .or_else(|| self.document.value().get("produces"))
                    .and_then(Value::as_array);
                let types: Vec<&str> = produces
                    .map(|types| types.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let media = pick_media_type(types.iter().copied())
                    .unwrap_or_else(|| "application/json".to_string());
                let others = other_media_types(types, &media);
                (media, others, response.get("schema")?)
            }
        };
        Some(Response {
            status: sanitize_text(status),
            content_type: sanitize_text(&content_type),
            other_content_types,
            schema: self.document.root_schema(raw_schema),
            description: description(response),
        })
    }
}

/// A `{name}` of the path template that no parameter declares, as a
/// required string path parameter: a hand-written description may leave
/// them out, and the template still has to be filled.
fn add_undeclared_path_parameters(path: &str, parameters: &mut Vec<Parameter>) {
    for name in path
        .split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}'))
        .map(|(name, _)| name.trim_start_matches('+'))
    {
        let declared = parameters
            .iter()
            .any(|p| p.location == ParameterLocation::Path && p.name == name);
        if !declared && !name.is_empty() {
            parameters.push(Parameter {
                name: name.to_string(),
                location: ParameterLocation::Path,
                required: true,
                schema: Schema {
                    type_name: "string".to_string(),
                    ..Schema::default()
                },
                description: None,
            });
        }
    }
}

/// The scopes named by a `security` requirement list, sorted and unique.
fn scopes_of(security: &Value) -> Vec<String> {
    let mut scopes: Vec<String> = security
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|requirement| requirement.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(Value::as_str)
        .map(sanitize_text)
        .collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

/// `application/json` when offered, else the first JSON media type, else
/// the first media type.
fn pick_media_type<'a>(types: impl Iterator<Item = &'a str>) -> Option<String> {
    let types: Vec<&str> = types.collect();
    types
        .iter()
        .find(|t| **t == "application/json")
        .or_else(|| types.iter().find(|t| t.contains("json")))
        .or_else(|| types.first())
        .map(|t| t.to_string())
}

/// The media types other than the one `pick_media_type` chose, in document
/// order.
fn other_media_types<'a>(types: impl IntoIterator<Item = &'a str>, chosen: &str) -> Vec<String> {
    types
        .into_iter()
        .filter(|media| *media != chosen)
        .map(sanitize_text)
        .collect()
}

#[cfg(test)]
#[path = "openapi_tests.rs"]
mod tests;
