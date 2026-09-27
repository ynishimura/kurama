//! `kurama api --dry-run --json`: the request a run would send and every
//! effect it would have, as one JSON document on stdout, built without
//! starting a credential source.
//!
//! The effects come from the same [`ApiCredential`] `ApiRuntime::call`
//! matches on, so a new kind of credential does not build until the plan
//! says what it does. What the dry run itself did (read or fetch the OpenAPI
//! description, read the body file) is `performed: true`; what the run would
//! do is `performed: false` with `when`, so `network: true` never reads as a
//! request that was sent.

use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use super::api_command::ApiPages;
use super::api_pages::follow_text;
use super::api_spec::source_json;
use crate::domain::functions::api_pages::PageStyle;
use crate::domain::types::http::mask_secret_headers_with;
use crate::domain::types::{GrantType, Profile, SecretRef, TokenPlacement};
use crate::ports::HttpRequest;
use crate::shell::api_runtime::ApiCredential;
use crate::shell::spec_loader::SpecOrigin;

/// The version of the plan document's shape.
const SCHEMA_VERSION: u32 = 1;

/// Everything the plan document is made of.
pub(crate) struct DryRunPlan<'a> {
    pub api: &'a str,
    pub target: &'a str,
    /// The request as the preview built it: placeholder credentials only.
    pub request: &'a HttpRequest,
    pub credential: &'a ApiCredential,
    /// The AWS profile an `aws-secrets://` / `aws-ssm://` reference of the
    /// credential names, read from the profile file.
    pub reference_profile: Option<&'a Profile>,
    /// Where the OpenAPI description came from, when the target needed it.
    pub description: Option<&'a SpecOrigin>,
    /// `--refresh-spec`: the run fetches the description again.
    pub refresh: bool,
    /// `-d @path`: the file the dry run read the body from.
    pub body_file: Option<&'a str>,
    /// `--output PATH`: the file the run writes a 2xx body to.
    pub output: Option<&'a Path>,
    /// `--pages N`: the requests that may follow the first.
    pub pages: Option<&'a ApiPages>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum When {
    Always,
    IfNeeded,
    /// After the API answered with a 2xx status.
    OnSuccess,
    /// After a 2xx response that names a next page (`--pages`).
    OnNextPage,
}

#[derive(Debug, Serialize)]
struct Effect {
    effect: &'static str,
    performed: bool,
    network: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    when: Option<When>,
    /// A person may have to act: a browser, a code to enter.
    may_require_human: bool,
    detail: String,
}

impl Effect {
    /// What the dry run did.
    fn done(effect: &'static str, network: bool, detail: String) -> Self {
        Self {
            effect,
            performed: true,
            network,
            when: None,
            may_require_human: false,
            detail,
        }
    }

    /// What the run would do.
    fn on_run(effect: &'static str, network: bool, when: When, detail: String) -> Self {
        Self {
            effect,
            performed: false,
            network,
            when: Some(when),
            may_require_human: false,
            detail,
        }
    }

    fn by_a_person(mut self) -> Self {
        self.may_require_human = true;
        self
    }
}

impl DryRunPlan<'_> {
    pub(crate) fn to_json(&self) -> Value {
        let description = self
            .description
            .map(|origin| (origin, origin_facts(origin, self.refresh)));
        let mut effects = Vec::new();
        if let Some((_, facts)) = &description {
            effects.extend(
                facts.performed.iter().map(|(effect, network, detail)| {
                    Effect::done(effect, *network, detail.clone())
                }),
            );
        }
        if let Some(path) = self.body_file {
            effects.push(Effect::done(
                "filesystem",
                false,
                format!("read the body file {path}"),
            ));
        }
        if let Some((origin, facts)) = &description
            && let (Some(when), Some(url)) = (facts.run_when, origin_url(origin))
        {
            effects.push(Effect::on_run(
                "description_fetch",
                true,
                when,
                format!("GET {url}"),
            ));
        }
        effects.extend(credential_effects(self.credential, self.reference_profile));
        effects.push(Effect::on_run(
            "api_request",
            true,
            When::Always,
            format!("{} {}", self.request.method, self.request.url),
        ));
        if let Some(pages) = self.pages.filter(|pages| pages.limit > 1) {
            effects.push(Effect::on_run(
                "api_request",
                true,
                When::OnNextPage,
                format!(
                    "up to {} more requests, each {}",
                    pages.limit - 1,
                    follow_text(&pages.style)
                ),
            ));
        }
        if let Some(path) = self.output {
            effects.push(Effect::on_run(
                "file_write",
                false,
                When::OnSuccess,
                format!("write the response body to {}", path.display()),
            ));
        }
        json!({
            "schema_version": SCHEMA_VERSION,
            "dry_run": true,
            "api": self.api,
            "target": self.target,
            "request": self.request_json(),
            "auth": auth_json(self.credential),
            "description": description.map(|(origin, facts)| json!({
                "source": source_json(origin),
                "cache_hit": facts.cache_hit,
                "network": facts.network,
            })),
            "pages": self.pages.map(pages_json),
            "effects": effects,
        })
    }

    fn request_json(&self) -> Value {
        let extra = self.credential.extra_secret_headers();
        let masked = mask_secret_headers_with(&self.request.headers, &extra);
        let headers: Vec<Value> = self
            .request
            .headers
            .iter()
            .zip(masked)
            .map(|((_, value), (name, shown))| {
                json!({"name": name, "value": shown, "masked": &shown != value})
            })
            .collect();
        // The body is the caller's own and may hold a secret: its size and
        // whether it is JSON, never its bytes.
        let body = self.request.body.as_ref().map(|body| {
            json!({
                "bytes": body.len(),
                "json": serde_json::from_slice::<serde::de::IgnoredAny>(body).is_ok(),
            })
        });
        json!({
            "method": self.request.method,
            "url": self.request.url,
            "headers": headers,
            "body": body,
        })
    }
}

/// `--pages`: the limit and how the next page is found.
fn pages_json(pages: &ApiPages) -> Value {
    match &pages.style {
        PageStyle::Link => json!({"limit": pages.limit, "follow": "link", "cursor": null}),
        PageStyle::Cursor(cursor) => json!({
            "limit": pages.limit,
            "follow": "cursor",
            "cursor": {"path": cursor.path, "param": cursor.param},
        }),
    }
}

/// `none`, `bearer` (the credential of an `[auth.*]` source, OAuth or
/// `kind = "token"`, in `header`) or `sigv4`.
fn auth_json(credential: &ApiCredential) -> Value {
    match credential {
        ApiCredential::None => json!({"mode": "none"}),
        ApiCredential::OAuth(client) => json!({
            "mode": "bearer",
            "kind": "oauth",
            "source": client.name,
            "header": "Authorization",
            "grant_type": client.grant_type.as_str(),
        }),
        ApiCredential::Token(source) => match &source.placement {
            TokenPlacement::Header { name, .. } => json!({
                "mode": "bearer",
                "kind": "token",
                "source": source.name,
                "header": name,
            }),
            TokenPlacement::Basic { .. } => json!({
                "mode": "basic",
                "kind": "token",
                "source": source.name,
                "header": "Authorization",
            }),
            TokenPlacement::Query { param } => json!({
                "mode": "query",
                "kind": "token",
                "source": source.name,
                "query": param,
            }),
        },
        ApiCredential::SigV4 { profile, target } => json!({
            "mode": "sigv4",
            "aws_profile": profile.name(),
            "service": target.service,
            "region": target.region,
        }),
    }
}

/// What getting the credential would do.
fn credential_effects(credential: &ApiCredential, reference: Option<&Profile>) -> Vec<Effect> {
    match credential {
        ApiCredential::None => Vec::new(),
        ApiCredential::OAuth(client) => {
            let interactive = client.grant_type != GrantType::ClientCredentials;
            let mut effects = vec![Effect::on_run(
                "token_store",
                false,
                When::Always,
                format!(
                    "read the stored token of [auth.{}] (keychain); a new token is stored there",
                    client.name
                ),
            )];
            let server = Effect::on_run(
                "authorization_server",
                true,
                When::IfNeeded,
                format!(
                    "refresh the token, or run the {} grant, when no usable token is stored; once more after an HTTP 401",
                    client.grant_type.as_str()
                ),
            );
            effects.push(if interactive {
                server.by_a_person()
            } else {
                server
            });
            if interactive {
                effects.push(
                    Effect::on_run(
                        "browser",
                        false,
                        When::IfNeeded,
                        format!("open the {} page", client.grant_type.as_str()),
                    )
                    .by_a_person(),
                );
            }
            if let Some(secret) = &client.client_secret {
                effects.extend(secret_effects(
                    secret,
                    When::IfNeeded,
                    "the client secret",
                    reference,
                ));
            }
            effects
        }
        ApiCredential::Token(source) => secret_effects(
            &source.token,
            When::Always,
            &format!("the credential of [auth.{}]", source.name),
            reference,
        ),
        ApiCredential::SigV4 { profile, .. } => aws_profile_effects(profile, When::Always),
    }
}

/// The reads resolving `secret` would make.
fn secret_effects(
    secret: &SecretRef,
    when: When,
    what: &str,
    profile: Option<&Profile>,
) -> Vec<Effect> {
    match secret {
        SecretRef::OnePassword(_) => vec![Effect::on_run(
            "1password",
            false,
            when,
            format!("read {what} (op read)"),
        )],
        SecretRef::Aws(reference) => {
            let mut effects = profile
                .map(|profile| aws_profile_effects(profile, when))
                .unwrap_or_default();
            effects.push(Effect::on_run(
                "aws_secret_store",
                true,
                when,
                format!("read {what} from {}", reference.store.scheme()),
            ));
            effects
        }
        SecretRef::Literal(_) => Vec::new(),
    }
}

/// Assuming the role of an AWS profile: with MFA, the cached session or a
/// new one from GetSessionToken with the code from 1Password, then
/// AssumeRole. Shared by SigV4 and `aws-*://` references.
fn aws_profile_effects(profile: &Profile, when: When) -> Vec<Effect> {
    let name = profile.name();
    let mut effects = Vec::new();
    if profile.requires_mfa() {
        effects.push(Effect::on_run(
            "keychain",
            false,
            when,
            format!("read the cached MFA session of AWS profile {name}; a new one is stored there"),
        ));
        effects.push(Effect::on_run(
            "1password",
            false,
            When::IfNeeded,
            format!("the MFA code of AWS profile {name} when no MFA session is cached"),
        ));
        effects.push(Effect::on_run(
            "sts",
            true,
            When::IfNeeded,
            format!("GetSessionToken for AWS profile {name} when no MFA session is cached"),
        ));
    }
    effects.push(Effect::on_run(
        "sts",
        true,
        when,
        format!("AssumeRole for AWS profile {name}"),
    ));
    effects
}

/// What one description origin says: whether a cached copy answered,
/// whether this dry run reached the network for it, what it did, and when
/// the run would fetch it.
struct OriginFacts {
    cache_hit: bool,
    network: bool,
    performed: Vec<(&'static str, bool, String)>,
    run_when: Option<When>,
}

fn origin_facts(origin: &SpecOrigin, refresh: bool) -> OriginFacts {
    let within_interval = if refresh {
        When::Always
    } else {
        When::IfNeeded
    };
    let read_cache = (
        "filesystem",
        false,
        "read the description cache".to_string(),
    );
    let (cache_hit, network, performed, run_when) = match origin {
        SpecOrigin::File(path) => (
            false,
            false,
            vec![(
                "filesystem",
                false,
                format!("read the description file {}", path.display()),
            )],
            None,
        ),
        SpecOrigin::Fetched { url } => (
            false,
            true,
            vec![
                ("description_fetch", true, format!("GET {url}")),
                (
                    "filesystem",
                    false,
                    "wrote the description cache".to_string(),
                ),
            ],
            Some(within_interval),
        ),
        SpecOrigin::Cached { .. } => (true, false, vec![read_cache], Some(within_interval)),
        SpecOrigin::Validated { url, .. } => (
            true,
            true,
            vec![
                (
                    "description_fetch",
                    true,
                    format!("GET {url} (conditional; unchanged)"),
                ),
                (
                    "filesystem",
                    false,
                    "updated the description cache".to_string(),
                ),
            ],
            Some(within_interval),
        ),
        // Past its interval: the run asks the server again.
        SpecOrigin::Stale { url, reason, .. } => (
            true,
            true,
            vec![
                (
                    "description_fetch",
                    true,
                    format!("GET {url} failed: {reason}"),
                ),
                read_cache,
            ],
            Some(When::Always),
        ),
        SpecOrigin::Unchecked { .. } => (true, false, vec![read_cache], Some(When::Always)),
    };
    OriginFacts {
        cache_hit,
        network,
        performed,
        run_when,
    }
}

fn origin_url(origin: &SpecOrigin) -> Option<&str> {
    match origin {
        SpecOrigin::File(_) => None,
        SpecOrigin::Fetched { url }
        | SpecOrigin::Cached { url, .. }
        | SpecOrigin::Validated { url, .. }
        | SpecOrigin::Stale { url, .. }
        | SpecOrigin::Unchecked { url, .. } => Some(url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::functions::signing_target::SigningTarget;
    use crate::domain::types::{EndpointSource, OAuthClientConfig, TokenSourceConfig};

    fn plan(
        request: &HttpRequest,
        credential: &ApiCredential,
        reference: Option<&Profile>,
        description: Option<&SpecOrigin>,
        refresh: bool,
    ) -> Value {
        DryRunPlan {
            api: "svc",
            target: "/items",
            request,
            credential,
            reference_profile: reference,
            description,
            refresh,
            body_file: None,
            output: None,
            pages: None,
        }
        .to_json()
    }

    #[test]
    fn an_output_file_is_written_after_the_request_and_only_on_success() {
        let request = HttpRequest::new("GET", "https://x/items");
        let document = DryRunPlan {
            api: "svc",
            target: "/items",
            request: &request,
            credential: &ApiCredential::None,
            reference_profile: None,
            description: None,
            refresh: false,
            body_file: None,
            output: Some(Path::new("/out/body.json")),
            pages: None,
        }
        .to_json();
        assert_eq!(
            document["effects"],
            json!([
                {"effect": "api_request", "performed": false, "network": true, "when": "always",
                 "may_require_human": false, "detail": "GET https://x/items"},
                {"effect": "file_write", "performed": false, "network": false, "when": "on_success",
                 "may_require_human": false, "detail": "write the response body to /out/body.json"},
            ])
        );
    }

    #[test]
    fn pages_add_the_requests_that_may_follow_the_first_and_one_page_adds_none() {
        use crate::domain::functions::api_pages::CursorStyle;
        let request = HttpRequest::new("GET", "https://x/items");
        let plan = |pages: &ApiPages| {
            DryRunPlan {
                api: "svc",
                target: "/items",
                request: &request,
                credential: &ApiCredential::None,
                reference_profile: None,
                description: None,
                refresh: false,
                body_file: None,
                output: None,
                pages: Some(pages),
            }
            .to_json()
        };
        let link = plan(&ApiPages {
            limit: 3,
            style: PageStyle::Link,
        });
        assert_eq!(
            link["pages"],
            json!({"limit": 3, "follow": "link", "cursor": null})
        );
        assert_eq!(
            link["effects"][1],
            json!({"effect": "api_request", "performed": false, "network": true,
                   "when": "on_next_page", "may_require_human": false,
                   "detail": "up to 2 more requests, each the URL of the Link rel=\"next\" of the response before it, on the origin of base_url"})
        );
        let cursor = plan(&ApiPages {
            limit: 1,
            style: PageStyle::Cursor(CursorStyle::parse("meta.next=cursor").unwrap()),
        });
        assert_eq!(
            cursor["pages"],
            json!({"limit": 1, "follow": "cursor", "cursor": {"path": "meta.next", "param": "cursor"}})
        );
        assert_eq!(cursor["effects"].as_array().unwrap().len(), 1);
    }

    fn effects(plan: &Value, performed: bool) -> Vec<String> {
        plan["effects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|effect| effect["performed"] == performed)
            .map(|effect| effect["effect"].as_str().unwrap().to_string())
            .collect()
    }

    fn oauth(grant_type: GrantType, client_secret: Option<SecretRef>) -> ApiCredential {
        ApiCredential::OAuth(OAuthClientConfig {
            name: "gh".into(),
            grant_type,
            endpoints: EndpointSource::Issuer("https://x".into()),
            client_id: "id".into(),
            client_secret,
            scopes: vec![],
            env_var: "GH".into(),
            redirect_port: None,
        })
    }

    fn token(reference: &str) -> ApiCredential {
        ApiCredential::Token(TokenSourceConfig {
            name: "example".into(),
            token: SecretRef::parse(reference).unwrap(),
            placement: crate::domain::types::TokenPlacement::Header {
                name: "xi-api-key".into(),
                format: "{token}".into(),
            },
            env_var: "EXAMPLE".into(),
        })
    }

    fn mfa_profile() -> Profile {
        Profile::new("ops-mfa").with_mfa_serial_raw("arn:aws:iam::123456789012:mfa/agent")
    }

    #[test]
    fn headers_are_masked_and_the_body_is_only_its_size() {
        let request = HttpRequest::new("POST", "https://api.example.com/v1/items")
            .with_header("Accept", "application/json")
            .with_header("xi-api-key", "<token>")
            .with_header("Private-Token", "caller-secret")
            .with_body(br#"{"password":"hunter2"}"#.to_vec());
        let plan = plan(&request, &token("op://v/i/f"), None, None, false);
        let text = plan.to_string();
        for secret in ["<token>", "hunter2", "caller-secret", "op://"] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
        assert_eq!(plan["schema_version"], 1);
        assert_eq!(plan["request"]["headers"][0]["masked"], false);
        assert_eq!(plan["request"]["headers"][1]["value"], "****");
        assert_eq!(plan["request"]["headers"][2]["masked"], true);
        assert_eq!(plan["request"]["body"], json!({"bytes": 22, "json": true}));
        assert_eq!(plan["description"], Value::Null);
        assert_eq!(
            plan["auth"],
            json!({"mode": "bearer", "kind": "token", "source": "example", "header": "xi-api-key"})
        );
        assert_eq!(effects(&plan, true), Vec::<String>::new());
        assert_eq!(effects(&plan, false), ["1password", "api_request"]);
    }

    #[test]
    fn each_credential_names_the_effects_the_run_would_have() {
        let request = HttpRequest::new("GET", "https://x/items");
        let interactive = plan(
            &request,
            &oauth(
                GrantType::AuthorizationCode,
                Some(SecretRef::OnePassword("op://v/i/f".into())),
            ),
            None,
            None,
            false,
        );
        assert_eq!(interactive["auth"]["mode"], "bearer");
        assert_eq!(interactive["auth"]["grant_type"], "authorization_code");
        assert_eq!(
            effects(&interactive, false),
            [
                "token_store",
                "authorization_server",
                "browser",
                "1password",
                "api_request"
            ]
        );
        assert_eq!(interactive["effects"][1]["may_require_human"], true);
        assert_eq!(interactive["effects"][2]["may_require_human"], true);
        assert_eq!(interactive["effects"][3]["when"], "if_needed");
        let service = plan(
            &request,
            &oauth(GrantType::ClientCredentials, None),
            None,
            None,
            false,
        );
        assert_eq!(
            effects(&service, false),
            ["token_store", "authorization_server", "api_request"]
        );
        assert_eq!(service["effects"][1]["may_require_human"], false);

        // An `aws-ssm://` reference through an MFA profile goes the same way
        // as SigV4 with that profile, plus the read itself.
        let ssm = plan(
            &request,
            &token("aws-ssm://ops-mfa/kurama/key"),
            Some(&mfa_profile()),
            None,
            false,
        );
        assert_eq!(
            effects(&ssm, false),
            [
                "keychain",
                "1password",
                "sts",
                "sts",
                "aws_secret_store",
                "api_request"
            ]
        );
        let sigv4 = ApiCredential::SigV4 {
            profile: mfa_profile(),
            target: SigningTarget {
                service: "execute-api".into(),
                region: "ap-northeast-1".into(),
            },
        };
        let signed = plan(&request, &sigv4, None, None, false);
        assert_eq!(
            signed["auth"],
            json!({"mode": "sigv4", "aws_profile": "ops-mfa", "service": "execute-api", "region": "ap-northeast-1"})
        );
        assert_eq!(
            effects(&signed, false),
            ["keychain", "1password", "sts", "sts", "api_request"]
        );
        assert_eq!(
            effects(
                &plan(&request, &ApiCredential::None, None, None, false),
                false
            ),
            ["api_request"]
        );
    }

    #[test]
    fn the_description_origin_decides_what_was_done_and_when_the_run_fetches() {
        let request = HttpRequest::new("GET", "https://x/items");
        let url = "https://x/openapi.json".to_string();
        let fetched = SpecOrigin::Fetched { url: url.clone() };
        let document = plan(&request, &ApiCredential::None, None, Some(&fetched), false);
        assert_eq!(
            document["description"],
            json!({"source": {"kind": "fetched", "url": url}, "cache_hit": false, "network": true})
        );
        assert_eq!(
            effects(&document, true),
            ["description_fetch", "filesystem"]
        );
        assert_eq!(
            effects(&document, false),
            ["description_fetch", "api_request"]
        );
        assert_eq!(document["effects"][2]["when"], "if_needed");
        let refreshed = plan(&request, &ApiCredential::None, None, Some(&fetched), true);
        assert_eq!(refreshed["effects"][2]["when"], "always");

        let file = SpecOrigin::File("/specs/petstore.json".into());
        let document = plan(&request, &ApiCredential::None, None, Some(&file), false);
        assert_eq!(document["description"]["network"], false);
        assert_eq!(document["description"]["cache_hit"], false);
        assert_eq!(effects(&document, true), ["filesystem"]);
        assert_eq!(effects(&document, false), ["api_request"]);

        let unchecked = SpecOrigin::Unchecked {
            url: url.clone(),
            fetched_at: chrono::Utc::now(),
        };
        let document = plan(
            &request,
            &ApiCredential::None,
            None,
            Some(&unchecked),
            false,
        );
        assert_eq!(document["description"]["network"], false);
        assert_eq!(document["description"]["cache_hit"], true);
        assert_eq!(effects(&document, true), ["filesystem"]);
        assert_eq!(document["effects"][1]["when"], "always");
    }
}
