//! Pure layout: terminal size in, rectangles out.
//!
//! Every screen computes its regions here, so the layout can be tested as
//! plain rectangles (no overlap, footer on the last row, modal inside the
//! viewport) without rendering anything. The breakpoints are the only place
//! that decides how a screen responds to the terminal width.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Below this width the home screen shows one pane.
const NORMAL_MIN_WIDTH: u16 = 100;
/// From this width on the detail pane gets a fixed, generous width.
const WIDE_MIN_WIDTH: u16 = 140;
/// Detail pane width on wide terminals: a full role ARN fits on one line.
const WIDE_DETAIL_WIDTH: u16 = 64;
/// Detail pane share of the width on normal terminals.
const NORMAL_DETAIL_PERCENT: u16 = 46;

/// Width class of the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breakpoint {
    /// `width < 100`: list only.
    Compact,
    /// `100 <= width < 140`: list and detail pane.
    Normal,
    /// `width >= 140`: list and a fixed-width detail pane.
    Wide,
}

impl Breakpoint {
    pub fn of(width: u16) -> Self {
        if width < NORMAL_MIN_WIDTH {
            Self::Compact
        } else if width < WIDE_MIN_WIDTH {
            Self::Normal
        } else {
            Self::Wide
        }
    }
}

/// Regions of the home screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeLayout {
    /// One row: title and mode badges.
    pub header: Rect,
    /// Profile table.
    pub list: Rect,
    /// Selected profile, absent on compact terminals.
    pub detail: Option<Rect>,
    /// One row of key hints, always the last row.
    pub footer: Rect,
}

pub fn home_layout(area: Rect) -> HomeLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);
    let (list, detail) = match Breakpoint::of(area.width) {
        Breakpoint::Compact => (rows[1], None),
        Breakpoint::Normal => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(0),
                    Constraint::Percentage(NORMAL_DETAIL_PERCENT),
                ])
                .split(rows[1]);
            (panes[0], Some(panes[1]))
        }
        Breakpoint::Wide => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(0), Constraint::Length(WIDE_DETAIL_WIDTH)])
                .split(rows[1]);
            (panes[0], Some(panes[1]))
        }
    };
    HomeLayout {
        header: rows[0],
        list,
        detail,
        footer: rows[2],
    }
}

/// The home screen with a row of tabs under the header: the tab row, and
/// the regions of `home_layout` in the rest.
pub fn home_tabs_layout(area: Rect) -> (Rect, HomeLayout) {
    let tab_row = area.height.min(1);
    let below = Rect {
        y: area.y + tab_row,
        height: area.height - tab_row,
        ..area
    };
    let layout = home_layout(below);
    let header = Rect {
        height: area.height.min(1),
        ..area
    };
    // The header of the region below is the tab row.
    (layout.header, HomeLayout { header, ..layout })
}

/// Width of the table list of the database explorer beside its result.
const DB_TABLES_WIDTH: u16 = 32;

/// Regions of the database explorer: the same rows as the home screen, with
/// a narrow table list and the rest for the result, which needs the width.
/// On a compact terminal the list takes the whole width and the screen shows
/// one of the two at a time.
pub fn db_layout(area: Rect) -> HomeLayout {
    let home = home_layout(area);
    let Some(detail) = home.detail else {
        return home;
    };
    let body = Rect {
        width: home.list.width + detail.width,
        ..home.list
    };
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(DB_TABLES_WIDTH), Constraint::Min(0)])
        .split(body);
    HomeLayout {
        list: panes[0],
        detail: Some(panes[1]),
        ..home
    }
}

/// A centered rectangle of at most `width` x `height`, never larger than
/// `area` and always inside it.
pub fn modal_area(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Row budgets leave room for the filter, captions, padding and modal borders.
pub const MAX_JQ_PREVIEW_ROWS: usize = 3;

pub struct JqModalRows {
    pub choices: usize,
    pub preview: usize,
}

/// Reserve at least half of a candidate row for its type and sample.
pub fn jq_choice_name_width(content_width: usize) -> usize {
    content_width / 2
}

/// How many rows of a response shape the detail pane may spend. The pane
/// cannot scroll, so everything after the shape -- the deprecated badge, the
/// inputs kurama cannot send, the next step -- is lost to whatever the shape
/// takes. `height` is the pane's outer height; the rest of the detail is
/// about a dozen rows, and one row is kept for the "N more lines" marker.
pub fn detail_shape_rows(height: u16) -> usize {
    usize::from(height.saturating_sub(18)).clamp(3, 12)
}

/// Matches the command palette lists at once: its query, captions, padding
/// and borders take eight rows, and the screen keeps a margin.
pub fn palette_rows(height: u16) -> usize {
    usize::from(height.saturating_sub(12)).clamp(1, 14)
}

/// Entries the history modal lists at once: the rest of the modal (borders,
/// padding, the caption) takes six rows, and the screen keeps a margin.
pub fn history_modal_rows(height: u16) -> usize {
    usize::from(height.saturating_sub(10)).clamp(1, 12)
}

pub fn jq_modal_rows(height: u16) -> JqModalRows {
    let available = usize::from(height.saturating_sub(11));
    let preview = (available / 3).min(MAX_JQ_PREVIEW_ROWS);
    JqModalRows {
        choices: available.saturating_sub(preview).min(8),
        preview,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_budget_shrinks_with_the_pane_and_never_fills_it() {
        // 30 rows is the shortest screen that shows the detail pane at all;
        // the rest of the detail plus the marker has to fit after the shape.
        assert_eq!(detail_shape_rows(28), 10);
        assert_eq!(detail_shape_rows(30), 12);
        assert_eq!(detail_shape_rows(50), 12, "a tall pane still stops at 12");
        assert_eq!(detail_shape_rows(21), 3, "never less than a useful glimpse");
        assert_eq!(detail_shape_rows(0), 3);
    }

    fn contains(outer: Rect, inner: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
    }

    #[test]
    fn breakpoints_split_at_100_and_140_columns() {
        assert_eq!(Breakpoint::of(80), Breakpoint::Compact);
        assert_eq!(Breakpoint::of(99), Breakpoint::Compact);
        assert_eq!(Breakpoint::of(100), Breakpoint::Normal);
        assert_eq!(Breakpoint::of(139), Breakpoint::Normal);
        assert_eq!(Breakpoint::of(140), Breakpoint::Wide);
        assert_eq!(Breakpoint::of(160), Breakpoint::Wide);
    }

    #[test]
    fn home_regions_never_overlap_and_the_footer_is_the_last_row() {
        for (width, height) in [(80, 24), (100, 30), (120, 40), (160, 50), (20, 5)] {
            let area = Rect::new(0, 0, width, height);
            let layout = home_layout(area);
            let mut regions = vec![layout.header, layout.list, layout.footer];
            regions.extend(layout.detail);
            for region in &regions {
                assert!(contains(area, *region), "{width}x{height}: {region:?}");
            }
            for (i, a) in regions.iter().enumerate() {
                for b in &regions[i + 1..] {
                    assert!(!overlap(*a, *b), "{width}x{height}: {a:?} overlaps {b:?}");
                }
            }
            assert_eq!(layout.header.y, 0);
            assert_eq!(layout.header.height, 1);
            assert_eq!(layout.footer.height, 1);
            assert_eq!(layout.footer.y, height - 1, "{width}x{height}");
            assert_eq!(
                layout.list.width + layout.detail.map_or(0, |d| d.width),
                width
            );
        }
    }

    #[test]
    fn the_tab_row_sits_under_the_header_and_above_the_panes() {
        for (width, height) in [(80, 24), (160, 50), (20, 5), (20, 1), (20, 0)] {
            let area = Rect::new(0, 0, width, height);
            let (tabs, layout) = home_tabs_layout(area);
            let mut regions = vec![layout.header, tabs, layout.list, layout.footer];
            regions.extend(layout.detail);
            for region in &regions {
                assert!(contains(area, *region), "{width}x{height}: {region:?}");
            }
            for (i, a) in regions.iter().enumerate() {
                for b in &regions[i + 1..] {
                    assert!(!overlap(*a, *b), "{width}x{height}: {a:?} overlaps {b:?}");
                }
            }
            if height >= 5 {
                assert_eq!((layout.header.y, tabs.y, layout.list.y), (0, 1, 2));
                assert_eq!(tabs.height, 1);
                assert_eq!(layout.footer.y, height - 1);
            }
        }
    }

    #[test]
    fn compact_has_no_detail_pane_and_wide_has_a_fixed_one() {
        assert!(home_layout(Rect::new(0, 0, 80, 24)).detail.is_none());
        let normal = home_layout(Rect::new(0, 0, 120, 40));
        assert_eq!(normal.detail.map(|d| d.width), Some(55));
        let wide = home_layout(Rect::new(0, 0, 160, 50));
        assert_eq!(wide.detail.map(|d| d.width), Some(WIDE_DETAIL_WIDTH));
        assert_eq!(wide.list.width, 160 - WIDE_DETAIL_WIDTH);
    }

    #[test]
    fn db_layout_gives_the_result_the_width_and_the_list_the_whole_compact_screen() {
        for (width, height) in [(100, 30), (120, 40), (160, 50)] {
            let area = Rect::new(0, 0, width, height);
            let layout = db_layout(area);
            let detail = layout.detail.unwrap();
            assert_eq!(layout.list.width, DB_TABLES_WIDTH);
            assert_eq!(layout.list.width + detail.width, width);
            assert_eq!(detail.x, DB_TABLES_WIDTH);
            assert_eq!(layout.footer.y, height - 1);
        }
        let compact = db_layout(Rect::new(0, 0, 80, 24));
        assert!(compact.detail.is_none());
        assert_eq!(compact.list.width, 80);
    }

    #[test]
    fn modal_is_centered_and_clamped_to_the_viewport() {
        let area = Rect::new(0, 0, 120, 40);
        let modal = modal_area(area, 60, 10);
        assert_eq!(modal, Rect::new(30, 15, 60, 10));

        let small = Rect::new(2, 3, 40, 8);
        let clamped = modal_area(small, 60, 10);
        assert_eq!(clamped, small);
        assert!(contains(small, clamped));
    }
}
