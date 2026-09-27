//! The rows and columns of a database result: column widths from the
//! content, the columns scrolled sideways to keep the selected cell in view,
//! the selected cell highlighted, NULL drawn apart from an empty string, and
//! a cell too wide for its column cut with `…`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use super::fit;
use crate::shell::tui::theme;

/// How a NULL is drawn: one glyph no value is printed as, so it cannot be
/// mistaken for the text `NULL` or for an empty string, which is blank.
pub const NULL_TEXT: &str = "∅";
/// No column is drawn wider than this; its cells are cut and `Enter` shows
/// the whole value.
const MAX_COLUMN_WIDTH: usize = 32;
/// Columns are at least this wide, so a one-letter name still has room.
const MIN_COLUMN_WIDTH: usize = 4;
/// Space between two columns.
const GAP: usize = 2;

pub struct ResultTable<'a> {
    pub columns: Vec<&'a str>,
    pub rows: &'a [Vec<Value>],
    pub selected_row: usize,
    pub selected_column: usize,
    /// Whether the selected cell is drawn as selected: only while the pane
    /// has the keys.
    pub focused: bool,
}

/// What a cell shows on one line: a string as it is, NULL as [`NULL_TEXT`],
/// anything else as JSON, with control characters escaped.
pub fn cell_text(value: &Value) -> String {
    match value {
        Value::Null => NULL_TEXT.to_owned(),
        Value::String(text) => crate::shell::cli::client::safe_text(text),
        other => crate::shell::cli::client::safe_text(&other.to_string()),
    }
}

impl ResultTable<'_> {
    /// Draw the table; says whether any visible cell was cut to fit, which
    /// is a display matter and not a cut in the result.
    pub fn render(self, frame: &mut Frame, area: Rect) -> bool {
        let widths = self.column_widths();
        let marker = theme::SELECTION_MARKER.width();
        let available = (area.width as usize).saturating_sub(marker);
        let first = first_visible_column(&widths, self.selected_column, available);
        let visible_rows = (area.height as usize).saturating_sub(1);
        let first_row = (self.selected_row + 1).saturating_sub(visible_rows.max(1));

        let mut cut = false;
        let mut lines = Vec::with_capacity(area.height as usize);
        let mut header = vec![Span::raw(" ".repeat(marker))];
        for (index, width) in visible(&widths, first, available) {
            header.push(Span::styled(
                pad(&fit(self.columns[index], width), width),
                theme::table_header(),
            ));
        }
        lines.push(Line::from(header));
        for (row_index, row) in self.rows.iter().enumerate().skip(first_row) {
            if lines.len() > visible_rows {
                break;
            }
            let selected_row = row_index == self.selected_row;
            let mut spans = vec![if selected_row {
                Span::styled(theme::SELECTION_MARKER, theme::selection())
            } else {
                Span::raw(" ".repeat(marker))
            }];
            for (index, width) in visible(&widths, first, available) {
                let value = row.get(index).unwrap_or(&Value::Null);
                let text = cell_text(value);
                cut |= text.width() > width.saturating_sub(GAP);
                let shown = pad(&fit(&text, width.saturating_sub(GAP)), width);
                let style = if self.focused && selected_row && index == self.selected_column {
                    theme::selection()
                } else if value.is_null() {
                    theme::absent()
                } else {
                    theme::plain()
                };
                spans.push(Span::styled(shown, style));
            }
            lines.push(Line::from(spans));
        }
        if self.rows.is_empty() {
            lines.push(Line::from(Span::styled("No rows.", theme::hint())));
        }
        frame.render_widget(Paragraph::new(lines), area);
        cut
    }

    /// Each column as wide as its name or its widest cell, within bounds,
    /// plus the gap after it.
    fn column_widths(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let content = self
                    .rows
                    .iter()
                    .filter_map(|row| row.get(index))
                    .map(|value| cell_text(value).width())
                    .chain(std::iter::once(name.width()))
                    .max()
                    .unwrap_or(0);
                content.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH) + GAP
            })
            .collect()
    }
}

/// The first column drawn: the leftmost one from which the selected column
/// still fits, so moving right scrolls exactly as far as it has to.
fn first_visible_column(widths: &[usize], selected: usize, available: usize) -> usize {
    let selected = selected.min(widths.len().saturating_sub(1));
    let mut first = 0;
    while first < selected && widths[first..=selected].iter().sum::<usize>() > available {
        first += 1;
    }
    first
}

/// The columns from `first` that fit in `available`; the last one is cut
/// to the room left rather than dropped, so a wide column is never hidden.
fn visible(widths: &[usize], first: usize, available: usize) -> Vec<(usize, usize)> {
    let mut used = 0;
    let mut shown = Vec::new();
    for (index, width) in widths.iter().enumerate().skip(first) {
        if used >= available {
            break;
        }
        let width = (*width).min(available - used);
        shown.push((index, width));
        used += width;
    }
    shown
}

fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_frame;
    use serde_json::json;

    fn render(table: ResultTable, width: u16, height: u16) -> (Vec<String>, bool) {
        let mut cut = false;
        let buffer = render_frame(width, height, |frame| {
            cut = table.render(frame, frame.area());
        });
        (crate::shell::tui::testing::buffer_lines(&buffer), cut)
    }

    #[test]
    fn null_and_an_empty_string_look_different_and_a_long_cell_is_cut() {
        let rows = vec![
            vec![json!("1"), Value::Null, json!("")],
            vec![
                json!("2"),
                json!("日本語のとても長い説明文です、もっと長くしてみます"),
                json!("x"),
            ],
        ];
        let (lines, cut) = render(
            ResultTable {
                columns: vec!["id", "note", "id"],
                rows: &rows,
                selected_row: 1,
                selected_column: 0,
                focused: true,
            },
            30,
            4,
        );
        assert!(lines[0].starts_with("  id    note"), "{lines:?}");
        assert_eq!(lines[1].trim_end(), "  1     ∅", "{lines:?}");
        assert!(
            lines[2].starts_with("▸ 2     日本語") && lines[2].contains('…'),
            "{lines:?}"
        );
        assert!(cut);
    }

    #[test]
    fn moving_right_scrolls_the_columns_until_the_selected_one_fits() {
        let rows = vec![vec![json!("a"), json!("b"), json!("c"), json!("d")]];
        let columns = vec!["first", "second", "third", "fourth"];
        let at = |selected_column| {
            render(
                ResultTable {
                    columns: columns.clone(),
                    rows: &rows,
                    selected_row: 0,
                    selected_column,
                    focused: true,
                },
                20,
                3,
            )
            .0
        };
        assert!(at(0)[0].starts_with("  first  second"), "{:?}", at(0));
        assert!(at(3)[0].starts_with("  third  fourth"), "{:?}", at(3));
    }

    #[test]
    fn the_selected_row_stays_in_view() {
        let rows: Vec<Vec<Value>> = (0..20).map(|i| vec![json!(i.to_string())]).collect();
        let (lines, _) = render(
            ResultTable {
                columns: vec!["n"],
                rows: &rows,
                selected_row: 15,
                selected_column: 0,
                focused: false,
            },
            12,
            4,
        );
        assert_eq!(lines[3].trim_end(), "▸ 15");
    }

    #[test]
    fn controls_in_a_cell_are_escaped() {
        assert_eq!(cell_text(&json!("a\u{1b}[31mb\nc")), "a\\u{1b}[31mb\\nc");
        assert_eq!(cell_text(&Value::Null), NULL_TEXT);
        assert_eq!(cell_text(&json!(12)), "12");
    }

    /// Only the selected cell of a focused table carries the selection
    /// style; the rest of its row, and an unfocused table, do not.
    #[test]
    fn only_the_selected_cell_of_a_focused_table_is_highlighted() {
        use crate::shell::tui::testing::{column_of, has_style};
        let rows = vec![
            vec![json!("aa"), json!("bb")],
            vec![json!("cc"), json!("dd")],
        ];
        for focused in [true, false] {
            let buffer = render_frame(20, 4, |frame| {
                ResultTable {
                    columns: vec!["x", "y"],
                    rows: &rows,
                    selected_row: 1,
                    selected_column: 1,
                    focused,
                }
                .render(frame, frame.area());
            });
            let lines = crate::shell::tui::testing::buffer_lines(&buffer);
            let at = |needle: &str| {
                let y = lines.iter().position(|l| l.contains(needle)).unwrap();
                &buffer[(column_of(&lines[y], needle).unwrap() as u16, y as u16)]
            };
            assert_eq!(has_style(at("dd"), theme::selection()), focused);
            assert!(!has_style(at("cc"), theme::selection()));
            assert!(!has_style(at("bb"), theme::selection()));
        }
    }

    /// A column that exactly fills the room left is drawn from the first
    /// column; one more column is what scrolls.
    #[test]
    fn a_selected_column_that_just_fits_does_not_scroll() {
        assert_eq!(first_visible_column(&[6, 6], 1, 12), 0);
        assert_eq!(first_visible_column(&[6, 6], 1, 11), 1);
    }

    #[test]
    fn an_empty_result_says_so_under_its_header() {
        let (lines, _) = render(
            ResultTable {
                columns: vec!["status"],
                rows: &[],
                selected_row: 0,
                selected_column: 0,
                focused: true,
            },
            20,
            3,
        );
        assert_eq!(lines[1].trim_end(), "No rows.");
    }
}
