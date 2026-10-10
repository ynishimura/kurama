//! The OpenAPI description of an `[api.*]` profile, loaded and normalized.
//!
//! A file is read as is (`~` expanded). A URL is fetched through the API's
//! HTTP client, with the API's own credential when `openapi_auth` is set,
//! and cached under `~/.cache/kurama/openapi/`. Loads within the configured
//! revalidation interval reuse the copy without I/O beyond reading the cache.
//! Later loads send `ETag` / `Last-Modified` back; a 304 starts a new interval.
//! When the server cannot be reached (or answers 5xx) the
//! cached copy is used with a warning. `--refresh-spec` fetches without
//! validators and replaces the cache. A document is cached only after it
//! parsed and normalized, so the cache never holds an unusable one.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::adapters::config::{ApiProfile, SpecSource};
use crate::adapters::openapi::{self, CachedSpec, FileReadError, SpecCache};
use crate::console::progress;
use crate::domain::functions::api_request::body_excerpt;
use crate::domain::functions::error_mapping::StsErrorKind;
use crate::domain::functions::graphql;
use crate::domain::types::api_spec::{ApiSpec, SpecFormat};
use crate::ports::HttpRequest;
use crate::shell::api_error::ApiError;
use crate::shell::api_runtime::ApiRuntime;
use crate::shell::executor::ExecutorError;
use crate::shell::oauth_executor::OAuthError;

/// Characters of an error body the message carries.
const ERROR_EXCERPT_CHARS: usize = 200;

#[derive(Debug)]
pub struct LoadedSpec {
    pub spec: Arc<ApiSpec>,
    pub origin: SpecOrigin,
}

/// Where the document came from this time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecOrigin {
    File(PathBuf),
    /// Fetched now and cached.
    Fetched {
        url: String,
    },
    /// The cache is still within the configured revalidation interval.
    Cached {
        url: String,
        fetched_at: DateTime<Utc>,
    },
    /// The server answered 304: the copy cached at `fetched_at` is current.
    Validated {
        url: String,
        fetched_at: DateTime<Utc>,
    },
    /// The server could not be asked; the copy cached at `fetched_at` is used.
    Stale {
        url: String,
        fetched_at: DateTime<Utc>,
        reason: String,
    },
    /// The copy cached at `fetched_at`, past its interval and not
    /// revalidated: fetching it needs the API's credential (`openapi_auth`)
    /// and a dry run starts no credential source.
    Unchecked {
        url: String,
        fetched_at: DateTime<Utc>,
    },
}

impl std::fmt::Display for SpecOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Fetched { url } => write!(f, "{url} (fetched)"),
            Self::Cached { url, fetched_at } => write!(
                f,
                "{url} (cached copy from {}; revalidation skipped)",
                fetched_at.to_rfc3339()
            ),
            Self::Validated { url, fetched_at } => {
                write!(f, "{url} (unchanged since {})", fetched_at.to_rfc3339())
            }
            Self::Stale {
                url,
                fetched_at,
                reason,
            } => write!(
                f,
                "{url} (cached copy from {}; {reason})",
                fetched_at.to_rfc3339()
            ),
            Self::Unchecked { url, fetched_at } => write!(
                f,
                "{url} (cached copy from {}; not revalidated: a dry run uses no credential)",
                fetched_at.to_rfc3339()
            ),
        }
    }
}

/// Whether loading a description may use the API's credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecAccess {
    /// A description behind `openapi_auth` is fetched with the API's
    /// credential, like any other request to the API.
    WithCredential,
    /// A dry run's target: a description behind `openapi_auth` is read from
    /// the cache as it is (`Unchecked`) and never fetched, so no credential
    /// source starts; without a cached copy the load fails. A description
    /// without `openapi_auth` loads as usual.
    WithoutCredential,
    /// `agent install --offline`: a URL description is read from the cache
    /// as it is and never fetched; without a cached copy the load fails.
    CacheOnly,
}

/// [`load_spec_with`] with the credential allowed: the tests' shorthand.
#[cfg(test)]
pub async fn load_spec(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    refresh: bool,
    needed: &str,
) -> Result<LoadedSpec> {
    load_spec_with(runtime, api, refresh, needed, SpecAccess::WithCredential).await
}

/// The description of `api`; `needed` names what asked for it, for the
/// error when the profile has no `openapi`, and `access` whether a
/// description behind `openapi_auth` may be fetched with the credential.
pub async fn load_spec_with(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    refresh: bool,
    needed: &str,
    access: SpecAccess,
) -> Result<LoadedSpec> {
    let Some(source) = api.spec.as_ref().map(|spec| &spec.source) else {
        return Err(ApiError::SpecRequired {
            api: api.name.clone(),
            needed: needed.to_string(),
        }
        .into());
    };
    match source {
        SpecSource::File(path) => load_file(api, path),
        SpecSource::Url(url) => load_url(runtime, api, url, refresh, access).await,
    }
}

/// Load a description and, when requested, report the source selected by the
/// cache and revalidation logic.
pub async fn load_spec_verbose(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    refresh: bool,
    needed: &str,
    access: SpecAccess,
    verbose: bool,
) -> Result<LoadedSpec> {
    let loaded = load_spec_with(runtime, api, refresh, needed, access).await?;
    if verbose {
        progress!("# API description: {}", loaded.origin);
    }
    Ok(loaded)
}

pub(crate) fn load_file(api: &ApiProfile, path: &str) -> Result<LoadedSpec> {
    let (expanded, bytes) = openapi::read_file(path).map_err(|error| match error {
        FileReadError::Home(error) => error,
        FileReadError::Read { path, source } => ApiError::SpecUnavailable {
            api: api.name.clone(),
            key: api.spec_key(),
            location: path.display().to_string(),
            message: source.to_string(),
        }
        .into(),
    })?;
    let spec = parse_spec(api, &expanded.display().to_string(), &bytes)?;
    Ok(LoadedSpec {
        spec,
        origin: SpecOrigin::File(expanded),
    })
}

async fn load_url(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    url: &str,
    refresh: bool,
    access: SpecAccess,
) -> Result<LoadedSpec> {
    let unavailable = |message: String| ApiError::SpecUnavailable {
        api: api.name.clone(),
        key: api.spec_key(),
        location: url.to_string(),
        message,
    };
    let cache = SpecCache::new(runtime.spec_cache_dir().to_path_buf());
    let cached = if refresh { None } else { cache.load(url) };
    if let Some(cached) = &cached
        && cached.is_fresh(Utc::now(), runtime.spec_revalidation_interval())
    {
        tracing::debug!(url, fetched_at = %cached.fetched_at, "API description revalidation skipped");
        return cached_copy(
            api,
            cached,
            SpecOrigin::Cached {
                url: url.to_string(),
                fetched_at: cached.fetched_at,
            },
        );
    }
    if access == SpecAccess::CacheOnly {
        return match cached {
            Some(cached) => cached_copy(
                api,
                &cached,
                SpecOrigin::Unchecked {
                    url: url.to_string(),
                    fetched_at: cached.fetched_at,
                },
            ),
            None => {
                Err(unavailable("no copy is cached and nothing is fetched offline".into()).into())
            }
        };
    }
    let graphql = matches!(
        api.spec.as_ref().map(|spec| &spec.format),
        Some(SpecFormat::GraphQl(_))
    );
    // GraphQL introspection is authenticated on most servers, so the schema
    // always goes with the API's credential.
    let with_credential = api.openapi_auth || graphql;
    if with_credential && access == SpecAccess::WithoutCredential {
        return match cached {
            Some(cached) => cached_copy(
                api,
                &cached,
                SpecOrigin::Unchecked {
                    url: url.to_string(),
                    fetched_at: cached.fetched_at,
                },
            ),
            None => Err(ApiError::SpecNeedsCredential {
                api: api.name.clone(),
                location: url.to_string(),
            }
            .into()),
        };
    }
    let mut request = if graphql {
        // A POST has no validators: an expired copy is introspected again.
        HttpRequest::new("POST", url)
            .with_header("Accept", "application/json")
            .with_header("Content-Type", "application/json")
            .with_body(graphql::introspection_request_body())
    } else {
        HttpRequest::new("GET", url).with_header(
            "Accept",
            "application/json, application/yaml, application/x-yaml, text/yaml;q=0.9, */*;q=0.8",
        )
    };
    if let Some(cached) = cached.as_ref().filter(|_| !graphql) {
        if let Some(etag) = &cached.etag {
            request = request.with_header("If-None-Match", etag.clone());
        }
        if let Some(last_modified) = &cached.last_modified {
            request = request.with_header("If-Modified-Since", last_modified.clone());
        }
    }
    let outcome = if with_credential {
        runtime.call(api, request).await
    } else {
        runtime
            .send_public_request(request)
            .await
            .map_err(Into::into)
    };
    let response = match outcome {
        Ok(response) => response,
        Err(error) => {
            return match (cached, transport_failure(&error)) {
                (Some(cached), Some(reason)) => stale(api, url, cached, reason),
                (None, Some(reason)) => Err(unavailable(reason).into()),
                // A credential decision (login required, a secret, a
                // rejected grant) is its own error; the cache does not hide it.
                _ => Err(error),
            };
        }
    };
    if graphql
        && let Some(message) = openapi::parse_document(&response.body)
            .ok()
            .as_ref()
            .and_then(graphql::introspection_refusal)
    {
        return Err(ApiError::IntrospectionRefused {
            api: api.name.clone(),
            location: url.to_string(),
            message,
        }
        .into());
    }
    if response.status == 304 {
        return match cached {
            Some(cached) => {
                tracing::debug!(url, "API description unchanged (304)");
                let loaded = cached_copy(
                    api,
                    &cached,
                    SpecOrigin::Validated {
                        url: url.to_string(),
                        fetched_at: cached.fetched_at,
                    },
                )?;
                if let Err(message) = cache
                    .update_fetched_at_async(&cached, Utc::now(), || {
                        progress!("# Waiting for another kurama process that is updating the API description cache")
                    })
                    .await
                {
                    progress!("# warning: the API description could not be cached: {message}");
                }
                Ok(loaded)
            }
            None => Err(unavailable("HTTP 304 without a cached copy".into()).into()),
        };
    }
    if response.is_success() {
        let entry = CachedSpec {
            url: url.to_string(),
            etag: response.header("etag").map(str::to_string),
            last_modified: response.header("last-modified").map(str::to_string),
            fetched_at: Utc::now(),
            body: response.body,
        };
        let spec = parse_spec(api, url, &entry.body)?;
        if let Err(message) = cache
            .store_async(&entry, || {
                progress!("# Waiting for another kurama process that is updating the API description cache")
            })
            .await
        {
            progress!("# warning: the API description could not be cached: {message}");
        }
        tracing::debug!(
            url,
            operations = spec.operations.len(),
            "API description fetched"
        );
        return Ok(LoadedSpec {
            spec,
            origin: SpecOrigin::Fetched {
                url: url.to_string(),
            },
        });
    }
    let status = match response.header("location") {
        // Redirects are never followed (the credential stays on one host).
        Some(location) if (300..400).contains(&response.status) => format!(
            "HTTP {} redirects to {location}; set {} to that URL",
            response.status,
            api.spec_key()
        ),
        _ => format!(
            "HTTP {}{}",
            response.status,
            match body_excerpt(&response.body, ERROR_EXCERPT_CHARS) {
                excerpt if excerpt.is_empty() => String::new(),
                excerpt => format!(": {excerpt}"),
            }
        ),
    };
    match cached {
        Some(cached) if response.status >= 500 => stale(api, url, cached, status),
        _ => Err(unavailable(status).into()),
    }
}

/// The reason when `error` says the API, the token endpoint or STS could
/// not be reached; `None` for anything else.
fn transport_failure(error: &anyhow::Error) -> Option<String> {
    error.chain().find_map(|cause| {
        if let Some(ApiError::RequestFailed(http)) = cause.downcast_ref::<ApiError>() {
            return Some(http.to_string());
        }
        if let Some(OAuthError::Failed(message)) = cause.downcast_ref::<OAuthError>() {
            return Some(message.clone());
        }
        if let Some(ExecutorError::StsFailed {
            kind: StsErrorKind::ServiceError,
            message,
        }) = cause.downcast_ref::<ExecutorError>()
        {
            return Some(message.clone());
        }
        None
    })
}

/// The cached copy, with a warning that says why.
fn stale(api: &ApiProfile, url: &str, cached: CachedSpec, reason: String) -> Result<LoadedSpec> {
    progress!(
        "# warning: using the cached API description of [api.{}] from {} ({reason})",
        api.name,
        cached.fetched_at.to_rfc3339()
    );
    cached_copy(
        api,
        &cached,
        SpecOrigin::Stale {
            url: url.to_string(),
            fetched_at: cached.fetched_at,
            reason,
        },
    )
}

fn cached_copy(api: &ApiProfile, cached: &CachedSpec, origin: SpecOrigin) -> Result<LoadedSpec> {
    Ok(LoadedSpec {
        spec: parse_spec(api, &cached.url, &cached.body)?,
        origin,
    })
}

pub(crate) fn parse_spec(
    api: &ApiProfile,
    location: &str,
    bytes: &[u8],
) -> Result<Arc<ApiSpec>, ApiError> {
    let invalid = |message: String| ApiError::SpecInvalid {
        api: api.name.clone(),
        key: api.spec_key(),
        location: location.to_string(),
        message,
    };
    let format = api
        .spec
        .as_ref()
        .map_or(&SpecFormat::OpenApi, |spec| &spec.format);
    let spec = openapi::parse_spec(format, bytes).map_err(invalid)?;
    Ok(Arc::new(spec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::{ApiToml, Config};
    use crate::domain::types::OAuthToken;
    use crate::ports::aws_credentials::MockAwsProfileCredentials;
    use crate::ports::http::MockHttpClient;
    use crate::ports::secret::MockSecretResolver;
    use crate::ports::token_store::MockTokenStore;
    use crate::ports::{HttpError, HttpResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PETSTORE: &str = include_str!("../../tests/fixtures/openapi/petstore.json");
    const URL: &str = "https://specs.example.com/openapi.json";

    fn api(toml: &str) -> ApiProfile {
        toml::from_str::<ApiToml>(toml)
            .unwrap()
            .typed("pets", &Default::default())
            .unwrap()
    }

    /// A runtime whose description cache lives in a directory removed with
    /// the returned guard.
    fn runtime(http: MockHttpClient) -> (ApiRuntime, tempfile::TempDir) {
        let mut config = Config::default();
        config.openapi.revalidate_after = std::time::Duration::ZERO;
        with_cache_dir(ApiRuntime::test(
            config,
            http,
            MockTokenStore::new(),
            MockSecretResolver::new(),
            MockAwsProfileCredentials::new(),
            false,
        ))
    }

    fn with_cache_dir(mut runtime: ApiRuntime) -> (ApiRuntime, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        runtime.set_spec_cache_dir(dir.path().join("openapi"));
        (runtime, dir)
    }

    fn ok(body: &str, etag: Option<&str>) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: etag
                .map(|etag| vec![("ETag".to_string(), etag.to_string())])
                .into_iter()
                .flatten()
                .chain(std::iter::once((
                    "Last-Modified".to_string(),
                    "Wed, 17 Sep 2026 00:00:00 GMT".to_string(),
                )))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn a_file_is_read_as_is_and_a_missing_or_invalid_one_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("spec.json");
        std::fs::write(&json, PETSTORE).unwrap();
        let yaml = dir.path().join("spec.yaml");
        std::fs::write(
            &yaml,
            include_str!("../../tests/fixtures/openapi/petstore.yaml"),
        )
        .unwrap();
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{\"openapi\": \"3.0.0\"}").unwrap();
        let (runtime, _cache) = runtime(MockHttpClient::new());

        let loaded = load_spec(
            &runtime,
            &api(&format!(
                "base_url = \"https://x\"\nopenapi = \"{}\"\n",
                json.display()
            )),
            false,
            "--ops",
        )
        .await
        .unwrap();
        assert_eq!(loaded.spec.title, "Petstore");
        assert_eq!(loaded.spec.operations.len(), 5);
        assert_eq!(loaded.origin, SpecOrigin::File(json));

        let loaded = load_spec(
            &runtime,
            &api(&format!(
                "base_url = \"https://x\"\nopenapi = \"{}\"\n",
                yaml.display()
            )),
            false,
            "--ops",
        )
        .await
        .unwrap();
        assert_eq!(loaded.spec.title, "Petstore YAML");
        assert_eq!(loaded.spec.operations.len(), 3);

        let error = load_spec(
            &runtime,
            &api("base_url = \"https://x\"\nopenapi = \"/nonexistent/spec.json\"\n"),
            false,
            "--ops",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ApiError>(), Some(ApiError::SpecUnavailable { location, .. }) if location == "/nonexistent/spec.json"),
            "{error:#}"
        );

        let error = load_spec(
            &runtime,
            &api(&format!(
                "base_url = \"https://x\"\nopenapi = \"{}\"\n",
                bad.display()
            )),
            false,
            "--ops",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ApiError>(), Some(ApiError::SpecInvalid { message, .. }) if message.contains("no `paths`")),
            "{error:#}"
        );

        let error = load_spec(&runtime, &api("base_url = \"https://x\"\n"), false, "--ops")
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "[api.pets] has no openapi description; --ops needs one"
        );
    }

    #[tokio::test]
    async fn a_recent_authenticated_copy_needs_no_credentials_or_http() {
        let config = Config::parse(&format!(
            r#"
[auth.pets]
kind = "oauth"
grant_type = "client_credentials"
token_url = "https://as/token"
client_id = "id"
[api.pets]
base_url = "https://specs.example.com/api"
openapi = "{URL}"
openapi_auth = true
auth = "pets"
"#
        ))
        .unwrap();
        let api = config.api_profile("pets").cloned().unwrap();
        // Empty mock expectations fail on any token, secret, AWS or HTTP access.
        let (runtime, _guard) = with_cache_dir(ApiRuntime::test(
            config,
            MockHttpClient::new(),
            MockTokenStore::new(),
            MockSecretResolver::new(),
            MockAwsProfileCredentials::new(),
            false,
        ));
        let cached = CachedSpec {
            url: URL.into(),
            etag: Some("v1".into()),
            last_modified: None,
            fetched_at: Utc::now(),
            body: PETSTORE.as_bytes().to_vec(),
        };
        SpecCache::new(runtime.spec_cache_dir().to_path_buf())
            .store(&cached)
            .unwrap();
        let loaded = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert_eq!(loaded.spec.operations.len(), 5);
        assert_eq!(
            loaded.origin,
            SpecOrigin::Cached {
                url: URL.into(),
                fetched_at: cached.fetched_at
            }
        );
    }

    #[tokio::test]
    async fn a_file_is_reread_inside_the_default_interval() {
        let (runtime, dir) = with_cache_dir(ApiRuntime::test(
            Config::default(),
            MockHttpClient::new(),
            MockTokenStore::new(),
            MockSecretResolver::new(),
            MockAwsProfileCredentials::new(),
            false,
        ));
        let path = dir.path().join("spec.json");
        let api = api(&format!(
            "base_url = \"https://x\"\nopenapi = \"{}\"\n",
            path.display()
        ));
        std::fs::write(&path, PETSTORE).unwrap();
        assert_eq!(
            load_spec(&runtime, &api, false, "--ops")
                .await
                .unwrap()
                .spec
                .title,
            "Petstore"
        );
        std::fs::write(&path, PETSTORE.replace("Petstore", "Updated Petstore")).unwrap();
        assert_eq!(
            load_spec(&runtime, &api, false, "--ops")
                .await
                .unwrap()
                .spec
                .title,
            "Updated Petstore"
        );
    }

    #[tokio::test]
    async fn a_url_is_fetched_once_then_revalidated_with_its_etag() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut http = MockHttpClient::new();
        let counter = Arc::clone(&calls);
        http.expect_send().returning(move |request| {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.url, URL);
            assert!(request.header("authorization").is_none());
            match call {
                0 => {
                    assert!(request.header("if-none-match").is_none());
                    Ok(ok(PETSTORE, Some("\"v1\"")))
                }
                1 => {
                    assert_eq!(request.header("if-none-match"), Some("\"v1\""));
                    assert_eq!(
                        request.header("if-modified-since"),
                        Some("Wed, 17 Sep 2026 00:00:00 GMT")
                    );
                    Ok(HttpResponse {
                        status: 304,
                        headers: vec![],
                        body: vec![],
                    })
                }
                _ => {
                    assert!(
                        request.header("if-none-match").is_none(),
                        "refresh is unconditional"
                    );
                    Ok(ok(PETSTORE, Some("\"v2\"")))
                }
            }
        });
        let (runtime, _cache) = runtime(http);
        let api = api(&format!("base_url = \"https://x\"\nopenapi = \"{URL}\"\n"));

        let first = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert_eq!(first.origin, SpecOrigin::Fetched { url: URL.into() });
        assert_eq!(first.spec.operations.len(), 5);
        let cached = SpecCache::new(runtime.spec_cache_dir().to_path_buf())
            .load(URL)
            .unwrap();
        assert_eq!(cached.etag.as_deref(), Some("\"v1\""));

        let second = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert_eq!(
            second.origin,
            SpecOrigin::Validated {
                url: URL.into(),
                fetched_at: cached.fetched_at
            }
        );

        let third = load_spec(&runtime, &api, true, "--ops").await.unwrap();
        assert_eq!(third.origin, SpecOrigin::Fetched { url: URL.into() });
        let cached = SpecCache::new(runtime.spec_cache_dir().to_path_buf())
            .load(URL)
            .unwrap();
        assert_eq!(cached.etag.as_deref(), Some("\"v2\""));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_delayed_304_does_not_roll_back_a_newer_cache_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("openapi");
        let cache = SpecCache::new(cache_path.clone());
        let old = CachedSpec {
            url: URL.into(),
            etag: Some("v1".into()),
            last_modified: None,
            fetched_at: Utc::now() - chrono::Duration::days(1),
            body: PETSTORE.as_bytes().to_vec(),
        };
        cache.store(&old).unwrap();
        let newer = CachedSpec {
            url: URL.into(),
            etag: Some("v2".into()),
            last_modified: None,
            fetched_at: Utc::now(),
            body: PETSTORE.replace("Petstore", "Newer Petstore").into_bytes(),
        };
        let replacement = newer.clone();
        let destination = cache_path.clone();
        let mut http = MockHttpClient::new();
        http.expect_send().times(1).returning(move |request| {
            assert_eq!(request.header("if-none-match"), Some("v1"));
            // A concurrent unconditional fetch finishes before the older 304 arrives.
            SpecCache::new(destination.clone())
                .store(&replacement)
                .unwrap();
            Ok(HttpResponse {
                status: 304,
                headers: vec![],
                body: vec![],
            })
        });
        let mut runtime = ApiRuntime::test(
            Config::default(),
            http,
            MockTokenStore::new(),
            MockSecretResolver::new(),
            MockAwsProfileCredentials::new(),
            false,
        );
        runtime.set_spec_cache_dir(cache_path);
        let api = api(&format!("base_url = \"https://x\"\nopenapi = \"{URL}\"\n"));
        load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert_eq!(cache.load(URL).unwrap(), newer);
        let loaded = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert_eq!(loaded.spec.title, "Newer Petstore");
        assert!(matches!(loaded.origin, SpecOrigin::Cached { .. }));
    }

    /// A dry run starts no credential source, so a description behind
    /// `openapi_auth` is not requested: the cached copy stands unchecked,
    /// and without one the load fails before any request.
    #[tokio::test]
    async fn a_dry_run_reads_an_authenticated_description_from_the_cache_only() {
        let mut http = MockHttpClient::new();
        http.expect_send().never();
        let (runtime, _cache) = runtime(http);
        let api = api(&format!(
            "base_url = \"https://specs.example.com\"\nopenapi = \"{URL}\"\nopenapi_auth = true\n"
        ));

        let load = || {
            load_spec_with(
                &runtime,
                &api,
                false,
                "--ops",
                SpecAccess::WithoutCredential,
            )
        };
        let error = load().await.unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<ApiError>(),
                Some(ApiError::SpecNeedsCredential { .. })
            ),
            "{error:#}"
        );
        let cached = CachedSpec {
            url: URL.into(),
            etag: None,
            last_modified: None,
            fetched_at: Utc::now() - chrono::Duration::days(1),
            body: PETSTORE.as_bytes().to_vec(),
        };
        SpecCache::new(runtime.spec_cache_dir().to_path_buf())
            .store(&cached)
            .unwrap();
        let loaded = load().await.unwrap();
        assert_eq!(
            loaded.origin,
            SpecOrigin::Unchecked {
                url: URL.into(),
                fetched_at: cached.fetched_at
            }
        );
        assert_eq!(loaded.spec.operations.len(), 5);
    }

    #[tokio::test]
    async fn an_unreachable_server_falls_back_to_the_cache_or_fails() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut http = MockHttpClient::new();
        let counter = Arc::clone(&calls);
        http.expect_send()
            .returning(move |_| match counter.fetch_add(1, Ordering::SeqCst) {
                0 => Err(HttpError::Connect("connection refused".into())),
                1 => Ok(ok(PETSTORE, None)),
                2 => Err(HttpError::Timeout { seconds: 1 }),
                3 => Ok(HttpResponse {
                    status: 503,
                    headers: vec![],
                    body: b"down".to_vec(),
                }),
                _ => Ok(HttpResponse {
                    status: 404,
                    headers: vec![],
                    body: b"{\"message\":\"gone\"}".to_vec(),
                }),
            });
        let (runtime, _cache) = runtime(http);
        let api = api(&format!("base_url = \"https://x\"\nopenapi = \"{URL}\"\n"));

        let error = load_spec(&runtime, &api, false, "--ops").await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ApiError>(), Some(ApiError::SpecUnavailable { message, .. }) if message.contains("connection refused")),
            "{error:#}"
        );
        load_spec(&runtime, &api, false, "--ops").await.unwrap();
        let stale = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert!(
            matches!(&stale.origin, SpecOrigin::Stale { reason, .. } if reason == "request timed out after 1s"),
            "the reason is stated once: {:?}",
            stale.origin
        );
        let stale = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert!(
            matches!(&stale.origin, SpecOrigin::Stale { reason, .. } if reason == "HTTP 503: down"),
            "{:?}",
            stale.origin
        );
        let error = load_spec(&runtime, &api, false, "--ops").await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ApiError>(), Some(ApiError::SpecUnavailable { message, .. }) if message == "HTTP 404: {\"message\":\"gone\"}"),
            "a 4xx is an error even with a cached copy: {error:#}"
        );
    }

    #[tokio::test]
    async fn a_redirect_names_the_location_to_configure() {
        let mut http = MockHttpClient::new();
        http.expect_send().times(1).returning(|_| {
            Ok(HttpResponse {
                status: 301,
                headers: vec![(
                    "Location".into(),
                    "https://specs.example.com/v2/openapi.json".into(),
                )],
                body: vec![],
            })
        });
        let (runtime, _cache) = runtime(http);
        let api = api(&format!("base_url = \"https://x\"\nopenapi = \"{URL}\"\n"));
        let error = load_spec(&runtime, &api, false, "--ops").await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ApiError>(), Some(ApiError::SpecUnavailable { message, .. }) if message == "HTTP 301 redirects to https://specs.example.com/v2/openapi.json; set openapi to that URL"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn openapi_auth_uses_the_cache_when_unreachable_but_reports_a_missing_login() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut http = MockHttpClient::new();
        let counter = Arc::clone(&calls);
        http.expect_send().returning(move |request| {
            assert_eq!(request.header("authorization"), Some("Bearer stored"));
            match counter.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(ok(PETSTORE, Some("\"v1\""))),
                _ => Err(HttpError::Connect("connection refused".into())),
            }
        });
        let loads = Arc::new(AtomicUsize::new(0));
        let mut store = MockTokenStore::new();
        let load_counter = Arc::clone(&loads);
        store.expect_load().returning(move |_| {
            // The third load finds no token: a person has to log in.
            Ok((load_counter.fetch_add(1, Ordering::SeqCst) < 2)
                .then(|| OAuthToken::bearer("stored")))
        });
        let mut config = Config::default();
        config.openapi.revalidate_after = std::time::Duration::ZERO;
        config.auth.insert(
            "pets".into(),
            toml::from_str(
                "kind = \"oauth\"\ngrant_type = \"authorization_code\"\nauth_url = \"https://as/auth\"\ntoken_url = \"https://as/token\"\nclient_id = \"id\"\n",
            )
            .unwrap(),
        );
        config.api.insert(
            "pets".into(),
            toml::from_str(&format!(
                "base_url = \"https://specs.example.com/api\"\nopenapi = \"{URL}\"\nopenapi_auth = true\nauth = \"pets\"\n"
            ))
            .unwrap(),
        );
        let api = config.api_profile("pets").cloned().unwrap();
        let (runtime, _cache) = with_cache_dir(ApiRuntime::test(
            config,
            http,
            store,
            MockSecretResolver::new(),
            MockAwsProfileCredentials::new(),
            false,
        ));
        load_spec(&runtime, &api, false, "--ops").await.unwrap();
        let stale = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert!(
            matches!(&stale.origin, SpecOrigin::Stale { reason, .. } if reason == "could not connect: connection refused"),
            "{:?}",
            stale.origin
        );
        let error = load_spec(&runtime, &api, false, "--ops").await.unwrap_err();
        assert!(
            error.chain().any(|cause| matches!(
                cause.downcast_ref::<OAuthError>(),
                Some(OAuthError::LoginRequired { .. })
            )),
            "the cache does not hide a login that is needed: {error:#}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// A GraphQL schema is introspected: a POST of the introspection query
    /// with the API's credential, cached like a description; an answer that
    /// is only `errors` is a refusal and is not cached.
    #[tokio::test]
    async fn graphql_is_introspected_with_the_credential_and_a_refusal_is_not_cached() {
        const LINEAR: &str = include_str!("../../tests/fixtures/openapi/graphql-linear.json");
        const REFUSED: &str = include_str!("../../tests/fixtures/openapi/graphql-refused.json");
        const ENDPOINT: &str = "https://api.linear.app/graphql";
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let mut http = MockHttpClient::new();
        http.expect_send().returning(move |request| {
            assert_eq!(request.method, "POST");
            assert_eq!(request.url, ENDPOINT);
            assert_eq!(request.header("authorization"), Some("Bearer stored"));
            assert_eq!(request.header("content-type"), Some("application/json"));
            assert_eq!(
                request.body.as_deref(),
                Some(crate::domain::functions::graphql::introspection_request_body().as_slice())
            );
            Ok(match counter.fetch_add(1, Ordering::SeqCst) {
                0 => ok(REFUSED, None),
                _ => ok(LINEAR, None),
            })
        });
        let mut store = MockTokenStore::new();
        store
            .expect_load()
            .returning(|_| Ok(Some(OAuthToken::bearer("stored"))));
        let config = Config::parse(
            r#"
[auth.linear]
kind = "oauth"
grant_type = "client_credentials"
token_url = "https://as/token"
client_id = "id"
[api.linear]
base_url = "https://api.linear.app"
graphql = "/graphql"
"#,
        )
        .unwrap();
        let api = config.api_profile("linear").cloned().unwrap();
        let (runtime, _cache) = with_cache_dir(ApiRuntime::test(
            config,
            http,
            store,
            MockSecretResolver::new(),
            MockAwsProfileCredentials::new(),
            false,
        ));
        let error = load_spec(&runtime, &api, false, "--ops").await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ApiError>(), Some(ApiError::IntrospectionRefused { message, .. }) if message.contains("introspection is not allowed")),
            "{error:#}"
        );
        assert!(
            SpecCache::new(runtime.spec_cache_dir().to_path_buf())
                .load(ENDPOINT)
                .is_none(),
            "a refusal is not cached"
        );
        let loaded = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert_eq!(
            loaded.origin,
            SpecOrigin::Fetched {
                url: ENDPOINT.into()
            }
        );
        assert_eq!(loaded.spec.operations[0].id, "query.attachmentIssue");
        let again = load_spec(&runtime, &api, false, "--ops").await.unwrap();
        assert!(
            matches!(again.origin, SpecOrigin::Cached { .. }),
            "{:?}",
            again.origin
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn openapi_auth_fetches_with_the_bearer_token_and_a_bad_document_is_not_cached() {
        let mut http = MockHttpClient::new();
        http.expect_send()
            .times(1)
            .withf(|request| request.header("authorization") == Some("Bearer stored"))
            .returning(|_| Ok(ok("openapi: 3.0.0\ninfo: {title: x}\n", Some("\"e\""))));
        let mut store = MockTokenStore::new();
        store
            .expect_load()
            .returning(|_| Ok(Some(OAuthToken::bearer("stored"))));
        let mut config = Config::default();
        config.auth.insert(
            "pets".into(),
            toml::from_str(
                "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://as/token\"\nclient_id = \"id\"\n",
            )
            .unwrap(),
        );
        config.api.insert(
            "pets".into(),
            toml::from_str(&format!(
                "base_url = \"https://specs.example.com/api\"\nopenapi = \"{URL}\"\nopenapi_auth = true\nauth = \"pets\"\n"
            ))
            .unwrap(),
        );
        let api = config.api_profile("pets").cloned().unwrap();
        let (runtime, _cache) = with_cache_dir(ApiRuntime::test(
            config,
            http,
            store,
            MockSecretResolver::new(),
            MockAwsProfileCredentials::new(),
            false,
        ));
        let error = load_spec(&runtime, &api, false, "--ops").await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ApiError>(), Some(ApiError::SpecInvalid { message, .. }) if message.contains("no `paths`")),
            "{error:#}"
        );
        assert!(
            SpecCache::new(runtime.spec_cache_dir().to_path_buf())
                .load(URL)
                .is_none()
        );
    }
}
