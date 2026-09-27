//! One-row header: application title, a subtitle, the time a credential has
//! left and mode badges.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::domain::functions::profile_status::Expiry;
use crate::shell::tui::theme;

/// A mode indicator such as `readonly` or `console`.
#[derive(Debug, Clone, Copy)]
pub struct Badge {
    pub label: &'static str,
    pub on: bool,
}

pub struct Header<'a> {
    title: &'a str,
    subtitle: Cow<'a, str>,
    time_left: Option<(String, Expiry)>,
    badges: Vec<Badge>,
}

impl<'a> Header<'a> {
    pub fn new(title: &'a str, subtitle: impl Into<Cow<'a, str>>) -> Self {
        Self {
            title,
            subtitle: subtitle.into(),
            time_left: None,
            badges: Vec::new(),
        }
    }

    /// `MFA 1h 30m`, drawn before the badges in the color of its expiry.
    pub fn time_left(mut self, time_left: Option<(String, Expiry)>) -> Self {
        self.time_left = time_left;
        self
    }

    pub fn badges(mut self, badges: impl IntoIterator<Item = Badge>) -> Self {
        self.badges.extend(badges);
        self
    }
}

impl Widget for Header<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut right: Vec<Span> = Vec::new();
        if let Some((text, expiry)) = self.time_left {
            right.push(Span::raw(" "));
            right.push(Span::styled(text, theme::expiry_style(expiry)));
            right.push(Span::raw(" "));
        }
        for badge in &self.badges {
            right.push(Span::raw(" "));
            let style = if badge.on {
                theme::badge_on()
            } else {
                theme::badge_off()
            };
            right.push(Span::styled(format!(" {} ", badge.label), style));
        }
        right.push(Span::raw(" "));
        let right = Line::from(right);
        let right_width = right.width() as u16;

        // Badges keep their width; the subtitle gives way on narrow terminals.
        let left_width = area.width.saturating_sub(right_width);
        let title = format!(" {} ", self.title);
        let subtitle = super::fit(
            &format!(" {}", self.subtitle),
            (left_width as usize).saturating_sub(title.width()),
        );
        let left = Line::from(vec![
            Span::styled(title, theme::title()),
            Span::styled(subtitle, theme::hint()),
        ]);
        Paragraph::new(left).render(
            Rect {
                width: left_width,
                ..area
            },
            buf,
        );
        Paragraph::new(right).alignment(Alignment::Right).render(
            Rect {
                x: area.x + left_width,
                width: right_width.min(area.width),
                ..area
            },
            buf,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::testing::render_lines;

    fn text(width: u16, header: Header) -> String {
        let area = Rect::new(0, 0, width, 1);
        render_lines(width, 1, |frame| frame.render_widget(header, area)).remove(0)
    }

    #[test]
    fn header_shows_title_subtitle_and_right_aligned_badges() {
        let header = Header::new("kurama", "AWS credential switcher").badges([
            Badge {
                label: "readonly",
                on: true,
            },
            Badge {
                label: "console",
                on: false,
            },
        ]);
        assert_eq!(
            text(60, header),
            " kurama  AWS credential switcher        readonly   console"
        );
    }

    #[test]
    fn time_left_sits_before_the_badges_in_the_color_of_its_expiry() {
        let header = Header::new("kurama", "AWS")
            .time_left(Some(("MFA 14m".into(), Expiry::Soon)))
            .badges([Badge {
                label: "readonly",
                on: false,
            }]);
        let area = Rect::new(0, 0, 40, 1);
        let buffer = crate::shell::tui::testing::render_frame(40, 1, |frame| {
            frame.render_widget(header, area)
        });
        let line: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
        assert_eq!(line, " kurama  AWS        MFA 14m   readonly  ");
        let x = line.find("MFA").unwrap() as u16;
        assert_eq!(
            buffer[(x, 0)].fg,
            theme::expiry_style(Expiry::Soon).fg.unwrap()
        );
    }

    #[test]
    fn narrow_header_cuts_the_subtitle_before_the_badges() {
        let header = Header::new("kurama", "AWS credential switcher").badges([Badge {
            label: "readonly",
            on: true,
        }]);
        assert_eq!(text(30, header), " kurama  AWS cred…  readonly");
    }
}
