//! Plain HTTP request and response data, shared by the OAuth token flows and
//! `kurama api`. The `HttpClient` port sends them; nothing here does I/O.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// One HTTP request. `Debug` masks the `Authorization` header.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    /// The first value of a header, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &mask_secret_headers(&self.headers))
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// One HTTP response; the body is kept as bytes until a caller decodes it.
/// `Debug` masks credential headers and shows only the body length: a token
/// endpoint answer must not reach a log through a state dump.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &mask_secret_headers(&self.headers))
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The first value of a header, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Headers whose values are credentials and never appear in logs or output.
/// Only these names (and the header a `kind = "token"` source names) are
/// masked: a credential in a header of another name is printed as given.
const SECRET_HEADERS: [&str; 10] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-amz-security-token",
    "private-token",
    "x-auth-token",
    "x-access-token",
    "x-api-token",
];

/// The same headers with every credential value replaced by `****`; a bearer
/// or basic scheme keeps its name so the masked line still says what was
/// sent, and a SigV4 signature keeps its credential scope and signed
/// headers (which say what the request was signed for) and loses the
/// signature.
pub fn mask_secret_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    mask_secret_headers_with(headers, &[])
}

/// The same, plus the headers a configured source presents its credential
/// in: a `[auth.*]` of kind `token` names its own header, which the fixed
/// list above has no way to know.
pub fn mask_secret_headers_with(
    headers: &[(String, String)],
    also: &[&str],
) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let secret =
                is_secret_header(name) || also.iter().any(|extra| extra.eq_ignore_ascii_case(name));
            let masked = if secret {
                match value.split_once(' ') {
                    Some((scheme @ "AWS4-HMAC-SHA256", rest))
                        if name.eq_ignore_ascii_case("authorization") =>
                    {
                        format!("{scheme} {}", mask_signature(rest))
                    }
                    Some((scheme, _)) if name.eq_ignore_ascii_case("authorization") => {
                        format!("{scheme} ****")
                    }
                    _ => "****".to_string(),
                }
            } else {
                value.clone()
            };
            (name.clone(), masked)
        })
        .collect()
}

/// Whether a header of this name carries a credential (the fixed list only).
pub fn is_secret_header(name: &str) -> bool {
    SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

/// A header name is a token of RFC 9110: no space, no colon, no control
/// character. The set is narrowed to what headers are actually spelled
/// with, so a value that slipped into a key is refused when the
/// configuration is read instead of inside the HTTP client.
pub fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `[api.*] headers`: what an API is sent with every request, checked when
/// the configuration is read, so a mistake is `CONFIG_INVALID` with its
/// line. A header whose value is a credential is refused: `Authorization`
/// is where kurama puts the API's own, and a key written into the file
/// would be a secret on disk. A value is printable ASCII on one line, so
/// no newline can start a second header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ApiHeaders(BTreeMap<String, String>);

impl ApiHeaders {
    pub fn new(headers: BTreeMap<String, String>) -> Result<Self, String> {
        let mut seen: Vec<&str> = Vec::new();
        for (name, value) in &headers {
            if !is_header_name(name) {
                return Err(format!(
                    "headers: {name:?} is not a header name such as Accept or X-Api-Version"
                ));
            }
            if SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                return Err(format!(
                    "headers cannot set {name}: it carries a credential, which comes from auth or aws_profile and never from the configuration file"
                ));
            }
            if !value.chars().all(|c| c == '\t' || (' '..='~').contains(&c)) {
                return Err(format!(
                    "headers: the value of {name} must be printable ASCII on one line"
                ));
            }
            if let Some(other) = seen.iter().find(|other| other.eq_ignore_ascii_case(name)) {
                return Err(format!("headers: {other} and {name} name the same header"));
            }
            seen.push(name);
        }
        Ok(Self(headers))
    }

    /// Each header in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Whether a header of this name is set, compared case-insensitively.
    pub fn contains(&self, name: &str) -> bool {
        self.0.keys().any(|key| key.eq_ignore_ascii_case(name))
    }
}

impl<'de> Deserialize<'de> for ApiHeaders {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(BTreeMap::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// `Credential=..., SignedHeaders=..., Signature=<hex>` without the hex.
fn mask_signature(parameters: &str) -> String {
    match parameters.split_once("Signature=") {
        Some((before, _)) => format!("{before}Signature=****"),
        None => "****".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_token_headers_are_masked_too() {
        let headers: Vec<(String, String)> = [
            "Private-Token",
            "X-Auth-Token",
            "X-Access-Token",
            "X-Api-Token",
        ]
        .iter()
        .map(|name| (name.to_string(), "secret".to_string()))
        .collect();
        for (name, value) in mask_secret_headers(&headers) {
            assert_eq!(value, "****", "{name}");
        }
    }

    #[test]
    fn debug_and_masking_hide_credentials_but_keep_the_scheme() {
        let request = HttpRequest::new("GET", "https://api.example.com/user")
            .with_header("Authorization", "Bearer secret-token")
            .with_header("X-Api-Key", "key-123")
            .with_header("Accept", "application/json");
        let masked = mask_secret_headers(&request.headers);
        assert_eq!(masked[0].1, "Bearer ****");
        assert_eq!(masked[1].1, "****");
        assert_eq!(masked[2].1, "application/json");
        let debug = format!("{request:?}");
        assert!(!debug.contains("secret-token"));
        assert!(!debug.contains("key-123"));
        assert!(debug.contains("Bearer ****"));
        let response = HttpResponse {
            status: 200,
            headers: vec![
                ("X-Api-Key".into(), "key-123".into()),
                ("SET-COOKIE".into(), "session=cookie-123".into()),
            ],
            body: br#"{"access_token":"issued-token"}"#.to_vec(),
        };
        let debug = format!("{response:?}");
        assert!(!debug.contains("issued-token"));
        assert!(!debug.contains("key-123"));
        assert!(!debug.contains("cookie-123"), "{debug}");
        assert!(debug.contains("body_bytes: 31"), "{debug}");
    }

    #[test]
    fn a_sigv4_signature_keeps_its_scope_and_loses_the_signature_and_the_token() {
        let masked = mask_secret_headers_with(&[
            (
                "authorization".into(),
                "AWS4-HMAC-SHA256 Credential=ASIAKEY/20260917/ap-northeast-1/execute-api/aws4_request, \
                 SignedHeaders=host;x-amz-date;x-amz-security-token, Signature=0123abcd"
                    .into(),
            ),
            ("x-amz-security-token".into(), "session-token".into()),
            ("x-amz-date".into(), "20260917T000000Z".into()),
        ], &[]);
        assert_eq!(
            masked[0].1,
            "AWS4-HMAC-SHA256 Credential=ASIAKEY/20260917/ap-northeast-1/execute-api/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-security-token, Signature=****"
        );
        assert_eq!(masked[1].1, "****");
        assert_eq!(masked[2].1, "20260917T000000Z");
        assert_eq!(mask_signature("garbage"), "****");
    }

    /// A `kind = "token"` source may present its credential in any header,
    /// and `--dry-run` / `-v` print what was built: the header the source
    /// names is masked whatever it is called.
    #[test]
    fn a_configured_credential_header_is_masked_by_name() {
        let headers = [
            ("xi-api-key".to_string(), "key-123".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ];
        assert_eq!(mask_secret_headers(&headers)[0].1, "key-123");
        let masked = mask_secret_headers_with(&headers, &["Xi-Api-Key"]);
        assert_eq!(masked[0].1, "****");
        assert_eq!(masked[1].1, "application/json");
    }

    fn headers(toml: &str) -> Result<ApiHeaders, String> {
        #[derive(Deserialize)]
        struct Section {
            headers: ApiHeaders,
        }
        toml::from_str::<Section>(toml)
            .map(|section| section.headers)
            .map_err(|error| error.message().to_string())
    }

    #[test]
    fn api_headers_read_an_inline_or_a_nested_table() {
        let inline = headers(
            "headers = { Accept = \"application/vnd.github+json\", \"X-GitHub-Api-Version\" = \"2022-11-28\" }\n",
        )
        .unwrap();
        let nested = headers(
            "[headers]\nAccept = \"application/vnd.github+json\"\nX-GitHub-Api-Version = \"2022-11-28\"\n",
        )
        .unwrap();
        assert_eq!(inline, nested);
        assert_eq!(
            inline.iter().collect::<Vec<_>>(),
            [
                ("Accept", "application/vnd.github+json"),
                ("X-GitHub-Api-Version", "2022-11-28"),
            ]
        );
        assert!(inline.contains("x-github-api-version"));
        assert!(!inline.contains("Content-Type"));
        assert!(headers("headers = { X-Tab = \"a\\tb\" }\n").is_ok());
    }

    #[rstest::rstest]
    #[case(
        "headers = { Authorization = \"Bearer x\" }",
        "cannot set Authorization"
    )]
    #[case(
        "headers = { authorization = \"Bearer x\" }",
        "cannot set authorization"
    )]
    #[case("headers = { X-API-Key = \"k\" }", "cannot set X-API-Key")]
    #[case("headers = { Cookie = \"k\" }", "cannot set Cookie")]
    #[case(
        "headers = { X-Trace = \"a\\nX-Injected: 1\" }",
        "value of X-Trace must be printable ASCII on one line"
    )]
    #[case(
        "headers = { X-Trace = \"a\\rb\" }",
        "value of X-Trace must be printable ASCII"
    )]
    #[case(
        "headers = { X-Trace = \"\\u0000\" }",
        "value of X-Trace must be printable ASCII"
    )]
    #[case(
        "headers = { X-Trace = \"caf\u{e9}\" }",
        "value of X-Trace must be printable ASCII"
    )]
    #[case(
        "headers = { \"X Trace\" = \"1\" }",
        "\"X Trace\" is not a header name"
    )]
    #[case("headers = { \"X-Trace:\" = \"1\" }", "is not a header name")]
    #[case("headers = { \"\" = \"1\" }", "\"\" is not a header name")]
    #[case(
        "headers = { Accept = \"a\", accept = \"b\" }",
        "Accept and accept name the same header"
    )]
    fn api_headers_refuse_a_credential_or_a_second_line(
        #[case] toml: &str,
        #[case] expected: &str,
    ) {
        let message = headers(toml).unwrap_err();
        assert!(message.contains(expected), "{message}");
    }

    #[test]
    fn header_lookup_ignores_case() {
        let response = HttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Vec::new(),
        };
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert!(response.is_success());
        assert!(
            !HttpResponse {
                status: 404,
                ..response
            }
            .is_success()
        );
    }
}
