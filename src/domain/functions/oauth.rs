//! Pure OAuth 2.0 / OpenID Connect functions: PKCE, the forms each grant
//! posts, the responses the server sends back, and discovery.
//!
//! Nothing here does I/O; the `oauth_token` workflow decides what to send
//! and the shell sends it.

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};

use super::api_request::body_excerpt;
use crate::domain::types::oauth_client::check_credential_url;
use crate::domain::types::{OAuthEndpoints, OAuthToken};

/// A token that expires within this margin is treated as expired, so a
/// request signed with it does not fail while in flight.
pub const TOKEN_REUSE_MARGIN: Duration = Duration::seconds(60);

/// How long a person may take in the browser or at the device code page.
pub const AUTHORIZATION_TIMEOUT_SECS: u64 = 300;

/// Characters of a token endpoint body an error message carries.
const ERROR_EXCERPT_CHARS: usize = 200;

/// The JSON object of a token or device authorization answer.
type Fields = serde_json::Map<String, serde_json::Value>;

/// Device flow polling interval when the server names none (RFC 8628).
pub const DEFAULT_DEVICE_POLL_INTERVAL_SECS: u64 = 5;

/// Device flow code lifetime when the server names none.
pub const DEFAULT_DEVICE_CODE_LIFETIME_SECS: u64 = 300;

/// The loopback redirect the authorization code grant listens on.
pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

/// Whether `token` can still sign a request at `now`; a token without an
/// expiration is used until the API rejects it.
pub fn is_token_usable(token: &OAuthToken, now: DateTime<Utc>) -> bool {
    match token.expires_at {
        Some(expires_at) => expires_at - now > TOKEN_REUSE_MARGIN,
        None => true,
    }
}

/// URL-safe base64 without padding, as PKCE and `state` values use.
pub fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The `S256` code challenge of a PKCE verifier (RFC 7636).
pub fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// The OpenID Connect discovery document of an issuer: the well-known path
/// appended to the issuer (OpenID Connect Discovery 1.0 section 4). RFC 8414
/// would insert it between host and path; issuers with a path (Okta, Auth0)
/// publish the appended form.
pub fn discovery_url(issuer: &str) -> String {
    format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    )
}

/// The endpoints named by a discovery document; `token_endpoint` is required.
pub fn parse_discovery(body: &[u8]) -> Result<OAuthEndpoints, String> {
    let document: serde_json::Value = serde_json::from_slice(body)
        .map_err(|error| format!("discovery document is not JSON: {error}"))?;
    // Each endpoint is checked as a configured one is: grant credentials are
    // posted to it.
    let field = |name: &str| -> Result<Option<String>, String> {
        let Some(url) = document[name].as_str() else {
            return Ok(None);
        };
        check_credential_url(&format!("discovery document {name}"), url)?;
        Ok(Some(url.to_string()))
    };
    Ok(OAuthEndpoints {
        auth_url: field("authorization_endpoint")?,
        token_url: field("token_endpoint")?
            .ok_or_else(|| "discovery document has no token_endpoint".to_string())?,
        device_auth_url: field("device_authorization_endpoint")?,
    })
}

/// How the client identifies itself at the token endpoint. No `Debug`: the
/// secret would be in it.
#[derive(Clone, Copy)]
pub struct ClientAuth<'a> {
    pub client_id: &'a str,
    pub client_secret: Option<&'a str>,
}

/// The authorization request URL the browser opens.
pub fn build_authorization_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[String],
    state: &str,
    code_challenge: &str,
) -> Result<String, String> {
    let mut url = url::Url::parse(auth_url).map_err(|error| format!("auth_url: {error}"))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256");
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
    }
    Ok(url.into())
}

/// What a token request asks for. No `Debug`: every variant but
/// `ClientCredentials` carries a credential.
#[derive(Clone, Copy)]
pub enum TokenGrant<'a> {
    AuthorizationCode {
        code: &'a str,
        redirect_uri: &'a str,
        code_verifier: &'a str,
    },
    RefreshToken {
        refresh_token: &'a str,
    },
    DeviceCode {
        device_code: &'a str,
    },
    ClientCredentials,
}

/// The form body of a token request. The client secret travels in the body
/// (`client_secret_post`), which every common provider accepts.
pub fn token_request_form(
    grant: &TokenGrant,
    client: &ClientAuth,
    scopes: &[String],
) -> Vec<(String, String)> {
    let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
    let mut form = match grant {
        TokenGrant::AuthorizationCode {
            code,
            redirect_uri,
            code_verifier,
        } => vec![
            pair("grant_type", "authorization_code"),
            pair("code", code),
            pair("redirect_uri", redirect_uri),
            pair("code_verifier", code_verifier),
        ],
        TokenGrant::RefreshToken { refresh_token } => vec![
            pair("grant_type", "refresh_token"),
            pair("refresh_token", refresh_token),
        ],
        TokenGrant::DeviceCode { device_code } => vec![
            pair("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            pair("device_code", device_code),
        ],
        TokenGrant::ClientCredentials => vec![pair("grant_type", "client_credentials")],
    };
    form.push(pair("client_id", client.client_id));
    if let Some(secret) = client.client_secret {
        form.push(pair("client_secret", secret));
    }
    if matches!(grant, TokenGrant::ClientCredentials) && !scopes.is_empty() {
        form.push(pair("scope", &scopes.join(" ")));
    }
    form
}

/// The form body of a device authorization request (RFC 8628 section 3.1).
pub fn device_authorization_form(client: &ClientAuth, scopes: &[String]) -> Vec<(String, String)> {
    let mut form = vec![("client_id".to_string(), client.client_id.to_string())];
    if let Some(secret) = client.client_secret {
        form.push(("client_secret".to_string(), secret.to_string()));
    }
    if !scopes.is_empty() {
        form.push(("scope".to_string(), scopes.join(" ")));
    }
    form
}

/// `application/x-www-form-urlencoded` encoding of a form.
pub fn encode_form(pairs: &[(String, String)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().map(|(name, value)| (name, value)))
        .finish()
}

/// Why a token endpoint did not hand out a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// The server answered with an OAuth error body (`invalid_grant`, ...):
    /// it judged the grant.
    Rejected {
        error: String,
        description: Option<String>,
    },
    /// A status outside 2xx without an OAuth error body (a gateway 502, an
    /// HTML 401): the server did not judge the grant.
    Status { status: u16, excerpt: String },
    /// A 2xx answer that is not a token response.
    Invalid(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected {
                error,
                description: Some(description),
            } => write!(f, "{error}: {description}"),
            Self::Rejected { error, .. } => f.write_str(error),
            Self::Status { status, excerpt } if excerpt.is_empty() => write!(f, "HTTP {status}"),
            Self::Status { status, excerpt } => write!(f, "HTTP {status}: {excerpt}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

/// The JSON object of a token endpoint answer (RFC 6749 section 5). Every
/// request carries `Accept: application/json`, so nothing else is parsed.
fn response_fields(body: &[u8]) -> Option<Fields> {
    match serde_json::from_slice(body) {
        Ok(serde_json::Value::Object(fields)) => Some(fields),
        _ => None,
    }
}

/// The fields of a token or device authorization answer. An OAuth error body
/// wins over the status (GitHub answers `authorization_pending` with 200),
/// then a status outside 2xx, then a body that is not a JSON object.
fn answer_fields(status: u16, body: &[u8]) -> Result<Fields, TokenError> {
    let fields = response_fields(body);
    if let Some(error) = fields.as_ref().and_then(rejection) {
        return Err(error);
    }
    if !(200..300).contains(&status) {
        return Err(TokenError::Status {
            status,
            excerpt: body_excerpt(body, ERROR_EXCERPT_CHARS),
        });
    }
    fields.ok_or_else(|| TokenError::Invalid(format!("HTTP {status} answer is not a JSON object")))
}

fn string_field(fields: &Fields, name: &str) -> Option<String> {
    fields.get(name).and_then(|value| match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn number_field(fields: &Fields, name: &str) -> Option<u64> {
    fields.get(name).and_then(|value| match value {
        serde_json::Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|f| *f >= 0.0)
                .map(|f| f.round() as u64)
        }),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    })
}

/// An `error` field wins over the status code: GitHub answers the device
/// flow's `authorization_pending` with 200.
fn rejection(fields: &Fields) -> Option<TokenError> {
    string_field(fields, "error").map(|error| TokenError::Rejected {
        description: string_field(fields, "error_description"),
        error,
    })
}

/// The token in a token endpoint answer, with `expires_in` counted from
/// `received_at`. A refresh answer may omit the refresh token and the scope,
/// in which case the previous ones stay valid and are kept (RFC 6749
/// sections 5.1 and 6).
pub fn parse_token_response(
    status: u16,
    body: &[u8],
    received_at: DateTime<Utc>,
    previous: Option<&OAuthToken>,
) -> Result<OAuthToken, TokenError> {
    let fields = answer_fields(status, body)?;
    let access_token = string_field(&fields, "access_token")
        .filter(|token| !token.is_empty())
        .ok_or_else(|| TokenError::Invalid("token response has no access_token".into()))?;
    Ok(OAuthToken {
        access_token,
        token_type: string_field(&fields, "token_type").unwrap_or_else(|| "Bearer".into()),
        refresh_token: string_field(&fields, "refresh_token")
            .or_else(|| previous.and_then(|token| token.refresh_token.clone())),
        expires_at: number_field(&fields, "expires_in")
            .map(|seconds| received_at + Duration::seconds(seconds as i64)),
        scope: string_field(&fields, "scope")
            .or_else(|| previous.and_then(|token| token.scope.clone())),
    })
}

/// What the device authorization endpoint hands out (RFC 8628 section 3.2).
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

impl std::fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("device_code", &"[REDACTED]")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

pub fn parse_device_authorization(
    status: u16,
    body: &[u8],
) -> Result<DeviceAuthorization, TokenError> {
    let fields = answer_fields(status, body)?;
    let required = |name: &str| {
        string_field(&fields, name)
            .ok_or_else(|| TokenError::Invalid(format!("device authorization has no {name}")))
    };
    let interval = number_field(&fields, "interval").unwrap_or(DEFAULT_DEVICE_POLL_INTERVAL_SECS);
    if interval == 0 {
        // Zero would poll the token endpoint back to back.
        return Err(TokenError::Invalid(
            "device authorization names a polling interval of 0 seconds".into(),
        ));
    }
    Ok(DeviceAuthorization {
        device_code: required("device_code")?,
        user_code: required("user_code")?,
        verification_uri: required("verification_uri")?,
        verification_uri_complete: string_field(&fields, "verification_uri_complete"),
        expires_in: number_field(&fields, "expires_in")
            .unwrap_or(DEFAULT_DEVICE_CODE_LIFETIME_SECS),
        interval,
    })
}

/// One poll of the token endpoint during the device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    Token(OAuthToken),
    /// The person has not finished yet; poll again after the interval.
    Pending,
    /// Polling too fast; add five seconds to the interval.
    SlowDown,
}

/// One poll of the token endpoint; any other answer is the token endpoint's
/// error, classified like every other token request.
pub fn classify_device_poll(
    status: u16,
    body: &[u8],
    received_at: DateTime<Utc>,
) -> Result<DevicePoll, TokenError> {
    match parse_token_response(status, body, received_at, None) {
        Ok(token) => Ok(DevicePoll::Token(token)),
        Err(TokenError::Rejected { error, .. }) if error == "authorization_pending" => {
            Ok(DevicePoll::Pending)
        }
        Err(TokenError::Rejected { error, .. }) if error == "slow_down" => Ok(DevicePoll::SlowDown),
        Err(error) => Err(error),
    }
}

/// The authorization code in the redirect the browser was sent to. `state`
/// is checked first: an error redirect carries it too (RFC 6749 section
/// 4.1.2.1), so a request that does not know it says nothing.
pub fn parse_callback_query(query: &str, expected_state: &str) -> Result<String, String> {
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let field = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    if field("state") != Some(expected_state) {
        return Err("state in the redirect does not match the request".into());
    }
    if let Some(error) = field("error") {
        return Err(match field("error_description") {
            Some(description) => format!("{error}: {description}"),
            None => error.to_string(),
        });
    }
    field("code")
        .filter(|code| !code.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "redirect carries no authorization code".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    #[test]
    fn pkce_challenge_matches_the_rfc_7636_example() {
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[rstest::rstest]
    #[case(None, true)]
    #[case(Some(3600), true)]
    #[case(Some(61), true)]
    #[case(Some(60), false)]
    #[case(Some(-10), false)]
    fn token_usability(#[case] remaining: Option<i64>, #[case] expected: bool) {
        let mut token = OAuthToken::bearer("abc");
        token.expires_at = remaining.map(|seconds| now() + Duration::seconds(seconds));
        assert_eq!(is_token_usable(&token, now()), expected);
    }

    #[test]
    fn discovery_url_and_document() {
        assert_eq!(
            discovery_url("https://accounts.example.com/"),
            "https://accounts.example.com/.well-known/openid-configuration"
        );
        let endpoints = parse_discovery(
            br#"{"issuer":"https://accounts.example.com","authorization_endpoint":"https://accounts.example.com/auth","token_endpoint":"https://accounts.example.com/token","device_authorization_endpoint":"https://accounts.example.com/device"}"#,
        )
        .unwrap();
        assert_eq!(
            endpoints.auth_url.as_deref(),
            Some("https://accounts.example.com/auth")
        );
        assert_eq!(endpoints.token_url, "https://accounts.example.com/token");
        assert_eq!(
            endpoints.device_auth_url.as_deref(),
            Some("https://accounts.example.com/device")
        );
        assert!(
            parse_discovery(br#"{"issuer":"x"}"#)
                .unwrap_err()
                .contains("token_endpoint")
        );
        assert!(parse_discovery(b"<html>").unwrap_err().contains("not JSON"));
    }

    #[test]
    fn discovered_endpoints_must_be_https_unless_loopback() {
        let loopback = parse_discovery(
            br#"{"authorization_endpoint":"http://127.0.0.1:9/auth","token_endpoint":"http://localhost:9/token","device_authorization_endpoint":"http://[::1]:9/device"}"#,
        );
        assert!(loopback.is_ok(), "{loopback:?}");
        for (field, document) in [
            (
                "token_endpoint",
                r#"{"token_endpoint":"http://as.example.com/token"}"#,
            ),
            (
                "authorization_endpoint",
                r#"{"authorization_endpoint":"http://as.example.com/auth","token_endpoint":"https://as.example.com/token"}"#,
            ),
            (
                "device_authorization_endpoint",
                r#"{"device_authorization_endpoint":"http://as.example.com/device","token_endpoint":"https://as.example.com/token"}"#,
            ),
        ] {
            let error = parse_discovery(document.as_bytes()).unwrap_err();
            assert!(
                error.contains(field) && error.contains("https"),
                "{field}: {error}"
            );
        }
    }

    #[test]
    fn authorization_url_carries_pkce_state_and_scopes() {
        let url = build_authorization_url(
            "https://github.com/login/oauth/authorize?allow_signup=false",
            "Iv1.abc",
            "http://127.0.0.1:50000/callback",
            &["repo".into(), "read:user".into()],
            "state-1",
            "challenge-1",
        )
        .unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        let pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
        let value = |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(value("allow_signup"), Some("false"));
        assert_eq!(value("response_type"), Some("code"));
        assert_eq!(value("client_id"), Some("Iv1.abc"));
        assert_eq!(
            value("redirect_uri"),
            Some("http://127.0.0.1:50000/callback")
        );
        assert_eq!(value("state"), Some("state-1"));
        assert_eq!(value("code_challenge"), Some("challenge-1"));
        assert_eq!(value("code_challenge_method"), Some("S256"));
        assert_eq!(value("scope"), Some("repo read:user"));
        assert!(build_authorization_url("nope", "", "", &[], "", "").is_err());
    }

    #[test]
    fn token_forms_per_grant() {
        let client = ClientAuth {
            client_id: "id",
            client_secret: Some("secret"),
        };
        let exchange = token_request_form(
            &TokenGrant::AuthorizationCode {
                code: "code-1",
                redirect_uri: "http://127.0.0.1:1/callback",
                code_verifier: "verifier",
            },
            &client,
            &["repo".into()],
        );
        assert_eq!(
            encode_form(&exchange),
            "grant_type=authorization_code&code=code-1&redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcallback&code_verifier=verifier&client_id=id&client_secret=secret"
        );
        let refresh = token_request_form(
            &TokenGrant::RefreshToken {
                refresh_token: "r-1",
            },
            &ClientAuth {
                client_id: "id",
                client_secret: None,
            },
            &["repo".into()],
        );
        assert_eq!(
            encode_form(&refresh),
            "grant_type=refresh_token&refresh_token=r-1&client_id=id"
        );
        let device =
            token_request_form(&TokenGrant::DeviceCode { device_code: "d-1" }, &client, &[]);
        assert_eq!(
            encode_form(&device),
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=d-1&client_id=id&client_secret=secret"
        );
        let credentials = token_request_form(
            &TokenGrant::ClientCredentials,
            &client,
            &["a".into(), "b".into()],
        );
        assert_eq!(
            encode_form(&credentials),
            "grant_type=client_credentials&client_id=id&client_secret=secret&scope=a+b"
        );
        assert_eq!(
            encode_form(&device_authorization_form(&client, &["repo".into()])),
            "client_id=id&client_secret=secret&scope=repo"
        );
    }

    #[test]
    fn json_token_response_becomes_a_token_with_expiration() {
        let token = parse_token_response(
            200,
            br#"{"access_token":"at","token_type":"bearer","expires_in":3600,"refresh_token":"rt","scope":"repo"}"#,
            now(),
            None,
        )
        .unwrap();
        assert_eq!(token.access_token, "at");
        assert_eq!(token.token_type, "bearer");
        assert_eq!(token.refresh_token.as_deref(), Some("rt"));
        assert_eq!(token.expires_at, Some(now() + Duration::seconds(3600)));
        assert_eq!(token.scope.as_deref(), Some("repo"));
    }

    #[test]
    fn a_refresh_answer_without_refresh_token_or_scope_keeps_the_previous_ones() {
        let mut previous = OAuthToken::bearer("old");
        previous.refresh_token = Some("previous-refresh".into());
        previous.scope = Some("repo".into());
        let token = parse_token_response(
            200,
            br#"{"access_token":"gho_abc","token_type":"bearer"}"#,
            now(),
            Some(&previous),
        )
        .unwrap();
        assert_eq!(token.access_token, "gho_abc");
        assert_eq!(token.refresh_token.as_deref(), Some("previous-refresh"));
        assert_eq!(token.scope.as_deref(), Some("repo"));
        assert!(token.expires_at.is_none());
    }

    #[test]
    fn a_refresh_answer_with_a_new_refresh_token_and_scope_replaces_the_previous_ones() {
        let mut previous = OAuthToken::bearer("old");
        previous.refresh_token = Some("rt1".into());
        previous.scope = Some("repo".into());
        let token = parse_token_response(
            200,
            br#"{"access_token":"at2","refresh_token":"rt2","scope":"repo read:user"}"#,
            now(),
            Some(&previous),
        )
        .unwrap();
        assert_eq!(token.refresh_token.as_deref(), Some("rt2"));
        assert_eq!(token.scope.as_deref(), Some("repo read:user"));
    }

    #[test]
    fn error_field_wins_over_status_and_a_bare_status_is_not_a_rejection() {
        assert_eq!(
            parse_token_response(
                200,
                br#"{"error":"authorization_pending","error_description":"not yet"}"#,
                now(),
                None
            )
            .unwrap_err(),
            TokenError::Rejected {
                error: "authorization_pending".into(),
                description: Some("not yet".into())
            }
        );
        assert_eq!(
            parse_token_response(400, br#"{"error":"invalid_client"}"#, now(), None)
                .unwrap_err()
                .to_string(),
            "invalid_client"
        );
        let gateway =
            parse_token_response(502, b"<html>Bad gateway</html>", now(), None).unwrap_err();
        assert!(matches!(gateway, TokenError::Status { status: 502, .. }));
        assert_eq!(gateway.to_string(), "HTTP 502: <html>Bad gateway</html>");
        assert_eq!(
            parse_token_response(503, b"", now(), None)
                .unwrap_err()
                .to_string(),
            "HTTP 503"
        );
        assert!(matches!(
            parse_token_response(200, b"<html>", now(), None).unwrap_err(),
            TokenError::Invalid(_)
        ));
        assert!(matches!(
            parse_token_response(200, b"access_token=gho_abc&token_type=bearer", now(), None)
                .unwrap_err(),
            TokenError::Invalid(_)
        ));
        assert!(matches!(
            parse_token_response(200, br#"{"token_type":"bearer"}"#, now(), None).unwrap_err(),
            TokenError::Invalid(message) if message.contains("access_token")
        ));
    }

    #[test]
    fn device_authorization_with_a_zero_interval_is_invalid() {
        let error = parse_device_authorization(
            200,
            br#"{"device_code":"dc","user_code":"u","verification_uri":"https://x","interval":0}"#,
        )
        .unwrap_err();
        assert!(
            matches!(&error, TokenError::Invalid(message) if message.contains("interval")),
            "{error:?}"
        );
    }

    #[test]
    fn device_authorization_and_polls() {
        let device = parse_device_authorization(
            200,
            br#"{"device_code":"dc","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
        )
        .unwrap();
        assert_eq!(device.user_code, "ABCD-1234");
        assert_eq!(device.interval, 5);
        assert_eq!(device.expires_in, 900);
        assert!(!format!("{device:?}").contains("\"dc\""));
        let defaults = parse_device_authorization(
            200,
            br#"{"device_code":"dc","user_code":"u","verification_uri":"https://x"}"#,
        )
        .unwrap();
        assert_eq!(defaults.interval, DEFAULT_DEVICE_POLL_INTERVAL_SECS);
        assert_eq!(defaults.expires_in, DEFAULT_DEVICE_CODE_LIFETIME_SECS);
        assert!(matches!(
            parse_device_authorization(400, br#"{"error":"invalid_scope"}"#).unwrap_err(),
            TokenError::Rejected { error, .. } if error == "invalid_scope"
        ));

        assert_eq!(
            classify_device_poll(400, br#"{"error":"authorization_pending"}"#, now()).unwrap(),
            DevicePoll::Pending
        );
        assert_eq!(
            classify_device_poll(400, br#"{"error":"slow_down"}"#, now()).unwrap(),
            DevicePoll::SlowDown
        );
        assert!(matches!(
            classify_device_poll(400, br#"{"error":"expired_token"}"#, now()).unwrap_err(),
            TokenError::Rejected { error, .. } if error == "expired_token"
        ));
        assert!(matches!(
            classify_device_poll(200, b"<html>", now()).unwrap_err(),
            TokenError::Invalid(_)
        ));
        assert!(matches!(
            classify_device_poll(200, br#"{"access_token":"at"}"#, now()).unwrap(),
            DevicePoll::Token(token) if token.access_token == "at"
        ));
    }

    #[test]
    fn callback_query_needs_the_matching_state_and_a_code() {
        assert_eq!(
            parse_callback_query("code=abc&state=s1", "s1").unwrap(),
            "abc"
        );
        assert!(
            parse_callback_query("code=abc&state=other", "s1")
                .unwrap_err()
                .contains("state")
        );
        assert!(
            parse_callback_query("error=access_denied", "s1")
                .unwrap_err()
                .contains("state")
        );
        assert!(
            parse_callback_query("state=s1", "s1")
                .unwrap_err()
                .contains("no authorization code")
        );
        assert_eq!(
            parse_callback_query(
                "error=access_denied&error_description=The+user+denied&state=s1",
                "s1"
            )
            .unwrap_err(),
            "access_denied: The user denied"
        );
        assert_eq!(redirect_uri(50000), "http://127.0.0.1:50000/callback");
    }
}
