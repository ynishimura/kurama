//! `[auth.<name>]`: credential sources that are not AWS profiles, and
//! `[api.<name>]`: the APIs `kurama api` calls with them.
//!
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
//! [api.github]
//! description = "GitHub REST API"
//! base_url = "https://api.github.com"
//! auth = "github"                       # default: the [auth.*] with the same name
//! headers = { Accept = "application/vnd.github+json", "X-GitHub-Api-Version" = "2022-11-28" }
//!
//! openapi = "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.json"
//!
//! [api.apigw-dev]
//! base_url = "https://abc123.execute-api.ap-northeast-1.amazonaws.com/prod"
//! aws_profile = "dev"                       # SigV4 with the profile's role credentials
//! # service = "execute-api"             # when the host does not name them
//! # region = "ap-northeast-1"           # (a custom domain); the CLI can override
//! openapi = "~/specs/member-api.yaml"   # a local file (absolute or ~), JSON or YAML
//! # openapi = "https://abc123.execute-api.ap-northeast-1.amazonaws.com/prod/openapi.json"
//! # openapi_auth = true                 # fetch it with the API's own credential: same origin as base_url
//! ```
//!
//! `kind` decides which keys a source takes: a key of the other kind is a
//! `CONFIG_INVALID` error naming it, because a `token` written under an
//! OAuth source would be a credential nobody sends. `issuer` and explicit
//! `auth_url` / `token_url` / `device_auth_url` are
//! exclusive, and so are `auth` and `aws_profile`. An `[auth.*]` name that
//! is also an AWS profile name is a `CONFIG_INVALID` error when a command
//! resolves it. `openapi` names the OpenAPI 3.x / Swagger 2.0 description
//! `kurama api --ops`, `--describe`, operation targets and the explorer
//! use: a URL (cached under `~/.cache/kurama/openapi/`) or a file. With
//! `openapi_auth` the URL must be on the origin of `base_url`, the only
//! host the API's credential is sent to. `headers` go with every request
//! (`-H` replaces one of the same name); a header that carries a
//! credential, `Authorization` or the one the source names, is refused.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::functions::api_request::join_base_path;
use crate::domain::functions::signing_target::SigningHint;
use crate::domain::types::api_spec::{GraphQlEndpoint, SpecFormat};
use crate::domain::types::oauth_client::check_http_url;
use crate::domain::types::{
    ApiHeaders, AuthKind, AuthSource, DEFAULT_TOKEN_ENV_VAR, DEFAULT_TOKEN_FORMAT,
    DEFAULT_TOKEN_HEADER, EndpointSource, GrantType, OAuthClientConfig, OAuthEndpoints, SecretRef,
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
    /// `"oauth"` (the default) or `"token"`; each kind takes its own keys.
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
    // Both kinds
    #[serde(default)]
    pub env_var: Option<String>,
}

impl AuthToml {
    /// The validated source; the message names the offending key.
    pub fn typed(&self, name: &str) -> Result<AuthSource, String> {
        let env_var = self
            .env_var
            .clone()
            .unwrap_or_else(|| DEFAULT_TOKEN_ENV_VAR.to_string());
        check_env_var(&env_var)?;
        match self.kind {
            AuthKind::OAuth => self.oauth(name, env_var).map(AuthSource::OAuth),
            AuthKind::Token => self.issued(name, env_var).map(AuthSource::Token),
        }
    }

    /// The keys of the other kind, refused by name: a `token` written under
    /// an OAuth source would otherwise be a credential nobody sends.
    fn reject_foreign_keys(&self, kind: AuthKind, keys: &[(&str, bool)]) -> Result<(), String> {
        match keys.iter().find(|(_, written)| *written) {
            Some((key, _)) => Err(format!(
                "{key} does not apply to kind = \"{}\"",
                kind.as_str()
            )),
            None => Ok(()),
        }
    }

    fn issued(&self, name: &str, env_var: String) -> Result<TokenSourceConfig, String> {
        self.reject_foreign_keys(
            AuthKind::Token,
            &[
                ("grant_type", self.grant_type.is_some()),
                ("issuer", self.issuer.is_some()),
                ("auth_url", self.auth_url.is_some()),
                ("token_url", self.token_url.is_some()),
                ("device_auth_url", self.device_auth_url.is_some()),
                ("client_id", self.client_id.is_some()),
                ("client_secret", self.client_secret.is_some()),
                ("scopes", !self.scopes.is_empty()),
                ("redirect_port", self.redirect_port.is_some()),
            ],
        )?;
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
        self.reject_foreign_keys(
            AuthKind::OAuth,
            &[
                ("token", self.token.is_some()),
                ("header", self.header.is_some()),
                ("format", self.format.is_some()),
                ("username", self.username.is_some()),
                ("query", self.query.is_some()),
            ],
        )?;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiToml {
    #[serde(default)]
    pub description: Option<String>,
    pub base_url: String,
    /// The `[auth.*]` source; defaults to the one with the API's name, and
    /// to no authentication when there is none.
    #[serde(default)]
    pub auth: Option<String>,
    /// The AWS profile whose role credentials sign the requests (SigV4);
    /// exclusive with `auth`, and the same-name default of `auth` does not
    /// apply.
    #[serde(default)]
    pub aws_profile: Option<String>,
    /// The SigV4 service name and region when the host does not name them
    /// (a custom domain or an unsupported endpoint alias). Common dualstack,
    /// -fips and VPC forms are inferred. `aws_profile` only.
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    /// The OpenAPI / Swagger description: an http(s) URL or a file path
    /// (`~` allowed), JSON or YAML.
    #[serde(default)]
    pub openapi: Option<String>,
    /// A Google Discovery Document (`discovery#restDescription`): an
    /// http(s) URL or a file path, read like `openapi` and exclusive with it.
    #[serde(default)]
    pub discovery: Option<String>,
    /// The GraphQL endpoint's path under `base_url` (`/graphql`): its schema,
    /// by introspection with the API's credential, is the description.
    /// Exclusive with `openapi` and `discovery`.
    #[serde(default)]
    pub graphql: Option<String>,
    /// A saved introspection result (a file) read in place of introspecting
    /// the `graphql` endpoint, for a server that refuses introspection.
    #[serde(default)]
    pub graphql_schema: Option<String>,
    /// Fetch the `openapi` / `discovery` URL with the API's own credential (bearer token
    /// or SigV4); default: fetched without one.
    #[serde(default)]
    pub openapi_auth: bool,
    /// Sent with every request; checked as they are read.
    #[serde(default)]
    pub headers: ApiHeaders,
    /// `[api.<name>.agent]`: the `[agent]` rules replaced for this API.
    #[serde(default)]
    pub agent: Option<super::agent::ApiAgentToml>,
}

/// A validated `[api.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiProfile {
    pub name: String,
    pub description: Option<String>,
    pub base_url: String,
    pub auth: Option<String>,
    pub aws_profile: Option<String>,
    /// `service` / `region` from the profile; `kurama api` puts
    /// `--service` / `--region` over them.
    pub signing: SigningHint,
    /// The description `openapi` or `discovery` names.
    pub spec: Option<ApiDescription>,
    pub openapi_auth: bool,
    pub headers: ApiHeaders,
}

impl ApiProfile {
    /// The key the description is configured under, for messages; `openapi`
    /// when there is none, which is what a person adds first.
    pub fn spec_key(&self) -> &'static str {
        match &self.spec {
            Some(ApiDescription {
                format: SpecFormat::GraphQl(_),
                source: SpecSource::File(_),
            }) => "graphql_schema",
            Some(spec) => spec.format.key(),
            None => SpecFormat::OpenApi.key(),
        }
    }
}

/// An `[api.*]` description: the format its key names, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiDescription {
    pub format: SpecFormat,
    pub source: SpecSource,
}

/// Where an API description comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecSource {
    Url(String),
    /// As written in the configuration; `~` is expanded when it is read.
    File(String),
}

impl SpecSource {
    /// The value of `key` (`openapi`, `discovery`): a URL or a file path.
    pub fn parse(key: &str, text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(format!("{key} must be a URL or a file path"));
        }
        if text.starts_with("http://") || text.starts_with("https://") {
            check_http_url(key, text)?;
            return Ok(Self::Url(text.to_string()));
        }
        if text.contains("://") {
            return Err(format!(
                "{key} must be an http(s) URL or a file path, got {text:?}"
            ));
        }
        // A relative path would depend on where kurama is run.
        if !(text.starts_with('/') || text == "~" || text.starts_with("~/")) {
            return Err(format!(
                "{key} must be an http(s) URL or an absolute file path (~ allowed), got {text:?}"
            ));
        }
        Ok(Self::File(text.to_string()))
    }
}

impl std::fmt::Display for SpecSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => f.write_str(url),
            Self::File(path) => f.write_str(path),
        }
    }
}

impl ApiToml {
    /// The description `openapi`, `discovery` or `graphql` (with
    /// `graphql_schema`) names; at most one of the three.
    fn description_source(&self) -> Result<Option<ApiDescription>, String> {
        let named: Vec<&str> = [
            ("openapi", self.openapi.is_some()),
            ("discovery", self.discovery.is_some()),
            ("graphql", self.graphql.is_some()),
        ]
        .into_iter()
        .filter_map(|(key, set)| set.then_some(key))
        .collect();
        if let [first, second, ..] = named[..] {
            return Err(format!(
                "{first} and {second} are exclusive: keep one of them"
            ));
        }
        if self.graphql_schema.is_some() && self.graphql.is_none() {
            return Err(
                "graphql_schema needs graphql, the endpoint's path under base_url".to_string(),
            );
        }
        if let Some(path) = &self.graphql {
            if !path.starts_with('/') {
                return Err(format!(
                    "graphql must be the endpoint's path under base_url, starting with '/', got {path:?}"
                ));
            }
            let source = match &self.graphql_schema {
                Some(file) => match SpecSource::parse("graphql_schema", file)? {
                    SpecSource::Url(_) => {
                        return Err(format!(
                            "graphql_schema must be a file holding an introspection result, got {file:?}"
                        ));
                    }
                    file => file,
                },
                None => SpecSource::Url(join_base_path(&self.base_url, path)),
            };
            return Ok(Some(ApiDescription {
                format: SpecFormat::GraphQl(GraphQlEndpoint { path: path.clone() }),
                source,
            }));
        }
        let (format, text) = match (&self.openapi, &self.discovery) {
            (Some(text), _) => (SpecFormat::OpenApi, text),
            (None, Some(text)) => (SpecFormat::Discovery, text),
            (None, None) => return Ok(None),
        };
        let source = SpecSource::parse(format.key(), text)?;
        Ok(Some(ApiDescription { format, source }))
    }

    pub fn typed(
        &self,
        name: &str,
        sources: &BTreeMap<String, AuthToml>,
    ) -> Result<ApiProfile, String> {
        check_http_url("base_url", &self.base_url)?;
        if let Some(agent) = &self.agent {
            agent.validate()?;
        }
        let auth = match (&self.auth, &self.aws_profile) {
            (Some(_), Some(_)) => {
                return Err("auth and aws_profile are exclusive: keep one of them".to_string());
            }
            (Some(reference), None) if sources.contains_key(reference) => Some(reference.clone()),
            (Some(reference), None) => {
                return Err(format!(
                    "auth = \"{reference}\" names no [auth.{reference}] section"
                ));
            }
            (None, Some(_)) => None,
            (None, None) => sources.contains_key(name).then(|| name.to_string()),
        };
        // The source puts its credential in this header and would replace
        // the configured value without a word.
        if let Some(header) = auth
            .as_ref()
            .and_then(|source| sources.get(source))
            .and_then(|source| source.header.as_deref())
            .filter(|header| self.headers.contains(header))
        {
            return Err(format!(
                "headers cannot set {header}: [auth.{}] sends its credential in it",
                auth.as_deref().unwrap_or_default()
            ));
        }
        if self.aws_profile.is_none() && (self.service.is_some() || self.region.is_some()) {
            return Err("service and region apply to aws_profile only".to_string());
        }
        let spec = self.description_source()?;
        if self.openapi_auth
            && let Some(ApiDescription {
                format: SpecFormat::GraphQl(_),
                ..
            }) = &spec
        {
            return Err(
                "openapi_auth applies to an openapi or discovery URL; graphql introspection always sends the API's credential"
                    .to_string(),
            );
        }
        if self.openapi_auth {
            match spec.as_ref().map(|spec| &spec.source) {
                Some(SpecSource::Url(spec_url)) => {
                    // The credential goes to the origin of base_url only, as
                    // for a TARGET URL.
                    let origin = |value: &str| url::Url::parse(value).ok().map(|u| u.origin());
                    if origin(spec_url) != origin(&self.base_url) {
                        return Err(format!(
                            "openapi_auth sends the credential with the description request, so the URL must be on the origin of base_url {:?}; got {spec_url:?}",
                            self.base_url
                        ));
                    }
                }
                Some(SpecSource::File(_)) => {
                    return Err(
                        "openapi_auth applies to an openapi or discovery URL; a file is read as is"
                            .to_string(),
                    );
                }
                None => return Err("openapi_auth = true needs openapi or discovery".to_string()),
            }
        }
        Ok(ApiProfile {
            name: name.to_string(),
            description: self.description.clone(),
            base_url: self.base_url.clone(),
            auth,
            aws_profile: self.aws_profile.clone(),
            signing: SigningHint {
                service: self.service.clone(),
                region: self.region.clone(),
            },
            spec,
            openapi_auth: self.openapi_auth,
            headers: self.headers.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(toml: &str) -> AuthToml {
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

    const GITHUB: &str = r#"
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

    #[test]
    fn api_profiles_reference_auth_sources_by_name_or_implicitly() {
        let mut sources = BTreeMap::new();
        sources.insert("github".to_string(), auth(GITHUB));
        let explicit: ApiToml =
            toml::from_str("base_url = \"https://api.github.com\"\nauth = \"github\"\n").unwrap();
        assert_eq!(
            explicit.typed("gh", &sources).unwrap().auth.as_deref(),
            Some("github")
        );
        let implicit: ApiToml =
            toml::from_str("description = \"GitHub\"\nbase_url = \"https://api.github.com\"\n")
                .unwrap();
        let profile = implicit.typed("github", &sources).unwrap();
        assert_eq!(profile.auth.as_deref(), Some("github"));
        assert_eq!(profile.description.as_deref(), Some("GitHub"));
        let none: ApiToml = toml::from_str("base_url = \"https://example.com/v1\"\n").unwrap();
        assert!(none.typed("public", &sources).unwrap().auth.is_none());
        let dangling: ApiToml =
            toml::from_str("base_url = \"https://example.com\"\nauth = \"nope\"\n").unwrap();
        assert!(
            dangling
                .typed("x", &sources)
                .unwrap_err()
                .contains("[auth.nope]")
        );
        let bad_url: ApiToml = toml::from_str("base_url = \"api.github.com\"\n").unwrap();
        assert!(
            bad_url
                .typed("x", &sources)
                .unwrap_err()
                .contains("base_url")
        );
    }

    #[test]
    fn aws_profile_apis_sign_instead_of_using_a_source() {
        let mut sources = BTreeMap::new();
        sources.insert("apigw".to_string(), auth(GITHUB));
        let custom: ApiToml = toml::from_str(
            "base_url = \"https://api.example.com\"\naws_profile = \"kurama-real\"\nservice = \"execute-api\"\nregion = \"ap-northeast-1\"\n",
        )
        .unwrap();
        let profile = custom.typed("apigw", &sources).unwrap();
        assert_eq!(profile.aws_profile.as_deref(), Some("kurama-real"));
        assert_eq!(profile.auth, None, "the same-name source is not implied");
        assert_eq!(
            profile.signing,
            SigningHint {
                service: Some("execute-api".into()),
                region: Some("ap-northeast-1".into()),
            }
        );
        let inferred: ApiToml = toml::from_str(
            "base_url = \"https://abc.execute-api.ap-northeast-1.amazonaws.com/prod\"\naws_profile = \"kurama-real\"\n",
        )
        .unwrap();
        assert_eq!(
            inferred.typed("x", &sources).unwrap().signing,
            SigningHint::default()
        );
        let both: ApiToml = toml::from_str(
            "base_url = \"https://x\"\nauth = \"apigw\"\naws_profile = \"kurama-real\"\n",
        )
        .unwrap();
        assert_eq!(
            both.typed("x", &sources).unwrap_err(),
            "auth and aws_profile are exclusive: keep one of them"
        );
        let stray: ApiToml =
            toml::from_str("base_url = \"https://x\"\nregion = \"ap-northeast-1\"\n").unwrap();
        assert_eq!(
            stray.typed("x", &sources).unwrap_err(),
            "service and region apply to aws_profile only"
        );
    }

    #[test]
    fn headers_are_kept_and_the_header_a_token_source_uses_is_refused() {
        let mut sources = BTreeMap::new();
        sources.insert(
            "example".to_string(),
            auth("kind = \"token\"\ntoken = \"op://Agent/x/y\"\nheader = \"xi-api-key\"\nformat = \"{token}\"\n"),
        );
        let section: ApiToml = toml::from_str(
            "base_url = \"https://x\"\nheaders = { Accept = \"application/vnd.x+json\" }\n",
        )
        .unwrap();
        let profile = section.typed("example", &sources).unwrap();
        assert_eq!(
            profile.headers.iter().collect::<Vec<_>>(),
            [("Accept", "application/vnd.x+json")]
        );
        let none: ApiToml = toml::from_str("base_url = \"https://x\"\n").unwrap();
        assert_eq!(
            none.typed("x", &sources).unwrap().headers,
            ApiHeaders::default()
        );
        let taken: ApiToml =
            toml::from_str("base_url = \"https://x\"\nheaders = { XI-API-KEY = \"k\" }\n").unwrap();
        assert_eq!(
            taken.typed("example", &sources).unwrap_err(),
            "headers cannot set xi-api-key: [auth.example] sends its credential in it"
        );
        let signed: ApiToml = toml::from_str(
            "base_url = \"https://x\"\naws_profile = \"dev\"\nheaders = { XI-API-KEY = \"k\" }\n",
        )
        .unwrap();
        assert!(
            signed.typed("example", &sources).is_ok(),
            "an aws_profile API uses no source"
        );
    }

    #[test]
    fn openapi_is_a_url_or_a_file_and_openapi_auth_needs_a_url() {
        let sources = BTreeMap::new();
        let url: ApiToml = toml::from_str(
            "base_url = \"https://api.github.com/v3\"\nopenapi = \"https://api.github.com/openapi.json\"\nopenapi_auth = true\n",
        )
        .unwrap();
        let profile = url.typed("github", &sources).unwrap();
        assert_eq!(
            profile.spec.clone().map(|spec| spec.source),
            Some(SpecSource::Url(
                "https://api.github.com/openapi.json".into()
            ))
        );
        assert!(profile.openapi_auth);
        let elsewhere: ApiToml = toml::from_str(
            "base_url = \"https://api.github.com\"\nopenapi = \"https://raw.githubusercontent.com/gh/api.json\"\n",
        )
        .unwrap();
        assert!(
            elsewhere.typed("github", &sources).unwrap().spec.is_some(),
            "another origin is fine without openapi_auth"
        );
        let file: ApiToml =
            toml::from_str("base_url = \"https://x\"\nopenapi = \"~/specs/internal.yaml\"\n")
                .unwrap();
        let profile = file.typed("internal", &sources).unwrap();
        assert_eq!(
            profile.spec.clone().map(|spec| spec.source),
            Some(SpecSource::File("~/specs/internal.yaml".into()))
        );
        assert!(!profile.openapi_auth);
        assert_eq!(
            profile.spec.unwrap().source.to_string(),
            "~/specs/internal.yaml"
        );
        let none: ApiToml = toml::from_str("base_url = \"https://x\"\n").unwrap();
        assert_eq!(none.typed("x", &sources).unwrap().spec, None);

        for (toml, expected) in [
            (
                "base_url = \"https://x\"\nopenapi = \"\"\n",
                "openapi must be a URL or a file path",
            ),
            (
                "base_url = \"https://x\"\nopenapi = \"ftp://x/spec.json\"\n",
                "openapi must be an http(s) URL or a file path",
            ),
            (
                "base_url = \"https://x\"\nopenapi = \"~/spec.json\"\nopenapi_auth = true\n",
                "openapi_auth applies to an openapi or discovery URL",
            ),
            (
                "base_url = \"https://x\"\nopenapi_auth = true\n",
                "openapi_auth = true needs openapi",
            ),
            (
                "base_url = \"https://api.github.com\"\nopenapi = \"https://raw.githubusercontent.com/gh/api.json\"\nopenapi_auth = true\n",
                "must be on the origin of base_url \"https://api.github.com\"",
            ),
            (
                "base_url = \"https://x\"\nopenapi = \"specs/x.yaml\"\n",
                "absolute file path (~ allowed), got \"specs/x.yaml\"",
            ),
        ] {
            let section: ApiToml = toml::from_str(toml).unwrap();
            let message = section.typed("x", &sources).unwrap_err();
            assert!(message.contains(expected), "{toml}: {message}");
        }
    }

    #[test]
    fn graphql_names_the_endpoint_and_graphql_schema_a_file_in_its_place() {
        let sources = BTreeMap::new();
        let typed = |toml: &str| {
            toml::from_str::<ApiToml>(toml)
                .unwrap()
                .typed("linear", &sources)
        };
        let endpoint = GraphQlEndpoint {
            path: "/graphql".into(),
        };
        assert_eq!(
            typed("base_url = \"https://api.linear.app/\"\ngraphql = \"/graphql\"\n")
                .unwrap()
                .spec,
            Some(ApiDescription {
                format: SpecFormat::GraphQl(endpoint.clone()),
                source: SpecSource::Url("https://api.linear.app/graphql".into()),
            }),
            "introspected at the endpoint under base_url"
        );
        let profile = typed(
            "base_url = \"https://api.linear.app\"\ngraphql = \"/graphql\"\ngraphql_schema = \"~/linear.json\"\n",
        )
        .unwrap();
        assert_eq!(
            profile.spec,
            Some(ApiDescription {
                format: SpecFormat::GraphQl(endpoint),
                source: SpecSource::File("~/linear.json".into()),
            })
        );
        assert_eq!(profile.spec_key(), "graphql_schema");
        for (toml, expected) in [
            (
                "base_url = \"https://x\"\nopenapi = \"/a.json\"\ngraphql = \"/graphql\"\n",
                "openapi and graphql are exclusive",
            ),
            (
                "base_url = \"https://x\"\ngraphql = \"graphql\"\n",
                "graphql must be the endpoint's path under base_url, starting with '/'",
            ),
            (
                "base_url = \"https://x\"\ngraphql_schema = \"/s.json\"\n",
                "graphql_schema needs graphql",
            ),
            (
                "base_url = \"https://x\"\ngraphql = \"/graphql\"\ngraphql_schema = \"https://x/s.json\"\n",
                "graphql_schema must be a file",
            ),
            (
                "base_url = \"https://x\"\ngraphql = \"/graphql\"\nopenapi_auth = true\n",
                "graphql introspection always sends the API's credential",
            ),
        ] {
            let message = typed(toml).unwrap_err();
            assert!(message.contains(expected), "{toml}: {message}");
        }
    }

    #[test]
    fn discovery_names_a_discovery_document_and_excludes_openapi() {
        let sources = BTreeMap::new();
        let url = "https://sheets.googleapis.com/$discovery/rest?version=v4";
        let section: ApiToml = toml::from_str(&format!(
            "base_url = \"https://sheets.googleapis.com\"\ndiscovery = \"{url}\"\nopenapi_auth = true\n"
        ))
        .unwrap();
        let profile = section.typed("sheets", &sources).unwrap();
        assert_eq!(
            profile.spec,
            Some(ApiDescription {
                format: SpecFormat::Discovery,
                source: SpecSource::Url(url.into()),
            })
        );
        let file: ApiToml =
            toml::from_str("base_url = \"https://x\"\ndiscovery = \"~/sheets.json\"\n").unwrap();
        assert_eq!(
            file.typed("x", &sources).unwrap().spec.unwrap().format,
            SpecFormat::Discovery
        );
        for (toml, expected) in [
            (
                "base_url = \"https://x\"\nopenapi = \"/a.json\"\ndiscovery = \"/b.json\"\n",
                "openapi and discovery are exclusive",
            ),
            (
                "base_url = \"https://x\"\ndiscovery = \"b.json\"\n",
                "discovery must be an http(s) URL or an absolute file path",
            ),
        ] {
            let section: ApiToml = toml::from_str(toml).unwrap();
            let message = section.typed("x", &sources).unwrap_err();
            assert!(message.contains(expected), "{toml}: {message}");
        }
    }
}
