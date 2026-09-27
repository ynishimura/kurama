//! The selectable operation table of the explorer: method and id, with its
//! loading and empty states. A deprecated operation is dimmed.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{fit, panel};
use crate::shell::tui::theme;

/// `DELETE` and the `METHOD` header both fit.
const METHOD_WIDTH: u16 = 6;

pub struct OperationRow<'a> {
    pub method: &'a str,
    pub id: &'a str,
    pub deprecated: bool,
}

pub struct OperationTable<'a> {
    pub title: &'a str,
    pub rows: Vec<OperationRow<'a>>,
    pub selected: usize,
    pub loading: bool,
    /// Shown instead of the table when `rows` is empty and not loading.
    pub empty_message: &'a str,
}

impl OperationTable<'_> {
    pub fn render(self, frame: &mut Frame, area: Rect) {
        let block = panel(self.title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if self.loading {
            frame.render_widget(
                Paragraph::new(Span::styled("Loading the API description…", theme::hint())),
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
        let id_width = inner
            .width
            .saturating_sub(marker_width + METHOD_WIDTH + 1)
            .max(1);
        let rows = self.rows.iter().map(|row| {
            let style = if row.deprecated {
                theme::deprecated()
            } else {
                theme::plain()
            };
            Row::new([
                Cell::from(Span::styled(row.method.to_string(), style)),
                Cell::from(Span::styled(fit(row.id, id_width as usize), style)),
            ])
        });
        let table = Table::new(rows, [Constraint::Length(METHOD_WIDTH), Constraint::Min(1)])
            .header(Row::new(["METHOD", "ID"]).style(theme::table_header()))
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

    fn render(table: OperationTable, width: u16, height: u16) -> Vec<String> {
        render_lines(width, height, |frame| table.render(frame, frame.area()))
    }

    #[test]
    fn rows_show_method_and_id_and_a_long_id_is_cut() {
        let lines = render(
            OperationTable {
                title: "Operations (2)",
                rows: vec![
                    OperationRow {
                        method: "GET",
                        id: "issues/list-for-repo",
                        deprecated: false,
                    },
                    OperationRow {
                        method: "DELETE",
                        id: "repos/delete-a-very-long-operation-id",
                        deprecated: true,
                    },
                ],
                selected: 1,
                loading: false,
                empty_message: "",
            },
            36,
            5,
        );
        assert_eq!(lines[0], "┏ Operations (2) ━━━━━━━━━━━━━━━━━━┓");
        assert_eq!(lines[1], "┃   METHOD ID                      ┃");
        assert_eq!(lines[2], "┃   GET    issues/list-for-repo    ┃");
        assert_eq!(lines[3], "┃ ▸ DELETE repos/delete-a-very-lo… ┃");
    }

    #[test]
    fn loading_and_empty_states_replace_the_table() {
        let loading = render(
            OperationTable {
                title: "Operations",
                rows: vec![],
                selected: 0,
                loading: true,
                empty_message: "",
            },
            40,
            4,
        );
        assert!(loading[1].contains("Loading the API description…"));
        let empty = render(
            OperationTable {
                title: "Operations",
                rows: vec![],
                selected: 0,
                loading: false,
                empty_message: "No operation matches \"zzz\".",
            },
            40,
            4,
        );
        assert!(empty[1].contains("No operation matches \"zzz\"."));
    }
}
