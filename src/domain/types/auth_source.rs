//! A `[auth.<name>]` credential source, whichever kind it is: an OAuth 2.0
//! client that gets a token from a grant, or a credential issued elsewhere
//! that is read from a secret store.

use serde::{Deserialize, Serialize};

use super::{OAuthClientConfig, OAuthToken, TokenSourceConfig};

/// What a `[auth.<name>]` section's `kind` says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// The default: a token from an OAuth 2.0 grant, kept in the token store.
    #[default]
    OAuth,
    /// A credential issued elsewhere, read from a secret store each time.
    Token,
}

impl AuthKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::Token => "token",
        }
    }
}

/// One validated `[auth.<name>]` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    OAuth(OAuthClientConfig),
    Token(TokenSourceConfig),
}

impl AuthSource {
    pub fn name(&self) -> &str {
        match self {
            Self::OAuth(client) => &client.name,
            Self::Token(source) => &source.name,
        }
    }

    /// The variable `kurama env` / `kurama exec` put the credential in.
    pub fn env_var(&self) -> &str {
        match self {
            Self::OAuth(client) => &client.env_var,
            Self::Token(source) => &source.env_var,
        }
    }

    pub fn kind(&self) -> AuthKind {
        match self {
            Self::OAuth(_) => AuthKind::OAuth,
            Self::Token(_) => AuthKind::Token,
        }
    }
}

/// What a source hands a command: a token a grant issued, or a credential
/// that was issued elsewhere and carries nothing but its value.
#[derive(Debug, Clone)]
pub enum SourceCredential {
    OAuth(OAuthToken),
    /// The resolved value of a `kind = "token"` source's reference.
    Issued(String),
}

impl SourceCredential {
    /// The credential itself, as `kurama token` prints it and `kurama env`
    /// exports it.
    pub fn value(&self) -> &str {
        match self {
            Self::OAuth(token) => &token.access_token,
            Self::Issued(value) => value,
        }
    }
}

/// A variable name a shell can export: letters, digits and `_`, not starting
/// with a digit. Both kinds of source name one, so both check it here.
pub fn check_env_var(env_var: &str) -> Result<(), String> {
    if env_var.is_empty()
        || !env_var
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        || env_var.starts_with(|c: char| c.is_ascii_digit())
    {
        return Err(format!(
            "env_var must be a shell variable name, got \"{env_var}\""
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{
        DEFAULT_TOKEN_ENV_VAR, EndpointSource, GrantType, OAuthEndpoints, SecretRef,
    };

    fn oauth() -> AuthSource {
        AuthSource::OAuth(OAuthClientConfig {
            name: "github".into(),
            grant_type: GrantType::ClientCredentials,
            endpoints: EndpointSource::Explicit(OAuthEndpoints {
                auth_url: None,
                token_url: "https://as/token".into(),
                device_auth_url: None,
            }),
            client_id: "id".into(),
            client_secret: None,
            scopes: vec![],
            env_var: "GITHUB_TOKEN".into(),
            redirect_port: None,
        })
    }

    fn issued() -> AuthSource {
        AuthSource::Token(TokenSourceConfig {
            name: "example".into(),
            token: SecretRef::parse("op://Agent/Example/credential").unwrap(),
            placement: crate::domain::types::TokenPlacement::Header {
                name: "X-API-Key".into(),
                format: "{token}".into(),
            },
            env_var: DEFAULT_TOKEN_ENV_VAR.into(),
        })
    }

    #[test]
    fn both_kinds_answer_their_name_kind_and_variable() {
        assert_eq!(oauth().name(), "github");
        assert_eq!(oauth().env_var(), "GITHUB_TOKEN");
        assert_eq!(oauth().kind(), AuthKind::OAuth);
        assert_eq!(oauth().kind().as_str(), "oauth");
        assert_eq!(issued().name(), "example");
        assert_eq!(issued().env_var(), "KURAMA_TOKEN");
        assert_eq!(issued().kind(), AuthKind::Token);
        assert_eq!(issued().kind().as_str(), "token");
        assert_eq!(AuthKind::default(), AuthKind::OAuth);
    }

    #[test]
    fn a_credential_is_the_token_or_the_issued_value() {
        assert_eq!(
            SourceCredential::OAuth(OAuthToken::bearer("at")).value(),
            "at"
        );
        assert_eq!(SourceCredential::Issued("key".into()).value(), "key");
    }

    #[rstest::rstest]
    #[case("GITHUB_TOKEN", true)]
    #[case("KURAMA_TOKEN", true)]
    #[case("a1", true)]
    #[case("", false)]
    #[case("1BAD-NAME", false)]
    #[case("HAS SPACE", false)]
    fn a_variable_name_is_what_a_shell_can_export(#[case] name: &str, #[case] valid: bool) {
        assert_eq!(check_env_var(name).is_ok(), valid, "{name}");
    }
}
