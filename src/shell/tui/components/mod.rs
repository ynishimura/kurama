//! Reusable widgets for the TUI. A screen composes these and never draws a
//! border, a badge or a key hint on its own, so every screen looks the same
//! and every snapshot exercises the same code.
//!
//! - [`Header`]: title, subtitle and mode badges on one row
//! - [`KeyHints`]: one row of `key action` pairs that fit the width
//! - [`panel`]: bordered block with a title, for lists and panes
//! - [`ProfileTable`]: the selectable profile table, with its loading and
//!   empty states
//! - [`DetailPane`]: label/value rows for the selected profile
//! - [`KeyValues`]: label/value rows for anything else (an operation)
//! - [`Modal`]: a centered message (help, MFA prompt, error, success)
//! - [`OperationTable`]: the selectable operation table of the explorer
//! - [`Form`]: parameter fields and the request body of an operation
//! - [`TextView`]: a scrollable text (the response of a call)
//! - [`ResultTable`]: the rows and columns of a database result
//! - [`Tabs`]: one row of numbered tabs

pub mod detail_pane;
pub mod form;
pub mod header;
pub mod key_hints;
pub mod key_values;
pub mod line_input;
pub mod list_navigation;
pub mod modal;
pub mod operation_table;
pub mod panel;
pub mod profile_table;
pub mod result_table;
pub mod tabs;
pub mod text_view;

pub use detail_pane::DetailPane;
pub use form::{Form, FormBody, FormField};
pub use header::{Badge, Header};
pub use key_hints::KeyHints;
pub use key_values::KeyValues;
pub use line_input::LineInput;
pub use modal::Modal;
pub use operation_table::{OperationRow, OperationTable};
pub use panel::{panel, panel_styled};
pub use profile_table::{PROFILE_COLUMNS, ProfileTable, TableRow};
pub use result_table::ResultTable;
pub use tabs::Tabs;
pub use text_view::TextView;

use unicode_width::UnicodeWidthStr;

/// Text cut to at most `width` columns, ending in `…` when cut. Display
/// width aware, so a full-width character counts as two columns.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    format!("{}…", take_width(text, width - 1))
}

/// Greedy word wrap by display width; a word wider than a line is cut.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![String::new()];
    for word in text.split(' ') {
        let mut rest = word.to_string();
        while !rest.is_empty() {
            let line = lines.last_mut().unwrap();
            let gap = usize::from(!line.is_empty());
            if line.width() + gap + rest.width() <= width {
                if gap == 1 {
                    line.push(' ');
                }
                line.push_str(&rest);
                rest.clear();
            } else if line.is_empty() {
                // A word wider than the line is cut; a character wider than
                // the line still goes on its own so the loop makes progress.
                let mut head = take_width(&rest, width);
                if head.is_empty() {
                    head = &rest[..rest.chars().next().map_or(0, char::len_utf8)];
                }
                let taken = head.len();
                line.push_str(head);
                rest.drain(..taken);
                lines.push(String::new());
            } else {
                lines.push(String::new());
            }
        }
    }
    if lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// The longest prefix of `text` that fits in `width` columns; empty when the
/// first character alone is wider.
pub fn take_width(text: &str, width: usize) -> &str {
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        let next = index + ch.len_utf8();
        if text[..next].width() > width {
            break;
        }
        end = next;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_short_text_and_cuts_long_text_with_an_ellipsis() {
        assert_eq!(fit("dev", 10), "dev");
        assert_eq!(fit("production-account", 10), "productio…");
        assert_eq!(fit("本番環境", 5), "本番…");
        assert_eq!(fit("本番環境", 8), "本番環境");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(fit("abc", 1), "…");
    }

    #[test]
    fn wrap_breaks_between_words_and_cuts_words_wider_than_a_line() {
        assert_eq!(
            wrap("arn:aws:iam::123456789012:role/OperationsAdministrator", 27),
            ["arn:aws:iam::123456789012:r", "ole/OperationsAdministrator"]
        );
        assert_eq!(
            wrap("this shell holds its credentials", 27),
            ["this shell holds its", "credentials"]
        );
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(
            wrap("本番環境の管理者", 5),
            ["本番", "環境", "の管", "理者"]
        );
    }

    #[test]
    fn take_width_stops_before_a_character_that_does_not_fit() {
        assert_eq!(take_width("本番環境", 5), "本番");
        assert_eq!(take_width("本番環境", 1), "");
        assert_eq!(take_width("abc", 10), "abc");
    }
}
