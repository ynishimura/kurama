//! `[auth.<name>]`: credential sources that are not AWS profiles, and the kinds that own each of their keys.
//!
//! ```toml
//! ```toml
//! [auth.github]
//! kind = "oauth"
//! grant_type = "authorization_code"     # or device_code, client_credentials
//! auth_url = "https://github.com/login/oauth/authorize"
//! token_url = "https://github.com/login/oauth/access_token"
//! client_id = "Iv1.xxxxxxxx"
//! client_secret = "op://Agent/GitHub OAuth App/client_secret"
//! scopes = ["repo", "read:user"]
//! env_var = "GITHUB_TOKEN"              # kurama env / exec; default KURAMA_TOKEN
//!
//! [auth.google]
//! kind = "oauth"
//! grant_type = "device_code"
//! issuer = "https://accounts.google.com" # endpoints from OpenID Connect discovery
//! client_id = "..."
//!
//! [auth.example]
//! kind = "token"                        # a credential issued elsewhere
//! token = "aws-secrets://dev/example/api-key"  # a reference, never the value
//! header = "X-API-Key"                  # default Authorization
//! format = "{token}"                    # default "Bearer {token}"
//! env_var = "EXAMPLE_TOKEN"             # kurama env / exec; default KURAMA_TOKEN
//!
//! [auth.example-login]
//! kind = "secrets"                      # values for the environment of `exec`
//! [auth.example-login.env]              # variable -> reference, never a value
//! SITE_USER = "op://Agent/<item-id>/username"
//! SITE_PASS = "op://Agent/<item-id>/password"
//! SITE_OTP = "op://Agent/<item-id>/one-time password?attribute=otp"
//! ```
//!
//! `kind` decides which keys a source takes: a key of another kind is a
//! `CONFIG_INVALID` error naming it, because a `token` written under an
//! OAuth source would be a credential nobody sends. Which kinds own a key
//! is one table, `AuthToml::keys`, that every kind's refusal reads.
//! `issuer` and explicit `auth_url` / `token_url` / `device_auth_url` are
//! exclusive.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::types::{
    AuthKind, AuthSource, DEFAULT_TOKEN_ENV_VAR, DEFAULT_TOKEN_FORMAT, DEFAULT_TOKEN_HEADER,
    EndpointSource, GrantType, OAuthClientConfig, OAuthEndpoints, SecretRef, SecretsSourceConfig,
    TokenPlacement, TokenSourceConfig, check_env_var,
};

fn exclusive_with(key: &str, others: &[(&str, bool)]) -> Result<(), String> {
    match others.iter().find(|(_, written)| *written) {
        Some((other, _)) => Err(format!("{key} and {other} are exclusive")),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthToml {
    /// `"oauth"` (the default), `"token"` or `"secrets"`; each kind takes
    /// its own keys.
    #[serde(default)]
    pub kind: AuthKind,
    // `kind = "oauth"`
    #[serde(default)]
    pub grant_type: Option<GrantType>,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub auth_url: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    #[serde(default)]
    pub device_auth_url: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    /// An `op://` reference or a literal; a literal is redacted in `Debug`.
    #[serde(default)]
    pub client_secret: Option<SecretRef>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub redirect_port: Option<u16>,
    // `kind = "token"`
    /// The reference the issued credential is read from; never a literal.
    #[serde(default)]
    pub token: Option<SecretRef>,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    /// HTTP Basic: the username the credential is the password of.
    #[serde(default)]
    pub username: Option<String>,
    /// The query parameter the credential goes in instead of a header.
    #[serde(default)]
    pub query: Option<String>,
    // `kind = "oauth"` and `kind = "token"`
    #[serde(default)]
    pub env_var: Option<String>,
    // `kind = "secrets"`
    /// Variable name -> the reference its value is read from.
    #[serde(default)]
    pub env: Option<BTreeMap<String, SecretRef>>,
}

impl AuthToml {
    /// The validated source; the message names the offending key.
    pub fn typed(&self, name: &str) -> Result<AuthSource, String> {
        match self.kind {
            AuthKind::OAuth => self.oauth(name, self.env_var()?).map(AuthSource::OAuth),
            AuthKind::Token => self.issued(name, self.env_var()?).map(AuthSource::Token),
            AuthKind::Secrets => self.secrets(name).map(AuthSource::Secrets),
        }
    }

    /// The one variable an `oauth` or `token` source exports into.
    fn env_var(&self) -> Result<String, String> {
        let env_var = self
            .env_var
            .clone()
            .unwrap_or_else(|| DEFAULT_TOKEN_ENV_VAR.to_string());
        check_env_var(&env_var)?;
        Ok(env_var)
    }

    fn secrets(&self, name: &str) -> Result<SecretsSourceConfig, String> {
        self.reject_foreign_keys()?;
        let config = SecretsSourceConfig {
            name: name.to_string(),
            env: self.env.clone().ok_or_else(|| {
                "env is required: a table of variable = secret reference".to_string()
            })?,
        };
        config.validate()?;
        Ok(config)
    }

    /// Every key besides `kind`: the kinds that own it and whether this
    /// section writes it, in the order a refusal looks for one. The
    /// destructure names every field, so a new key does not build until it
    /// has a row here.
    fn keys(&self) -> [(&'static str, &'static [AuthKind], bool); 16] {
        const OAUTH: &[AuthKind] = &[AuthKind::OAuth];
        const TOKEN: &[AuthKind] = &[AuthKind::Token];
        const EXPORTED: &[AuthKind] = &[AuthKind::OAuth, AuthKind::Token];
        const SECRETS: &[AuthKind] = &[AuthKind::Secrets];
        let Self {
            kind: _,
            grant_type,
            issuer,
            auth_url,
            token_url,
            device_auth_url,
            client_id,
            client_secret,
            scopes,
            redirect_port,
            token,
            header,
            format,
            username,
            query,
            env_var,
            env,
        } = self;
        [
            ("grant_type", OAUTH, grant_type.is_some()),
            ("issuer", OAUTH, issuer.is_some()),
            ("auth_url", OAUTH, auth_url.is_some()),
            ("token_url", OAUTH, token_url.is_some()),
            ("device_auth_url", OAUTH, device_auth_url.is_some()),
            ("client_id", OAUTH, client_id.is_some()),
            ("client_secret", OAUTH, client_secret.is_some()),
            ("scopes", OAUTH, !scopes.is_empty()),
            ("redirect_port", OAUTH, redirect_port.is_some()),
            ("token", TOKEN, token.is_some()),
            ("header", TOKEN, header.is_some()),
            ("format", TOKEN, format.is_some()),
            ("username", TOKEN, username.is_some()),
            ("query", TOKEN, query.is_some()),
            ("env_var", EXPORTED, env_var.is_some()),
            ("env", SECRETS, env.is_some()),
        ]
    }

    /// The first key written that this section's kind does not own, refused
    /// by name: a `token` written under an OAuth source would otherwise be a
    /// credential nobody sends.
    fn reject_foreign_keys(&self) -> Result<(), String> {
        match self
            .keys()
            .into_iter()
            .find(|(_, owners, written)| *written && !owners.contains(&self.kind))
        {
            Some((key, ..)) => Err(format!(
                "{key} does not apply to kind = \"{}\"",
                self.kind.as_str()
            )),
            None => Ok(()),
        }
    }

    fn issued(&self, name: &str, env_var: String) -> Result<TokenSourceConfig, String> {
        self.reject_foreign_keys()?;
        let config = TokenSourceConfig {
            name: name.to_string(),
            token: self
                .token
                .clone()
                .ok_or_else(|| "token is required".to_string())?,
            placement: self.placement()?,
            env_var,
        };
        config.validate()?;
        Ok(config)
    }

    /// `username` makes it HTTP Basic and `query` a query parameter; each
    /// excludes the header keys and the other, because the credential would
    /// otherwise go to two places or to one nobody reads.
    fn placement(&self) -> Result<TokenPlacement, String> {
        let header_keys = [
            ("header", self.header.is_some()),
            ("format", self.format.is_some()),
        ];
        match (&self.username, &self.query) {
            (Some(_), Some(_)) => Err("username and query are exclusive: the credential goes \
                 in HTTP Basic or in a query parameter"
                .to_string()),
            (Some(username), None) => {
                exclusive_with("username", &header_keys)?;
                Ok(TokenPlacement::Basic {
                    username: username.clone(),
                })
            }
            (None, Some(param)) => {
                exclusive_with("query", &header_keys)?;
                Ok(TokenPlacement::Query {
                    param: param.clone(),
                })
            }
            (None, None) => Ok(TokenPlacement::Header {
                name: self
                    .header
                    .clone()
                    .unwrap_or_else(|| DEFAULT_TOKEN_HEADER.to_string()),
                format: self
                    .format
                    .clone()
                    .unwrap_or_else(|| DEFAULT_TOKEN_FORMAT.to_string()),
            }),
        }
    }

    fn oauth(&self, name: &str, env_var: String) -> Result<OAuthClientConfig, String> {
        self.reject_foreign_keys()?;
        let grant_type = self
            .grant_type
            .ok_or_else(|| "grant_type is required".to_string())?;
        let explicit = [
            ("auth_url", &self.auth_url),
            ("token_url", &self.token_url),
            ("device_auth_url", &self.device_auth_url),
        ];
        let endpoints = match &self.issuer {
            Some(issuer) => {
                if let Some((key, _)) = explicit.iter().find(|(_, value)| value.is_some()) {
                    return Err(format!(
                        "issuer and {key} are exclusive: keep issuer (discovery) or write the URLs"
                    ));
                }
                EndpointSource::Issuer(issuer.clone())
            }
            None => EndpointSource::Explicit(OAuthEndpoints {
                auth_url: self.auth_url.clone(),
                token_url: self
                    .token_url
                    .clone()
                    .ok_or_else(|| "issuer or token_url is required".to_string())?,
                device_auth_url: self.device_auth_url.clone(),
            }),
        };
        let config = OAuthClientConfig {
            name: name.to_string(),
            grant_type,
            endpoints,
            client_id: self
                .client_id
                .clone()
                .ok_or_else(|| "client_id is required".to_string())?,
            client_secret: self.client_secret.clone(),
            scopes: self.scopes.clone(),
            env_var,
            redirect_port: self.redirect_port,
        };
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn auth(toml: &str) -> AuthToml {
        toml::from_str(toml).unwrap()
    }

    /// The OAuth source a valid `[auth.*]` of the default kind types to.
    fn oauth_of(toml: &str, name: &str) -> OAuthClientConfig {
        match auth(toml).typed(name).unwrap() {
            AuthSource::OAuth(client) => client,
            other => panic!("expected an oauth source, got {other:?}"),
        }
    }

    /// The issued-credential source a valid `kind = "token"` types to.
    fn token_of(toml: &str, name: &str) -> TokenSourceConfig {
        match auth(toml).typed(name).unwrap() {
            AuthSource::Token(source) => source,
            other => panic!("expected a token source, got {other:?}"),
        }
    }

    pub(crate) const GITHUB: &str = r#"
kind = "oauth"
grant_type = "authorization_code"
auth_url = "https://github.com/login/oauth/authorize"
token_url = "https://github.com/login/oauth/access_token"
client_id = "Iv1.abc"
client_secret = "op://Agent/GitHub/client_secret"
scopes = ["repo"]
env_var = "GITHUB_TOKEN"
"#;

    #[test]
    fn kind_defaults_to_oauth() {
        let config = oauth_of(
            "grant_type = \"client_credentials\"\ntoken_url = \"https://x/token\"\nclient_id = \"id\"\n",
            "svc",
        );
        assert_eq!(config.grant_type, GrantType::ClientCredentials);
        let error = toml::from_str::<AuthToml>(
            "kind = \"saml\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://x/token\"\nclient_id = \"id\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unknown variant `saml`"), "{error}");
        assert!(
            error.contains("oauth") && error.contains("token"),
            "{error}"
        );
    }

    /// The whole of a `kind = "token"` source: where the credential is read
    /// from, and how the request presents it.
    #[test]
    fn a_token_source_reads_a_reference_and_names_its_header() {
        let source = token_of(
            "kind = \"token\"\ntoken = \"aws-secrets://dev/example/api-key\"\nheader = \"X-API-Key\"\nformat = \"{token}\"\nenv_var = \"EXAMPLE_TOKEN\"\n",
            "example",
        );
        assert_eq!(source.name, "example");
        assert!(matches!(source.token, SecretRef::Aws(_)));
        assert_eq!(
            source.placement,
            TokenPlacement::Header {
                name: "X-API-Key".into(),
                format: "{token}".into()
            }
        );
        assert_eq!(source.env_var, "EXAMPLE_TOKEN");
    }

    #[test]
    fn a_token_source_defaults_to_a_bearer_authorization_header() {
        let source = token_of(
            "kind = \"token\"\ntoken = \"op://Agent/Example/credential\"\n",
            "example",
        );
        assert_eq!(
            source.placement,
            TokenPlacement::Header {
                name: DEFAULT_TOKEN_HEADER.into(),
                format: DEFAULT_TOKEN_FORMAT.into()
            }
        );
        assert_eq!(source.env_var, DEFAULT_TOKEN_ENV_VAR);
    }

    #[test]
    fn username_makes_it_basic_and_query_a_query_parameter() {
        let basic = token_of(
            "kind = \"token\"\ntoken = \"op://A/jira/credential\"\nusername = \"me@example.com\"\n",
            "jira",
        );
        assert_eq!(
            basic.placement,
            TokenPlacement::Basic {
                username: "me@example.com".into()
            }
        );
        let query = token_of(
            "kind = \"token\"\ntoken = \"op://A/backlog/credential\"\nquery = \"apiKey\"\n",
            "backlog",
        );
        assert_eq!(
            query.placement,
            TokenPlacement::Query {
                param: "apiKey".into()
            }
        );
    }

    /// The credential goes to one place: a header key beside `username` or
    /// `query`, or both of them, is refused by name.
    #[rstest::rstest]
    #[case(
        "username = \"me\"\nquery = \"apiKey\"\n",
        "username and query are exclusive"
    )]
    #[case(
        "username = \"me\"\nheader = \"X-Key\"\n",
        "username and header are exclusive"
    )]
    #[case(
        "query = \"apiKey\"\nformat = \"{token}\"\n",
        "query and format are exclusive"
    )]
    #[case(
        "username = \"me:you\"\n",
        "username must be non-empty and carry no colon"
    )]
    fn a_second_place_for_the_credential_is_refused(#[case] keys: &str, #[case] expected: &str) {
        let message = auth(&format!(
            "kind = \"token\"\ntoken = \"op://A/x/credential\"\n{keys}"
        ))
        .typed("x")
        .unwrap_err();
        assert!(message.contains(expected), "{message}");
    }

    #[test]
    fn username_and_query_do_not_apply_to_an_oauth_source() {
        for key in ["username = \"me\"", "query = \"apiKey\""] {
            let message = auth(&format!("grant_type = \"client_credentials\"\n{key}\n"))
                .typed("x")
                .unwrap_err();
            assert!(
                message.contains("does not apply to kind = \"oauth\""),
                "{message}"
            );
        }
    }

    /// Each kind takes its own keys: a key of the other one is a section
    /// nobody can read as written, not a key that is quietly ignored.
    #[rstest::rstest]
    #[case(
        "kind = \"token\"\ntoken = \"op://Agent/x/y\"\ngrant_type = \"client_credentials\"\n",
        "grant_type does not apply to kind = \"token\""
    )]
    #[case(
        "kind = \"token\"\ntoken = \"op://Agent/x/y\"\nclient_id = \"id\"\n",
        "client_id does not apply to kind = \"token\""
    )]
    #[case(
        "kind = \"token\"\ntoken = \"op://Agent/x/y\"\nscopes = [\"read\"]\n",
        "scopes does not apply to kind = \"token\""
    )]
    #[case(
        "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://x/token\"\nclient_id = \"id\"\ntoken = \"op://Agent/x/y\"\n",
        "token does not apply to kind = \"oauth\""
    )]
    #[case(
        "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://x/token\"\nclient_id = \"id\"\nheader = \"X-API-Key\"\n",
        "header does not apply to kind = \"oauth\""
    )]
    #[case("kind = \"token\"\n", "token is required")]
    #[case(
        "kind = \"token\"\ntoken = \"plain-api-key\"\n",
        "token must be a secret reference"
    )]
    #[case(
        "kind = \"token\"\ntoken = \"op://Agent/x/y\"\nformat = \"{token}{api_key}\"\n",
        "takes no placeholder besides {token}"
    )]
    #[case(
        "kind = \"token\"\ntoken = \"op://Agent/x/y\"\nheader = \"X API Key\"\n",
        "header must be a header name"
    )]
    #[case(
        "kind = \"token\"\ntoken = \"op://Agent/x/y\"\nenv_var = \"1BAD\"\n",
        "env_var must be a shell variable name"
    )]
    fn a_key_of_the_other_kind_or_an_unusable_value_is_refused(
        #[case] toml: &str,
        #[case] expected: &str,
    ) {
        let message = auth(toml).typed("example").unwrap_err();
        assert!(message.contains(expected), "{message}");
    }

    #[test]
    fn explicit_endpoints_and_secret_reference_are_typed() {
        let config = oauth_of(GITHUB, "github");
        assert_eq!(config.name, "github");
        assert_eq!(config.grant_type, GrantType::AuthorizationCode);
        assert_eq!(
            config.endpoints,
            EndpointSource::Explicit(OAuthEndpoints {
                auth_url: Some("https://github.com/login/oauth/authorize".into()),
                token_url: "https://github.com/login/oauth/access_token".into(),
                device_auth_url: None,
            })
        );
        assert_eq!(
            config.client_secret,
            Some(SecretRef::OnePassword(
                "op://Agent/GitHub/client_secret".into()
            ))
        );
        assert_eq!(config.env_var, "GITHUB_TOKEN");
        assert_eq!(config.scopes, ["repo"]);
    }

    #[test]
    fn a_literal_client_secret_is_redacted_in_debug() {
        let section = auth(
            "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://as.example.com/token\"\nclient_id = \"id\"\nclient_secret = \"plain-secret\"\n",
        );
        assert!(!format!("{section:?}").contains("plain-secret"));
        let AuthSource::OAuth(client) = section.typed("svc").unwrap() else {
            panic!("expected an oauth source");
        };
        assert_eq!(
            client.client_secret,
            Some(SecretRef::Literal("plain-secret".into()))
        );
    }

    #[test]
    fn issuer_means_discovery_and_env_var_defaults() {
        let config = oauth_of(
            "kind = \"oauth\"\ngrant_type = \"client_credentials\"\nissuer = \"https://accounts.example.com\"\nclient_id = \"id\"\n",
            "svc",
        );
        assert_eq!(
            config.endpoints,
            EndpointSource::Issuer("https://accounts.example.com".into())
        );
        assert_eq!(config.env_var, "KURAMA_TOKEN");
        assert!(config.client_secret.is_none());
    }

    #[rstest::rstest]
    #[case(
        "kind = \"oauth\"\nissuer = \"https://x\"\nclient_id = \"id\"\n",
        "grant_type is required"
    )]
    #[case(
        "kind = \"oauth\"\ngrant_type = \"client_credentials\"\nissuer = \"https://x\"\n",
        "client_id is required"
    )]
    #[case(
        "kind = \"oauth\"\ngrant_type = \"device_code\"\nissuer = \"https://x\"\ntoken_url = \"https://y\"\nclient_id = \"id\"\n",
        "issuer and token_url are exclusive"
    )]
    #[case(
        "kind = \"oauth\"\ngrant_type = \"client_credentials\"\nclient_id = \"id\"\n",
        "issuer or token_url is required"
    )]
    #[case(
        "kind = \"oauth\"\ngrant_type = \"authorization_code\"\ntoken_url = \"https://y\"\nclient_id = \"id\"\n",
        "authorization_code needs auth_url"
    )]
    #[case(
        "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://y\"\nclient_id = \"id\"\nenv_var = \"1BAD-NAME\"\n",
        "env_var must be a shell variable name"
    )]
    fn invalid_sources_name_the_key(#[case] toml: &str, #[case] expected: &str) {
        let message = auth(toml).typed("x").unwrap_err();
        assert!(message.contains(expected), "{message}");
    }

    /// The grant is read by serde, so a typo is refused with the values it
    /// could have been, before `typed` runs.
    #[test]
    fn an_unknown_grant_type_is_rejected_by_serde() {
        let error = toml::from_str::<AuthToml>(
            "kind = \"oauth\"\ngrant_type = \"implicit\"\nissuer = \"https://x\"\nclient_id = \"id\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unknown variant `implicit`"), "{error}");
        assert!(error.contains("`client_credentials`"), "{error}");
    }

    #[test]
    fn unknown_keys_are_rejected_by_serde() {
        let error = toml::from_str::<AuthToml>(
            "kind = \"oauth\"\ngrant_type = \"device_code\"\nissuer = \"https://x\"\nclient_id = \"id\"\naudience = \"x\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("audience"), "{error}");
    }

    pub(crate) const SITE_LOGIN: &str = "kind = \"secrets\"\n[env]\nSITE_USER = \"op://Agent/site/username\"\nSITE_OTP = \"op://Agent/site/one-time password?attribute=otp\"\n";

    /// The whole of a `kind = "secrets"` source: every variable and the
    /// reference it is read from, in name order.
    #[test]
    fn a_secrets_source_maps_each_variable_to_its_reference() {
        let AuthSource::Secrets(source) = auth(SITE_LOGIN).typed("site").unwrap() else {
            panic!("expected a secrets source");
        };
        assert_eq!(source.name, "site");
        assert_eq!(
            source.env.keys().collect::<Vec<_>>(),
            ["SITE_OTP", "SITE_USER"]
        );
        assert_eq!(
            source.env["SITE_USER"],
            SecretRef::parse("op://Agent/site/username").unwrap()
        );
    }

    /// A key of another kind is refused by name, both ways: `env` under a
    /// token source would be values nobody exports, and `env_var` or
    /// `header` under a secrets source would be a place nothing is put.
    #[rstest::rstest]
    #[case(
        "kind = \"secrets\"\nenv_var = \"X\"\n[env]\nA = \"op://v/i/f\"\n",
        "env_var does not apply to kind = \"secrets\""
    )]
    #[case(
        "kind = \"secrets\"\nheader = \"X\"\n[env]\nA = \"op://v/i/f\"\n",
        "header does not apply to kind = \"secrets\""
    )]
    #[case(
        "kind = \"secrets\"\ntoken = \"op://v/i/f\"\n[env]\nA = \"op://v/i/f\"\n",
        "token does not apply to kind = \"secrets\""
    )]
    #[case(
        "kind = \"secrets\"\nclient_id = \"id\"\n[env]\nA = \"op://v/i/f\"\n",
        "client_id does not apply to kind = \"secrets\""
    )]
    #[case(
        "kind = \"token\"\ntoken = \"op://v/i/f\"\n[env]\nA = \"op://v/i/f\"\n",
        "env does not apply to kind = \"token\""
    )]
    #[case(
        "grant_type = \"client_credentials\"\ntoken_url = \"https://x/token\"\nclient_id = \"id\"\n[env]\nA = \"op://v/i/f\"\n",
        "env does not apply to kind = \"oauth\""
    )]
    #[case("kind = \"secrets\"\n", "env is required")]
    #[case("kind = \"secrets\"\n[env]\n", "at least one variable")]
    #[case(
        "kind = \"secrets\"\n[env]\nA = \"plain\"\n",
        "env.A must be a secret reference"
    )]
    #[case(
        "kind = \"secrets\"\n[env]\nAWS_PROFILE = \"op://v/i/f\"\n",
        "env.AWS_PROFILE: variables starting with AWS_"
    )]
    fn a_secrets_source_takes_its_own_keys_only(#[case] toml: &str, #[case] expected: &str) {
        let message = auth(toml).typed("site").unwrap_err();
        assert!(message.contains(expected), "{toml}: {message}");
    }

    /// Every key, written alone under each kind: refused by name under a
    /// kind that does not own it, and not refused as foreign under one that
    /// does. The owners are written out here, not read from the table, so a
    /// row given the wrong kinds fails.
    #[test]
    fn every_key_is_refused_by_name_under_each_kind_that_does_not_own_it() {
        let keys: [(&str, &str, &[&str]); 16] = [
            ("grant_type", "\"client_credentials\"", &["oauth"]),
            ("issuer", "\"https://x\"", &["oauth"]),
            ("auth_url", "\"https://x/auth\"", &["oauth"]),
            ("token_url", "\"https://x/token\"", &["oauth"]),
            ("device_auth_url", "\"https://x/device\"", &["oauth"]),
            ("client_id", "\"id\"", &["oauth"]),
            ("client_secret", "\"op://v/i/f\"", &["oauth"]),
            ("scopes", "[\"read\"]", &["oauth"]),
            ("redirect_port", "8080", &["oauth"]),
            ("token", "\"op://v/i/f\"", &["token"]),
            ("header", "\"X-Key\"", &["token"]),
            ("format", "\"{token}\"", &["token"]),
            ("username", "\"me\"", &["token"]),
            ("query", "\"apiKey\"", &["token"]),
            ("env_var", "\"X_TOKEN\"", &["oauth", "token"]),
            ("env", "{ A = \"op://v/i/f\" }", &["secrets"]),
        ];
        for (key, value, owners) in keys {
            for kind in ["oauth", "token", "secrets"] {
                let toml = format!("kind = \"{kind}\"\n{key} = {value}\n");
                let refusal = format!("{key} does not apply to kind = \"{kind}\"");
                let result = auth(&toml).typed("x");
                if owners.contains(&kind) {
                    assert!(
                        result
                            .as_ref()
                            .err()
                            .is_none_or(|message| !message.contains("does not apply")),
                        "{toml}: {result:?}"
                    );
                } else {
                    assert_eq!(result.unwrap_err(), refusal, "{toml}");
                }
            }
        }
    }

    /// Two foreign keys: the refusal names the one written first in the
    /// table's order, whatever the order in the file.
    #[test]
    fn the_first_foreign_key_in_table_order_is_the_one_named() {
        let message = auth("kind = \"secrets\"\nenv_var = \"X\"\ngrant_type = \"client_credentials\"\n[env]\nA = \"op://v/i/f\"\n")
            .typed("x")
            .unwrap_err();
        assert_eq!(message, "grant_type does not apply to kind = \"secrets\"");
    }
}
