//! One-line text editing at UTF-8 boundaries and a display-width-aware cursor window.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::shell::tui::theme;

pub const CURSOR: &str = "▏";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    cursor: usize,
}

impl From<String> for LineInput {
    fn from(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
        }
    }
}

impl From<&str> for LineInput {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl LineInput {
    /// Set text and its cursor from a completion produced at a UTF-8 boundary.
    pub fn with_cursor(text: String, cursor: usize) -> Self {
        assert!(text.is_char_boundary(cursor));
        Self { text, cursor }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn cursor_column(&self) -> usize {
        self.text[..self.cursor].width()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Apply an editing key; return whether the text changed. Cursor movement
    /// does not reset a search's selection or a form's validation message.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let old_len = self.text.len();
        match (key.code, key.modifiers) {
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => self.cursor = 0,
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => self.cursor = self.text.len(),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => self.delete_before(0),
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                let trimmed = self.text[..self.cursor].trim_end_matches(char::is_whitespace);
                let start = trimmed.rfind(char::is_whitespace).map_or(0, |index| {
                    index + trimmed[index..].chars().next().unwrap().len_utf8()
                });
                self.delete_before(start);
            }
            (code, _) if typed(key) => match code {
                KeyCode::Char(ch) if !ch.is_control() => {
                    self.text.insert(self.cursor, ch);
                    self.cursor += ch.len_utf8();
                }
                KeyCode::Backspace => self.delete_before(self.previous()),
                KeyCode::Delete => {
                    self.text.drain(self.cursor..self.next());
                }
                KeyCode::Left => self.cursor = self.previous(),
                KeyCode::Right => self.cursor = self.next(),
                KeyCode::Home => self.cursor = 0,
                KeyCode::End => self.cursor = self.text.len(),
                _ => {}
            },
            _ => {}
        }
        self.text.len() != old_len
    }

    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next(&self) -> usize {
        self.cursor
            + self.text[self.cursor..]
                .chars()
                .next()
                .map_or(0, char::len_utf8)
    }

    fn delete_before(&mut self, start: usize) {
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Visible text on each side of the cursor, including ellipses when clipped.
    /// `width` includes the cursor's one column.
    pub fn visible_parts(&self, width: usize) -> (String, String) {
        let (before, after) = self.as_str().split_at(self.cursor());
        let available = width.saturating_sub(1);
        if self.cursor_column() + after.width() <= available {
            return (before.into(), after.into());
        }
        let right = super::fit(after, available / 2);
        let left = clip_start(before, available.saturating_sub(right.width()));
        let right = super::fit(after, available.saturating_sub(left.width()));
        (left, right)
    }

    pub fn line(&self, width: usize) -> Line<'static> {
        if width == 0 {
            return Line::default();
        }
        let (before, after) = self.visible_parts(width);
        Line::from(vec![
            Span::raw(before),
            Span::styled(CURSOR, theme::cursor()),
            Span::raw(after),
        ])
    }
}

/// Plain or shifted text, shared by all unrestricted one-line inputs.
pub fn typed(key: KeyEvent) -> bool {
    key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT
}

fn clip_start(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut start = text.len();
    for (index, _) in text.char_indices().rev() {
        if text[index..].width() >= width {
            break;
        }
        start = index;
    }
    format!("…{}", &text[start..])
}
#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn control(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn check(input: &LineInput, text: &str, cursor: usize) {
        assert_eq!(input.as_str(), text);
        assert_eq!(input.cursor(), cursor);
        assert!(text.is_char_boundary(cursor));
        assert_eq!(input.cursor_column(), text[..cursor].width());
    }

    #[test]
    fn line_input_edits_unicode_at_character_boundaries() {
        let mut input = LineInput::default();
        for ch in "a界\u{1f600}e\u{301}".chars() {
            assert!(input.handle_key(key(KeyCode::Char(ch))));
            assert!(input.as_str().is_char_boundary(input.cursor()));
        }
        check(&input, "a界\u{1f600}e\u{301}", 11);
        assert_eq!(input.cursor_column(), 6);
        assert!(!input.handle_key(key(KeyCode::Left)));
        check(&input, "a界\u{1f600}e\u{301}", 9);
        assert!(input.handle_key(key(KeyCode::Backspace)));
        check(&input, "a界\u{1f600}\u{301}", 8);
        assert!(input.handle_key(key(KeyCode::Delete)));
        check(&input, "a界\u{1f600}", 8);
        input.handle_key(key(KeyCode::Left));
        input.handle_key(key(KeyCode::Delete));
        check(&input, "a界", 4);
        input.handle_key(key(KeyCode::Left));
        input.handle_key(key(KeyCode::Char('語')));
        check(&input, "a語界", 4);
        input.handle_key(key(KeyCode::Home));
        input.handle_key(key(KeyCode::Delete));
        check(&input, "語界", 0);
        input.handle_key(key(KeyCode::End));
        input.handle_key(key(KeyCode::Backspace));
        check(&input, "語", 3);
        input.handle_key(control('u'));
        check(&input, "", 0);
        assert!(!input.handle_key(key(KeyCode::Backspace)));
        assert!(!input.handle_key(key(KeyCode::Delete)));
    }

    #[test]
    fn line_input_moves_and_kills_only_before_the_cursor() {
        let mut input = LineInput::from("one 二三  tail");
        for _ in 0..4 {
            input.handle_key(key(KeyCode::Left));
        }
        assert!(input.handle_key(control('w')));
        check(&input, "one tail", 4);
        assert!(input.handle_key(control('u')));
        check(&input, "tail", 0);
        input.handle_key(control('e'));
        check(&input, "tail", 4);
        input.handle_key(key(KeyCode::Right));
        check(&input, "tail", 4);
        input.handle_key(control('a'));
        input.handle_key(key(KeyCode::Left));
        check(&input, "tail", 0);
        input.handle_key(key(KeyCode::Right));
        check(&input, "tail", 1);
        input.handle_key(key(KeyCode::Char('X')));
        check(&input, "tXail", 2);
        input.clear();
        check(&input, "", 0);
    }

    #[test]
    fn line_input_accepts_shift_but_not_shortcuts_or_control_characters() {
        let mut input = LineInput::default();
        assert!(input.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)));
        for event in [
            control('c'),
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT),
            key(KeyCode::Char('\n')),
            key(KeyCode::Tab),
        ] {
            assert!(!input.handle_key(event));
        }
        check(&input, "A", 1);
    }

    #[test]
    fn line_input_clips_emoji_sequences_by_their_combined_display_width() {
        let mut input = LineInput::from("#\u{fe0f}zz");
        input.handle_key(key(KeyCode::Home));
        let (before, after) = input.visible_parts(3);
        assert!(
            before.width() + 1 + after.width() <= 3,
            "{before:?}|{after:?}"
        );
        assert_eq!(input.line(3).width(), 3);
    }

    #[test]
    fn line_input_shows_text_on_both_sides_of_a_clipped_middle_cursor() {
        let mut input = LineInput::from("abcdefghijk");
        input.handle_key(key(KeyCode::Home));
        for _ in 0..5 {
            input.handle_key(key(KeyCode::Right));
        }
        assert_eq!(input.visible_parts(7), ("…de".into(), "fg…".into()));
    }

    #[test]
    fn line_input_keeps_the_cursor_visible_at_start_middle_and_end() {
        let mut input = LineInput::from("先頭ab\u{1f600}e\u{301}末尾");
        for code in [
            KeyCode::Home,
            KeyCode::Right,
            KeyCode::Right,
            KeyCode::Right,
            KeyCode::End,
        ] {
            input.handle_key(key(code));
            for width in 1..=20 {
                let (before, after) = input.visible_parts(width);
                assert!(before.width() < width, "{before:?} at {width}");
                assert!(
                    before.width() + 1 + after.width() <= width,
                    "{before:?}|{after:?} at {width}"
                );
                let rendered = input.line(width).to_string();
                assert_eq!(rendered.matches(CURSOR).count(), 1);
                assert_eq!(rendered, format!("{before}{CURSOR}{after}"));
            }
        }
        assert_eq!(input.line(0).to_string(), "");
        input = "abcdef".into();
        assert_eq!(input.visible_parts(4), ("…ef".into(), "".into()));
        input.handle_key(key(KeyCode::Home));
        assert_eq!(input.visible_parts(4), ("".into(), "ab…".into()));
        input.handle_key(key(KeyCode::Right));
        assert_eq!(input.visible_parts(10), ("a".into(), "bcdef".into()));
    }
}
