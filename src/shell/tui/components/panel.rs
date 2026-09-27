//! Bordered block with a title: the frame of every list and pane.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding};

use crate::shell::tui::theme;

/// Heavy border in the panel color, the title on the top border and one
/// column of padding on each side so content never touches the border.
pub fn panel(title: &str) -> Block<'_> {
    panel_styled(title, theme::panel_title())
}

/// A panel whose title carries a tone (the status of a response).
pub fn panel_styled(title: &str, title_style: Style) -> Block<'_> {
    Block::bordered()
        .border_set(theme::BORDER)
        .border_style(theme::panel())
        .title(Line::from(Span::styled(format!(" {title} "), title_style)))
        .padding(Padding::horizontal(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    #[test]
    fn panel_draws_the_theme_border_with_the_title_and_pads_the_content() {
        let area = Rect::new(0, 0, 16, 3);
        let block = panel("Profiles");
        assert_eq!(block.inner(area), Rect::new(2, 1, 12, 1));
        let mut buf = Buffer::empty(area);
        block.render(area, &mut buf);
        let top: String = (0..16).map(|x| buf[(x, 0)].symbol()).collect();
        assert_eq!(top, "┏ Profiles ━━━━┓");
    }
}
