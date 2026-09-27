//! Fixtures for every activity monitor state and the monitor's UI contract, for the render regression (`view_snapshot_tests.rs`).

use ratatui::buffer::Buffer;

use super::update::{ActivityMessage, ActivityModel, Focus, update};
use super::view::{APP_TITLE, hints, render};
use crate::domain::types::audit::AuditEntry;
use crate::shell::tui::layout::Breakpoint;
use crate::shell::tui::testing::{buffer_lines, render_frame, selection_violations};
use crate::shell::tui::theme;

pub fn render_buffer(model: &ActivityModel, width: u16, height: u16) -> Buffer {
    render_frame(width, height, |frame| render(frame, model))
}

/// Violations of the monitor's contract: the title in the header, the
/// primary hint in the footer, a panel ending above the footer, and the
/// selected call marked in the selection style while the list is on screen.
pub fn check_contract(model: &ActivityModel, buffer: &Buffer) -> Vec<String> {
    let lines = buffer_lines(buffer);
    let height = lines.len();
    let mut violations = Vec::new();
    if !lines[0].contains(APP_TITLE) {
        violations.push(format!(
            "header row does not show the title: {:?}",
            lines[0]
        ));
    }
    let footer = &lines[height - 1];
    if let Some((key, action)) = hints(model).first()
        && !footer.contains(&format!("{key} {action}"))
    {
        violations.push(format!(
            "footer lacks the primary hint {key} {action}: {footer:?}"
        ));
    }
    if height >= 4 && !lines[height - 2].contains(theme::BORDER.bottom_left) {
        violations.push(format!(
            "row above the footer is not a panel bottom border: {:?}",
            lines[height - 2]
        ));
    }
    let list_shown =
        model.focus == Focus::List || Breakpoint::of(buffer.area.width) != Breakpoint::Compact;
    if list_shown && let Some(entry) = model.selected_entry() {
        let time = entry.time.format("%H:%M:%S").to_string();
        violations.extend(selection_violations(buffer, &lines, &time));
    }
    violations
}

pub mod fixtures {
    use super::*;

    fn at(second: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_800_000_000 + second, 0).unwrap()
    }

    fn api(second: i64, method: &str, path: &str, status: Option<u16>) -> AuditEntry {
        AuditEntry {
            time: at(second),
            command: "api".into(),
            target: "pets".into(),
            agent: true,
            method: Some(method.into()),
            path: Some(path.into()),
            status,
            program: None,
            sql_sha256: None,
            exit_code: Some(0),
            error_code: None,
            duration_ms: 142,
        }
    }

    /// Oldest first, as the log holds them: a read, a write the policy
    /// refused, a 404, a database query, an exec, and a person's call.
    pub fn calls() -> Vec<AuditEntry> {
        vec![
            api(0, "GET", "/v1/pets", Some(200)),
            AuditEntry {
                exit_code: Some(3),
                error_code: Some("AGENT_POLICY_DENIED".into()),
                duration_ms: 3,
                ..api(61, "POST", "/v1/pets", None)
            },
            AuditEntry {
                exit_code: Some(4),
                error_code: Some("API_HTTP_ERROR".into()),
                ..api(122, "GET", "/v1/pets/404", Some(404))
            },
            AuditEntry {
                command: "db".into(),
                target: "app".into(),
                method: None,
                path: None,
                status: None,
                sql_sha256: Some(
                    "sha256:a82418a16c77ffbd4e34a013e06efa56f567f38d6ca800f21af95b60cb34c41e"
                        .into(),
                ),
                duration_ms: 1_234,
                ..api(183, "GET", "/", None)
            },
            AuditEntry {
                command: "exec".into(),
                target: "dev".into(),
                method: None,
                path: None,
                status: None,
                program: Some("aws".into()),
                exit_code: None,
                duration_ms: 812,
                ..api(244, "GET", "/", None)
            },
            AuditEntry {
                agent: false,
                ..api(305, "DELETE", "/v1/pets/7", Some(204))
            },
        ]
    }

    fn read(entries: Vec<AuditEntry>) -> ActivityModel {
        let mut model = ActivityModel {
            base_paths: [("pets".to_owned(), "/v1".to_owned())].into(),
            ..Default::default()
        };
        update(&mut model, ActivityMessage::Read(Ok(entries)));
        model
    }

    pub fn loading() -> ActivityModel {
        ActivityModel::default()
    }

    pub fn empty() -> ActivityModel {
        read(Vec::new())
    }

    pub fn loaded() -> ActivityModel {
        read(calls())
    }

    /// The refused write selected: its row and its details.
    pub fn refused() -> ActivityModel {
        let mut model = loaded();
        model.selected = 4;
        model
    }

    /// The details with the keys: what a compact terminal shows after Enter.
    pub fn detail() -> ActivityModel {
        let mut model = refused();
        model.focus = Focus::Detail;
        model
    }

    pub fn copied() -> ActivityModel {
        let mut model = refused();
        update(
            &mut model,
            ActivityMessage::Copied(Ok("kurama api pets -X POST /pets".into())),
        );
        model
    }

    pub fn read_error() -> ActivityModel {
        let mut model = ActivityModel::default();
        update(
            &mut model,
            ActivityMessage::Read(Err("Permission denied (os error 13)".into())),
        );
        model
    }

    /// A long target and path, full-width text and an escape sequence,
    /// selected so the details show them whole.
    pub fn long_text() -> ActivityModel {
        let mut model = read(vec![
            AuditEntry {
                target: "an-api-whose-name-is-longer-than-its-column".into(),
                ..api(
                    0,
                    "GET",
                    "/v1/organizations/1234567890/projects/0987654321/members/ユーザー\u{1b}[31m",
                    Some(200),
                )
            },
            api(1, "GET", "/v1/pets", Some(200)),
        ]);
        model.selected = 1;
        model
    }

    pub fn all() -> Vec<(&'static str, ActivityModel)> {
        vec![
            ("loading", loading()),
            ("empty", empty()),
            ("loaded", loaded()),
            ("refused", refused()),
            ("detail", detail()),
            ("copied", copied()),
            ("read_error", read_error()),
            ("long_text", long_text()),
        ]
    }
}
