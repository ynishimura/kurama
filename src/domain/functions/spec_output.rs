//! What `kurama api --ops` and `--describe` print: the operation table,
//! the operation description, and their `--json` forms.

use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

use crate::domain::functions::operation_command::example_command;
use crate::domain::types::api_spec::{
    Operation, Parameter, RequestBody, Response, Schema, scalar_text,
};

/// Columns between two table cells.
const GAP: &str = "  ";

/// `--ops --json`: one object per operation.
pub fn operations_json(operations: &[&Operation]) -> Value {
    Value::Array(
        operations
            .iter()
            .map(|operation| {
                json!({
                    "id": operation.id,
                    "method": operation.method,
                    "path": operation.path,
                    "summary": operation.summary,
                    "tags": operation.tags,
                    "scopes": operation.scopes,
                    "deprecated": operation.deprecated,
                })
            })
            .collect(),
    )
}

/// `--ops`: an `ID  METHOD  PATH  SUMMARY` table, one line per operation.
pub fn render_operations_table(operations: &[&Operation]) -> String {
    let rows: Vec<[String; 4]> = operations
        .iter()
        .map(|operation| {
            [
                operation.id.clone(),
                operation.method.clone(),
                operation.path.clone(),
                first_line(operation.summary.as_deref().unwrap_or_default()),
            ]
        })
        .collect();
    render_table(&["ID", "METHOD", "PATH", "SUMMARY"], &rows)
}

/// `--describe --json`.
pub fn operation_json(api: &str, operation: &Operation) -> Value {
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
        "parameters": parameters.iter().map(parameter_json).collect::<Vec<_>>(),
        "request_body": request_body.as_ref().map(request_body_json),
        "response": response.as_ref().map(response_json),
        "unsupported": unsupported,
        "example_command": example_command(api, operation),
    })
}

fn response_json(response: &Response) -> Value {
    let Response {
        status,
        content_type,
        other_content_types: _,
        schema,
        description,
    } = response;
    json!({"status": status, "content_type": content_type, "description": description, "shape": schema.shape()})
}

fn request_body_json(body: &RequestBody) -> Value {
    let RequestBody {
        content_type,
        other_content_types: _,
        required,
        schema,
        description,
    } = body;
    json!({
        "content_type": content_type,
        "required": required,
        "description": description,
        "skeleton": schema.skeleton(),
        "optional": schema.optional_properties(),
    })
}

fn parameter_json(parameter: &Parameter) -> Value {
    let Parameter {
        name,
        location,
        required,
        schema,
        description,
    } = parameter;
    json!({
        "name": name,
        "in": location.as_str(),
        "required": required,
        "type": if schema.type_name.is_empty() { "any" } else { schema.type_name.as_str() },
        "enum": schema.enum_values,
        "default": schema.default,
        "description": description,
    })
}

/// `--describe`: the operation, its parameters, the body skeleton, the
/// scopes and the example command.
pub fn render_operation_description(api: &str, operation: &Operation) -> String {
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
    let mut out = format!("{id}\n  {method} {path}\n");
    if let Some(summary) = summary {
        out.push_str(&format!("  {summary}\n"));
    }
    if *deprecated {
        out.push_str("  deprecated\n");
    }
    if let Some(description) = description.as_deref().map(first_paragraph)
        && Some(description.as_str()) != summary.as_deref()
    {
        out.push_str(&format!("  {description}\n"));
    }
    if !tags.is_empty() {
        out.push_str(&format!("  tags: {}\n", tags.join(", ")));
    }
    if let Some(url) = external_docs {
        out.push_str(&format!("  docs: {url}\n"));
    }

    out.push_str("\nParameters\n");
    if parameters.is_empty() {
        out.push_str("  none\n");
    } else {
        let rows: Vec<[String; 5]> = parameters
            .iter()
            .map(|parameter| {
                let Parameter {
                    name,
                    location,
                    required,
                    schema,
                    description,
                } = parameter;
                [
                    name.clone(),
                    location.as_str().to_string(),
                    schema.display_type(),
                    if *required { "yes" } else { "no" }.to_string(),
                    parameter_notes(schema, description.as_deref()),
                ]
            })
            .collect();
        push_indented(
            &mut out,
            &render_table(&["NAME", "IN", "TYPE", "REQUIRED", "DESCRIPTION"], &rows),
        );
    }

    if let Some(body) = request_body {
        let RequestBody {
            content_type,
            other_content_types: _,
            required,
            schema,
            description,
        } = body;
        out.push_str(&format!(
            "\nRequest body  {}{}\n",
            content_type,
            if *required { " (required)" } else { "" }
        ));
        if let Some(description) = description.as_deref().map(first_paragraph) {
            out.push_str(&format!("  {description}\n"));
        }
        push_indented(
            &mut out,
            &serde_json::to_string_pretty(&schema.skeleton()).unwrap_or_default(),
        );
        let optional = schema.optional_properties();
        if !optional.is_empty() {
            out.push_str(&format!("  optional: {}\n", optional.join(", ")));
        }
    }

    if let Some(response) = response {
        let Response {
            status,
            content_type,
            other_content_types: _,
            schema,
            description,
        } = response;
        out.push_str(&format!("\nResponse  {status}  {content_type}\n"));
        if let Some(description) = description.as_deref().map(first_paragraph) {
            out.push_str(&format!("  {description}\n"));
        }
        push_indented(
            &mut out,
            &serde_json::to_string_pretty(&schema.shape()).unwrap_or_default(),
        );
    }

    if !scopes.is_empty() {
        out.push_str(&format!("\nScopes\n  {}\n", scopes.join(", ")));
    }
    if !unsupported.is_empty() {
        out.push_str(&format!(
            "\nUnsupported\n  {} (kurama does not send it; use -d with a plain path)\n",
            unsupported.join(", ")
        ));
    }
    out.push_str(&format!(
        "\nExample\n  {}\n",
        example_command(api, operation)
    ));
    out
}

/// Every line of `text` under a section heading, indented by two spaces.
fn push_indented(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
}

/// The parameter's description with its default, on one line.
fn parameter_notes(schema: &Schema, description: Option<&str>) -> String {
    let mut notes = Vec::new();
    if let Some(default) = &schema.default {
        notes.push(format!("default {}", scalar_text(default)));
    }
    if let Some(description) = description {
        notes.push(first_line(description));
    }
    notes.join("; ")
}

/// Cells padded to the widest cell of each column; the last column is not
/// padded, so lines carry no trailing spaces.
pub fn render_table<const N: usize>(header: &[&str; N], rows: &[[String; N]]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|cell| cell.width()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.width());
        }
    }
    let line = |cells: &[&str]| -> String {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            line.push_str(cell);
            if index + 1 < cells.len() {
                line.push_str(&" ".repeat(widths[index] - cell.width()));
                line.push_str(GAP);
            }
        }
        line.trim_end().to_string()
    };
    let mut out = line(header);
    out.push('\n');
    for row in rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        out.push_str(&line(&cells));
        out.push('\n');
    }
    out
}

/// The first line of a text.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_string()
}

/// The first paragraph of a markdown text, on one line.
pub fn first_paragraph(text: &str) -> String {
    text.split("\n\n")
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::api_spec::{ParameterLocation, Property, RequestBody, Schema};

    #[test]
    fn a_table_pads_every_column_but_the_last() {
        let rows = [
            [
                "github".to_owned(),
                "token".to_owned(),
                "https://a".to_owned(),
            ],
            [
                "google-sheets".to_owned(),
                "oauth".to_owned(),
                "".to_owned(),
            ],
        ];
        assert_eq!(
            render_table(&["ID", "AUTH", "SETUP"], &rows),
            "ID             AUTH   SETUP
github         token  https://a
google-sheets  oauth
"
        );
    }

    fn list() -> Operation {
        Operation {
            id: "issues/list-for-repo".into(),
            has_operation_id: true,
            method: "GET".into(),
            path: "/repos/{owner}/{repo}/issues".into(),
            summary: Some("List repository issues".into()),
            description: Some(
                "List issues in a repository.\nOnly open issues by default.\n\n**Note**: more."
                    .into(),
            ),
            tags: vec!["issues".into()],
            scopes: vec!["repo".into()],
            parameters: vec![
                Parameter {
                    name: "owner".into(),
                    location: ParameterLocation::Path,
                    required: true,
                    schema: Schema {
                        type_name: "string".into(),
                        ..Schema::default()
                    },
                    description: Some("The account owner.\nMore.".into()),
                },
                Parameter {
                    name: "state".into(),
                    location: ParameterLocation::Query,
                    required: false,
                    schema: Schema {
                        type_name: "string".into(),
                        enum_values: vec![json!("open"), json!("closed")],
                        default: Some(json!("open")),
                        ..Schema::default()
                    },
                    description: None,
                },
            ],
            response: None,
            request_body: None,
            deprecated: false,
            external_docs: Some("https://docs.github.com/issues".into()),
            unsupported: vec![],
            graphql: None,
        }
    }

    fn create() -> Operation {
        Operation {
            id: "issues/create".into(),
            method: "POST".into(),
            summary: Some("Create an issue".into()),
            description: None,
            tags: vec![],
            parameters: vec![list().parameters[0].clone()],
            response: None,
            request_body: Some(RequestBody {
                content_type: "application/json".into(),
                other_content_types: vec![],
                required: true,
                schema: Schema {
                    type_name: "object".into(),
                    properties: vec![
                        Property {
                            name: "title".into(),
                            required: true,
                            schema: Schema {
                                type_name: "string".into(),
                                ..Schema::default()
                            },
                        },
                        Property {
                            name: "body".into(),
                            required: false,
                            schema: Schema {
                                type_name: "string".into(),
                                ..Schema::default()
                            },
                        },
                    ],
                    ..Schema::default()
                },
                description: Some("The issue".into()),
            }),
            external_docs: None,
            unsupported: vec!["formData parameter 'file'".into()],
            graphql: None,
            ..list()
        }
    }

    #[test]
    fn the_operation_table_aligns_columns_without_trailing_spaces() {
        let list = list();
        let create = create();
        let table = render_operations_table(&[&list, &create]);
        assert_eq!(
            table,
            "ID                    METHOD  PATH                          SUMMARY\n\
             issues/list-for-repo  GET     /repos/{owner}/{repo}/issues  List repository issues\n\
             issues/create         POST    /repos/{owner}/{repo}/issues  Create an issue\n"
        );
        assert_eq!(render_operations_table(&[]), "ID  METHOD  PATH  SUMMARY\n");
    }

    #[test]
    fn operations_json_carries_id_method_path_summary_tags_and_scopes() {
        let list = list();
        let value = operations_json(&[&list]);
        assert_eq!(
            value,
            json!([{
                "id": "issues/list-for-repo",
                "method": "GET",
                "path": "/repos/{owner}/{repo}/issues",
                "summary": "List repository issues",
                "tags": ["issues"],
                "scopes": ["repo"],
                "deprecated": false,
            }])
        );
    }

    #[test]
    fn the_description_lists_parameters_body_scopes_and_the_example() {
        let text = render_operation_description("github", &list());
        assert_eq!(
            text,
            "issues/list-for-repo\n\
             \x20 GET /repos/{owner}/{repo}/issues\n\
             \x20 List repository issues\n\
             \x20 List issues in a repository. Only open issues by default.\n\
             \x20 tags: issues\n\
             \x20 docs: https://docs.github.com/issues\n\
             \n\
             Parameters\n\
             \x20 NAME   IN     TYPE         REQUIRED  DESCRIPTION\n\
             \x20 owner  path   string       yes       The account owner.\n\
             \x20 state  query  open|closed  no        default open\n\
             \n\
             Scopes\n\
             \x20 repo\n\
             \n\
             Example\n\
             \x20 kurama api github issues/list-for-repo -P 'owner=<owner>'\n"
        );
        let text = render_operation_description("github", &create());
        assert!(text.contains("\nRequest body  application/json (required)\n  The issue\n  {\n    \"title\": \"\"\n  }\n  optional: body\n"), "{text}");
        assert!(
            text.contains("\nUnsupported\n  formData parameter 'file' (kurama does not send it"),
            "{text}"
        );
        assert!(!text.contains("tags:"));
    }

    #[test]
    fn describe_json_has_the_skeleton_and_the_example_command() {
        let value = operation_json("github", &create());
        assert_eq!(value["request_body"]["skeleton"], json!({"title": ""}));
        assert_eq!(value["request_body"]["optional"], json!(["body"]));
        assert_eq!(value["request_body"]["content_type"], "application/json");
        assert_eq!(value["parameters"][0]["in"], "path");
        assert_eq!(value["parameters"][0]["type"], "string");
        assert_eq!(value["unsupported"], json!(["formData parameter 'file'"]));
        assert_eq!(
            value["example_command"],
            "kurama api github issues/create -P 'owner=<owner>' -d '{\"title\":\"\"}'"
        );
        let value = operation_json("github", &list());
        assert_eq!(value["request_body"], Value::Null);
        assert_eq!(
            value["parameters"][1]["type"], "string",
            "the enum has its own key"
        );
        assert_eq!(value["parameters"][1]["enum"], json!(["open", "closed"]));
        assert_eq!(value["parameters"][1]["default"], json!("open"));
    }

    #[test]
    fn the_first_paragraph_is_one_line() {
        assert_eq!(
            first_paragraph("Line one\nline two\n\nSecond paragraph"),
            "Line one line two"
        );
        assert_eq!(first_paragraph(""), "");
    }
}
