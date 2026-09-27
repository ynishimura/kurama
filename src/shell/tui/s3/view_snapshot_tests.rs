//! Render regression for the S3 explorer: every fixture state at every verified terminal size is compared with `tests/tui_snapshots/s3_*` and checked against the explorer's UI contract.

use std::collections::BTreeSet;

use crate::shell::tui::s3::testing::{check_contract, fixtures, render_buffer};
use crate::shell::tui::testing::{
    SIZES, buffer_lines, buffer_text, column_of, snapshot_dir, snapshot_mismatch,
};
use crate::shell::tui::theme;
use ratatui::style::{Color, Modifier};

#[test]
fn s3_explorer_matches_its_snapshots_and_contract_at_every_size() {
    let mut problems = Vec::new();
    let mut expected_files = BTreeSet::new();
    for (state, model) in fixtures::all() {
        for (width, height) in SIZES {
            let buffer = render_buffer(&model, width, height);
            let name = format!("s3_{state}_{width}x{height}");
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
        if file.starts_with("s3_") && file.ends_with(".txt") && !expected_files.contains(&file) {
            problems.push(format!(
                "stale snapshot {file}: no fixture renders it, delete it"
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

/// The contract bites: a selection drawn without its style is reported.
#[test]
fn s3_explorer_contract_reports_a_missing_selection_style() {
    let model = fixtures::selected();
    let mut buffer = render_buffer(&model, 120, 40);
    let lines = buffer_lines(&buffer);
    let y = lines
        .iter()
        .position(|line| line.contains("▸ orders.csv"))
        .unwrap();
    let x = column_of(&lines[y], theme::SELECTION_MARKER).unwrap();
    let marker = &mut buffer[(x as u16, y as u16)];
    marker.set_fg(Color::Reset).set_bg(Color::Reset);
    marker.modifier = Modifier::empty();
    let violations = check_contract(&model, &buffer);
    assert!(
        violations.iter().any(|v| v.contains("theme::selection")),
        "{violations:?}"
    );
}

/// The three ways of narrowing the list read differently on screen.
#[test]
fn s3_explorer_tells_the_filter_the_key_search_and_the_content_search_apart() {
    let title = |model| buffer_lines(&render_buffer(&model, 120, 40))[1].clone();
    assert!(title(fixtures::filtering()).contains("List 1/5+  filter /csv"));
    assert!(title(fixtures::stopped()).contains("Key search \"invoice\" (2)"));
    assert!(title(fixtures::content_search()).contains("Content search \"request-id-123\" (2)"));
}

/// Object text is untrusted: a control character is drawn escaped.
#[test]
fn s3_explorer_draws_an_escape_sequence_escaped() {
    let text = buffer_text(&render_buffer(&fixtures::content_search(), 160, 50));
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(text.contains("500\\u{1b}[31m"), "{text}");
}

/// On a compact terminal the preview replaces the list while it has the
/// keys, and the list comes back with Esc.
#[test]
fn s3_explorer_compact_shows_the_side_that_has_the_keys() {
    let model = fixtures::preview();
    let text = buffer_text(&render_buffer(&model, 80, 24));
    assert!(text.contains("┏ Preview"), "{text}");
    assert!(!text.contains("┏ List"), "{text}");
    let text = buffer_text(&render_buffer(&fixtures::selected(), 80, 24));
    assert!(text.contains("┏ List"), "{text}");
}
