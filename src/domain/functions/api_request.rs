//! Pure functions of `kurama api`: the TARGET argument, the URL it resolves
//! to, the dry-run rendering, the `--json` envelope and the layout of a JSON
//! body for a terminal.

use crate::domain::types::http::{HttpRequest, HttpResponse, mask_secret_headers_with};

/// Where a TARGET points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetLocation {
    /// A path under the API's `base_url`.
    Path(String),
    /// A complete URL.
    Url(String),
}

/// A parsed TARGET: `/path`, `https://...` or `METHOD /path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiTarget {
    pub method: Option<String>,
    pub location: TargetLocation,
}

const TARGET_FORMS: &str =
    "TARGET must be a path starting with '/', an http(s) URL, or `METHOD /path`";

pub fn parse_target(target: &str) -> Result<ApiTarget, String> {
    let target = target.trim();
    let (method, location) = match target.split_once(char::is_whitespace) {
        Some((verb, rest)) if !verb.is_empty() && verb.chars().all(|c| c.is_ascii_uppercase()) => {
            (Some(verb.to_string()), rest.trim())
        }
        _ => (None, target),
    };
    let location = if location.starts_with('/') {
        TargetLocation::Path(location.to_string())
    } else if location.starts_with("http://") || location.starts_with("https://") {
        url::Url::parse(location).map_err(|error| format!("TARGET is not a URL: {error}"))?;
        TargetLocation::Url(location.to_string())
    } else {
        return Err(format!("{TARGET_FORMS}; got {target:?}"));
    };
    Ok(ApiTarget { method, location })
}

/// The request URL: the API's `base_url` followed by the path, or a URL on
/// the origin of `base_url`. The bearer token goes with the request, so no
/// other host may be named. A trailing slash on `base_url` is not doubled.
pub fn resolve_url(base_url: &str, location: &TargetLocation) -> Result<String, String> {
    let url = match location {
        TargetLocation::Path(path) => join_base_path(base_url, path),
        TargetLocation::Url(url) => url.clone(),
    };
    let parsed = url::Url::parse(&url)
        .map_err(|error| format!("request URL {url:?} is invalid: {error}"))?;
    if matches!(location, TargetLocation::Url(_)) {
        let base = url::Url::parse(base_url)
            .map_err(|error| format!("base_url {base_url:?} is invalid: {error}"))?;
        if parsed.origin() != base.origin() {
            return Err(format!(
                "URL {url:?} is not on the origin of base_url {base_url:?}; the bearer token is only sent there"
            ));
        }
    }
    Ok(url)
}

/// Join an API base URL and an OpenAPI path without doubling a slash.
pub fn join_base_path(base_url: &str, path: &str) -> String {
    format!("{}{path}", base_url.trim_end_matches('/'))
}

/// `headers` on `request`, each in place of any header of the same name
/// (compared case-insensitively) it already carries: `[api.*] headers`
/// over what an operation sets, then `-H` over both.
pub fn with_headers<'a>(
    mut request: HttpRequest,
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> HttpRequest {
    for (name, value) in headers {
        request
            .headers
            .retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        request = request.with_header(name, value);
    }
    request
}

/// `Accept: application/json` unless set, and `Content-Type:
/// application/json` unless set when there is a body: the defaults of
/// `kurama api` and of the explorer.
pub fn with_default_headers(mut request: HttpRequest) -> HttpRequest {
    if request.header("accept").is_none() {
        request = request.with_header("Accept", "application/json");
    }
    if request.body.is_some() && request.header("content-type").is_none() {
        request = request.with_header("Content-Type", "application/json");
    }
    request
}

/// The request as `--dry-run` prints it on stderr: method, URL, headers
/// with credentials masked, and the body. `credential_headers` names the
/// header a configured source presents its credential in, on top of the
/// ones that always carry one.
pub fn render_dry_run(request: &HttpRequest, credential_headers: &[&str]) -> String {
    let mut lines = vec![format!("> {} {}", request.method, request.url)];
    for (name, value) in mask_secret_headers_with(&request.headers, credential_headers) {
        lines.push(format!("> {name}: {value}"));
    }
    match &request.body {
        Some(body) => match std::str::from_utf8(body) {
            Ok(text) => lines.push(format!("> body ({} bytes): {text}", body.len())),
            Err(_) => lines.push(format!("> body ({} bytes, binary)", body.len())),
        },
        None => lines.push("> (no body)".to_string()),
    }
    lines.join("\n")
}

/// The body as JSON when it is JSON.
pub fn body_as_json(body: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_slice(body).ok()
}

/// A JSON body laid out for a person: the tokens the server sent, with the
/// whitespace outside strings replaced by `to_string_pretty`'s two spaces.
/// `None` when the bytes are not exactly one JSON document, UTF-8 included.
///
/// Reserializing a `serde_json::Value` would print what the server never
/// sent: `10.00` as `10.0`, `1E5` as `100000.0`, a thirty-digit id in
/// exponent notation, one of two duplicate keys, `\/` unescaped. Nothing but
/// the space between tokens is touched here, so a number, a key order and an
/// escape reach the screen as they arrived.
pub fn indent_json(body: &[u8]) -> Option<String> {
    // `IgnoredAny` skips the string contents, so a body that is not UTF-8
    // gets past it; the text is what the tokens are then copied from.
    let text = std::str::from_utf8(body).ok()?;
    serde_json::from_str::<serde::de::IgnoredAny>(text).ok()?;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let mut pending = Pending::Nothing;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            _ if byte.is_ascii_whitespace() => index += 1,
            b':' => {
                out.push_str(": ");
                index += 1;
            }
            b',' => {
                out.push(',');
                pending = Pending::Break;
                index += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if !matches!(pending, Pending::BreakUnlessEmpty) {
                    break_line(&mut out, depth);
                }
                out.push(char::from(byte));
                pending = Pending::Nothing;
                index += 1;
            }
            _ => {
                if !matches!(pending, Pending::Nothing) {
                    break_line(&mut out, depth);
                }
                // A token starts and ends on an ASCII byte, so the slice is
                // never cut through a character.
                let end = token_end(bytes, index)?;
                out.push_str(&text[index..end]);
                index = end;
                pending = if byte == b'{' || byte == b'[' {
                    depth += 1;
                    Pending::BreakUnlessEmpty
                } else {
                    Pending::Nothing
                };
            }
        }
    }
    Some(out)
}

/// What the next token owes the line it lands on.
enum Pending {
    /// It continues the line it is on: the start of the document, or the
    /// value after a `:`.
    Nothing,
    /// It starts a new line: the token after a `,`.
    Break,
    /// It starts a new line unless it closes the container that just
    /// opened, which keeps `{}` and `[]` on one line.
    BreakUnlessEmpty,
}

fn break_line(out: &mut String, depth: usize) {
    out.push('\n');
    out.extend(std::iter::repeat_n(' ', depth * 2));
}

/// The end of the token at `start`: an opening bracket, a string with its
/// escapes, or a number or literal up to the next structural character.
fn token_end(body: &[u8], start: usize) -> Option<usize> {
    match body[start] {
        b'{' | b'[' => Some(start + 1),
        b'"' => {
            let mut index = start + 1;
            loop {
                match *body.get(index)? {
                    b'\\' => index += 2,
                    b'"' => return Some(index + 1),
                    _ => index += 1,
                }
            }
        }
        _ => Some(
            body[start..]
                .iter()
                .position(|byte| {
                    byte.is_ascii_whitespace()
                        || matches!(byte, b'{' | b'}' | b'[' | b']' | b',' | b':' | b'"')
                })
                .map_or(body.len(), |offset| start + offset),
        ),
    }
}

/// The `--json` envelope: `{"status", "headers", "body"}`. Header names are
/// lowercased and repeated headers are joined with `, `, except `set-cookie`,
/// which is always an array with one string per cookie (RFC 6265 §3.1: an
/// `Expires` date has a comma of its own); the body is JSON when it parses
/// as JSON, otherwise a string.
pub fn response_envelope(response: &HttpResponse) -> serde_json::Value {
    let mut headers = serde_json::Map::new();
    for (name, value) in &response.headers {
        let name = name.to_ascii_lowercase();
        if name == "set-cookie" {
            let entry = headers
                .entry(name)
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            if let serde_json::Value::Array(cookies) = entry {
                cookies.push(serde_json::Value::String(value.clone()));
            }
            continue;
        }
        let joined = match headers.get(&name).and_then(serde_json::Value::as_str) {
            Some(previous) => format!("{previous}, {value}"),
            None => value.clone(),
        };
        headers.insert(name, serde_json::Value::String(joined));
    }
    let body = body_as_json(&response.body).unwrap_or_else(|| {
        serde_json::Value::String(String::from_utf8_lossy(&response.body).into_owned())
    });
    serde_json::json!({
        "status": response.status,
        "headers": headers,
        "body": body,
    })
}

/// The reason phrase of a status code, for messages and the explorer.
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// One line of at most `limit` characters from a response body, for error
/// messages.
pub fn body_excerpt(body: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(body)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut excerpt: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        excerpt.push('…');
    }
    excerpt
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn targets_are_paths_urls_or_method_and_path() {
        assert_eq!(
            parse_target("/user").unwrap(),
            ApiTarget {
                method: None,
                location: TargetLocation::Path("/user".into())
            }
        );
        assert_eq!(
            parse_target("https://api.example.com/items?x=1").unwrap(),
            ApiTarget {
                method: None,
                location: TargetLocation::Url("https://api.example.com/items?x=1".into())
            }
        );
        assert_eq!(
            parse_target("DELETE /items/1").unwrap(),
            ApiTarget {
                method: Some("DELETE".into()),
                location: TargetLocation::Path("/items/1".into())
            }
        );
        assert_eq!(
            parse_target("  POST   https://api.example.com/items ").unwrap(),
            ApiTarget {
                method: Some("POST".into()),
                location: TargetLocation::Url("https://api.example.com/items".into())
            }
        );
        for bad in ["user", "issues/list", "get /items", "", "https://"] {
            assert!(parse_target(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn urls_join_base_and_path_without_doubling_the_slash() {
        assert_eq!(
            resolve_url(
                "https://api.example.com/v1/",
                &TargetLocation::Path("/items".into())
            )
            .unwrap(),
            "https://api.example.com/v1/items"
        );
        assert_eq!(
            resolve_url(
                "https://api.example.com/v1",
                &TargetLocation::Url("https://api.example.com/v2/x?page=2".into())
            )
            .unwrap(),
            "https://api.example.com/v2/x?page=2"
        );
        assert!(
            resolve_url(
                "https://api.example.com",
                &TargetLocation::Url("https://other.example.com/x".into())
            )
            .unwrap_err()
            .contains("origin")
        );
        assert!(resolve_url("nope", &TargetLocation::Path("/x".into())).is_err());
    }

    #[test]
    fn dry_run_masks_the_bearer_token_and_shows_the_body() {
        let request = HttpRequest::new("POST", "https://api.example.com/items")
            .with_header("Authorization", "Bearer secret")
            .with_header("Content-Type", "application/json")
            .with_body(br#"{"name":"x"}"#.to_vec());
        let rendered = render_dry_run(&request, &[]);
        assert_eq!(
            rendered,
            "> POST https://api.example.com/items\n\
             > Authorization: Bearer ****\n\
             > Content-Type: application/json\n\
             > body (12 bytes): {\"name\":\"x\"}"
        );
        assert!(
            render_dry_run(&HttpRequest::new("GET", "https://x"), &[]).ends_with("> (no body)")
        );
    }

    /// The header a `kind = "token"` source names is masked as well; without
    /// it, `--dry-run` on an `xi-api-key` source would print the placeholder
    /// where `-v` on a real run prints the credential.
    #[test]
    fn dry_run_masks_the_header_the_source_names() {
        let request = HttpRequest::new("GET", "https://api.example.com/v1/voices")
            .with_header("xi-api-key", "<token>");
        assert!(
            render_dry_run(&request, &["xi-api-key"]).contains("> xi-api-key: ****"),
            "{}",
            render_dry_run(&request, &["xi-api-key"])
        );
    }

    #[test]
    fn envelope_parses_json_bodies_and_joins_repeated_headers() {
        let response = HttpResponse {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("Vary".into(), "Accept".into()),
                ("vary".into(), "Origin".into()),
                (
                    "Set-Cookie".into(),
                    "a=1; Expires=Wed, 21 Oct 2026 07:28:00 GMT".into(),
                ),
                ("set-cookie".into(), "b=2".into()),
            ],
            body: br#"{"login":"octocat"}"#.to_vec(),
        };
        let envelope = response_envelope(&response);
        assert_eq!(envelope["status"], 200);
        assert_eq!(envelope["headers"]["content-type"], "application/json");
        assert_eq!(envelope["headers"]["vary"], "Accept, Origin");
        // RFC 6265 §3.1: cookies are never folded into one field, and an
        // Expires date carries a comma of its own.
        assert_eq!(
            envelope["headers"]["set-cookie"],
            json!(["a=1; Expires=Wed, 21 Oct 2026 07:28:00 GMT", "b=2"])
        );
        let one = response_envelope(&HttpResponse {
            status: 200,
            headers: vec![("Set-Cookie".into(), "a=1".into())],
            body: Vec::new(),
        });
        assert_eq!(one["headers"]["set-cookie"], json!(["a=1"]));
        assert_eq!(envelope["body"]["login"], "octocat");
        let text = response_envelope(&HttpResponse {
            status: 204,
            headers: vec![],
            body: b"plain".to_vec(),
        });
        assert_eq!(text["body"], "plain");
    }

    #[test]
    fn headers_replace_the_same_name_whatever_its_case_and_keep_the_rest() {
        let request = with_headers(
            HttpRequest::new("GET", "https://x")
                .with_header("accept", "application/json")
                .with_header("X-Trace", "1"),
            [
                ("Accept", "application/vnd.github+json"),
                ("X-Api-Version", "2"),
            ],
        );
        assert_eq!(
            request.headers,
            [
                ("X-Trace".to_string(), "1".to_string()),
                (
                    "Accept".to_string(),
                    "application/vnd.github+json".to_string()
                ),
                ("X-Api-Version".to_string(), "2".to_string()),
            ]
        );
        let request = with_headers(request, [("x-api-version", "3")]);
        assert_eq!(request.header("X-Api-Version"), Some("3"));
        assert_eq!(request.headers.len(), 3);
    }

    #[test]
    fn default_headers_fill_accept_and_content_type_only_when_absent() {
        let request = with_default_headers(
            HttpRequest::new("POST", "https://x")
                .with_header("accept", "text/plain")
                .with_body(vec![]),
        );
        assert_eq!(request.header("accept"), Some("text/plain"));
        assert_eq!(request.header("content-type"), Some("application/json"));
        let request = with_default_headers(HttpRequest::new("GET", "https://x"));
        assert_eq!(request.header("accept"), Some("application/json"));
        assert!(request.header("content-type").is_none());
    }

    #[test]
    fn reason_phrases_cover_the_common_statuses() {
        assert_eq!(reason_phrase(200), "OK");
        assert_eq!(reason_phrase(404), "Not Found");
        assert_eq!(reason_phrase(599), "");
    }

    #[test]
    fn excerpt_is_one_line_and_bounded() {
        assert_eq!(body_excerpt(b"{\n  \"a\": 1\n}", 100), "{ \"a\": 1 }");
        assert_eq!(body_excerpt(b"abcdef", 3), "abc…");
    }

    #[test]
    fn base_and_path_join_with_one_slash() {
        assert_eq!(
            join_base_path("https://x/v1/", "/items"),
            "https://x/v1/items"
        );
        assert_eq!(join_base_path("https://x", "/items"), "https://x/items");
    }

    fn indented(body: &str) -> Option<String> {
        indent_json(body.as_bytes())
    }

    #[test]
    fn indenting_opens_containers_and_leaves_scalars_and_empty_ones_on_one_line() {
        assert_eq!(
            indented(r#"{"login":"octocat","ids":[1,2]}"#).unwrap(),
            "{\n  \"login\": \"octocat\",\n  \"ids\": [\n    1,\n    2\n  ]\n}"
        );
        assert_eq!(
            indented(r#"[{"a":{}},[]]"#).unwrap(),
            "[\n  {\n    \"a\": {}\n  },\n  []\n]"
        );
        for scalar in [r#""ok""#, "1", "null", "{}", "[]"] {
            assert_eq!(indented(scalar).unwrap(), scalar);
        }
    }

    #[test]
    fn indenting_prints_the_numbers_keys_and_escapes_the_server_sent() {
        for (body, expected) in [
            (r#"{"amount":10.00}"#, "{\n  \"amount\": 10.00\n}"),
            (r#"{"n":1E5}"#, "{\n  \"n\": 1E5\n}"),
            (
                r#"{"id":123456789012345678901234567890}"#,
                "{\n  \"id\": 123456789012345678901234567890\n}",
            ),
            (r#"{"a":1,"a":2}"#, "{\n  \"a\": 1,\n  \"a\": 2\n}"),
            (r#"{"s":"é\/"}"#, "{\n  \"s\": \"é\\/\"\n}"),
        ] {
            assert_eq!(indented(body).unwrap(), expected, "body {body:?}");
        }
    }

    #[test]
    fn indenting_never_changes_the_inside_of_a_string() {
        assert_eq!(
            indented(r#"{"s":"a{b, c: d \"e\" \\ ","t":"  "}"#).unwrap(),
            "{\n  \"s\": \"a{b, c: d \\\"e\\\" \\\\ \",\n  \"t\": \"  \"\n}"
        );
    }

    #[test]
    fn indenting_an_already_indented_body_changes_nothing() {
        let once = indented(r#"{"a":[1,{"b":null}],"c":{}}"#).unwrap();
        assert_eq!(indented(&once).unwrap(), once);
    }

    #[test]
    fn only_one_whole_json_document_is_indented() {
        for body in [
            "",
            "   ",
            "not json",
            "{\"a\":1}\n{\"a\":2}",
            "{\"a\":1} trailing",
            "{\"a\":1",
            "{\"a\":\"unterminated}",
        ] {
            assert!(indent_json(body.as_bytes()).is_none(), "{body:?} passed");
        }
        assert!(indent_json(&[b'"', 0xff, b'"']).is_none(), "not UTF-8");
    }

    #[test]
    fn a_body_is_json_only_when_it_parses() {
        assert_eq!(
            body_as_json(br#"{"a":1}"#),
            Some(serde_json::json!({"a": 1}))
        );
        assert!(body_as_json(b"not json").is_none());
    }
}
