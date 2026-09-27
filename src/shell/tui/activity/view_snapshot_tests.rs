//! Render regression for the activity monitor: every fixture state at every verified terminal size is compared with `tests/tui_snapshots/activity_*` and checked against the monitor's UI contract.

use std::collections::BTreeSet;

use crate::shell::tui::activity::testing::{check_contract, fixtures, render_buffer};
use crate::shell::tui::testing::{
    SIZES, buffer_lines, buffer_text, cell, column_of, snapshot_dir, snapshot_mismatch,
};
use crate::shell::tui::theme;
use ratatui::style::{Color, Modifier};

#[test]
fn activity_monitor_matches_its_snapshots_and_contract_at_every_size() {
    let mut problems = Vec::new();
    let mut expected_files = BTreeSet::new();
    for (state, model) in fixtures::all() {
        for (width, height) in SIZES {
            let buffer = render_buffer(&model, width, height);
            let name = format!("activity_{state}_{width}x{height}");
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
        if file.starts_with("activity_")
            && file.ends_with(".txt")
            && !expected_files.contains(&file)
        {
            problems.push(format!(
                "stale snapshot {file}: no fixture renders it, delete it"
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

/// The contract bites: a selection drawn without its style is reported.
#[test]
fn activity_monitor_contract_reports_a_missing_selection_style() {
    let model = fixtures::loaded();
    let mut buffer = render_buffer(&model, 120, 40);
    let lines = buffer_lines(&buffer);
    let y = lines
        .iter()
        .position(|line| line.contains("▸ 08:05:05"))
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

/// A refused call reads `refused` and is drawn in the warning color, a
/// failed one in the danger color, a successful one in plain text.
#[test]
fn activity_monitor_colors_a_refused_call_apart_from_a_failed_one() {
    let model = fixtures::loaded();
    let buffer = render_buffer(&model, 120, 40);
    let lines = buffer_lines(&buffer);
    let color_of = |needle: &str| {
        let y = lines.iter().position(|line| line.contains(needle)).unwrap();
        let x = column_of(&lines[y], needle).unwrap();
        cell(&buffer, x, y).fg
    };
    assert_eq!(color_of("POST /v1/pets"), theme::WARNING);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("POST /v1/pets") && line.contains("refused"))
    );
    assert_eq!(color_of("GET /v1/pets/404"), theme::DANGER);
    assert_eq!(color_of("GET /v1/pets "), Color::Reset);
}

/// The details name the command that makes the call again, under the API's
/// base path.
#[test]
fn activity_monitor_shows_the_command_that_makes_the_call_again() {
    let text = buffer_text(&render_buffer(&fixtures::refused(), 160, 50));
    assert!(
        text.contains("Run again kurama api pets -X POST /pets"),
        "{text}"
    );
    assert!(text.contains("Error     AGENT_POLICY_DENIED"), "{text}");
}

/// An entry's text is drawn with its control characters escaped.
#[test]
fn activity_monitor_draws_an_escape_sequence_escaped() {
    let text = buffer_text(&render_buffer(&fixtures::long_text(), 160, 50));
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(text.contains("\\u{1b}[31m"), "{text}");
}

/// On a compact terminal the details replace the list while they have the
/// keys.
#[test]
fn activity_monitor_compact_shows_the_side_that_has_the_keys() {
    let text = buffer_text(&render_buffer(&fixtures::detail(), 80, 24));
    assert!(text.contains("┏ Call"), "{text}");
    assert!(!text.contains("┏ Calls"), "{text}");
    let text = buffer_text(&render_buffer(&fixtures::refused(), 80, 24));
    assert!(text.contains("┏ Calls"), "{text}");
}
