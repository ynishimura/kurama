//! Label/value rows for the selected profile. A value longer than the pane
//! wraps onto indented continuation lines, so a full role ARN is always
//! readable and the labels stay aligned.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::key_values::entry_lines;
use super::panel;
use crate::shell::tui::tea::messages::ProfileRow;
use crate::shell::tui::theme::{self, Tone};

const LABEL_WIDTH: usize = 8;

pub struct DetailPane<'a> {
    pub title: &'a str,
    pub row: Option<&'a ProfileRow>,
}

impl DetailPane<'_> {
    pub fn render(self, frame: &mut Frame, area: Rect) {
        let block = panel(self.title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(row) = self.row else {
            frame.render_widget(
                Paragraph::new(Span::styled("No profile selected.", theme::hint())),
                inner,
            );
            return;
        };

        let value = |text: Option<&str>| match text {
            Some(text) => Span::raw(text.to_string()),
            None => Span::styled(theme::NONE_TEXT, theme::absent()),
        };
        let shell = if row.active {
            Span::styled("this shell holds its credentials", theme::selection())
        } else {
            Span::styled("not active", theme::absent())
        };
        let next = if row.needs_human {
            Span::styled(
                "Enter asks for the MFA code (no provider configured)",
                Tone::Warning.style(),
            )
        } else {
            Span::styled("Enter assumes the role", theme::hint())
        };
        let entries: Vec<(String, Span)> = vec![
            ("Profile".into(), Span::raw(row.name.as_str())),
            ("Kind".into(), Span::raw(row.kind)),
            ("Role".into(), value(row.role_arn.as_deref())),
            ("Region".into(), value(row.region.as_deref())),
            ("MFA".into(), value(row.mfa_serial.as_deref())),
            (
                "Session".into(),
                Span::styled(row.session.as_str(), theme::session_style(&row.session)),
            ),
            ("Shell".into(), shell),
            (String::new(), Span::raw("")),
            ("Next".into(), next),
        ];

        let lines = entry_lines(&entries, LABEL_WIDTH, inner.width as usize);
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    fn render(pane: DetailPane, width: u16, height: u16) -> Vec<String> {
        render_lines(width, height, |frame| pane.render(frame, frame.area()))
            .into_iter()
            .map(|line| line.trim_end_matches([' ', '┃']).to_string())
            .collect()
    }

    #[test]
    fn detail_lists_every_field_and_wraps_long_values_under_the_label() {
        let row = ProfileRow {
            name: "ops-mfa".into(),
            kind: "aws",
            session: "valid (1h 30m)".into(),
            session_expires_at: None,
            active: true,
            role_arn: Some("arn:aws:iam::123456789012:role/OperationsAdministrator".into()),
            region: Some("ap-northeast-1".into()),
            mfa_serial: None,
            needs_human: false,
        };
        let lines = render(
            DetailPane {
                title: "Details",
                row: Some(&row),
            },
            40,
            13,
        );
        assert_eq!(lines[1], "┃ Profile ops-mfa");
        assert_eq!(lines[3], "┃ Role    arn:aws:iam::123456789012:ro");
        assert_eq!(lines[4], "┃         le/OperationsAdministrator");
        assert_eq!(lines[6], "┃ MFA     -");
        assert_eq!(lines[8], "┃ Shell   this shell holds its");
        assert_eq!(lines[9], "┃         credentials");
        assert_eq!(lines[10], "");
        assert_eq!(lines[11], "┃ Next    Enter assumes the role");
    }
}
