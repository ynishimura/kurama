//! `kurama api --pages N`: up to N requests, each through `ApiRuntime::call`,
//! each page one line on stdout as it arrives, and why paging stopped as the
//! last line on stderr.
//!
//! A page's line is its body as JSON (a body that is not JSON is a JSON
//! string, as in the envelope), its `--json` envelope, or its `--jq` results.
//! The stop report is `# pages: ...` text, or with `--json` / `--jq` one JSON
//! document `{"schema_version": 1, "pages": {"fetched", "limit", "stopped",
//! "next", "detail"}}`. A page that fails ends the run with the error of a
//! single request, its message starting `page N:`, in place of the report:
//! every line already on stdout is a page that was fetched whole.

use std::io::Write;

use anyhow::{Context, Result};
use serde_json::json;

use super::api::{fitted, http_error, result_lines, send_request};
use super::api_command::{ApiOutputOptions, ApiPages};
use crate::adapters::config::ApiProfile;
use crate::console::progress;
use crate::domain::functions::api_pages::{NextPage, PageStyle, StopReason, next_page};
use crate::domain::functions::api_request::response_envelope;
use crate::ports::{HttpRequest, HttpResponse};
use crate::shell::api_runtime::ApiRuntime;

/// The version of the stop report's shape.
const SCHEMA_VERSION: u32 = 1;

pub(super) async fn fetch_pages(
    runtime: &ApiRuntime,
    api: &ApiProfile,
    options: &ApiOutputOptions,
    pages: &ApiPages,
    mut request: HttpRequest,
) -> Result<()> {
    let first_url = request.url.clone();
    let mut requested = vec![first_url.clone()];
    let mut fetched = 0;
    loop {
        let page = fetched + 1;
        let response = send_request(runtime, api, options.verbose, request.clone())
            .await
            .with_context(|| format!("page {page}"))?;
        print_page(&fitted(&response, options), options).with_context(|| format!("page {page}"))?;
        if !response.is_success() {
            return Err(anyhow::Error::from(http_error(&response)).context(format!("page {page}")));
        }
        fetched = page;
        let next = next_page(
            &pages.style,
            &api.base_url,
            &first_url,
            &request.url,
            &response,
            page == 1,
        );
        let (stopped, next, detail) = match next {
            // A server that ignores the cursor (a PARAM it does not know)
            // or echoes it back would answer the same page up to the limit.
            NextPage::Url(url) if requested.contains(&url) => (
                StopReason::Unsupported,
                None,
                Some(format!("the next page is one already fetched: {url}")),
            ),
            NextPage::Url(url) if fetched < pages.limit => {
                requested.push(url.clone());
                request.url = url;
                continue;
            }
            NextPage::Url(url) => (StopReason::Limit, Some(url), None),
            NextPage::Last => (StopReason::Last, None, None),
            NextPage::Unsupported(reason) => (StopReason::Unsupported, None, Some(reason)),
            NextPage::Refused(reason) => (StopReason::Refused, None, Some(reason)),
        };
        let report = StopReport {
            fetched,
            limit: pages.limit,
            stopped,
            next,
            detail,
        };
        if options.json || options.jq.is_some() {
            // Written past the console: a JSON run silences progress lines,
            // and this one is the run's answer about the pages.
            eprintln!("{}", report.to_json());
        } else {
            progress!("{}", report.to_text());
        }
        return Ok(());
    }
}

/// One page on stdout, flushed so a reader sees it before the next request.
/// A status outside 2xx prints only its `--json` envelope, as a single
/// request does.
fn print_page(response: &HttpResponse, options: &ApiOutputOptions) -> Result<()> {
    if !response.is_success() && !options.json {
        return Ok(());
    }
    let lines = match result_lines(response, options)? {
        Some(lines) => lines,
        None => vec![response_envelope(response)["body"].to_string()],
    };
    let mut stdout = std::io::stdout().lock();
    for line in lines {
        writeln!(stdout, "{line}")?;
    }
    stdout.flush()?;
    Ok(())
}

/// Why paging stopped after `fetched` pages.
#[derive(Debug, PartialEq, Eq)]
struct StopReport {
    fetched: u32,
    limit: u32,
    stopped: StopReason,
    /// With `limit`: the URL of the page that was not requested, a TARGET to
    /// go on from.
    next: Option<String>,
    /// With `unsupported` and `refused`: what the response lacked, or why
    /// the page it named is not requested.
    detail: Option<String>,
}

impl StopReport {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "schema_version": SCHEMA_VERSION,
            "pages": {
                "fetched": self.fetched,
                "limit": self.limit,
                "stopped": self.stopped.as_str(),
                "next": self.next,
                "detail": self.detail,
            },
        })
    }

    fn to_text(&self) -> String {
        let count = format!("# pages: {} fetched (--pages {})", self.fetched, self.limit);
        match (self.stopped, &self.next, &self.detail) {
            (StopReason::Limit, Some(next), _) => {
                format!("{count}; stopped: limit, a next page was named: {next}")
            }
            (StopReason::Unsupported, _, Some(detail)) => {
                format!("{count}; stopped: unsupported, more pages cannot be told: {detail}")
            }
            (StopReason::Refused, _, Some(detail)) => {
                format!("{count}; stopped: refused, a next page is not followed: {detail}")
            }
            _ => format!("{count}; stopped: {}", self.stopped.as_str()),
        }
    }
}

/// What a dry run says about the pages it would fetch.
pub(super) fn dry_run_note(pages: &ApiPages) -> String {
    format!(
        "# up to {} pages: each later request is this one with {}",
        pages.limit,
        follow_text(&pages.style)
    )
}

/// How the URL of each later request is found, for the dry run and the plan.
pub(super) fn follow_text(style: &PageStyle) -> String {
    match style {
        PageStyle::Link => {
            "the URL of the Link rel=\"next\" of the response before it, on the origin of base_url"
                .into()
        }
        PageStyle::Cursor(cursor) => format!(
            "the query parameter {} set to {} of the response before it",
            cursor.param, cursor.path
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::functions::api_pages::CursorStyle;

    fn report(stopped: StopReason, next: Option<&str>, detail: Option<&str>) -> StopReport {
        StopReport {
            fetched: 2,
            limit: 2,
            stopped,
            next: next.map(str::to_string),
            detail: detail.map(str::to_string),
        }
    }

    #[test]
    fn the_stop_report_names_the_reason_the_next_page_and_what_was_missing() {
        assert_eq!(
            report(StopReason::Limit, Some("https://x/?page=3"), None).to_json(),
            json!({"schema_version": 1, "pages": {
                "fetched": 2, "limit": 2, "stopped": "limit",
                "next": "https://x/?page=3", "detail": null,
            }})
        );
        assert_eq!(
            report(StopReason::Limit, Some("https://x/?page=3"), None).to_text(),
            "# pages: 2 fetched (--pages 2); stopped: limit, a next page was named: https://x/?page=3"
        );
        assert_eq!(
            report(StopReason::Last, None, None).to_text(),
            "# pages: 2 fetched (--pages 2); stopped: last"
        );
        assert_eq!(
            report(StopReason::Unsupported, None, Some("no Link header")).to_text(),
            "# pages: 2 fetched (--pages 2); stopped: unsupported, more pages cannot be told: no Link header"
        );
        assert_eq!(
            report(StopReason::Refused, None, Some("off origin")).to_text(),
            "# pages: 2 fetched (--pages 2); stopped: refused, a next page is not followed: off origin"
        );
        assert_eq!(
            report(StopReason::Unsupported, None, Some("no Link header")).to_json()["pages"]["detail"],
            "no Link header"
        );
    }

    #[test]
    fn the_dry_run_note_says_how_each_later_request_is_made() {
        let note = |style| dry_run_note(&ApiPages { limit: 3, style });
        assert_eq!(
            note(PageStyle::Link),
            "# up to 3 pages: each later request is this one with the URL of the Link rel=\"next\" of the response before it, on the origin of base_url"
        );
        assert_eq!(
            note(PageStyle::Cursor(
                CursorStyle::parse("meta.next=cursor").unwrap()
            )),
            "# up to 3 pages: each later request is this one with the query parameter cursor set to meta.next of the response before it"
        );
    }
}
