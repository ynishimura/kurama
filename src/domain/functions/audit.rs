//! The pure parts of the audit log: a URL cut to its path, the SQL fingerprint, `--since`, the listing `kurama audit` prints, and how an entry's request and outcome are worded.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};

use crate::domain::types::audit::AuditEntry;

/// The path of `url`, without its query or fragment; a URL that does not
/// parse keeps whatever comes before the first `?` or `#`.
pub fn path_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(url) => url.path().to_owned(),
        Err(_) => url.split(['?', '#']).next().unwrap_or_default().to_owned(),
    }
}

/// `sha256:<hex>` of a statement: tells two runs of one SQL apart from two
/// runs of different ones without keeping either.
pub fn sql_fingerprint(sql: &str) -> String {
    let hex: String = Sha256::digest(sql.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// `--since`: a number followed by `s`, `m`, `h` or `d`.
pub fn parse_since(text: &str) -> Result<Duration, String> {
    let error = || format!("--since takes a number and a unit (30m, 12h, 7d), got {text:?}");
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let number: i64 = number.parse().map_err(|_| error())?;
    let seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(error()),
    };
    number
        .checked_mul(seconds)
        .and_then(Duration::try_seconds)
        .filter(|duration| *duration > Duration::zero())
        .ok_or_else(error)
}

/// The entries at or after `cutoff`, in the order they were written.
pub fn entries_since(entries: Vec<AuditEntry>, cutoff: Option<DateTime<Utc>>) -> Vec<AuditEntry> {
    entries
        .into_iter()
        .filter(|entry| cutoff.is_none_or(|cutoff| entry.time >= cutoff))
        .collect()
}

/// What an entry did: `METHOD /path`, exec's program, the SQL fingerprint,
/// or `-`.
pub fn request_text(entry: &AuditEntry) -> String {
    match (
        &entry.method,
        &entry.path,
        &entry.program,
        &entry.sql_sha256,
    ) {
        (Some(method), Some(path), _, _) => format!("{method} {path}"),
        (_, _, Some(program), _) => program.clone(),
        (_, _, _, Some(sql)) => sql.clone(),
        _ => "-".to_owned(),
    }
}

/// The error code of a call the `[agent]` policy refused.
pub const POLICY_REFUSAL_CODE: &str = "AGENT_POLICY_DENIED";

/// How a call ended, as the activity monitor tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// The `[agent]` policy stopped it before anything was called.
    Refused,
    /// It ended with an exit code other than 0.
    Failed,
    /// It ended with exit code 0.
    Succeeded,
    /// kurama replaced itself with the `exec` command, whose end it never
    /// sees.
    HandedOff,
}

pub fn outcome(entry: &AuditEntry) -> AuditOutcome {
    match (entry.error_code.as_deref(), entry.exit_code) {
        (Some(POLICY_REFUSAL_CODE), _) => AuditOutcome::Refused,
        (_, Some(0)) => AuditOutcome::Succeeded,
        (_, Some(_)) => AuditOutcome::Failed,
        (_, None) => AuditOutcome::HandedOff,
    }
}

/// The STATUS column of the monitor: `refused`, the HTTP status, `ok`,
/// `exit N`, or `exec` for a call handed to its program.
pub fn outcome_text(entry: &AuditEntry) -> String {
    match (outcome(entry), entry.status, entry.exit_code) {
        (AuditOutcome::Refused, _, _) => "refused".to_owned(),
        (_, Some(status), _) => status.to_string(),
        (AuditOutcome::Succeeded, None, _) => "ok".to_owned(),
        (_, None, Some(code)) => format!("exit {code}"),
        (_, None, None) => "exec".to_owned(),
    }
}

/// `850ms` under a second, `1.2s` from there on.
pub fn duration_text(milliseconds: u64) -> String {
    if milliseconds < 1000 {
        format!("{milliseconds}ms")
    } else {
        format!("{:.1}s", milliseconds as f64 / 1000.0)
    }
}

/// One line per entry, oldest first: time, who, command, target, what, and
/// how it ended.
pub fn render_listing(entries: &[AuditEntry]) -> String {
    let rows: Vec<[String; 7]> = entries
        .iter()
        .map(|entry| {
            let what = request_text(entry);
            let optional = |value: Option<String>| value.unwrap_or_else(|| "-".to_owned());
            [
                entry.time.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                if entry.agent { "agent" } else { "person" }.to_owned(),
                entry.command.clone(),
                entry.target.clone(),
                what,
                optional(entry.status.map(|status| status.to_string())),
                optional(entry.exit_code.map(|code| code.to_string())),
            ]
        })
        .collect();
    let header = [
        "TIME", "BY", "COMMAND", "TARGET", "REQUEST", "STATUS", "EXIT",
    ]
    .map(str::to_owned);
    let widths: Vec<usize> = (0..header.len())
        .map(|column| {
            std::iter::once(&header)
                .chain(&rows)
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or_default()
        })
        .collect();
    std::iter::once(&header)
        .chain(&rows)
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:width$}"))
                .collect();
            format!("{}\n", cells.join("  ").trim_end())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(minute: u32, method: Option<&str>) -> AuditEntry {
        AuditEntry {
            time: DateTime::parse_from_rfc3339(&format!("2026-09-27T10:{minute:02}:00Z"))
                .unwrap()
                .with_timezone(&Utc),
            command: "api".into(),
            target: "pets".into(),
            agent: true,
            method: method.map(str::to_owned),
            path: method.map(|_| "/pets".to_owned()),
            status: Some(200),
            program: None,
            sql_sha256: None,
            exit_code: Some(0),
            error_code: None,
            duration_ms: 12,
        }
    }

    fn ended(exit_code: Option<u8>, error_code: Option<&str>, status: Option<u16>) -> AuditEntry {
        AuditEntry {
            exit_code,
            error_code: error_code.map(str::to_owned),
            status,
            ..entry(1, Some("GET"))
        }
    }

    #[test]
    fn the_outcome_tells_a_refusal_from_a_failure_with_the_same_exit_code() {
        let refused = ended(Some(3), Some(POLICY_REFUSAL_CODE), None);
        let login_needed = ended(Some(3), Some("OAUTH_LOGIN_REQUIRED"), None);
        assert_eq!(outcome(&refused), AuditOutcome::Refused);
        assert_eq!(outcome(&login_needed), AuditOutcome::Failed);
        assert_eq!(
            outcome(&ended(Some(0), None, Some(200))),
            AuditOutcome::Succeeded
        );
        assert_eq!(outcome(&ended(None, None, None)), AuditOutcome::HandedOff);
        assert_eq!(outcome_text(&refused), "refused");
        assert_eq!(outcome_text(&login_needed), "exit 3");
        assert_eq!(
            outcome_text(&ended(Some(4), Some("API_HTTP_ERROR"), Some(404))),
            "404"
        );
        assert_eq!(outcome_text(&ended(Some(0), None, None)), "ok");
        assert_eq!(outcome_text(&ended(None, None, None)), "exec");
    }

    #[rstest::rstest]
    #[case(0, "0ms")]
    #[case(999, "999ms")]
    #[case(1000, "1.0s")]
    #[case(12_345, "12.3s")]
    fn a_duration_is_in_milliseconds_under_a_second(#[case] milliseconds: u64, #[case] text: &str) {
        assert_eq!(duration_text(milliseconds), text);
    }

    #[test]
    fn the_request_is_the_method_and_path_the_program_or_the_sql_fingerprint() {
        assert_eq!(request_text(&entry(1, Some("GET"))), "GET /pets");
        let exec = AuditEntry {
            program: Some("aws".into()),
            ..entry(1, None)
        };
        assert_eq!(request_text(&exec), "aws");
        let db = AuditEntry {
            sql_sha256: Some("sha256:ab".into()),
            ..entry(1, None)
        };
        assert_eq!(request_text(&db), "sha256:ab");
        assert_eq!(request_text(&entry(1, None)), "-");
    }

    #[rstest::rstest]
    #[case("https://api.example.com/v1/items?token=secret&q=1", "/v1/items")]
    #[case("https://api.example.com/v1/items#frag", "/v1/items")]
    #[case("not a url?token=secret", "not a url")]
    fn the_path_never_keeps_the_query(#[case] url: &str, #[case] path: &str) {
        assert_eq!(path_of(url), path);
    }

    #[test]
    fn the_sql_fingerprint_is_its_sha256_and_not_the_sql() {
        let fingerprint = sql_fingerprint("SELECT 1");
        assert_eq!(
            fingerprint,
            "sha256:e004ebd5b5532a4b85984a62f8ad48a81aa3460c1ca07701f386135d72cdecf5"
        );
        assert_ne!(fingerprint, sql_fingerprint("SELECT 2"));
    }

    #[rstest::rstest]
    #[case("30s", 30)]
    #[case("15m", 900)]
    #[case("2h", 7200)]
    #[case("7d", 604800)]
    fn since_takes_a_number_and_a_unit(#[case] text: &str, #[case] seconds: i64) {
        assert_eq!(parse_since(text), Ok(Duration::seconds(seconds)));
    }

    #[rstest::rstest]
    #[case("")]
    #[case("h")]
    #[case("10")]
    #[case("10w")]
    #[case("0h")]
    #[case("-1h")]
    #[case("99999999999999999d")]
    fn since_refuses_anything_else(#[case] text: &str) {
        assert!(parse_since(text).is_err(), "{text}");
    }

    #[test]
    fn entries_since_keeps_the_cutoff_and_what_came_after() {
        let entries = vec![entry(1, None), entry(5, None), entry(9, None)];
        let cutoff = entries[1].time;
        let kept = entries_since(entries.clone(), Some(cutoff));
        assert_eq!(kept, entries[1..]);
        assert_eq!(entries_since(entries.clone(), None), entries);
    }

    #[test]
    fn the_listing_has_a_header_and_one_aligned_line_per_entry() {
        let listing = render_listing(&[entry(1, Some("GET")), entry(2, None)]);
        let lines: Vec<&str> = listing.lines().collect();
        assert_eq!(lines.len(), 3, "{listing}");
        assert!(lines[0].starts_with("TIME"), "{listing}");
        assert!(
            lines[1].contains("agent  api      pets    GET /pets  200"),
            "{listing}"
        );
        assert!(lines[2].contains(" -  "), "{listing}");
        assert_eq!(lines[1].find("pets"), lines[0].find("TARGET"));
    }
}
