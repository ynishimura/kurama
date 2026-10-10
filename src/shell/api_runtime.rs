//! `ApiRuntime`: the dependencies of every command that uses an `[auth.*]`
//! source or an `[api.*]` profile (`login`, `logout`, `token`, `status`,
//! `env` / `exec` on an auth source, and `api`). The explorer plugs
//! into the same container.
//!
//! `ensure_token` runs the OAuth token workflow under a per-source lock, so
//! parallel kurama processes refresh a token once. `call` sends a request
//! with the bearer token and retries once after a 401, or, for an API with
//! `aws_profile`, signs it (SigV4) with the profile's role credentials from
//! the shared AssumeRole path and sends it once. A `kind = "token"` source
//! reads its credential from the secret store its reference names and sends
//! it in the header the source configures, once: nothing was cached and a
//! retry would read the same value again.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::adapters::config::{ApiProfile, Config};
use crate::adapters::error::CoreError;
use crate::adapters::http::ReqwestHttpClient;
use crate::adapters::profile_lock::ProfileLock;
use crate::adapters::secret_resolver::ConfiguredSecrets;
use crate::adapters::sigv4::sign_request;
use crate::adapters::token_store::create_token_store;
use crate::adapters::utils::path;
use crate::console::progress;
use crate::domain::functions::oauth::base64url;
use crate::domain::functions::signing_target::{SigningTarget, resolve_signing_target};
use crate::domain::types::{
    Credentials, OAuthClientConfig, OAuthToken, Profile, RequestAuth, Secret, SecretsSourceConfig,
    SourceCredential, TokenSourceConfig,
};
use crate::ports::{
    AwsProfileCredentials, HttpClient, HttpError, HttpRequest, HttpResponse, SecretResolver,
    TokenStore,
};
use crate::shell::api_error::ApiError;
use crate::shell::aws_profile_credentials::AssumedRoles;
use crate::shell::oauth_executor::{OAuthError, execute_token_workflow};
use crate::workflows::oauth_token::{TokenInput, TokenMode, TokenOutput};

/// What `kurama api` passes on the command line.
#[derive(Debug, Clone, Copy)]
pub struct ApiRuntimeOptions {
    pub timeout: Duration,
    pub accept_invalid_certs: bool,
    /// Open the browser for the authorization code and device grants;
    /// `false` prints the URL instead (`login --no-browser`).
    pub open_browser: bool,
    /// Say each secret store read on stderr (`kurama api -v`).
    pub report_secret_reads: bool,
}

impl Default for ApiRuntimeOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(
                crate::adapters::config::constants::oauth::HTTP_TIMEOUT_SECS,
            ),
            accept_invalid_certs: false,
            open_browser: true,
            report_secret_reads: false,
        }
    }
}

pub struct ApiRuntime {
    config: Arc<Config>,
    /// API requests: the only client `kurama api -k` applies to.
    http: Arc<dyn HttpClient>,
    /// Discovery and token endpoint requests: certificates are always
    /// verified, whatever `-k` says about the API.
    token_http: Arc<dyn HttpClient>,
    pub(crate) token_store: Arc<dyn TokenStore>,
    secrets: Arc<dyn SecretResolver>,
    /// The role credentials an `aws_profile` API is signed with.
    aws: Arc<dyn AwsProfileCredentials>,
    /// stdin and stderr are terminals: a person can use a browser or type a
    /// code. Without it a grant that needs a person fails with exit code 3.
    interactive: bool,
    open_browser: bool,
    /// Directory of the per-source lock files.
    locks_dir: PathBuf,
    /// Directory of the cached OpenAPI descriptions (`spec_loader`).
    spec_cache_dir: PathBuf,
}

impl ApiRuntime {
    pub fn from_config(config: Arc<Config>, options: ApiRuntimeOptions) -> Result<Self> {
        let http = ReqwestHttpClient::new(options.timeout, options.accept_invalid_certs)
            .map_err(ApiError::RequestFailed)?;
        let token_http =
            ReqwestHttpClient::new(options.timeout, false).map_err(ApiError::RequestFailed)?;
        let cache_dir = path::get_home_dir()?.join(".cache").join("kurama");
        // One AssumeRole cache for both: an API signed with `aws_profile` and
        // an `aws-*://` client secret that names the same profile assume it
        // once between them.
        let aws: Arc<dyn AwsProfileCredentials> =
            Arc::new(AssumedRoles::of_config(Arc::clone(&config)));
        let secrets = ConfiguredSecrets::new(&config.onepassword, Arc::clone(&aws))
            .reporting_reads(options.report_secret_reads);
        Ok(Self {
            aws,
            config,
            http: Arc::new(http),
            token_http: Arc::new(token_http),
            token_store: create_token_store(),
            secrets: Arc::new(secrets),
            interactive: crate::shell::tui::terminal::can_prompt_a_person(),
            open_browser: options.open_browser,
            locks_dir: cache_dir.join("locks"),
            spec_cache_dir: crate::adapters::openapi::default_cache_dir()?,
        })
    }

    /// A runtime with mock dependencies and a temporary lock directory; the
    /// one HTTP mock answers API and token requests alike.
    #[cfg(test)]
    pub fn test(
        config: Config,
        http: impl HttpClient + 'static,
        token_store: impl TokenStore + 'static,
        secrets: impl SecretResolver + 'static,
        aws: impl AwsProfileCredentials + 'static,
        interactive: bool,
    ) -> Self {
        let http: Arc<dyn HttpClient> = Arc::new(http);
        Self {
            config: Arc::new(config),
            token_http: Arc::clone(&http),
            http,
            token_store: Arc::new(token_store),
            secrets: Arc::new(secrets),
            aws: Arc::new(aws),
            interactive,
            open_browser: false,
            locks_dir: std::env::temp_dir()
                .join(format!("kurama-test-locks-{}", std::process::id())),
            // A test that stores a description points this at its own
            // `tempfile::tempdir()`.
            spec_cache_dir: std::env::temp_dir()
                .join(format!("kurama-test-openapi-{}", std::process::id())),
        }
    }

    /// A usable token for `client`: the stored one, a refreshed one, or a
    /// new one from the grant. Load, refresh and store are serialized per
    /// source across kurama processes.
    pub async fn ensure_token(
        &self,
        client: &OAuthClientConfig,
        mode: TokenMode,
    ) -> Result<TokenOutput> {
        let locks_dir = self.locks_dir.clone();
        let name = client.name.clone();
        let lock = tokio::task::spawn_blocking(move || {
            ProfileLock::acquire(&locks_dir, &name, || {
                progress!(
                    "# Waiting for another kurama process that is getting a token for '{name}'"
                )
            })
        })
        .await
        .context("Failed to wait for the token lock")?
        .with_context(|| format!("Failed to lock the token store for '{}'", client.name))?;
        let input = TokenInput {
            client: client.clone(),
            mode,
            interactive: self.interactive,
            now: Utc::now(),
            pkce_verifier: random_value(),
            state: random_value(),
        };
        let output = execute_token_workflow(self, input).await;
        drop(lock);
        output
    }

    /// Resolve a client secret through the runtime's secret boundary.
    pub(crate) async fn resolve_secret(
        &self,
        secret: &crate::domain::types::SecretRef,
    ) -> std::result::Result<Secret, crate::ports::SecretError> {
        self.secrets.resolve(secret).await
    }

    /// Send a token/discovery request with the certificate-verifying client.
    pub(crate) async fn send_token_request(
        &self,
        request: HttpRequest,
    ) -> std::result::Result<HttpResponse, HttpError> {
        self.token_http.send(request).await
    }

    /// Send an unauthenticated API request. This is used for public OpenAPI
    /// descriptions and deliberately does not add a bearer token or a
    /// signature.
    pub(crate) async fn send_public_request(
        &self,
        request: HttpRequest,
    ) -> std::result::Result<HttpResponse, ApiError> {
        self.http
            .send(request)
            .await
            .map_err(ApiError::RequestFailed)
    }

    pub(crate) fn should_open_browser(&self) -> bool {
        self.open_browser
    }

    pub(crate) fn set_interactive(&mut self, interactive: bool) {
        self.interactive = interactive;
    }

    pub(crate) fn spec_revalidation_interval(&self) -> Duration {
        self.config.openapi.revalidate_after
    }

    pub(crate) fn spec_cache_dir(&self) -> &std::path::Path {
        &self.spec_cache_dir
    }

    #[cfg(test)]
    pub(crate) fn set_spec_cache_dir(&mut self, path: PathBuf) {
        self.spec_cache_dir = path;
    }

    #[cfg(test)]
    pub(crate) fn set_token_http(&mut self, http: Arc<dyn HttpClient>) {
        self.token_http = http;
    }

    /// The `[auth.*]` source an API profile refers to. An `ApiProfile`
    /// never names a `secrets` source -- loading it refuses one -- so that
    /// kind is the same configuration error here.
    pub fn auth_source(&self, api: &ApiProfile) -> Result<Option<RequestAuth>> {
        let Some(name) = &api.auth else {
            return Ok(None);
        };
        let source = self.config.auth_source(name).cloned().ok_or_else(|| {
            CoreError::config(format!(
                "[api.{}] auth = \"{name}\" names no [auth.{name}] section",
                api.name
            ))
        })?;
        source.request_auth().map(Some).map_err(|_| {
            CoreError::config(format!(
                "[api.{}]: [auth.{name}] is kind = \"secrets\", which authenticates no request",
                api.name
            ))
            .into()
        })
    }

    /// How `request` to `api` would be authenticated, from the
    /// configuration and the AWS profile file alone: nothing is asked of a
    /// token store, a secret store or STS to decide it.
    pub async fn credential(
        &self,
        api: &ApiProfile,
        request: &HttpRequest,
    ) -> Result<ApiCredential> {
        if let Some(profile_name) = &api.aws_profile {
            let (profile, target) = self.signing_target(api, profile_name, request).await?;
            return Ok(ApiCredential::SigV4 { profile, target });
        }
        Ok(match self.auth_source(api)? {
            None => ApiCredential::None,
            Some(RequestAuth::OAuth(client)) => ApiCredential::OAuth(client),
            Some(RequestAuth::Token(source)) => ApiCredential::Token(source),
        })
    }

    /// An AWS profile from the profile file, read without any credential.
    pub(crate) async fn load_aws_profile(&self, name: &str) -> Result<Profile> {
        self.aws.load_profile(name).await
    }

    /// A usable credential for `source`, whichever kind it is: the OAuth
    /// token workflow, or the value the reference of a `kind = "token"`
    /// source names. Nothing is stored for the second: the value is read
    /// again next time, and there is no token store entry to go stale.
    pub async fn ensure_credential(
        &self,
        source: &RequestAuth,
        mode: TokenMode,
    ) -> Result<SourceCredential> {
        match source {
            RequestAuth::OAuth(client) => self
                .ensure_token(client, mode)
                .await
                .map(|output| SourceCredential::OAuth(output.token)),
            RequestAuth::Token(issued) => {
                Ok(SourceCredential::Issued(self.issued_value(issued).await?))
            }
        }
    }

    /// Every variable of a `kind = "secrets"` source with its value, in
    /// name order, read when this runs: nothing is stored, so a one-time
    /// password is as fresh as the command it is for. The first reference
    /// that cannot be read fails the whole, so no command starts with part
    /// of them; fields of one 1Password item, or keys of one managed secret,
    /// are one read through the process's secret cache.
    pub async fn resolve_secrets(
        &self,
        source: &SecretsSourceConfig,
    ) -> Result<Vec<(String, Secret)>> {
        progress!(
            "# Reading the secrets of '{}' from their secret stores",
            source.name
        );
        let mut values = Vec::with_capacity(source.env.len());
        for (var, reference) in &source.env {
            let value = self
                .resolve_secret(reference)
                .await
                .with_context(|| format!("Failed to read {var} of '{}'", source.name))?;
            values.push((var.clone(), value));
        }
        Ok(values)
    }

    /// The value behind a `kind = "token"` source's reference.
    async fn issued_value(&self, source: &TokenSourceConfig) -> Result<Secret> {
        progress!(
            "# Reading the credential of '{}' from its secret store",
            source.name
        );
        Ok(self.resolve_secret(&source.token).await?)
    }

    /// Send `request` to the API with its credential. An `aws_profile` API
    /// gets a SigV4 signature with the profile's role credentials and is
    /// sent once: the credentials are fresh, so a rejection is the
    /// caller's to report, and so is a `kind = "token"` source's, whose
    /// value was just read. With an OAuth source, a 401 treats the token as
    /// rejected: it is refreshed or replaced once and the request is sent
    /// again; a second failure is the caller's to report. A token another
    /// process stored meanwhile is sent as is, so parallel calls that were
    /// rejected together refresh once.
    pub async fn call(&self, api: &ApiProfile, request: HttpRequest) -> Result<HttpResponse> {
        let client = match self.credential(api, &request).await? {
            ApiCredential::None => return self.send(request).await,
            ApiCredential::SigV4 { profile, target } => {
                progress!(
                    "# Signing with AWS profile '{}' for {} in {}",
                    profile.name(),
                    target.service,
                    target.region
                );
                let credentials = self.aws.assume_role(&profile).await?;
                let signed = sign_request(request, &credentials, &target, Utc::now())
                    .map_err(signing_failed)?;
                return self.send(signed).await;
            }
            // Nothing was cached and nothing can be refreshed: reading the
            // reference again after a 401 would send the same value twice.
            ApiCredential::Token(source) => {
                let value = self.issued_value(&source).await?;
                return self.send(source.apply(request, value.expose())).await;
            }
            ApiCredential::OAuth(client) => client,
        };
        let token = self.ensure_token(&client, TokenMode::Reuse).await?;
        let response = self
            .send(with_bearer(request.clone(), &token.token))
            .await?;
        if response.status != 401 {
            return Ok(response);
        }
        progress!("# The API rejected the token (HTTP 401); getting a new one and retrying once");
        let retry = match self.ensure_token(&client, TokenMode::Reuse).await {
            Ok(current) if current.token == token.token => {
                self.ensure_token(&client, TokenMode::ForceRefresh).await
            }
            other => other,
        };
        if retry.as_ref().is_err_and(is_login_required) {
            // The rejected token is dead; `status` must not call it valid.
            let _ = self.token_store.remove(&client.name).await;
        }
        let token = retry?;
        self.send(with_bearer(request, &token.token)).await
    }

    /// A preview signed separately from `call`, with placeholder credentials:
    /// `<token>` as the bearer token, or a signature with placeholder keys.
    /// `--dry-run` and `-v` print it with `mask_secret_headers`. Nothing is
    /// requested from a token store, an authorization server or STS.
    pub fn preview(credential: &ApiCredential, request: HttpRequest) -> Result<HttpRequest> {
        Ok(match credential {
            ApiCredential::SigV4 { target, .. } => {
                let placeholder = Credentials::new(
                    "<access-key-id>".into(),
                    "<secret-access-key>".into(),
                    Some("<session-token>".into()),
                    None,
                );
                sign_request(request, &placeholder, target, Utc::now()).map_err(signing_failed)?
            }
            ApiCredential::Token(source) => source.apply(request, "<token>"),
            ApiCredential::OAuth(_) => with_bearer(request, &OAuthToken::bearer("<token>")),
            ApiCredential::None => request,
        })
    }

    /// The AWS profile and what the request is signed for. Decided before
    /// any credential is requested, so an open question costs no STS call.
    async fn signing_target(
        &self,
        api: &ApiProfile,
        profile_name: &str,
        request: &HttpRequest,
    ) -> Result<(Profile, SigningTarget)> {
        let profile = self.aws.load_profile(profile_name).await?;
        let host = url::Url::parse(&request.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        let target = resolve_signing_target(&api.signing, &host, profile.region_raw()).map_err(
            |message| ApiError::SigningTargetRequired {
                api: api.name.clone(),
                message,
            },
        )?;
        Ok((profile, target))
    }

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
        Ok(self
            .http
            .send(request)
            .await
            .map_err(ApiError::RequestFailed)?)
    }
}

/// How a request to an `[api.*]` is authenticated. `call`, `preview`, the
/// masked headers and the `--dry-run --json` plan each match on it, so a new
/// kind is a build error in every one of them.
pub enum ApiCredential {
    /// No credential: the request goes as built.
    None,
    /// The bearer token of an OAuth source.
    OAuth(OAuthClientConfig),
    /// The credential a `kind = "token"` source's reference names, in the
    /// header it configures.
    Token(TokenSourceConfig),
    /// A SigV4 signature with the role credentials of an AWS profile.
    SigV4 {
        profile: Profile,
        target: SigningTarget,
    },
}

impl ApiCredential {
    /// The header this credential goes in on top of the ones that always
    /// carry one; `--dry-run` and `-v` mask it.
    pub fn extra_secret_headers(&self) -> Vec<&str> {
        match self {
            Self::Token(source) => source.header_name().into_iter().collect(),
            Self::None | Self::OAuth(_) | Self::SigV4 { .. } => Vec::new(),
        }
    }
}

fn signing_failed(message: String) -> ApiError {
    ApiError::RequestFailed(HttpError::Other(format!("SigV4 signing: {message}")))
}

fn is_login_required(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<OAuthError>(),
            Some(OAuthError::LoginRequired { .. })
        )
    })
}

/// The request with the bearer token as its only `Authorization` header.
pub fn with_bearer(mut request: HttpRequest, token: &OAuthToken) -> HttpRequest {
    request
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
    request
        .headers
        .push(("Authorization".to_string(), token.authorization_header()));
    request
}

/// 32 random bytes, URL-safe base64: a PKCE verifier or a `state`.
fn random_value() -> String {
    base64url(&rand::random::<[u8; 32]>())
}

#[cfg(test)]
#[path = "api_runtime_tests.rs"]
mod tests;
