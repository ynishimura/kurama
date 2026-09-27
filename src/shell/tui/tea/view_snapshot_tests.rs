//! Render regression for the home screen: every fixture state at every
//! verified terminal size is compared with `tests/tui_snapshots/` and checked
//! against the UI contract (`testing::check_contract`).
//!
//! A snapshot difference is reported with the first changed lines and the
//! `.txt.new` file to read; accept it with `KURAMA_UPDATE_SNAPSHOTS=1` only
//! after the new rendering has been reviewed.

use std::collections::BTreeSet;

use crate::shell::tui::testing::{
    SIZES, buffer_lines, buffer_text, check_contract, fixtures, render_buffer, snapshot_dir,
    snapshot_mismatch,
};

#[test]
fn home_screen_matches_its_snapshots_and_contract_at_every_size() {
    let mut problems = Vec::new();
    let mut expected_files = BTreeSet::new();
    for (state, model) in fixtures::all() {
        for (width, height) in SIZES {
            let buffer = render_buffer(&model, width, height);
            let name = format!("home_{state}_{width}x{height}");
            for violation in check_contract(&model, &buffer) {
                problems.push(format!("{name}: {violation}"));
            }
            if let Some(mismatch) = snapshot_mismatch(&name, &buffer_text(&buffer)) {
                problems.push(mismatch);
            }
            expected_files.insert(format!("{name}.txt"));
        }
    }
    // A renamed or removed fixture leaves its snapshots behind; they would
    // show screens no code renders.
    for entry in std::fs::read_dir(snapshot_dir()).unwrap().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with("home_") && file.ends_with(".txt") && !expected_files.contains(&file) {
            problems.push(format!(
                "stale snapshot {file}: no fixture renders it, delete it"
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

#[test]
fn compact_terminals_show_one_pane_and_wider_ones_add_the_detail_pane() {
    let model = fixtures::selected();
    let compact = buffer_text(&render_buffer(&model, 80, 24));
    assert!(!compact.contains("┏ Details "), "{compact}");
    assert!(compact.contains("▸ * aws  ops-mfa"), "{compact}");

    let normal = buffer_lines(&render_buffer(&model, 120, 40));
    let top = &normal[2];
    assert!(
        top.contains("┏ Profiles (4) ") && top.contains("┏ Details "),
        "{top}"
    );
    assert!(
        normal
            .iter()
            .any(|line| line.contains("Role    arn:aws:iam::123456789012:role/ops-mfa")),
        "{normal:#?}"
    );
}

#[test]
fn tiny_terminals_render_without_panicking() {
    for (state, model) in fixtures::all() {
        for (width, height) in [(1, 1), (10, 3), (40, 6), (79, 23)] {
            let buffer = render_buffer(&model, width, height);
            assert_eq!(buffer.area.width, width, "{state} {width}x{height}");
        }
    }
}

#[test]
fn the_header_shows_the_selected_session_time_left_in_the_color_of_its_expiry() {
    use crate::domain::functions::profile_status::Expiry;
    let cases = [
        (fixtures::selected(), "MFA 11h 59m", Some(Expiry::Later)),
        (fixtures::session_soon(), "MFA 14m", Some(Expiry::Soon)),
        (
            fixtures::session_expired(),
            "MFA expired",
            Some(Expiry::Expired),
        ),
        // `default` has no MFA device: nothing to count down.
        (fixtures::loaded(), "MFA", None),
    ];
    for (model, text, expiry) in cases {
        let buffer = render_buffer(&model, 120, 40);
        let header = &buffer_lines(&buffer)[0];
        match expiry {
            Some(expiry) => {
                let x = header
                    .find(text)
                    .unwrap_or_else(|| panic!("{text:?} in {header:?}"));
                assert_eq!(
                    buffer[(
                        unicode_width::UnicodeWidthStr::width(&header[..x]) as u16,
                        0
                    )]
                        .fg,
                    crate::shell::tui::theme::expiry_style(expiry).fg.unwrap(),
                    "{text}"
                );
            }
            None => assert!(!header.contains(text), "{header:?}"),
        }
    }
}
