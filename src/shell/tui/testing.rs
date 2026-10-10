//! Test support for the TUI views: render a model into a buffer or text,
//! fixtures for every screen state, plain-text snapshot files and the
//! semantic checks of the UI contract.
//!
//! Snapshots live in `tests/tui_snapshots/<name>.txt`. A mismatch writes
//! `<name>.txt.new` next to the file and fails; run the test with
//! `KURAMA_UPDATE_SNAPSHOTS=1` to accept, after reading the diff.

use strum::VariantArray;

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthStr;

use super::tea::sources::Tab;
use super::tea::update::{ErrorModel, MfaInputModel, Screen, SuccessModel, TuiModel};
use super::tea::view::{hints, render};
use super::theme::{self, Tone};

/// The terminal sizes every screen is verified at.
pub const SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 40), (160, 50)];

/// Draw with `draw` on a `width` x `height` test terminal.
pub fn render_frame(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> Buffer {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(draw).unwrap();
    terminal.backend().buffer().clone()
}

/// The screen `draw` produces, as lines of text (see `buffer_lines`).
pub fn render_lines(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> Vec<String> {
    buffer_lines(&render_frame(width, height, draw))
}

pub fn render_buffer(model: &TuiModel, width: u16, height: u16) -> Buffer {
    render_frame(width, height, |frame| render(frame, model))
}

/// The buffer as lines of text, trailing spaces removed. A full-width
/// character occupies two cells; the second cell is skipped so the text has
/// the same shape as the terminal.
pub fn buffer_lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < area.width {
                let symbol = buffer[(area.x + x, area.y + y)].symbol();
                line.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect()
}

pub fn buffer_text(buffer: &Buffer) -> String {
    buffer_lines(buffer).join("\n")
}

/// Display column of the first `needle` in a `buffer_lines` row: one
/// character per cell, so the width of the text before it is its column.
pub fn column_of(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|index| line[..index].width())
}

/// Whether the cell is drawn in `style`. A style leaves colors unset where
/// the buffer holds `Reset`.
pub fn has_style(cell: &Cell, style: Style) -> bool {
    cell.fg == style.fg.unwrap_or(Color::Reset)
        && cell.bg == style.bg.unwrap_or(Color::Reset)
        && cell.modifier == style.add_modifier
}

pub fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/tui_snapshots")
}

/// `None` when `actual` matches `tests/tui_snapshots/<name>.txt`, otherwise
/// a description of the difference; the new rendering is written to
/// `<name>.txt.new` (or to the snapshot itself with `KURAMA_UPDATE_SNAPSHOTS=1`).
pub fn snapshot_mismatch(name: &str, actual: &str) -> Option<String> {
    let dir = snapshot_dir();
    let path = dir.join(format!("{name}.txt"));
    let expected = std::fs::read_to_string(&path).ok();
    let actual_file = format!("{actual}\n");
    if expected.as_deref() == Some(actual_file.as_str()) {
        let _ = std::fs::remove_file(path.with_extension("txt.new"));
        return None;
    }
    std::fs::create_dir_all(&dir).unwrap();
    if std::env::var_os("KURAMA_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, &actual_file).unwrap();
        let _ = std::fs::remove_file(path.with_extension("txt.new"));
        return None;
    }
    let new_path = path.with_extension("txt.new");
    std::fs::write(&new_path, &actual_file).unwrap();
    let mut message = match &expected {
        None => format!("new snapshot {name}: review {}", new_path.display()),
        Some(_) => format!(
            "snapshot {name} changed: compare {} with {}",
            path.display(),
            new_path.display()
        ),
    };
    if let Some(expected) = expected {
        for (number, (old, new)) in expected
            .lines()
            .zip(actual_file.lines())
            .enumerate()
            .filter(|(_, (old, new))| old != new)
            .take(3)
        {
            message.push_str(&format!("\n  line {}:\n  - {old}\n  + {new}", number + 1));
        }
    }
    Some(message)
}

/// Violations of the home screen contract in a rendered buffer: the title,
/// the badges (each in its on or off style) and the primary key hint are
/// visible, the selected profile is in view and highlighted, a modal fits in
/// the viewport and is drawn in its tone, the footer stays one row.
pub fn check_contract(model: &TuiModel, buffer: &Buffer) -> Vec<String> {
    let lines = buffer_lines(buffer);
    let width = buffer.area.width as usize;
    let height = lines.len();
    let mut violations = Vec::new();

    if !lines[0].contains(super::tea::view::APP_TITLE) {
        violations.push(format!(
            "header row does not show the title: {:?}",
            lines[0]
        ));
    }
    if width >= 60 {
        let badges = [
            ("readonly", model.mode.readonly),
            ("console", model.mode.console_launch),
        ];
        for (label, on) in badges {
            let expected = if on {
                theme::badge_on()
            } else {
                theme::badge_off()
            };
            match column_of(&lines[0], label) {
                None => violations.push(format!(
                    "header row does not show the {label} badge: {:?}",
                    lines[0]
                )),
                Some(x) if !has_style(cell(buffer, x, 0), expected) => violations.push(format!(
                    "the {label} badge is {} but not drawn in that style: {:?}",
                    if on { "on" } else { "off" },
                    cell(buffer, x, 0).style()
                )),
                Some(_) => {}
            }
        }
    }
    if width >= 60 && height >= 3 {
        violations.extend(tab_violations(model, buffer, &lines[1]));
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

    if let Some((title, tone)) = modal_title(model) {
        violations.extend(modal_violations(buffer, &lines, title, tone));
    } else if !model.profiles.loading {
        let selected = match model.sources.list(model.tab) {
            None => model
                .profiles
                .filtered_profiles
                .get(model.profiles.selected_index)
                .map(|row| row.name.as_str()),
            Some(list) => list.rows.get(list.selected).map(|row| row.name.as_str()),
        };
        if let Some(name) = selected {
            violations.extend(selection_violations(buffer, &lines, name));
        }
    }
    violations
}

/// The tab row names every tab and draws the one on screen selected.
fn tab_violations(model: &TuiModel, buffer: &Buffer, line: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for (index, tab) in Tab::VARIANTS.iter().enumerate() {
        let label = format!("{} {}", index + 1, tab.label());
        let selected = *tab == model.tab;
        match column_of(line, &label) {
            None => violations.push(format!("tab row does not show {label:?}: {line:?}")),
            Some(x) if selected != has_style(cell(buffer, x, 1), theme::selection()) => violations
                .push(format!(
                    "tab {label:?} is {} but drawn as {:?}",
                    if selected {
                        "on screen"
                    } else {
                        "not on screen"
                    },
                    cell(buffer, x, 1).style()
                )),
            Some(_) => {}
        }
    }
    violations
}

pub fn cell(buffer: &Buffer, x: usize, y: usize) -> &Cell {
    &buffer[(buffer.area.x + x as u16, buffer.area.y + y as u16)]
}

/// The selected profile's row shows the marker and the name (or its cut
/// prefix), and the marker is drawn in the selection style.
pub fn selection_violations(buffer: &Buffer, lines: &[String], name: &str) -> Vec<String> {
    let marker = theme::SELECTION_MARKER.trim();
    let shows_name = |line: &str| {
        line.contains(name)
            || line
                .split_whitespace()
                .any(|word| word.ends_with('…') && name.starts_with(word.trim_end_matches('…')))
    };
    let Some(y) = lines
        .iter()
        .position(|line| line.contains(marker) && shows_name(line))
    else {
        return vec![format!(
            "selected profile {name:?} is not marked with {marker} on any row"
        )];
    };
    let x = column_of(&lines[y], marker).unwrap();
    let marker_cell = cell(buffer, x, y);
    if !has_style(marker_cell, theme::selection()) {
        return vec![format!(
            "selection marker at ({x}, {y}) is not drawn in theme::selection(): {:?}",
            marker_cell.style()
        )];
    }
    Vec::new()
}

/// Title and tone of the modal the screen shows, if any.
fn modal_title(model: &TuiModel) -> Option<(&'static str, Tone)> {
    if model.palette.is_some() {
        return Some((crate::shell::tui::tea::palette_view::TITLE, Tone::Info));
    }
    match model.screen {
        Screen::ProfileList => None,
        Screen::MfaInput => Some(("MFA code", Tone::Warning)),
        Screen::Processing { .. } => Some(("Processing", Tone::Info)),
        Screen::Success(_) => Some(("Success", Tone::Success)),
        Screen::Error(_) => Some(("Error", Tone::Danger)),
        Screen::Help => Some(("Help", Tone::Info)),
    }
}

/// The modal's top border carries its title and both corners, its bottom
/// border has both corners in the same columns (anything else means the box
/// was clipped by the viewport), and the border is drawn in the modal's tone.
pub fn modal_violations(buffer: &Buffer, lines: &[String], title: &str, tone: Tone) -> Vec<String> {
    let marker = format!("{} {title} ", theme::BORDER.top_left);
    let Some(top) = lines.iter().position(|line| line.contains(&marker)) else {
        return vec![format!("no row shows the modal title {marker:?}")];
    };
    let top_line = &lines[top];
    let left_index = top_line.find(&marker).unwrap();
    let left = top_line[..left_index].width();
    let after = &top_line[left_index + theme::BORDER.top_left.len()..];
    let Some(right_index) = after.find(theme::BORDER.top_right) else {
        return vec![format!("modal top border is clipped: {top_line:?}")];
    };
    let right = left + 1 + after[..right_index].width();
    let bottom = lines[top + 1..].iter().any(|line| {
        column_of(line, theme::BORDER.bottom_left) == Some(left)
            && column_of(line, theme::BORDER.bottom_right) == Some(right)
    });
    if !bottom {
        return vec![format!(
            "modal bottom border with corners at columns {left} and {right} not found"
        )];
    }
    let corner = cell(buffer, left, top);
    if !has_style(corner, tone.style()) {
        return vec![format!(
            "modal border is not drawn in {tone:?}: {:?}",
            corner.style()
        )];
    }
    Vec::new()
}

/// Every home screen state the view is verified in.
pub mod fixtures {
    use super::*;
    use crate::shell::tui::tea::messages::{ProfileRow, TuiMessage};
    use crate::shell::tui::tea::sources::Sources;
    use crate::shell::tui::tea::update::tea_update;
    use chrono::{DateTime, Duration, Utc};

    /// The clock every fixture is drawn at.
    pub fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    pub fn row(name: &str, session: &str, active: bool) -> ProfileRow {
        ProfileRow {
            name: name.to_string(),
            kind: "aws",
            session: session.to_string(),
            session_expires_at: None,
            active,
            role_arn: Some(format!("arn:aws:iam::123456789012:role/{name}")),
            region: Some("ap-northeast-1".into()),
            mfa_serial: if session == "-" {
                None
            } else {
                Some("arn:aws:iam::123456789012:mfa/agent".into())
            },
            needs_human: session == "none",
        }
    }

    fn with_rows(rows: Vec<ProfileRow>) -> TuiModel {
        let mut model = TuiModel::new(false, false);
        model.now = now();
        model.profiles.loading = false;
        model.profiles.all_profiles = rows.clone();
        model.profiles.filtered_profiles = rows;
        model
    }

    pub fn loading() -> TuiModel {
        TuiModel::new(false, false)
    }

    pub fn loaded() -> TuiModel {
        with_rows(vec![
            row("default", "-", false),
            row("dev", "-", false),
            ProfileRow {
                session_expires_at: Some(now() + Duration::minutes(719)),
                ..row("ops-mfa", "valid (11h 59m)", true)
            },
            row("sandbox-mfa", "none", false),
        ])
    }

    /// The selected profile's MFA session has less than 15 minutes left.
    pub fn session_soon() -> TuiModel {
        let mut model = loaded();
        model.profiles.selected_index = 2;
        model.now = now() + Duration::minutes(719 - 14);
        model
    }

    /// The selected profile's MFA session ended while the TUI was open.
    pub fn session_expired() -> TuiModel {
        let mut model = loaded();
        model.profiles.selected_index = 2;
        model.now = now() + Duration::minutes(720);
        model
    }

    /// A later row selected, readonly and console switched on.
    pub fn selected() -> TuiModel {
        let mut model = loaded();
        model.profiles.selected_index = 2;
        model.mode.readonly = true;
        model.mode.console_launch = true;
        model
    }

    /// Enough rows that the selected one is below the first page at 80x24.
    pub fn scrolled() -> TuiModel {
        let rows = (1..=40)
            .map(|index| row(&format!("account-{index:02}"), "-", index == 37))
            .collect();
        let mut model = with_rows(rows);
        model.profiles.selected_index = 36;
        model
    }

    pub fn searching() -> TuiModel {
        let mut model = loaded();
        model.searching = true;
        model.search_query = "mfa".into();
        model.profiles.filtered_profiles = model
            .profiles
            .all_profiles
            .iter()
            .filter(|row| row.name.contains("mfa"))
            .cloned()
            .collect();
        model
    }

    pub fn empty() -> TuiModel {
        let mut model = loaded();
        model.searching = true;
        model.search_query = "zzz".into();
        model.profiles.filtered_profiles.clear();
        model
    }

    pub fn no_profiles() -> TuiModel {
        with_rows(vec![])
    }

    pub fn error() -> TuiModel {
        let mut model = loaded();
        model.screen = Screen::Error(ErrorModel {
            message: "STS AssumeRole failed: User: arn:aws:iam::123456789012:user/agent is not authorized to perform: sts:AssumeRole on resource: arn:aws:iam::123456789012:role/Dev".into(),
            console_url_on_quit: false,
        });
        model
    }

    /// The console was asked for and the URL opener failed.
    pub fn browser_failed() -> TuiModel {
        let mut model = loaded();
        model.profiles.selected_index = 1;
        model.mode.console_launch = true;
        model.screen = Screen::Error(ErrorModel {
            message: "Failed to open the browser: `open` exited with exit status: 1".into(),
            console_url_on_quit: true,
        });
        model
    }

    pub fn mfa() -> TuiModel {
        let mut model = loaded();
        model.profiles.selected_index = 2;
        model.screen = Screen::MfaInput;
        model.mfa_input = MfaInputModel {
            value: "123".into(),
            serial: "arn:aws:iam::123456789012:mfa/agent".into(),
            profile: "ops-mfa".into(),
        };
        model
    }

    pub fn help() -> TuiModel {
        let mut model = loaded();
        model.screen = Screen::Help;
        model
    }

    pub fn processing() -> TuiModel {
        let mut model = loaded();
        model.profiles.selected_index = 1;
        model.screen = Screen::Processing {
            message: "Assuming role for profile: dev".into(),
        };
        model
    }

    pub fn success() -> TuiModel {
        let mut model = loaded();
        model.profiles.selected_index = 1;
        model.screen = Screen::Success(SuccessModel {
            profile: "dev".into(),
            access_key_id: "ASIARESULTKEY000001".into(),
        });
        model
    }

    pub fn long_text() -> TuiModel {
        let mut model = with_rows(vec![
            row("dev", "-", false),
            ProfileRow {
                role_arn: Some(
                    "arn:aws:iam::123456789012:role/organization-wide-platform-engineering-administrator-with-a-very-long-name".into(),
                ),
                ..row(
                    "platform-engineering-production-administrator-eu-central-1-very-long-profile-name",
                    "valid (11h 59m)",
                    true,
                )
            },
        ]);
        model.profiles.selected_index = 1;
        model
    }

    pub fn unicode() -> TuiModel {
        let mut model = with_rows(vec![
            row("本番環境-管理者", "valid (1h 30m)", true),
            row("開発環境", "-", false),
            row("ステージング-読み取り専用", "none", false),
            row("naïve-café", "-", false),
        ]);
        model.profiles.selected_index = 2;
        model
    }

    /// A modal drawn over full-width names.
    pub fn unicode_help() -> TuiModel {
        let mut model = unicode();
        model.screen = Screen::Help;
        model
    }

    /// The rows of every source tab, as the runtime builds them from the
    /// facts `kurama status` reports.
    pub fn sources() -> Sources {
        use crate::shell::tui::tea::sources::{Enter, SourceList, SourceRow};
        let row = |kind: &'static str,
                   name: &str,
                   state: &str,
                   details: Vec<(&'static str, String)>,
                   next: &str,
                   enter: Enter| SourceRow {
            name: name.into(),
            kind,
            state: state.into(),
            active: false,
            details,
            next: next.into(),
            enter,
        };
        let list = |rows| SourceList { rows, selected: 0 };
        Sources {
            auth: list(vec![
                SourceRow {
                    active: true,
                    ..row("auth", "github", "valid (59m)", vec![
                        ("Source", "github".into()),
                        ("Kind", "oauth".into()),
                        ("Grant", "authorization_code".into()),
                        ("Env var", "KURAMA_TOKEN".into()),
                        ("Token", "valid (59m)".into()),
                        ("Expires", "2027-01-15T09:30:00Z".into()),
                        ("Shell", "holds its token".into()),
                    ], "Enter runs kurama login github", Enter::Run(vec!["login".into(), "github".into()]))
                },
                row("auth", "openai", "not_checked", vec![
                    ("Source", "openai".into()),
                    ("Kind", "token".into()),
                    ("Header", "Authorization".into()),
                ], "openai uses a credential issued elsewhere; there is nothing to log in to", Enter::Explain("openai uses a credential issued elsewhere; there is nothing to log in to".into())),
            ]),
            api: list(vec![
                row("api", "github", "valid (59m)", vec![
                    ("API", "github".into()),
                    ("Base URL", "https://api.github.com".into()),
                    ("About", "-".into()),
                    ("Auth", "github".into()),
                    ("Token", "valid (59m)".into()),
                    ("AWS profile", "-".into()),
                    ("OpenAPI", "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.json".into()),
                ], "Enter opens the explorer", Enter::Run(vec!["api".into(), "github".into()])),
                row("api", "internal", "-", vec![
                    ("API", "internal".into()),
                    ("Base URL", "https://internal.example.com".into()),
                ], "[api.internal] has no openapi description to explore", Enter::Explain("[api.internal] has no openapi description to explore".into())),
            ]),
            db: list(vec![row("db", "app", "unknown", vec![
                ("Database", "app".into()),
                ("Engine", "postgres".into()),
                ("Name", "app".into()),
                ("Host", "db.internal:5432".into()),
                ("Allow write", "false".into()),
            ], "Enter opens the database explorer", Enter::Run(vec!["db".into(), "app".into()]))]),
            data: list(Vec::new()),
            s3: list(vec![
                row("s3", "assets", "-", vec![
                    ("Connection", "assets".into()),
                    ("AWS profile", "dev".into()),
                    ("Session", "-".into()),
                    ("Region", "-".into()),
                    ("Start", "s3://example-assets/reports/".into()),
                ], "Enter opens the S3 explorer", Enter::Run(vec!["s3".into(), "assets".into()])),
                row("s3", "public", "valid (11h 59m)", vec![
                    ("Connection", "public".into()),
                    ("AWS profile", "cm".into()),
                    ("Session", "valid (11h 59m)".into()),
                    ("Region", "us-east-1".into()),
                    ("Start", "the bucket list".into()),
                ], "Enter opens the S3 explorer", Enter::Run(vec!["s3".into(), "public".into()])),
            ]),
        }
    }

    fn on_tab(tab: Tab) -> TuiModel {
        let mut model = loaded();
        model.sources = sources();
        model.tab = tab;
        model
    }

    pub fn auth_tab() -> TuiModel {
        on_tab(Tab::Auth)
    }

    /// The second API selected: it has no description, and Enter said so.
    pub fn api_tab() -> TuiModel {
        let mut model = on_tab(Tab::Api);
        model.sources.api.selected = 1;
        model.notice = Some("[api.internal] has no openapi description to explore".into());
        model
    }

    pub fn db_tab() -> TuiModel {
        on_tab(Tab::Db)
    }

    /// No `[data.*]` section.
    pub fn data_tab() -> TuiModel {
        on_tab(Tab::Data)
    }

    /// Two connections, one that starts from the bucket list.
    pub fn s3_tab() -> TuiModel {
        on_tab(Tab::S3)
    }

    pub fn searching_middle() -> TuiModel {
        let mut model = searching();
        model
            .search_query
            .handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Left,
                crossterm::event::KeyModifiers::NONE,
            ));
        model
    }

    /// The palette over the loaded screen with the tabs' rows and two
    /// descriptions' operations, `get` typed.
    pub fn palette() -> TuiModel {
        use crate::domain::types::request_history::HistoryEntry;
        use crate::shell::tui::tea::palette::{PaletteItem, PaletteTarget};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut model = loaded();
        model.sources = sources();
        let key = |model, code, modifiers| {
            tea_update(model, TuiMessage::Key(KeyEvent::new(code, modifiers))).model
        };
        let mut model = key(model, KeyCode::Char('k'), KeyModifiers::CONTROL);
        let op = |api: &str, id: &str, request: &str| {
            PaletteItem::new(
                "op",
                id.into(),
                format!("{api}  {request}"),
                PaletteTarget::Explore {
                    api: api.into(),
                    entry: Some(HistoryEntry {
                        operation: id.into(),
                        params: Vec::new(),
                        body: None,
                        name: None,
                    }),
                },
            )
        };
        model = tea_update(
            model,
            TuiMessage::PaletteItemsLoaded(vec![
                op("github", "repos/get", "GET /repos/{owner}/{repo}"),
                op(
                    "github",
                    "issues/list-for-repo",
                    "GET /repos/{owner}/{repo}/issues",
                ),
                op("pets", "pets/get", "GET /pets/{petId}"),
            ]),
        )
        .model;
        for c in "get".chars() {
            model = key(model, KeyCode::Char(c), KeyModifiers::NONE);
        }
        key(model, KeyCode::Down, KeyModifiers::NONE)
    }

    /// A query nothing matches.
    pub fn palette_no_match() -> TuiModel {
        let mut model = palette();
        for c in "zzz".chars() {
            model = tea_update(
                model,
                TuiMessage::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
            )
            .model;
        }
        model
    }

    pub fn all() -> Vec<(&'static str, TuiModel)> {
        vec![
            ("palette", palette()),
            ("palette_no_match", palette_no_match()),
            ("loading", loading()),
            ("loaded", loaded()),
            ("selected", selected()),
            ("scrolled", scrolled()),
            ("searching", searching()),
            ("searching_middle", searching_middle()),
            ("empty", empty()),
            ("no_profiles", no_profiles()),
            ("error", error()),
            ("browser_failed", browser_failed()),
            ("mfa", mfa()),
            ("help", help()),
            ("processing", processing()),
            ("success", success()),
            ("session_soon", session_soon()),
            ("session_expired", session_expired()),
            ("long_text", long_text()),
            ("unicode", unicode()),
            ("unicode_help", unicode_help()),
            ("auth_tab", auth_tab()),
            ("api_tab", api_tab()),
            ("db_tab", db_tab()),
            ("data_tab", data_tab()),
            ("s3_tab", s3_tab()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    /// Coordinates of the first `needle` on the screen.
    fn position_of(buffer: &Buffer, needle: &str) -> (u16, u16) {
        let lines = buffer_lines(buffer);
        let y = lines.iter().position(|line| line.contains(needle)).unwrap();
        let x = column_of(&lines[y], needle).unwrap();
        (x as u16, y as u16)
    }

    #[test]
    fn the_contract_catches_a_selection_marker_that_lost_its_style() {
        let model = fixtures::loaded();
        let mut buffer = render_buffer(&model, 120, 40);
        assert_eq!(check_contract(&model, &buffer), Vec::<String>::new());
        let (x, y) = position_of(&buffer, "▸");
        buffer[(x, y)].modifier = Modifier::empty();
        buffer[(x, y)].set_fg(Color::Reset);
        let violations = check_contract(&model, &buffer);
        assert!(
            violations.iter().any(|v| v.contains("selection marker")),
            "{violations:?}"
        );
    }

    #[test]
    fn the_contract_catches_a_modal_border_in_the_wrong_tone() {
        let model = fixtures::error();
        let mut buffer = render_buffer(&model, 120, 40);
        assert_eq!(check_contract(&model, &buffer), Vec::<String>::new());
        let (x, y) = position_of(&buffer, &format!("{} Error ", theme::BORDER.top_left));
        buffer[(x, y)].set_style(Tone::Info.style());
        let violations = check_contract(&model, &buffer);
        assert!(
            violations.iter().any(|v| v.contains("Danger")),
            "{violations:?}"
        );
    }

    #[test]
    fn the_contract_catches_a_tab_on_screen_that_is_not_highlighted() {
        let model = fixtures::auth_tab();
        let mut buffer = render_buffer(&model, 120, 40);
        assert_eq!(check_contract(&model, &buffer), Vec::<String>::new());
        let (x, y) = position_of(&buffer, "2 Auth");
        buffer[(x, y)].set_style(theme::hint());
        let violations = check_contract(&model, &buffer);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("\"2 Auth\" is on screen")),
            "{violations:?}"
        );
    }

    #[test]
    fn the_contract_catches_a_badge_drawn_in_the_wrong_state() {
        let model = fixtures::selected();
        let mut buffer = render_buffer(&model, 120, 40);
        assert_eq!(check_contract(&model, &buffer), Vec::<String>::new());
        let (x, y) = position_of(&buffer, "readonly");
        buffer[(x, y)].set_style(theme::badge_off());
        let violations = check_contract(&model, &buffer);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("readonly badge is on")),
            "{violations:?}"
        );
    }
}
