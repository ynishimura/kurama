//! A `[auth.<name>]` credential source, whichever kind it is: an OAuth 2.0
//! client that gets a token from a grant, a credential issued elsewhere
//! that is read from a secret store, or a set of secrets for the
//! environment of a command.

use serde::{Deserialize, Serialize};

use super::{OAuthClientConfig, OAuthToken, Secret, SecretsSourceConfig, TokenSourceConfig};

/// What a `[auth.<name>]` section's `kind` says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// The default: a token from an OAuth 2.0 grant, kept in the token store.
    #[default]
    OAuth,
    /// A credential issued elsewhere, read from a secret store each time.
    Token,
    /// Several secrets, each read into a variable of its own for `env` /
    /// `exec`; no request ever carries them.
    Secrets,
}

impl AuthKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::Token => "token",
            Self::Secrets => "secrets",
        }
    }
}

/// One validated `[auth.<name>]` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    OAuth(OAuthClientConfig),
    Token(TokenSourceConfig),
    Secrets(SecretsSourceConfig),
}

impl AuthSource {
    pub fn name(&self) -> &str {
        match self {
            Self::OAuth(client) => &client.name,
            Self::Token(source) => &source.name,
            Self::Secrets(source) => &source.name,
        }
    }

    /// The variables `kurama env` / `kurama exec` put the credential in:
    /// one for a token, one per secret for a `secrets` source.
    pub fn env_vars(&self) -> Vec<&str> {
        match self {
            Self::OAuth(client) => vec![&client.env_var],
            Self::Token(source) => vec![&source.env_var],
            Self::Secrets(source) => source.variables().collect(),
        }
    }

    pub fn kind(&self) -> AuthKind {
        match self {
            Self::OAuth(_) => AuthKind::OAuth,
            Self::Token(_) => AuthKind::Token,
            Self::Secrets(_) => AuthKind::Secrets,
        }
    }
}

/// A source whose one credential a request, `kurama token` and the token
/// variables carry: every kind but `secrets`, which holds several values
/// for the environment of a command and none for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestAuth {
    OAuth(OAuthClientConfig),
    Token(TokenSourceConfig),
}

impl RequestAuth {
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
}

impl AuthSource {
    /// The source as the one credential it holds, or the `secrets` source,
    /// which holds none.
    pub fn request_auth(self) -> Result<RequestAuth, SecretsSourceConfig> {
        match self {
            Self::OAuth(client) => Ok(RequestAuth::OAuth(client)),
            Self::Token(source) => Ok(RequestAuth::Token(source)),
            Self::Secrets(source) => Err(source),
        }
    }
}

/// What a source hands a command: a token a grant issued, or a credential
/// that was issued elsewhere and carries nothing but its value. `Debug` is
/// written out, so a variant added later has to say how it prints.
#[derive(Clone)]
pub enum SourceCredential {
    OAuth(OAuthToken),
    /// The resolved value of a `kind = "token"` source's reference.
    Issued(Secret),
}

impl SourceCredential {
    /// The credential itself, as `kurama token` prints it and `kurama env`
    /// exports it.
    pub fn expose(&self) -> &str {
        match self {
            Self::OAuth(token) => &token.access_token,
            Self::Issued(value) => value.expose(),
        }
    }
}

impl std::fmt::Debug for SourceCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OAuth(token) => f.debug_tuple("OAuth").field(token).finish(),
            Self::Issued(value) => f.debug_tuple("Issued").field(value).finish(),
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

    fn secrets() -> AuthSource {
        AuthSource::Secrets(SecretsSourceConfig {
            name: "site".into(),
            env: [
                ("SITE_USER", "op://Agent/site/username"),
                ("SITE_PASS", "op://Agent/site/password"),
            ]
            .into_iter()
            .map(|(var, reference)| (var.into(), SecretRef::parse(reference).unwrap()))
            .collect(),
        })
    }

    #[test]
    fn every_kind_answers_its_name_kind_and_variables() {
        assert_eq!(oauth().name(), "github");
        assert_eq!(oauth().env_vars(), ["GITHUB_TOKEN"]);
        assert_eq!(oauth().kind(), AuthKind::OAuth);
        assert_eq!(oauth().kind().as_str(), "oauth");
        assert_eq!(issued().name(), "example");
        assert_eq!(issued().env_vars(), ["KURAMA_TOKEN"]);
        assert_eq!(issued().kind(), AuthKind::Token);
        assert_eq!(issued().kind().as_str(), "token");
        assert_eq!(secrets().name(), "site");
        assert_eq!(secrets().env_vars(), ["SITE_PASS", "SITE_USER"]);
        assert_eq!(secrets().kind(), AuthKind::Secrets);
        assert_eq!(secrets().kind().as_str(), "secrets");
        assert_eq!(AuthKind::default(), AuthKind::OAuth);
    }

    #[test]
    fn only_a_secrets_source_has_no_request_credential() {
        let oauth = oauth().request_auth().unwrap();
        assert_eq!((oauth.name(), oauth.env_var()), ("github", "GITHUB_TOKEN"));
        let issued = issued().request_auth().unwrap();
        assert_eq!(
            (issued.name(), issued.env_var()),
            ("example", "KURAMA_TOKEN")
        );
        assert_eq!(secrets().request_auth().unwrap_err().name, "site");
    }

    #[test]
    fn a_credential_is_the_token_or_the_issued_value() {
        assert_eq!(
            SourceCredential::OAuth(OAuthToken::bearer("at")).expose(),
            "at"
        );
        assert_eq!(SourceCredential::Issued("key".into()).expose(), "key");
    }

    #[test]
    fn debug_prints_neither_kind_of_credential() {
        for credential in [
            SourceCredential::OAuth(OAuthToken::bearer("oauth-s3cret")),
            SourceCredential::Issued("issued-s3cret".into()),
        ] {
            let printed = format!("{credential:?}");
            assert!(!printed.contains("s3cret"), "{printed}");
            assert!(printed.contains("REDACTED"), "{printed}");
        }
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
