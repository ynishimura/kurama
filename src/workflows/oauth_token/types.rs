//! Pure data for the OAuth token workflow.

use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::domain::types::{
    OAuthClientConfig, OAuthEndpoints, OAuthToken, SecretFailure, SecretRef,
};
use crate::workflows::common::LogLevel;

/// How the stored token is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenMode {
    /// Keep a usable stored token; refresh or run the grant otherwise.
    Reuse,
    /// Ignore the stored token and run the grant (`login --force`).
    ForceGrant,
    /// The API rejected the stored token: refresh it when a refresh token
    /// exists, otherwise run the grant.
    ForceRefresh,
}

#[derive(Clone)]
pub struct TokenInput {
    pub client: OAuthClientConfig,
    pub mode: TokenMode,
    /// A person can use a browser or type a code.
    pub interactive: bool,
    pub now: DateTime<Utc>,
    /// PKCE verifier, a random value the shell generated.
    pub pkce_verifier: String,
    /// The `state` of the authorization request, a random value the shell generated.
    pub state: String,
}

impl std::fmt::Debug for TokenInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenInput")
            .field("client", &self.client)
            .field("mode", &self.mode)
            .field("interactive", &self.interactive)
            .field("now", &self.now)
            .field("pkce_verifier", &"[REDACTED]")
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenOutput {
    pub token: OAuthToken,
    /// A usable stored token was kept; nothing was requested.
    pub reused: bool,
    /// Why the token could not be cached, when it could not. The token is
    /// still usable, but `login` exists to fill the cache, so it must know.
    pub cache_error: Option<String>,
}

/// Form fields that are credentials and never appear in `Debug`.
const SECRET_FORM_FIELDS: [&str; 5] = [
    "client_secret",
    "code",
    "refresh_token",
    "code_verifier",
    "device_code",
];

#[derive(Clone)]
pub enum TokenEffect {
    LoadToken {
        key: String,
    },
    /// GET a JSON document (OpenID Connect discovery).
    Get {
        url: String,
    },
    /// POST a form to the token or device authorization endpoint.
    PostForm {
        url: String,
        form: Vec<(String, String)>,
    },
    ResolveSecret {
        secret: SecretRef,
    },
    /// Open a loopback listener; the shell answers with the redirect URI.
    ListenForCallback {
        port: Option<u16>,
    },
    /// Open the authorization URL in the browser, or show it.
    Authorize {
        url: String,
    },
    /// Wait for the browser to be redirected to the loopback listener; the
    /// workflow checks `state` when the redirect arrives.
    WaitForCallback,
    /// Show the person where to enter the device code.
    ShowDeviceCode {
        verification_uri: String,
        user_code: String,
        verification_uri_complete: Option<String>,
    },
    Sleep {
        seconds: u64,
    },
    /// Store the token; the executor fails the workflow when it cannot.
    StoreToken {
        key: String,
        token: OAuthToken,
    },
    Log {
        level: LogLevel,
        message: String,
    },
}

impl std::fmt::Debug for TokenEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadToken { key } => f.debug_struct("LoadToken").field("key", key).finish(),
            Self::Get { url } => f.debug_struct("Get").field("url", url).finish(),
            Self::PostForm { url, form } => {
                let masked: Vec<(&str, &str)> = form
                    .iter()
                    .map(|(name, value)| {
                        if SECRET_FORM_FIELDS.contains(&name.as_str()) {
                            (name.as_str(), "[REDACTED]")
                        } else {
                            (name.as_str(), value.as_str())
                        }
                    })
                    .collect();
                f.debug_struct("PostForm")
                    .field("url", url)
                    .field("form", &masked)
                    .finish()
            }
            Self::ResolveSecret { secret } => f
                .debug_struct("ResolveSecret")
                .field("secret", secret)
                .finish(),
            Self::ListenForCallback { port } => f
                .debug_struct("ListenForCallback")
                .field("port", port)
                .finish(),
            Self::Authorize { url } => f.debug_struct("Authorize").field("url", url).finish(),
            Self::WaitForCallback => f.write_str("WaitForCallback"),
            Self::ShowDeviceCode {
                verification_uri,
                user_code,
                verification_uri_complete,
            } => f
                .debug_struct("ShowDeviceCode")
                .field("verification_uri", verification_uri)
                .field("user_code", user_code)
                .field("verification_uri_complete", verification_uri_complete)
                .finish(),
            Self::Sleep { seconds } => f.debug_struct("Sleep").field("seconds", seconds).finish(),
            Self::StoreToken { key, token } => f
                .debug_struct("StoreToken")
                .field("key", key)
                .field("token", token)
                .finish(),
            Self::Log { level, message } => f
                .debug_struct("Log")
                .field("level", level)
                .field("message", message)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub enum TokenEvent {
    Start {
        input: TokenInput,
    },
    /// The stored token, expired or not; `None` when there is none or the
    /// store could not be read.
    TokenLoaded {
        token: Option<OAuthToken>,
    },
    /// The answer to `Get` or `PostForm`, whatever its status; `expires_in`
    /// counts from `received_at`, and the device code's deadline from
    /// `received_instant`, the same moment on the monotonic clock.
    Responded {
        status: u16,
        body: Vec<u8>,
        received_at: DateTime<Utc>,
        received_instant: Instant,
    },
    RequestFailed {
        error: String,
    },
    SecretResolved {
        secret: String,
    },
    SecretFailed {
        failure: SecretFailure,
        error: String,
    },
    CallbackListening {
        redirect_uri: String,
    },
    CallbackListenFailed {
        error: String,
    },
    /// The query string of the redirect the browser was sent to.
    CallbackReceived {
        query: String,
    },
    CallbackFailed {
        error: String,
    },
    Slept,
}

impl std::fmt::Debug for TokenEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start { input } => f.debug_struct("Start").field("input", input).finish(),
            Self::TokenLoaded { token } => {
                f.debug_struct("TokenLoaded").field("token", token).finish()
            }
            Self::Responded {
                status,
                body,
                received_at,
                ..
            } => f
                .debug_struct("Responded")
                .field("status", status)
                .field("body_bytes", &body.len())
                .field("received_at", received_at)
                .finish(),
            Self::RequestFailed { error } => f
                .debug_struct("RequestFailed")
                .field("error", error)
                .finish(),
            Self::SecretResolved { .. } => f.write_str("SecretResolved([REDACTED])"),
            Self::SecretFailed { failure, error } => f
                .debug_struct("SecretFailed")
                .field("failure", failure)
                .field("error", error)
                .finish(),
            Self::CallbackListening { redirect_uri } => f
                .debug_struct("CallbackListening")
                .field("redirect_uri", redirect_uri)
                .finish(),
            Self::CallbackListenFailed { error } => f
                .debug_struct("CallbackListenFailed")
                .field("error", error)
                .finish(),
            Self::CallbackReceived { .. } => f.write_str("CallbackReceived([REDACTED])"),
            Self::CallbackFailed { error } => f
                .debug_struct("CallbackFailed")
                .field("error", error)
                .finish(),
            Self::Slept => f.write_str("Slept"),
        }
    }
}

/// Why the workflow stopped; the shell maps it to an error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenFailure {
    /// No usable token, nothing to refresh, and the grant needs a person
    /// but there is no terminal.
    LoginRequired,
    /// The authorization server rejected the request.
    Rejected,
    /// The server could not be reached or answered nonsense.
    Transport,
    /// The client secret could not be resolved; the kind decides the code.
    Secret(SecretFailure),
    /// The endpoints do not cover the grant.
    Config,
}

/// What to do once the endpoints and the secret are known.
#[derive(Debug, Clone)]
pub enum Plan {
    Refresh(OAuthToken),
    Grant,
}

/// Everything the states share after `Start`.
#[derive(Clone)]
pub struct Session {
    pub input: TokenInput,
    pub endpoints: Option<OAuthEndpoints>,
    /// The resolved client secret, when the client has one.
    pub secret: Option<String>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("input", &self.input)
            .field("endpoints", &self.endpoints)
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Where the device flow stands between polls.
#[derive(Clone)]
pub struct DevicePolling {
    pub device_code: String,
    pub interval: u64,
    /// When the device code expires on the monotonic clock: no poll is made
    /// at or after it.
    pub deadline: Instant,
}

impl std::fmt::Debug for DevicePolling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevicePolling")
            .field("device_code", &"[REDACTED]")
            .field("interval", &self.interval)
            .field("deadline", &self.deadline)
            .finish()
    }
}

#[derive(Debug, Clone, Default)]
pub enum TokenState {
    #[default]
    Initial,
    LoadingToken(Session),
    Discovering {
        session: Session,
        plan: Plan,
    },
    ResolvingSecret {
        session: Session,
        plan: Plan,
    },
    Refreshing {
        session: Session,
        previous: OAuthToken,
    },
    ListeningForCallback(Session),
    WaitingForCallback {
        session: Session,
        redirect_uri: String,
    },
    ExchangingCode(Session),
    RequestingDeviceCode(Session),
    DeviceSleeping {
        session: Session,
        device: DevicePolling,
    },
    PollingDevice {
        session: Session,
        device: DevicePolling,
    },
    RequestingClientCredentials(Session),
    Completed {
        output: TokenOutput,
    },
    Failed {
        kind: TokenFailure,
        message: String,
    },
}
