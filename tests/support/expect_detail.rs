//! Detailed expectations of [`Verification`]: what one API request, one STS
//! call or the description requests of the focused run carried, and which
//! files were written by pattern. Each reads the run the `expect_*` methods
//! are pointed at (the last one, or the `[[expect.runs]]` block's) and
//! records one check, so a multi-process case says which process was wrong.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{StsCallExpect, Verification};

/// What one API request is expected to carry; an absent key is not checked.
#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields, default)]
pub struct ApiCallExpect {
    pub method: Option<String>,
    /// Path and query.
    pub path: Option<String>,
    pub body: Option<String>,
    /// Headers by lower-cased name, each with exactly this value.
    pub headers: BTreeMap<String, String>,
    /// Headers the request carries, whatever their value (lower-cased).
    pub headers_include: Vec<String>,
    pub authorization: Option<String>,
    /// The request carries no `Authorization` header.
    pub no_authorization: bool,
    /// The SigV4 signature covers these headers (lower-cased).
    pub signed_headers_include: Vec<String>,
    /// The access key id of the SigV4 credential scope.
    pub signing_access_key: Option<String>,
    /// The region of the SigV4 credential scope: where the request was sent
    /// as far as the signature says.
    pub signing_region: Option<String>,
}

/// How many requests of a run match a shape, whatever their order: for a
/// run whose requests are made concurrently and land in any order.
#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields, default)]
pub struct ApiRequestMatch {
    pub method: Option<String>,
    /// Path and query, exactly.
    pub path: Option<String>,
    pub path_contains: Option<String>,
    pub path_excludes: Option<String>,
    pub count: usize,
}

impl ApiCallExpect {
    /// Whether the entry says what the `Authorization` header is -- a
    /// value, none, or a signature described by its region -- so the
    /// run-level check can leave that request to it.
    pub fn names_authorization(&self) -> bool {
        self.authorization.is_some() || self.no_authorization || self.signing_region.is_some()
    }
}

/// `pattern` with `*` standing for any text, against the whole of `text`.
pub fn wildcard_matches(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => {
            text.starts_with(head)
                && (0..=text.len() - head.len())
                    .filter(|offset| text.is_char_boundary(head.len() + offset))
                    .any(|offset| wildcard_matches(tail, &text[head.len() + offset..]))
        }
    }
}

impl Verification {
    pub fn expect_stdout_starts_with(&mut self, prefix: &str) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            &format!("stdout starts with {prefix:?}"),
            stdout.starts_with(prefix),
            format!("observed {stdout:?}"),
        )
    }

    /// stdout carries none of `needle`: a control character that must not
    /// reach the terminal, a value that must stay masked.
    pub fn expect_stdout_excludes(&mut self, needle: &str) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            &format!("stdout excludes {needle:?}"),
            !stdout.contains(needle),
            format!("observed {stdout:?}"),
        )
    }

    pub fn expect_stdout_lines(&mut self, count: usize) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        let actual = stdout.lines().count();
        self.check(
            &format!("stdout has {count} line(s)"),
            actual == count,
            format!("observed {actual} line(s): {stdout:?}"),
        )
    }

    /// The API received exactly `count` requests, whatever each carried:
    /// for a run whose requests are checked one by one.
    pub fn expect_api_call_count(&mut self, count: usize) -> &mut Self {
        let calls = self.focused().api_calls.clone();
        self.check(
            &format!("the API receives {count} request(s)"),
            calls.len() == count,
            format!("observed {calls:?}"),
        )
    }

    /// Request `call` of the run carries what `expect` names.
    pub fn expect_api_call(&mut self, call: usize, expect: &ApiCallExpect) -> &mut Self {
        let observed = self.focused().api_calls.get(call).cloned();
        let ok = observed.as_ref().is_some_and(|request| {
            expect.method.as_ref().is_none_or(|m| &request.method == m)
                && expect.path.as_ref().is_none_or(|p| &request.path == p)
                && expect.body.as_ref().is_none_or(|b| &request.body == b)
                && expect
                    .headers
                    .iter()
                    .all(|(name, value)| request.headers.get(name) == Some(value))
                && expect
                    .headers_include
                    .iter()
                    .all(|name| request.headers.contains_key(name))
                && expect
                    .authorization
                    .as_ref()
                    .is_none_or(|a| request.authorization.as_ref() == Some(a))
                && (!expect.no_authorization || request.authorization.is_none())
                && (expect.signed_headers_include.is_empty()
                    && expect.signing_access_key.is_none()
                    && expect.signing_region.is_none()
                    || request.sigv4.as_ref().is_some_and(|signature| {
                        signature.valid
                            && expect
                                .signed_headers_include
                                .iter()
                                .all(|name| signature.signed_headers.contains(name))
                            && expect
                                .signing_access_key
                                .as_ref()
                                .is_none_or(|key| &signature.access_key == key)
                            && expect
                                .signing_region
                                .as_ref()
                                .is_none_or(|region| &signature.region == region)
                    }))
        });
        self.check(
            &format!("API request {call} carries {expect:?}"),
            ok,
            format!("observed {observed:?}"),
        )
    }

    /// Exactly `count` requests of the run match each shape, in any order.
    pub fn expect_api_requests_matching(&mut self, matches: &[ApiRequestMatch]) -> &mut Self {
        let calls = self.focused().api_calls.clone();
        let counted: Vec<usize> = matches
            .iter()
            .map(|expect| {
                calls
                    .iter()
                    .filter(|call| {
                        expect.method.as_ref().is_none_or(|m| &call.method == m)
                            && expect.path.as_ref().is_none_or(|p| &call.path == p)
                            && expect
                                .path_contains
                                .as_ref()
                                .is_none_or(|text| call.path.contains(text.as_str()))
                            && expect
                                .path_excludes
                                .as_ref()
                                .is_none_or(|text| !call.path.contains(text.as_str()))
                    })
                    .count()
            })
            .collect();
        let ok = counted
            .iter()
            .zip(matches)
            .all(|(found, expect)| *found == expect.count);
        self.check(
            &format!("API requests matching {matches:?}"),
            ok,
            format!(
                "counted {counted:?}; observed {:?}",
                calls
                    .iter()
                    .map(|call| format!("{} {}", call.method, call.path))
                    .collect::<Vec<_>>()
            ),
        )
    }

    /// Every API request of the run carries a valid signature with the role
    /// credentials for `service` in `region`, however many there are.
    pub fn expect_api_signed(&mut self, service: &str, region: &str) -> &mut Self {
        let calls = self.focused().api_calls.clone();
        let ok = !calls.is_empty()
            && calls.iter().all(|call| {
                call.sigv4.as_ref().is_some_and(|signature| {
                    signature.valid
                        && signature.service == service
                        && signature.region == region
                        && signature.security_token.as_deref() == Some(super::RESULT_TOKEN)
                })
            });
        self.check(
            &format!("every API request is validly signed for {service} in {region}"),
            ok,
            format!("observed {calls:?}"),
        )
    }

    /// The `error[...]` line of stderr is exactly `error[CODE]: message`.
    pub fn expect_error_line(&mut self, code: &str, message: &str) -> &mut Self {
        let stderr = self.focused().stderr.clone();
        let line = stderr.lines().find(|line| line.starts_with("error["));
        self.check(
            &format!("the error line is `error[{code}]: {message}`"),
            line == Some(format!("error[{code}]: {message}").as_str()),
            format!("observed {line:?}"),
        )
    }

    /// Some line of stdout has exactly these whitespace-separated words.
    pub fn expect_stdout_line_words(&mut self, words: &[&str]) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        self.check(
            &format!("some stdout line reads {words:?}"),
            stdout
                .lines()
                .any(|line| line.split_whitespace().collect::<Vec<_>>() == words),
            format!("observed {stdout:?}"),
        )
    }

    /// A table on stdout: some line holds `cell` at the column where the
    /// first line holds `header`.
    pub fn expect_stdout_aligned_under(&mut self, header: &str, cell: &str) -> &mut Self {
        let stdout = self.focused().stdout.clone();
        let column = stdout.lines().next().and_then(|line| line.find(header));
        let ok = column.is_some()
            && stdout
                .lines()
                .skip(1)
                .any(|line| line.find(cell).is_some() && line.find(cell) == column);
        self.check(
            &format!("{cell:?} sits under the {header:?} column"),
            ok,
            format!("header column {column:?}; observed {stdout:?}"),
        )
    }

    /// The exact sequence of STS calls of the run, each carrying what its
    /// expectation names.
    pub fn expect_sts_calls(&mut self, calls: &[StsCallExpect]) -> &mut Self {
        let observed = self.focused().sts_calls.clone();
        let ok = observed.len() == calls.len()
            && observed
                .iter()
                .zip(calls)
                .all(|(call, expect)| expect.matches(call));
        self.check(
            &format!("STS calls are exactly {calls:?}"),
            ok,
            format!("observed {observed:?}"),
        )
    }

    /// The description was requested `count` time(s) in the run, every
    /// request conditional or not, every one answered with `status`.
    pub fn expect_spec_calls(&mut self, count: usize, conditional: bool, status: u16) -> &mut Self {
        let index = self.focused_index();
        self.expect_run_spec_calls(index, count, conditional, status)
    }

    /// Every description request of the run carries this `Authorization`
    /// header (`None`: none at all).
    pub fn expect_spec_authorization(&mut self, authorization: Option<&str>) -> &mut Self {
        let calls = self.focused().spec_calls.clone();
        let ok = !calls.is_empty()
            && calls
                .iter()
                .all(|call| call.authorization.as_deref() == authorization);
        self.check(
            &format!("the description is requested with authorization {authorization:?}"),
            ok,
            format!("observed {calls:?}"),
        )
    }

    /// Across all runs, a file matching each `include` pattern was written
    /// and none matching an `exclude` pattern was (`*` stands for any text).
    pub fn expect_files_written_matching(
        &mut self,
        include: &[&str],
        exclude: &[&str],
    ) -> &mut Self {
        let files = self.observed.files_written.clone();
        let ok = include
            .iter()
            .all(|pattern| files.iter().any(|file| wildcard_matches(pattern, file)))
            && exclude
                .iter()
                .all(|pattern| !files.iter().any(|file| wildcard_matches(pattern, file)));
        self.check(
            &format!("files written match {include:?} and none match {exclude:?}"),
            ok,
            format!("written: {files:?}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::wildcard_matches;

    #[test]
    fn a_wildcard_stands_for_any_text_including_slashes() {
        assert!(wildcard_matches(
            "*/openapi/*.body",
            "home/.cache/kurama/openapi/a.body"
        ));
        assert!(wildcard_matches("*.body", "a.body"));
        assert!(wildcard_matches("a.body", "a.body"));
        assert!(!wildcard_matches("*.body", "a.json"));
        assert!(!wildcard_matches("openapi/*.body", "home/openapi/a.body"));
    }
}
