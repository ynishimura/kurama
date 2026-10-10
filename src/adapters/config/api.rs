//! `[api.<name>]`: the APIs `kurama api` calls, each with an `[auth.*]` source or the role of an AWS profile.
//!
//! ```toml
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
//! A `secrets` source puts values in the environment of a command and
//! authenticates no request, so an `[api.*]` that names it is refused.
//! `auth` and `aws_profile` are exclusive. An `[auth.*]` name that
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

use super::AuthToml;
use crate::domain::functions::api_request::join_base_path;
use crate::domain::functions::signing_target::SigningHint;
use crate::domain::types::api_spec::{GraphQlEndpoint, SpecFormat};
use crate::domain::types::oauth_client::check_http_url;
use crate::domain::types::{ApiHeaders, AuthKind};

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
        // Its values go to the environment of a command; no request carries
        // them, so an API that named it would be sent without a credential.
        if let Some(source) = auth.as_deref().filter(|source| {
            sources
                .get(*source)
                .is_some_and(|toml| toml.kind == AuthKind::Secrets)
        }) {
            return Err(format!(
                "[auth.{source}] is kind = \"secrets\", which authenticates no request: \
                 name a kind = \"oauth\" or \"token\" source in auth"
            ));
        }
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
    use crate::adapters::config::auth::tests::{GITHUB, SITE_LOGIN, auth};

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

    /// An API cannot use a secrets source, whether it names it or shares
    /// its name: the request would go without a credential.
    #[test]
    fn an_api_cannot_authenticate_with_a_secrets_source() {
        let sources = BTreeMap::from([("site".to_string(), auth(SITE_LOGIN))]);
        for toml in [
            "base_url = \"https://x\"\nauth = \"site\"\n",
            "base_url = \"https://x\"\n",
        ] {
            let section: ApiToml = toml::from_str(toml).unwrap();
            let message = section.typed("site", &sources).unwrap_err();
            assert!(
                message
                    .contains("[auth.site] is kind = \"secrets\", which authenticates no request"),
                "{toml}: {message}"
            );
        }
    }
}
