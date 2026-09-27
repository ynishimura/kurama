//! `kurama api --pages`: where the next page of a response is (an RFC 8288
//! `Link: rel="next"`, or a cursor at a body path the caller names), the
//! request URL that fetches it, and why paging stops.

use crate::domain::functions::api_request::{TargetLocation, body_as_json, resolve_url};
use crate::domain::types::http::HttpResponse;

/// How the next page is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageStyle {
    /// The `Link` header's `rel="next"` URL.
    Link,
    /// A value at `path` in the JSON body, sent as the query parameter
    /// `param` of the first request.
    Cursor(CursorStyle),
}

/// `--cursor PATH=PARAM`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorStyle {
    /// The body path as given, for messages and the plan.
    pub path: String,
    /// The same path split into its keys (or array indexes).
    pub keys: Vec<String>,
    pub param: String,
}

impl CursorStyle {
    /// `PATH=PARAM`: PATH a dot-separated body path (`meta.next_cursor`,
    /// a leading `.` allowed), PARAM a query parameter name.
    pub fn parse(option: &str) -> Result<Self, String> {
        let invalid = || {
            format!(
                "--cursor {option:?} is not PATH=PARAM (a body path such as meta.next_cursor, then the query parameter it is sent as)"
            )
        };
        let (path, param) = option.rsplit_once('=').ok_or_else(invalid)?;
        let path = path.trim();
        let param = param.trim();
        let keys: Vec<String> = path
            .strip_prefix('.')
            .unwrap_or(path)
            .split('.')
            .map(str::to_string)
            .collect();
        if param.is_empty() || keys.iter().any(String::is_empty) {
            return Err(invalid());
        }
        Ok(Self {
            path: path.to_string(),
            keys,
            param: param.to_string(),
        })
    }
}

/// What a response says about the page after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextPage {
    /// The URL of the next page's request.
    Url(String),
    /// The response says there is no next page.
    Last,
    /// The response carries nothing this style reads; the reason says what.
    Unsupported(String),
    /// The response names a next page kurama will not request; the reason
    /// says why.
    Refused(String),
}

/// Why paging stopped, as the stop report names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `--pages` requests were made and a next page was still named.
    Limit,
    /// A response said it was the last page.
    Last,
    /// Whether more pages exist could not be told.
    Unsupported,
    /// A next page was named and not requested.
    Refused,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Limit => "limit",
            Self::Last => "last",
            Self::Unsupported => "unsupported",
            Self::Refused => "refused",
        }
    }
}

/// The page after `response`, which answered the request to `request_url`.
///
/// A marker missing from the first page cannot be told apart from an API
/// that pages another way, so it is `Unsupported`; missing from a later
/// page, after earlier ones carried it, it is the last page (GitHub drops
/// `Link` on a single page, AWS drops `NextToken` on the last). A body that
/// is not JSON is `Unsupported` on any page: nothing in it says "last".
pub fn next_page(
    style: &PageStyle,
    base_url: &str,
    first_url: &str,
    request_url: &str,
    response: &HttpResponse,
    first_page: bool,
) -> NextPage {
    let missing = |what: String| {
        if first_page {
            NextPage::Unsupported(what)
        } else {
            NextPage::Last
        }
    };
    let url = match style {
        PageStyle::Link => match link_next(&response.headers) {
            None => {
                return missing(
                    "the response carries no Link header; name the cursor with --cursor PATH=PARAM"
                        .into(),
                );
            }
            Some(None) => return NextPage::Last,
            Some(Some(next)) => {
                match url::Url::parse(request_url).and_then(|url| url.join(&next)) {
                    Ok(url) => url.to_string(),
                    Err(error) => {
                        return NextPage::Refused(format!(
                            "the Link rel=\"next\" {next:?} is not a URL: {error}"
                        ));
                    }
                }
            }
        },
        PageStyle::Cursor(cursor) => {
            let Some(body) = body_as_json(&response.body) else {
                return NextPage::Unsupported(format!(
                    "the response body is not JSON, so it has no {}",
                    cursor.path
                ));
            };
            let value = match value_at(&body, &cursor.keys) {
                None => return missing(format!("the response body has no {}", cursor.path)),
                Some(serde_json::Value::Null) => return NextPage::Last,
                Some(serde_json::Value::String(text)) if text.is_empty() => return NextPage::Last,
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(serde_json::Value::Number(number)) => number.to_string(),
                Some(other) => {
                    return NextPage::Refused(format!(
                        "{} is {other}, not a string or a number",
                        cursor.path
                    ));
                }
            };
            match with_query_param(first_url, &cursor.param, &value) {
                Ok(url) => url,
                Err(error) => return NextPage::Refused(error),
            }
        }
    };
    // The credential goes with the request, so the next page has to be on
    // the API's own origin, as a TARGET URL has to.
    match resolve_url(base_url, &TargetLocation::Url(url)) {
        Ok(url) => NextPage::Url(url),
        Err(error) => NextPage::Refused(format!("the next page is not requested: {error}")),
    }
}

/// The `rel="next"` target of the `Link` headers: `None` without any
/// `Link` header, `Some(None)` when none of its links is `next`. Commas and
/// semicolons inside `<...>` or a quoted parameter value do not split, so
/// `title="a; rel=next"` is a title and not a relation.
pub fn link_next(headers: &[(String, String)]) -> Option<Option<String>> {
    let values: Vec<&str> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("link"))
        .map(|(_, value)| value.as_str())
        .collect();
    if values.is_empty() {
        return None;
    }
    let next = values
        .into_iter()
        .flat_map(|value| split_unquoted(value, ','))
        .find_map(|link| {
            let mut parts = split_unquoted(link, ';').into_iter();
            let target = parts.next()?.trim().strip_prefix('<')?.strip_suffix('>')?;
            parts.any(is_next).then(|| target.trim().to_string())
        });
    Some(next)
}

/// `text` split at `separator` where it is outside `<...>` and outside a
/// quoted string (whose `\` escapes the next character).
fn split_unquoted(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut angle, mut escaped) = (0, false, false, false);
    for (index, c) in text.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' if !angle => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            _ if c == separator && !quoted && !angle => {
                parts.push(&text[start..index]);
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Whether one link parameter (`rel="next last"`) names `next`.
fn is_next(param: &str) -> bool {
    param.split_once('=').is_some_and(|(name, value)| {
        name.trim().eq_ignore_ascii_case("rel")
            && value
                .trim()
                .trim_matches('"')
                .split_whitespace()
                .any(|rel| rel.eq_ignore_ascii_case("next"))
    })
}

/// The value at `keys` in `body`: an object key, or an array index.
fn value_at<'a>(body: &'a serde_json::Value, keys: &[String]) -> Option<&'a serde_json::Value> {
    keys.iter().try_fold(body, |value, key| match value {
        serde_json::Value::Object(fields) => fields.get(key),
        serde_json::Value::Array(items) => items.get(key.parse::<usize>().ok()?),
        _ => None,
    })
}

/// `url` with the query parameter `name` set to `value`, in place of any
/// value it had. The other parameters keep the bytes they were written
/// with (`?a` stays `a`, `%20` stays `%20`).
fn with_query_param(url: &str, name: &str, value: &str) -> Result<String, String> {
    let mut url = url::Url::parse(url).map_err(|error| format!("{url:?} is invalid: {error}"))?;
    let encode =
        |text: &str| url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    let mut pieces: Vec<String> = url
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|piece| {
            !piece.is_empty()
                && url::form_urlencoded::parse(piece.as_bytes())
                    .next()
                    .is_none_or(|(key, _)| key != name)
        })
        .map(str::to_string)
        .collect();
    pieces.push(format!("{}={}", encode(name), encode(value)));
    url.set_query(Some(&pieces.join("&")));
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://api.example.com/v1";

    fn response(headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn cursor(option: &str) -> PageStyle {
        PageStyle::Cursor(CursorStyle::parse(option).unwrap())
    }

    #[test]
    fn a_link_header_names_its_next_target_whatever_else_it_holds() {
        let headers = |value: &str| vec![("Link".to_string(), value.to_string())];
        assert_eq!(
            link_next(&headers(
                r#"<https://x/items?page=1>; rel="prev", <https://x/items?page=3>; rel="next""#
            )),
            Some(Some("https://x/items?page=3".into()))
        );
        assert_eq!(
            link_next(&headers(
                "</items?page=2>; title=\"a, b\"; rel=\"last next\""
            )),
            Some(Some("/items?page=2".into()))
        );
        assert_eq!(
            link_next(&headers("<https://x/?page=2>; REL=Next")),
            Some(Some("https://x/?page=2".into()))
        );
        assert_eq!(
            link_next(&headers(r#"<https://x/?page=1>; rel="prev""#)),
            Some(None)
        );
        assert_eq!(
            link_next(&[
                ("link".into(), r#"<https://x/a>; rel="first""#.into()),
                ("LINK".into(), r#"<https://x/b>; rel="next""#.into()),
            ]),
            Some(Some("https://x/b".into()))
        );
        assert_eq!(link_next(&[("x-next".into(), "<a>".into())]), None);
        assert_eq!(
            link_next(&headers(
                r#"<https://x/?p=1>; title="a; rel=next, b"; rel="prev""#
            )),
            Some(None),
            "a quoted title is not split into a relation"
        );
        assert_eq!(
            link_next(&headers(r#"<https://x/?a=1,2;b>; rel="next""#)),
            Some(Some("https://x/?a=1,2;b".into()))
        );
    }

    #[test]
    fn the_link_next_resolves_against_the_request_and_stays_on_the_origin() {
        let next = |link: &str, first: bool| {
            next_page(
                &PageStyle::Link,
                BASE,
                "https://api.example.com/v1/items",
                "https://api.example.com/v1/items?page=1",
                &response(&[("Link", link)], "[]"),
                first,
            )
        };
        assert_eq!(
            next(r#"</v1/items?page=2>; rel="next""#, true),
            NextPage::Url("https://api.example.com/v1/items?page=2".into())
        );
        assert_eq!(
            next(r#"</v1/items?page=1>; rel="prev""#, true),
            NextPage::Last
        );
        assert!(matches!(
            next(r#"<https://evil.example/items>; rel="next""#, false),
            NextPage::Refused(reason) if reason.contains("not on the origin")
        ));
    }

    #[test]
    fn a_missing_marker_is_unsupported_on_the_first_page_and_the_end_after_it() {
        let next = |style: &PageStyle, body: &str, first: bool| {
            next_page(style, BASE, BASE, BASE, &response(&[], body), first)
        };
        assert!(matches!(
            next(&PageStyle::Link, "[]", true),
            NextPage::Unsupported(reason) if reason.contains("no Link header")
        ));
        assert_eq!(next(&PageStyle::Link, "[]", false), NextPage::Last);
        let style = cursor("meta.next=cursor");
        assert!(matches!(
            next(&style, "{}", true),
            NextPage::Unsupported(reason) if reason == "the response body has no meta.next"
        ));
        assert!(matches!(
            next(&style, "not json", true),
            NextPage::Unsupported(reason) if reason.contains("not JSON")
        ));
        assert_eq!(next(&style, "{}", false), NextPage::Last);
        assert_eq!(next(&style, r#"{"meta":{}}"#, false), NextPage::Last);
        assert!(matches!(
            next(&style, "<html>", false),
            NextPage::Unsupported(reason) if reason.contains("not JSON")
        ));
    }

    #[test]
    fn a_cursor_goes_into_the_first_request_as_its_parameter() {
        let next = |body: &str| {
            next_page(
                &cursor(".meta.next=cursor"),
                BASE,
                "https://api.example.com/v1/items?limit=2&cursor=old",
                "https://api.example.com/v1/items?limit=2&cursor=c2",
                &response(&[], body),
                true,
            )
        };
        assert_eq!(
            next(r#"{"meta":{"next":"c 3"}}"#),
            NextPage::Url("https://api.example.com/v1/items?limit=2&cursor=c+3".into())
        );
        assert_eq!(
            next(r#"{"meta":{"next":40}}"#),
            NextPage::Url("https://api.example.com/v1/items?limit=2&cursor=40".into())
        );
        assert_eq!(next(r#"{"meta":{"next":null}}"#), NextPage::Last);
        assert_eq!(next(r#"{"meta":{"next":""}}"#), NextPage::Last);
        assert!(matches!(
            next(r#"{"meta":{"next":{"a":1}}}"#),
            NextPage::Refused(reason) if reason.contains("not a string or a number")
        ));
        assert_eq!(
            next_page(
                &cursor("pages.1.token=t"),
                BASE,
                BASE,
                BASE,
                &response(&[], r#"{"pages":[{"token":"a"},{"token":"b"}]}"#),
                true,
            ),
            NextPage::Url("https://api.example.com/v1?t=b".into())
        );
    }

    #[test]
    fn only_the_cursor_parameter_of_the_query_is_rewritten() {
        assert_eq!(
            with_query_param("https://x/items?a&b=%20c&cursor=old", "cursor", "n/1").unwrap(),
            "https://x/items?a&b=%20c&cursor=n%2F1"
        );
        assert_eq!(
            with_query_param("https://x/items", "after", "f 1").unwrap(),
            "https://x/items?after=f+1"
        );
    }

    #[test]
    fn a_cursor_option_is_a_body_path_and_a_parameter() {
        let parsed = CursorStyle::parse("meta.next_cursor=cursor").unwrap();
        assert_eq!(parsed.keys, ["meta", "next_cursor"]);
        assert_eq!(parsed.param, "cursor");
        assert_eq!(parsed.path, "meta.next_cursor");
        assert_eq!(CursorStyle::parse(".next=t").unwrap().keys, ["next"]);
        for bad in ["next", "next=", "=t", "a..b=t", ".=t"] {
            assert!(CursorStyle::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn stop_reasons_have_stable_names() {
        assert_eq!(StopReason::Limit.as_str(), "limit");
        assert_eq!(StopReason::Last.as_str(), "last");
        assert_eq!(StopReason::Unsupported.as_str(), "unsupported");
        assert_eq!(StopReason::Refused.as_str(), "refused");
    }
}
