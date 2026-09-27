//! Pure presentation helpers for repeating an OpenAPI operation from the CLI.

use crate::domain::types::api_spec::Operation;

/// `kurama api <API> <OP> -P k=v ... -d '<body>' [--jq '<filter>']`: the
/// command line that repeats a call.
pub fn cli_command(
    api: &str,
    operation: &Operation,
    params: &[(String, String)],
    body: Option<&str>,
    jq: Option<&str>,
) -> String {
    let mut words = vec![
        "kurama".to_string(),
        "api".to_string(),
        shell_quote(api),
        shell_quote(&operation.id),
    ];
    for (name, value) in params {
        words.push("-P".to_string());
        words.push(shell_quote(&format!("{name}={value}")));
    }
    if let Some(body) = body {
        words.push("-d".to_string());
        words.push(shell_quote(body));
    }
    if let Some(filter) = jq {
        words.push("--jq".to_string());
        words.push(shell_quote(filter));
    }
    words.join(" ")
}

/// The command `--describe` prints: every required parameter as
/// `-P name=<name>`, and the body when the operation takes one: the JSON
/// skeleton, or `@<file>` for another media type.
pub fn example_command(api: &str, operation: &Operation) -> String {
    let params: Vec<(String, String)> = operation
        .parameters
        .iter()
        .filter(|p| p.required)
        .map(|p| (p.name.clone(), format!("<{}>", p.name)))
        .collect();
    let body = operation.request_body.as_ref().map(|body| {
        if is_json(&body.content_type) {
            body.schema.skeleton().to_string()
        } else {
            "@<file>".to_string()
        }
    });
    cli_command(api, operation, &params, body.as_deref(), None)
}

/// Whether a media type carries JSON (`application/json`,
/// `application/vnd.github+json`, ...).
pub fn is_json(content_type: &str) -> bool {
    content_type.contains("json")
}

/// `text` as one shell word: as is when it is safe, otherwise in single
/// quotes.
pub fn shell_quote(text: &str) -> String {
    let safe = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c));
    if safe {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::api_spec::{
        Parameter, ParameterLocation, Property, RequestBody, Schema,
    };

    fn schema(type_name: &str) -> Schema {
        Schema {
            type_name: type_name.into(),
            ..Schema::default()
        }
    }

    fn path_parameter(name: &str) -> Parameter {
        Parameter {
            name: name.into(),
            location: ParameterLocation::Path,
            required: true,
            schema: schema("string"),
            description: None,
        }
    }

    /// `issues/create`: two required path parameters and a JSON body with a
    /// required `title`.
    fn create_issue() -> Operation {
        Operation {
            id: "issues/create".into(),
            has_operation_id: true,
            method: "POST".into(),
            path: "/repos/{owner}/{repo}/issues".into(),
            summary: None,
            description: None,
            tags: vec![],
            scopes: vec![],
            parameters: vec![path_parameter("owner"), path_parameter("repo")],
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
    fn commands_are_shell_safe_and_the_example_names_required_inputs() {
        assert_eq!(
            example_command("github", &create_issue()),
            "kurama api github issues/create -P 'owner=<owner>' -P 'repo=<repo>' -d '{\"title\":\"\"}'"
        );
        assert_eq!(
            cli_command(
                "gh",
                &create_issue(),
                &params(&[("owner", "o"), ("state", "open")]),
                Some("{\"title\":\"it's\"}"),
                Some(".number")
            ),
            "kurama api gh issues/create -P owner=o -P state=open -d '{\"title\":\"it'\\''s\"}' --jq .number"
        );
        let mut no_id = create_issue();
        no_id.id = "GET /repos/{owner}".into();
        assert!(cli_command("gh", &no_id, &[], None, None).ends_with("'GET /repos/{owner}'"));
        assert_eq!(shell_quote(""), "''");

        let mut markdown = create_issue();
        markdown.request_body.as_mut().unwrap().content_type = "text/plain".into();
        assert_eq!(
            example_command("github", &markdown),
            "kurama api github issues/create -P 'owner=<owner>' -P 'repo=<repo>' -d '@<file>'",
            "no JSON skeleton for a text body"
        );
        assert!(is_json("application/vnd.github+json"));
        assert!(!is_json("application/octet-stream"));
    }
}
