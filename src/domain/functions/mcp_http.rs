//! `kurama mcp --listen`, pure: what one HTTP request to the Streamable HTTP endpoint is answered with, before its body is read and after.
//!
//! The order of the checks is the point: the token first, whatever the path
//! or method, so a caller without it learns nothing about what the endpoint
//! accepts; then `Origin`, the path and method, the headers, and the body.
//! Only JSON responses and no session: the part of the transport cloud
//! clients use.

use super::mcp::{Incoming, PROTOCOL_VERSIONS, read_value};
use crate::domain::types::Secret;
use crate::domain::types::limits::MCP_HTTP;
use serde_json::{Value, json};

/// The one path the endpoint answers on.
pub const ENDPOINT: &str = "/mcp";

/// The fixed token a client sends in `Authorization`, redacted in `Debug`
/// and zeroized on drop by the `Secret` it holds.
#[derive(Clone)]
pub struct McpToken(Secret);

impl McpToken {
    /// The token, or `None` when it is empty: an empty token would admit an
    /// empty `Authorization` and `Bearer ` alike.
    pub fn new(token: Secret) -> Option<Self> {
        (!token.expose().is_empty()).then_some(Self(token))
    }
}

impl std::fmt::Debug for McpToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpToken([REDACTED])")
    }
}

/// The request line and headers, header names in lowercase.
pub struct RequestHead<'a> {
    pub method: &'a str,
    /// The path without the query.
    pub path: &'a str,
    pub headers: &'a [(String, String)],
}

impl RequestHead<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A request answered without reaching JSON-RPC.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// `401` with `WWW-Authenticate: Bearer` and no body.
    Unauthorized,
    /// `403`: a browser sent it.
    Forbidden,
    /// `404`: a path other than [`ENDPOINT`].
    NotFound,
    /// `405`: no SSE stream (`GET`) and no session (`DELETE`).
    MethodNotAllowed,
    /// `415`: the body is not `application/json`.
    UnsupportedMediaType,
    /// `413`: the body is longer than the limit.
    PayloadTooLarge,
    /// `400`, with the JSON-RPC error that says why when there is one.
    BadRequest(Option<Value>),
}

impl Refusal {
    pub fn status(&self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
            Self::UnsupportedMediaType => 415,
            Self::PayloadTooLarge => 413,
            Self::BadRequest(_) => 400,
        }
    }
}

/// Whether the request line and headers may go on to the body.
pub fn admit(head: &RequestHead<'_>, token: &McpToken) -> Result<(), Refusal> {
    if !head
        .header("authorization")
        .is_some_and(|value| token_matches(value, token))
    {
        return Err(Refusal::Unauthorized);
    }
    if head.header("origin").is_some() {
        return Err(Refusal::Forbidden);
    }
    if head.path != ENDPOINT {
        return Err(Refusal::NotFound);
    }
    if head.method != "POST" {
        return Err(Refusal::MethodNotAllowed);
    }
    let media_type = head
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !media_type.is_some_and(|media| media.eq_ignore_ascii_case("application/json")) {
        return Err(Refusal::UnsupportedMediaType);
    }
    let length = head
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok());
    if length.is_some_and(|length| length > MCP_HTTP.body_bytes as u64) {
        return Err(Refusal::PayloadTooLarge);
    }
    Ok(())
}

/// What the body asks for. A notification or a response is `202` with no
/// body ([`Incoming::Nothing`]); a request is answered with `200`.
/// `version` is the `MCP-Protocol-Version` header: a message other than
/// `initialize`, which negotiates the version in its body, that names one
/// this server does not speak is `400`.
pub fn read_body(
    body: &[u8],
    version: Option<&str>,
    exposed: &[String],
) -> Result<Incoming, Refusal> {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return Err(Refusal::BadRequest(Some(error(
            -32700,
            "the body is not JSON",
        ))));
    };
    let is_message = message
        .as_object()
        .is_some_and(|object| object.contains_key("method") || object.contains_key("id"));
    if !is_message {
        return Err(Refusal::BadRequest(Some(error(
            -32600,
            "the body is not one JSON-RPC message",
        ))));
    }
    let initializes = message.get("method").and_then(Value::as_str) == Some("initialize");
    if let Some(version) = version
        && !initializes
        && !PROTOCOL_VERSIONS.contains(&version)
    {
        return Err(Refusal::BadRequest(Some(error(
            -32600,
            &format!(
                "MCP-Protocol-Version {version} is not one of {}",
                PROTOCOL_VERSIONS.join(", ")
            ),
        ))));
    }
    Ok(read_value(&message, exposed))
}

/// The JSON-RPC error a `400` carries.
fn error(code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": message } })
}

/// Whether `Authorization` carries the token, as `Bearer <token>` or as the
/// token alone: clients differ in whether they add the scheme. Each byte is
/// compared whatever the earlier ones were, so the time taken says nothing
/// about how much of a guess was right.
fn token_matches(header: &str, token: &McpToken) -> bool {
    let sent = match header.split_once(' ') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bearer") => rest,
        _ => header,
    };
    let (sent, expected) = (sent.as_bytes(), token.0.expose().as_bytes());
    if sent.len() != expected.len() {
        return false;
    }
    sent.iter()
        .zip(expected)
        .fold(0u8, |differ, (a, b)| differ | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "s3cr3t-token-of-the-test";

    fn token() -> McpToken {
        McpToken::new(Secret::new(TOKEN)).expect("a token")
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn check(method: &str, path: &str, pairs: &[(&str, &str)]) -> Result<(), Refusal> {
        let headers = headers(pairs);
        admit(
            &RequestHead {
                method,
                path,
                headers: &headers,
            },
            &token(),
        )
    }

    fn every() -> Vec<String> {
        super::super::mcp::TOOL_NAMES.map(str::to_owned).to_vec()
    }

    const BEARER: (&str, &str) = ("authorization", "Bearer s3cr3t-token-of-the-test");
    const JSON: (&str, &str) = ("content-type", "application/json");

    #[test]
    fn the_token_is_accepted_with_or_without_the_bearer_scheme() {
        assert_eq!(check("POST", ENDPOINT, &[BEARER, JSON]), Ok(()));
        assert_eq!(
            check("POST", ENDPOINT, &[("authorization", TOKEN), JSON]),
            Ok(())
        );
        assert_eq!(
            check(
                "POST",
                ENDPOINT,
                &[("authorization", "bearer s3cr3t-token-of-the-test"), JSON]
            ),
            Ok(())
        );
    }

    #[rstest::rstest]
    #[case(None)]
    #[case(Some(""))]
    #[case(Some("Bearer "))]
    #[case(Some("Bearer s3cr3t-token-of-the-tesT"))]
    #[case(Some("Bearer s3cr3t-token-of-the-test "))]
    #[case(Some("Bearer s3cr3t-token-of-the-tes"))]
    #[case(Some("Basic s3cr3t-token-of-the-test"))]
    #[case(Some("Bearer  s3cr3t-token-of-the-test"))]
    fn anything_but_the_token_is_unauthorized(#[case] authorization: Option<&str>) {
        let mut pairs = vec![JSON];
        if let Some(value) = authorization {
            pairs.push(("authorization", value));
        }
        assert_eq!(check("POST", ENDPOINT, &pairs), Err(Refusal::Unauthorized));
    }

    #[test]
    fn without_the_token_nothing_says_which_path_method_or_format_is_accepted() {
        for (method, path) in [("GET", "/"), ("DELETE", ENDPOINT), ("POST", "/admin")] {
            assert_eq!(
                check(method, path, &[("origin", "https://x.example")]),
                Err(Refusal::Unauthorized),
                "{method} {path}"
            );
        }
    }

    #[test]
    fn with_the_token_each_check_answers_in_order() {
        assert_eq!(
            check("GET", "/other", &[BEARER, ("origin", "https://x.example")]),
            Err(Refusal::Forbidden)
        );
        assert_eq!(check("GET", "/other", &[BEARER]), Err(Refusal::NotFound));
        assert_eq!(
            check("GET", ENDPOINT, &[BEARER]),
            Err(Refusal::MethodNotAllowed)
        );
        assert_eq!(
            check("DELETE", ENDPOINT, &[BEARER, JSON]),
            Err(Refusal::MethodNotAllowed)
        );
        assert_eq!(
            check("POST", ENDPOINT, &[BEARER, ("content-type", "text/plain")]),
            Err(Refusal::UnsupportedMediaType)
        );
        assert_eq!(
            check("POST", ENDPOINT, &[BEARER]),
            Err(Refusal::UnsupportedMediaType)
        );
        assert_eq!(
            check(
                "POST",
                ENDPOINT,
                &[BEARER, ("content-type", "Application/JSON; charset=utf-8")]
            ),
            Ok(())
        );
    }

    #[test]
    fn a_protocol_version_is_one_this_server_speaks_or_absent_except_on_initialize() {
        let ping = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        for version in PROTOCOL_VERSIONS.map(Some).into_iter().chain([None]) {
            assert!(
                matches!(read_body(ping, version, &every()), Ok(Incoming::Answer(_))),
                "{version:?}"
            );
        }
        let refused = read_body(ping, Some("1999-01-01"), &every());
        assert!(
            matches!(&refused, Err(Refusal::BadRequest(Some(error))) if error["error"]["message"].as_str().unwrap().contains("1999-01-01")),
            "{refused:?}"
        );
        // A newer client may name its own latest version before the two
        // sides agreed on one; initialize answers with one this server speaks.
        let initialize = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#;
        let Ok(Incoming::Answer(answer)) = read_body(initialize, Some("2099-01-01"), &every())
        else {
            panic!("initialize negotiates");
        };
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn an_empty_token_is_no_token() {
        assert!(McpToken::new(Secret::new("")).is_none());
    }

    #[test]
    fn a_declared_length_past_the_limit_is_refused_before_the_body_is_read() {
        let at = MCP_HTTP.body_bytes.to_string();
        let past = (MCP_HTTP.body_bytes + 1).to_string();
        assert_eq!(
            check("POST", ENDPOINT, &[BEARER, JSON, ("content-length", &at)]),
            Ok(())
        );
        assert_eq!(
            check("POST", ENDPOINT, &[BEARER, JSON, ("content-length", &past)]),
            Err(Refusal::PayloadTooLarge)
        );
    }

    #[test]
    fn a_body_is_one_json_rpc_message() {
        for body in [
            "{not json",
            "[]",
            "[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]",
            "{}",
            "1",
        ] {
            let refused = read_body(body.as_bytes(), None, &every());
            assert!(
                matches!(refused, Err(Refusal::BadRequest(Some(_)))),
                "{body}: {refused:?}"
            );
        }
        assert_eq!(
            read_body(
                br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                None,
                &every()
            ),
            Ok(Incoming::Nothing)
        );
        assert_eq!(
            read_body(br#"{"jsonrpc":"2.0","id":3,"result":{}}"#, None, &every()),
            Ok(Incoming::Nothing)
        );
        let Ok(Incoming::Answer(answer)) = read_body(
            br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            None,
            &every(),
        ) else {
            panic!("a request is answered");
        };
        assert_eq!(answer["id"], 1);
    }

    #[test]
    fn the_token_never_appears_in_debug() {
        assert_eq!(format!("{:?}", token()), "McpToken([REDACTED])");
    }
}
