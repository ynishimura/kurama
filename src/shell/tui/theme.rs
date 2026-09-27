//! Visual tokens shared by every TUI screen.
//!
//! Screens and components never pick a color, a modifier or a border set
//! themselves; they name a token from this module. Changing the look of the
//! whole TUI is a change here, and every snapshot shows the effect.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;

use crate::domain::functions::audit::AuditOutcome;
use crate::domain::functions::profile_status::Expiry;

/// Apply NO_COLOR before the backend diffs the frame. Suppressing backend
/// color commands this way preserves bold and other non-color attributes.
pub fn remove_colors(buffer: &mut ratatui::buffer::Buffer) {
    for cell in &mut buffer.content {
        cell.set_fg(Color::Reset).set_bg(Color::Reset);
    }
}

// A cool Japanese palette on the terminal's own background: lapis (ruri)
// for the title seal and the selection, dayflower blue (tsuyukusa) frames,
// pale green-blue (asagi) keys and titles, moon white (geppaku) on blue, and
// the traditional colors below for states. Colors are 24-bit; NO_COLOR
// removes every one of them (`remove_colors`).

/// Dayflower blue (tsuyukusa-iro): panel frames, the cursor, information.
pub const ACCENT: Color = Color::Rgb(0x38, 0xA1, 0xDB);
/// Lapis (ruri-iro): the ground of the title seal and the selection.
pub const SEAL: Color = Color::Rgb(0x1E, 0x50, 0xA2);
/// Pale green-blue (asagi-iro): key labels, panel titles, badges that are on.
pub const HIGHLIGHT: Color = Color::Rgb(0x00, 0xA3, 0xAF);
/// Moon white (geppaku): text on a lapis ground.
pub const ON_ACCENT: Color = Color::Rgb(0xEA, 0xF4, 0xFC);
/// Ink (sumi-iro): text on a pale green-blue ground.
pub const INK: Color = Color::Rgb(0x1C, 0x1C, 0x1C);
/// Secondary text (nibi-iro): hints, separators, values that are absent.
pub const MUTED: Color = Color::Rgb(0x8C, 0x8C, 0x8C);
/// Young bamboo (wakatake-iro).
pub const SUCCESS: Color = Color::Rgb(0x68, 0xBE, 0x8D);
/// Kerria yellow (yamabuki-iro).
pub const WARNING: Color = Color::Rgb(0xF8, 0xB5, 0x00);
/// Crimson (karakurenai): the only red on the screen.
pub const DANGER: Color = Color::Rgb(0xE9, 0x54, 0x64);

/// Border set used by every panel and modal: heavy lines, like a torii's
/// beams.
pub const BORDER: border::Set = border::THICK;

/// Marker in front of the selected row.
pub const SELECTION_MARKER: &str = "▸ ";
/// Markers in front of an open and a closed node of a tree.
pub const EXPANDED_MARKER: &str = "▾ ";
pub const COLLAPSED_MARKER: &str = "▸ ";
/// Marker on the profile whose credentials the shell holds.
pub const ACTIVE_MARKER: &str = "*";
/// Text for a value that is not set.
pub const NONE_TEXT: &str = "-";

/// Application title in the header: a lapis seal.
pub const fn title() -> Style {
    Style::new()
        .fg(ON_ACCENT)
        .bg(SEAL)
        .add_modifier(Modifier::BOLD)
}

/// Column headers of a table.
pub const fn table_header() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// The selected row of a list or table: moon white on lapis.
pub const fn selection() -> Style {
    Style::new()
        .fg(ON_ACCENT)
        .bg(SEAL)
        .add_modifier(Modifier::BOLD)
}

/// Border of a panel: a dayflower-blue frame.
pub const fn panel() -> Style {
    Style::new().fg(ACCENT)
}

/// Title of a panel (drawn on the border).
pub const fn panel_title() -> Style {
    Style::new().fg(HIGHLIGHT).add_modifier(Modifier::BOLD)
}

/// The key part of a key hint (`Enter`, `q`).
pub const fn key() -> Style {
    Style::new().fg(HIGHLIGHT).add_modifier(Modifier::BOLD)
}

/// The action part of a key hint (`assume`, `quit`).
pub const fn hint() -> Style {
    Style::new().fg(MUTED)
}

/// Label column of a key/value pane.
pub const fn label() -> Style {
    Style::new().fg(MUTED)
}

/// A value that is not set.
pub const fn absent() -> Style {
    Style::new().fg(MUTED)
}

/// A mode badge that is switched on: ink on pale green-blue.
pub const fn badge_on() -> Style {
    Style::new()
        .fg(INK)
        .bg(HIGHLIGHT)
        .add_modifier(Modifier::BOLD)
}

/// A mode badge that is switched off.
pub const fn badge_off() -> Style {
    Style::new().fg(MUTED)
}

/// Ordinary text in a table cell.
pub const fn plain() -> Style {
    Style::new()
}

/// A row that the description marks deprecated.
pub const fn deprecated() -> Style {
    Style::new().fg(MUTED).add_modifier(Modifier::CROSSED_OUT)
}

/// The cursor after the text being typed.
pub const fn cursor() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Title of a result pane: the tone of the HTTP status.
pub const fn status_style(status: u16) -> Style {
    let color = match status {
        200..=299 => SUCCESS,
        300..=399 => ACCENT,
        400..=499 => WARNING,
        _ => DANGER,
    };
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

/// Tone of a message: modal border, title and message color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub const fn color(self) -> Color {
        match self {
            Self::Info => ACCENT,
            Self::Success => SUCCESS,
            Self::Warning => WARNING,
            Self::Danger => DANGER,
        }
    }

    pub const fn style(self) -> Style {
        Style::new().fg(self.color())
    }

    /// Title of a modal in this tone.
    pub const fn title(self) -> Style {
        Style::new().fg(self.color()).add_modifier(Modifier::BOLD)
    }
}

/// Color of a session column value, as `kurama status` describes it.
pub fn session_style(session: &str) -> Style {
    if session.starts_with("valid") {
        Style::new().fg(SUCCESS)
    } else if session == "none" {
        Style::new().fg(WARNING)
    } else if session == "unreadable" {
        Style::new().fg(DANGER)
    } else {
        absent()
    }
}

/// Color of the time a credential has left: warned under 15 minutes,
/// danger once it is gone.
pub fn expiry_style(expiry: Expiry) -> Style {
    match expiry {
        Expiry::Later => Style::new().fg(SUCCESS),
        Expiry::Soon => Style::new().fg(WARNING),
        Expiry::Expired => Style::new().fg(DANGER).add_modifier(Modifier::BOLD),
    }
}

/// A row of the activity monitor: a call the `[agent]` policy refused in
/// kerria yellow, one that failed in crimson, the rest in plain text.
pub fn outcome_style(outcome: AuditOutcome) -> Style {
    match outcome {
        AuditOutcome::Refused => Style::new().fg(WARNING),
        AuditOutcome::Failed => Style::new().fg(DANGER),
        AuditOutcome::Succeeded | AuditOutcome::HandedOff => plain(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_call_is_colored_apart_from_a_failed_one() {
        assert_eq!(outcome_style(AuditOutcome::Refused).fg, Some(WARNING));
        assert_eq!(outcome_style(AuditOutcome::Failed).fg, Some(DANGER));
        assert_eq!(outcome_style(AuditOutcome::Succeeded), plain());
        assert_eq!(outcome_style(AuditOutcome::HandedOff), plain());
    }

    #[test]
    fn time_left_colors_follow_the_expiry() {
        assert_eq!(expiry_style(Expiry::Later).fg, Some(SUCCESS));
        assert_eq!(expiry_style(Expiry::Soon).fg, Some(WARNING));
        assert_eq!(expiry_style(Expiry::Expired).fg, Some(DANGER));
    }

    #[test]
    fn monochrome_frames_preserve_text_and_emphasis_at_every_size() {
        use crate::shell::tui::{explorer::testing as explorer, testing as home};

        for (width, height) in home::SIZES {
            let home_buffers = home::fixtures::all()
                .into_iter()
                .map(|(_, model)| home::render_buffer(&model, width, height));
            let explorer_buffers = explorer::fixtures::all()
                .into_iter()
                .map(|(_, model)| explorer::render_buffer(&model, width, height));
            for original in home_buffers.chain(explorer_buffers) {
                let mut monochrome = original.clone();
                remove_colors(&mut monochrome);
                for (before, after) in original.content.iter().zip(&monochrome.content) {
                    assert_eq!(after.fg, Color::Reset);
                    assert_eq!(after.bg, Color::Reset);
                    assert_eq!(after.symbol(), before.symbol());
                    assert_eq!(after.modifier, before.modifier);
                }
            }
        }
    }

    #[test]
    fn status_colors_follow_the_class_of_the_status() {
        assert_eq!(status_style(200).fg, Some(SUCCESS));
        assert_eq!(status_style(304).fg, Some(ACCENT));
        assert_eq!(status_style(404).fg, Some(WARNING));
        assert_eq!(status_style(503).fg, Some(DANGER));
    }

    #[test]
    fn session_colors_follow_the_status_words() {
        assert_eq!(session_style("valid (1h 30m)").fg, Some(SUCCESS));
        assert_eq!(session_style("none").fg, Some(WARNING));
        assert_eq!(session_style("unreadable").fg, Some(DANGER));
        assert_eq!(session_style("-").fg, Some(MUTED));
        assert_eq!(session_style("cache disabled").fg, Some(MUTED));
    }
}
