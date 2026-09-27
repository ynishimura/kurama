//! Render regression for the database explorer: every fixture state at
//! every verified terminal size is compared with `tests/tui_snapshots/db_*`
//! and checked against the explorer's UI contract.

use std::collections::BTreeSet;

use crate::shell::tui::database::model::Focus;
use crate::shell::tui::database::testing::{check_contract, fixtures, render_buffer};
use crate::shell::tui::testing::{
    SIZES, buffer_lines, buffer_text, column_of, snapshot_dir, snapshot_mismatch,
};
use crate::shell::tui::theme;
use ratatui::style::{Color, Modifier};

#[test]
fn db_explorer_matches_its_snapshots_and_contract_at_every_size() {
    let mut problems = Vec::new();
    let mut expected_files = BTreeSet::new();
    for (state, model) in fixtures::all() {
        for (width, height) in SIZES {
            let buffer = render_buffer(&model, width, height);
            let name = format!("db_{state}_{width}x{height}");
            for violation in check_contract(&model, &buffer) {
                problems.push(format!("{name}: {violation}"));
            }
            if let Some(mismatch) = snapshot_mismatch(&name, &buffer_text(&buffer)) {
                problems.push(mismatch);
            }
            expected_files.insert(format!("{name}.txt"));
        }
    }
    for entry in std::fs::read_dir(snapshot_dir()).unwrap().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with("db_") && file.ends_with(".txt") && !expected_files.contains(&file) {
            problems.push(format!(
                "stale snapshot {file}: no fixture renders it, delete it"
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

/// The contract bites: a selection drawn without its style, and a header
/// without the badge, are reported.
#[test]
fn db_explorer_contract_reports_a_missing_selection_style_and_badge() {
    let model = fixtures::loaded();
    let mut buffer = render_buffer(&model, 120, 40);
    let lines = buffer_lines(&buffer);
    let y = lines
        .iter()
        .position(|line| line.contains("▸ aws_accounts"))
        .unwrap();
    let x = column_of(&lines[y], theme::SELECTION_MARKER).unwrap();
    let marker = &mut buffer[(x as u16, y as u16)];
    marker.set_fg(Color::Reset).set_bg(Color::Reset);
    marker.modifier = Modifier::empty();
    let violations = check_contract(&model, &buffer);
    assert!(
        violations.iter().any(|v| v.contains("selection style")),
        "{violations:?}"
    );
    let mut buffer = render_buffer(&model, 120, 40);
    for x in 0..120 {
        buffer[(x, 0)].set_symbol(" ");
    }
    let violations = check_contract(&model, &buffer);
    assert!(
        violations.iter().any(|v| v.contains("READ ONLY")),
        "{violations:?}"
    );
}

#[test]
fn db_explorer_compact_shows_the_side_that_has_the_keys() {
    let mut model = fixtures::preview();
    let detail = buffer_text(&render_buffer(&model, 80, 24));
    assert!(detail.contains("[Preview]"), "{detail}");
    assert!(!detail.contains("Tables ("), "{detail}");
    model.focus = Focus::Tables;
    let tables = buffer_text(&render_buffer(&model, 80, 24));
    assert!(tables.contains("Tables (4+)"), "{tables}");
    assert!(!tables.contains("[Preview]"), "{tables}");
    let normal = buffer_text(&render_buffer(&model, 120, 40));
    assert!(normal.contains("Tables (4+)") && normal.contains("[Preview]"));
}

/// NULL, an empty string, a cell cut to fit and a result cut at a bound are
/// four different things on the screen.
#[test]
fn db_explorer_tells_null_empty_a_cut_cell_and_a_cut_result_apart() {
    for (width, height) in [(120, 40), (160, 50)] {
        let text = buffer_text(&render_buffer(&fixtures::preview(), width, height));
        assert!(text.contains('∅'), "{text}");
        assert!(
            text.contains("truncated at max_rows, more rows exist"),
            "{text}"
        );
        assert!(text.contains("… cell cut to fit (Enter)"), "{text}");
        assert!(text.contains("tab\\there"), "{text}");
        assert!(text.contains("日本語の名前"), "{text}");
    }
    // The selected column is the fourth; from 160 columns the first fits too.
    let wide = buffer_text(&render_buffer(&fixtures::preview(), 160, 50));
    assert!(wide.contains("9223372036854775807"), "{wide}");
    let text = buffer_text(&render_buffer(&fixtures::result(), 120, 40));
    assert!(!text.contains("truncated"), "{text}");
    assert!(!text.contains("cut to fit"), "{text}");
}

#[test]
fn db_explorer_tiny_terminals_render_without_panicking() {
    for (state, model) in fixtures::all() {
        for (width, height) in [(1, 1), (10, 3), (40, 6), (79, 23)] {
            let buffer = render_buffer(&model, width, height);
            assert_eq!(buffer.area.width, width, "{state} {width}x{height}");
        }
    }
}

/// The last visible row sits above the status line, never under it, and a
/// filter title fits its panel however long the filter is.
#[test]
fn db_explorer_keeps_the_last_row_above_the_status_line_and_the_filter_in_its_title() {
    let mut model = fixtures::result();
    if let Some(grid) = &mut model.result {
        grid.result = fixtures::rows(
            &["n"],
            (0..50)
                .map(|n| vec![serde_json::json!(format!("row{n}"))])
                .collect(),
        );
        grid.row = 49;
    }
    for (width, height) in SIZES {
        let lines = buffer_lines(&render_buffer(&model, width, height));
        let marked = lines
            .iter()
            .position(|l| l.contains("▸ row49"))
            .unwrap_or_else(|| {
                panic!(
                    "{width}x{height}: the selected row is hidden\n{}",
                    lines.join("\n")
                )
            });
        assert!(
            lines[marked + 1].contains("50 rows"),
            "{width}x{height}: the status line follows the last row\n{}",
            lines.join("\n")
        );
    }
    let mut model = fixtures::loaded();
    model.searching = true;
    model.search = "a_very_long_filter_that_goes_past_the_panel_width".into();
    model.apply_search();
    let text = buffer_text(&render_buffer(&model, 120, 40));
    let title = text.lines().nth(1).unwrap();
    // The filter keeps its end, where the cursor is, inside the list panel.
    let list_title = &title[..title.find("┓┏").expect("two panels")];
    assert!(
        list_title.starts_with("┏ Tables 0/4+  /") && list_title.contains("width_"),
        "{title}"
    );
}
