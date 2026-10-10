//! Pure, ordered completion values and descriptions from profiles and API metadata.

use crate::domain::types::api_spec::{ApiSpec, Operation, Parameter, scalar_text};
use crate::domain::types::{AuthSource, EndpointSource, Profile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub help: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileScope {
    All,
    Aws,
    Auth,
}

/// Preserve actual names; omit names that cannot be represented in a line protocol.
pub fn named_candidates(entries: impl IntoIterator<Item = (String, String)>) -> Vec<Candidate> {
    let mut candidates: Vec<_> = entries
        .into_iter()
        .filter(|(value, _)| !value.is_empty() && !value.chars().any(char::is_control))
        .map(|(value, help)| Candidate {
            value,
            help: help
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .filter(|c| !c.is_control())
                .collect(),
        })
        .collect();
    candidates.sort_by(|left, right| left.value.cmp(&right.value));
    candidates.dedup_by(|left, right| left.value == right.value);
    candidates
}

pub fn profile_candidates(
    aws: &[Profile],
    auth: &[AuthSource],
    scope: ProfileScope,
) -> Vec<Candidate> {
    // The same shared namespace that source command resolution requires.
    if auth
        .iter()
        .any(|source| aws.iter().any(|profile| profile.name() == source.name()))
    {
        return Vec::new();
    }
    let mut entries = Vec::new();
    if scope != ProfileScope::Auth {
        entries.extend(aws.iter().map(|profile| {
            let mut help = [profile.account_id(), profile.region_raw()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            if help.is_empty() {
                help = profile.role_arn_raw().unwrap_or("AWS profile").to_string();
            }
            (profile.name().to_string(), help)
        }));
    }
    if scope != ProfileScope::Aws {
        entries.extend(auth.iter().map(|source| {
            // The help says what the source is, never where its credential
            // is kept: completion prints what it reads, and a reference
            // names a vault, a profile and an item.
            let help = match source {
                AuthSource::Token(issued) => format!("token {}", issued.placement.summary()),
                AuthSource::Secrets(secrets) => {
                    format!(
                        "secrets {}",
                        secrets.variables().collect::<Vec<_>>().join(",")
                    )
                }
                AuthSource::OAuth(client) => {
                    let endpoint = match &client.endpoints {
                        EndpointSource::Explicit(endpoints) => &endpoints.token_url,
                        EndpointSource::Issuer(issuer) => issuer,
                    };
                    let host = url::Url::parse(endpoint)
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_string))
                        .unwrap_or_default();
                    match client.endpoints {
                        EndpointSource::Issuer(_) => {
                            format!("{} issuer {host}", client.grant_type.as_str())
                        }
                        EndpointSource::Explicit(_) => {
                            format!("{} {host}", client.grant_type.as_str())
                        }
                    }
                }
            };
            (source.name().to_string(), help)
        }));
    }
    named_candidates(entries)
}

/// The methods `kurama api -X` signs and sends. A description can name any
/// of them, so the list is the standard set rather than the document's.
pub fn http_method_candidates() -> Vec<Candidate> {
    named_candidates([
        ("GET".to_string(), "read".to_string()),
        ("POST".to_string(), "create or act".to_string()),
        ("PUT".to_string(), "replace".to_string()),
        ("PATCH".to_string(), "update".to_string()),
        ("DELETE".to_string(), "remove".to_string()),
        ("HEAD".to_string(), "headers only".to_string()),
        (
            "OPTIONS".to_string(),
            "what the endpoint allows".to_string(),
        ),
    ])
}

pub fn operation_candidates(spec: &ApiSpec) -> Vec<Candidate> {
    named_candidates(spec.operations.iter().map(|operation| {
        let mut help = operation.label();
        if let Some(summary) = &operation.summary {
            help.push_str(": ");
            help.push_str(summary);
        }
        (operation.id.clone(), help)
    }))
}

/// Required parameters come first; declaration order is retained within each group.
pub fn parameter_candidates(operation: &Operation, given: &[String]) -> Vec<Candidate> {
    let mut parameters: Vec<_> = operation
        .parameters
        .iter()
        .filter(|parameter| {
            !given.iter().any(|given| {
                given
                    .split_once('=')
                    .and_then(|(name, _)| operation.parameter(name))
                    == Some(*parameter)
            })
        })
        .collect();
    parameters.sort_by_key(|parameter| !parameter.required);
    parameters
        .into_iter()
        .map(|parameter| {
            let mut help = format!(
                "{} {}",
                parameter.location.as_str(),
                parameter.schema.display_type()
            );
            if let Some(default) = &parameter.schema.default {
                help.push_str(&format!(" (default {})", scalar_text(default)));
            }
            if parameter.required {
                help.push_str(" (required)");
            }
            Candidate {
                value: format!("{}=", parameter.name),
                help,
            }
        })
        .collect()
}

pub fn parameter_value_candidates(parameter: &Parameter) -> Vec<Candidate> {
    let schema = &parameter.schema;
    let mut values: Vec<_> = schema
        .enum_values
        .iter()
        .cloned()
        .map(|value| (value, "enum"))
        .collect();
    if schema.type_name == "boolean" && schema.enum_values.is_empty() {
        values.extend([
            (serde_json::Value::Bool(true), "boolean"),
            (serde_json::Value::Bool(false), "boolean"),
        ]);
    }
    values.extend(
        schema
            .default
            .iter()
            .cloned()
            .map(|value| (value, "default")),
    );
    values.extend(
        schema
            .example
            .iter()
            .cloned()
            .map(|value| (value, "example")),
    );
    let mut candidates: Vec<Candidate> = Vec::new();
    for (value, help) in values {
        let value = scalar_text(&value);
        if !candidates.iter().any(|candidate| candidate.value == value) {
            candidates.push(Candidate {
                value,
                help: help.into(),
            });
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::oauth_client::{
        EndpointSource, GrantType, OAuthClientConfig, OAuthEndpoints,
    };
    use crate::domain::types::{SecretRef, TokenSourceConfig};

    fn auth(name: &str) -> AuthSource {
        AuthSource::OAuth(OAuthClientConfig {
            name: name.into(),
            grant_type: GrantType::ClientCredentials,
            endpoints: EndpointSource::Explicit(OAuthEndpoints {
                auth_url: None,
                token_url: "https://tokens.example.test/oauth".into(),
                device_auth_url: None,
            }),
            client_id: "client".into(),
            client_secret: None,
            scopes: vec![],
            env_var: "API_TOKEN".into(),
            redirect_port: None,
        })
    }

    fn issued(name: &str) -> AuthSource {
        AuthSource::Token(TokenSourceConfig {
            name: name.into(),
            token: SecretRef::parse("op://Agent/Example/credential").unwrap(),
            placement: crate::domain::types::TokenPlacement::Header {
                name: "X-API-Key".into(),
                format: "{token}".into(),
            },
            env_var: "EXAMPLE_TOKEN".into(),
        })
    }
    fn names(candidates: Vec<Candidate>) -> Vec<String> {
        candidates
            .into_iter()
            .map(|candidate| candidate.value)
            .collect()
    }

    #[test]
    fn profiles_cover_aws_only_auth_only_combined_and_empty_sources() {
        let aws = [Profile::new("b-aws")
            .with_role_arn_raw("arn:aws:iam::123456789012:role/Read")
            .with_region_raw("ap-northeast-1")];
        let auth = [auth("a-auth")];
        assert_eq!(
            names(profile_candidates(&aws, &[], ProfileScope::All)),
            ["b-aws"]
        );
        assert_eq!(
            names(profile_candidates(&[], &auth, ProfileScope::All)),
            ["a-auth"]
        );
        assert_eq!(
            names(profile_candidates(&aws, &auth, ProfileScope::All)),
            ["a-auth", "b-aws"]
        );
        assert_eq!(
            names(profile_candidates(&aws, &auth, ProfileScope::Aws)),
            ["b-aws"]
        );
        assert_eq!(
            names(profile_candidates(&aws, &auth, ProfileScope::Auth)),
            ["a-auth"]
        );
        assert!(profile_candidates(&[], &[], ProfileScope::All).is_empty());
        let all = profile_candidates(&aws, &auth, ProfileScope::All);
        assert_eq!(all[0].help, "client_credentials tokens.example.test");
        assert_eq!(all[1].help, "123456789012 ap-northeast-1");
    }

    /// Completion says what a source is without reading anything: a
    /// `kind = "token"` source is named by the header it sends, never by the
    /// reference its credential is kept behind.
    #[test]
    fn a_token_source_is_described_by_its_header() {
        let candidates = profile_candidates(&[], &[issued("example")], ProfileScope::Auth);
        assert_eq!(candidates[0].value, "example");
        assert_eq!(candidates[0].help, "token X-API-Key");
        assert!(!candidates[0].help.contains("op://"));
    }

    #[test]
    fn a_shared_profile_namespace_collision_yields_no_candidates() {
        for scope in [ProfileScope::All, ProfileScope::Aws, ProfileScope::Auth] {
            assert!(profile_candidates(&[Profile::new("same")], &[auth("same")], scope).is_empty());
        }
    }

    #[test]
    fn named_candidates_are_sorted_unique_and_safe_for_one_line_protocols() {
        let candidates = named_candidates([
            ("zeta".into(), "Last\nsecond paragraph".into()),
            ("alpha".into(), "API: a\\b\u{1b}[31m".into()),
            ("alpha".into(), "duplicate".into()),
            ("bad\nname".into(), "not a single value".into()),
        ]);
        assert_eq!(
            candidates,
            [
                Candidate {
                    value: "alpha".into(),
                    help: "API: a\\b[31m".into()
                },
                Candidate {
                    value: "zeta".into(),
                    help: "Last".into()
                }
            ]
        );
    }

    #[test]
    fn operation_candidates_carry_the_method_path_and_summary() {
        let document = serde_json::from_str(include_str!(
            "../../../tests/fixtures/openapi/petstore.json"
        ))
        .unwrap();
        let spec = crate::domain::functions::openapi::normalize_spec(&document).unwrap();
        let candidates = operation_candidates(&spec);
        let list = candidates
            .iter()
            .find(|candidate| candidate.value == "pets/list")
            .unwrap();
        assert!(list.help.starts_with("GET /pets"));
        assert!(list.help.contains("List pets"));
        assert_eq!(candidates.len(), spec.operations.len());
        assert!(
            candidates
                .windows(2)
                .all(|pair| pair[0].value < pair[1].value)
        );
    }
}

#[cfg(test)]
mod http_method_tests {
    use super::*;

    #[test]
    fn http_methods_are_the_standard_set_with_distinct_descriptions() {
        let candidates = http_method_candidates();
        let values: Vec<&str> = candidates.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(
            values,
            ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"],
            "named_candidates sorts, and every standard method is offered"
        );
        let mut helps: Vec<&str> = candidates.iter().map(|c| c.help.as_str()).collect();
        helps.sort_unstable();
        helps.dedup();
        assert_eq!(
            helps.len(),
            candidates.len(),
            "each method says something different about itself"
        );
    }
}

#[cfg(test)]
mod parameter_tests {
    use super::*;
    use crate::domain::types::api_spec::{Operation, Parameter, ParameterLocation, Schema};
    use serde_json::json;

    fn operation() -> Operation {
        let spec = crate::domain::functions::openapi::normalize_spec(&json!({
            "openapi": "3.0.3", "info": {"title": "Parameters", "version": "1"},
            "paths": {"/items/{id}": {"get": {"operationId": "items/get", "parameters": [
                {"name": "zeta", "in": "query", "schema": {"type": "integer", "default": 20}},
                {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}},
                {"name": "token", "in": "header", "required": true, "schema": {"type": "string"}},
                {"name": "alpha", "in": "query", "schema": {"type": "boolean"}}
            ], "responses": {"200": {"description": "ok"}}}}}
        }))
        .unwrap();
        spec.operations.into_iter().next().unwrap()
    }

    #[test]
    fn parameter_names_prioritize_required_then_preserve_definition_order() {
        assert_eq!(
            parameter_candidates(&operation(), &[]),
            [
                Candidate {
                    value: "id=".into(),
                    help: "path string (required)".into()
                },
                Candidate {
                    value: "token=".into(),
                    help: "header string (required)".into()
                },
                Candidate {
                    value: "zeta=".into(),
                    help: "query integer (default 20)".into()
                },
                Candidate {
                    value: "alpha=".into(),
                    help: "query boolean".into()
                },
            ]
        );
    }

    #[test]
    fn parameter_names_omit_given_names_and_allow_an_empty_operation() {
        let candidates = parameter_candidates(&operation(), &["id=123".into(), "zeta=".into()]);
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["token=", "alpha="]
        );
        let mut op = operation();
        op.parameters.clear();
        assert!(parameter_candidates(&op, &[]).is_empty());
    }

    #[test]
    fn parameter_names_match_headers_without_changing_query_case_rules() {
        let op = operation();
        let candidates = parameter_candidates(&op, &["TOKEN=secret".into(), "ZETA=1".into()]);
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["id=", "zeta=", "alpha="]
        );
        assert_eq!(op.parameter("TOKEN"), op.parameter("token"));
        assert!(op.parameter("ZETA").is_none());

        let mut op = op;
        let mut header = op.parameter("token").unwrap().clone();
        header.name = "Zeta".into();
        header.required = false;
        op.parameters.push(header);
        let candidates = parameter_candidates(&op, &["zeta=1".into()]);
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["id=", "token=", "alpha=", "Zeta="]
        );
    }

    #[test]
    fn parameter_values_combine_enum_default_example_without_duplicates() {
        let parameter = Parameter {
            name: "state".into(),
            location: ParameterLocation::Query,
            required: false,
            description: None,
            schema: Schema {
                type_name: "string".into(),
                enum_values: vec![json!("available"), json!("sold")],
                default: Some(json!("available")),
                example: Some(json!("pending")),
                ..Schema::default()
            },
        };
        let candidates = parameter_value_candidates(&parameter);
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["available", "sold", "pending"]
        );
        let mut boolean = parameter;
        boolean.schema = Schema {
            type_name: "boolean".into(),
            default: Some(json!(false)),
            example: Some(json!(true)),
            ..Schema::default()
        };
        assert_eq!(
            parameter_value_candidates(&boolean)
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["true", "false"]
        );
        boolean.schema = Schema {
            type_name: "integer".into(),
            default: Some(json!(0)),
            example: Some(json!(42)),
            ..Schema::default()
        };
        assert_eq!(
            parameter_value_candidates(&boolean)
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            ["0", "42"]
        );
        boolean.schema = Schema::default();
        assert!(parameter_value_candidates(&boolean).is_empty());
    }
}
