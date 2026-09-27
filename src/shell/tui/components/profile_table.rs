//! The selectable profile table with its loading and empty states: AWS
//! profiles, and the rows of the other source tabs under their own column
//! names.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{fit, panel};
use crate::shell::tui::tea::messages::ProfileRow;
use crate::shell::tui::theme;

const KIND_WIDTH: u16 = 4;
/// `valid (11h 59m)` is the longest session text; it always fits.
const SESSION_MIN_WIDTH: u16 = 15;
/// Below this width the profile column shows an ellipsis instead of
/// squeezing the session column.
const NAME_MIN_WIDTH: u16 = 8;

/// One row as the table shows it.
pub struct TableRow<'a> {
    pub active: bool,
    pub kind: &'a str,
    pub name: &'a str,
    /// The last column, colored as `kurama status` words it.
    pub state: &'a str,
}

impl<'a> From<&'a ProfileRow> for TableRow<'a> {
    fn from(row: &'a ProfileRow) -> Self {
        Self {
            active: row.active,
            kind: row.kind,
            name: &row.name,
            state: &row.session,
        }
    }
}

/// Column names of AWS profiles.
pub const PROFILE_COLUMNS: [&str; 2] = ["PROFILE", "SESSION"];

pub struct ProfileTable<'a> {
    pub title: &'a str,
    /// The names of the name and state columns.
    pub columns: [&'a str; 2],
    pub rows: Vec<TableRow<'a>>,
    pub selected: usize,
    pub loading: bool,
    /// Shown instead of the table when `rows` is empty and not loading.
    pub empty_message: &'a str,
}

impl ProfileTable<'_> {
    pub fn render(self, frame: &mut Frame, area: Rect) {
        let block = panel(self.title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if self.loading {
            frame.render_widget(
                Paragraph::new(Span::styled("Loading…", theme::hint())),
                inner,
            );
            return;
        }
        if self.rows.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(self.empty_message, theme::hint()))
                    .wrap(Wrap { trim: true }),
                inner,
            );
            return;
        }

        let marker_width = theme::SELECTION_MARKER.width() as u16;
        let column_gaps = 3;
        let fixed = marker_width + 1 + KIND_WIDTH + SESSION_MIN_WIDTH + column_gaps;
        let longest_name = self
            .rows
            .iter()
            .map(|row| row.name.width() as u16)
            .chain(std::iter::once(self.columns[0].width() as u16))
            .max()
            .unwrap_or(0);
        let name_width = longest_name
            .min(inner.width.saturating_sub(fixed))
            .max(NAME_MIN_WIDTH.min(longest_name));

        let rows = self.rows.iter().map(|row| {
            Row::new([
                Cell::from(if row.active { theme::ACTIVE_MARKER } else { "" }),
                Cell::from(row.kind),
                Cell::from(fit(row.name, name_width as usize)),
                Cell::from(Span::styled(row.state, theme::session_style(row.state))),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(1),
                Constraint::Length(KIND_WIDTH),
                Constraint::Length(name_width),
                Constraint::Min(SESSION_MIN_WIDTH),
            ],
        )
        .header(
            Row::new(["", "KIND", self.columns[0], self.columns[1]]).style(theme::table_header()),
        )
        .row_highlight_style(theme::selection())
        .highlight_symbol(theme::SELECTION_MARKER);
        let mut state = TableState::default();
        state.select(Some(self.selected));
        frame.render_stateful_widget(table, inner, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    fn rows(names: &[&str]) -> Vec<ProfileRow> {
        names
            .iter()
            .map(|name| ProfileRow {
                name: name.to_string(),
                kind: "aws",
                session: "-".into(),
                session_expires_at: None,
                active: false,
                role_arn: None,
                region: None,
                mfa_serial: None,
                needs_human: false,
            })
            .collect()
    }

    fn render(table: ProfileTable, width: u16, height: u16) -> Vec<String> {
        render_lines(width, height, |frame| table.render(frame, frame.area()))
    }

    #[test]
    fn long_profile_names_are_cut_with_an_ellipsis_and_the_session_stays_visible() {
        let rows = rows(&["a-very-long-profile-name-that-does-not-fit"]);
        let lines = render(
            ProfileTable {
                title: "Profiles",
                columns: PROFILE_COLUMNS,
                rows: rows.iter().map(TableRow::from).collect(),
                selected: 0,
                loading: false,
                empty_message: "",
            },
            40,
            4,
        );
        assert_eq!(lines[1], "┃     KIND PROFILE     SESSION         ┃");
        assert_eq!(lines[2], "┃ ▸   aws  a-very-lon… -               ┃");
    }

    #[test]
    fn the_selected_row_scrolls_into_view() {
        let names: Vec<String> = (0..20).map(|i| format!("profile-{i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let rows = rows(&names);
        let lines = render(
            ProfileTable {
                title: "Profiles",
                columns: PROFILE_COLUMNS,
                rows: rows.iter().map(TableRow::from).collect(),
                selected: 19,
                loading: false,
                empty_message: "",
            },
            40,
            6,
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("▸   aws  profile-19")),
            "{lines:#?}"
        );
    }
}
