//! Where a list key moves the selection: one row for the arrows and `j` /
//! `k`, a page for PageUp / PageDown, the ends for Home / End. Every
//! selectable list of the TUI moves by these rules.

use crossterm::event::KeyCode;

/// Rows a page key moves.
pub const PAGE: usize = 10;

/// The index `key` selects in a list of `total` rows, from `current`; `None`
/// for a key that does not move a selection. An empty list keeps index 0.
pub fn moved_selection(key: KeyCode, current: usize, total: usize) -> Option<usize> {
    let last = total.saturating_sub(1);
    let index = match key {
        KeyCode::Up | KeyCode::Char('k') => current.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => current + 1,
        KeyCode::PageUp => current.saturating_sub(PAGE),
        KeyCode::PageDown => current + PAGE,
        KeyCode::Home => 0,
        KeyCode::End => last,
        _ => return None,
    };
    Some(index.min(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_move_within_the_list() {
        assert_eq!(moved_selection(KeyCode::Up, 0, 10), Some(0));
        assert_eq!(moved_selection(KeyCode::Down, 0, 10), Some(1));
        assert_eq!(moved_selection(KeyCode::Char('k'), 5, 10), Some(4));
        assert_eq!(moved_selection(KeyCode::Char('j'), 9, 10), Some(9));
        assert_eq!(moved_selection(KeyCode::Home, 5, 10), Some(0));
        assert_eq!(moved_selection(KeyCode::End, 5, 10), Some(9));
        assert_eq!(moved_selection(KeyCode::PageUp, 5, 10), Some(0));
        assert_eq!(moved_selection(KeyCode::PageDown, 5, 30), Some(15));
        assert_eq!(moved_selection(KeyCode::PageDown, 5, 10), Some(9));
    }

    #[test]
    fn an_empty_list_keeps_index_zero_and_other_keys_do_not_move() {
        for key in [KeyCode::Up, KeyCode::Down, KeyCode::End, KeyCode::PageDown] {
            assert_eq!(moved_selection(key, 0, 0), Some(0));
        }
        assert_eq!(moved_selection(KeyCode::Char('x'), 3, 10), None);
    }
}
