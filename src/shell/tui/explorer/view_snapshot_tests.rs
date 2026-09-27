//! Render regression for the explorer: every fixture state at every
//! verified terminal size is compared with `tests/tui_snapshots/explorer_*`
//! and checked against the explorer's UI contract.

use std::collections::BTreeSet;

use crate::domain::types::api_spec::{Property, Schema};
use crate::shell::tui::explorer::model::Screen;
use crate::shell::tui::explorer::testing::{check_contract, fixtures, render_buffer};
use crate::shell::tui::testing::{
    SIZES, buffer_lines, buffer_text, snapshot_dir, snapshot_mismatch,
};

#[test]
fn explorer_matches_its_snapshots_and_contract_at_every_size() {
    let mut problems = Vec::new();
    let mut expected_files = BTreeSet::new();
    for (state, model) in fixtures::all() {
        for (width, height) in SIZES {
            let buffer = render_buffer(&model, width, height);
            let name = format!("explorer_{state}_{width}x{height}");
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
        if !file.ends_with(".txt") {
            continue;
        }
        if file.starts_with("explorer_") && !expected_files.contains(&file) {
            problems.push(format!(
                "stale snapshot {file}: no fixture renders it, delete it"
            ));
        } else if !["explorer_", "home_", "db_", "s3_", "activity_"]
            .iter()
            .any(|prefix| file.starts_with(prefix))
        {
            problems.push(format!("snapshot {file} belongs to no render test"));
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

#[test]
fn the_list_has_a_detail_pane_from_100_columns_and_the_form_takes_the_width() {
    let list = fixtures::selected();
    let compact = buffer_text(&render_buffer(&list, 80, 24));
    assert!(!compact.contains("┏ Operation "), "{compact}");
    assert!(compact.contains("▸ POST   pets/create"), "{compact}");
    let normal = buffer_lines(&render_buffer(&list, 120, 40));
    assert!(
        normal[1].contains("┏ Operations (5) ") && normal[1].contains("┏ Operation "),
        "{}",
        normal[1]
    );
    assert!(
        normal
            .iter()
            .any(|line| line.contains("Scopes    read:pets, write:pets")),
        "{normal:#?}"
    );

    let form = fixtures::form();
    assert!(matches!(form.screen, Screen::Form(_)));
    let wide = buffer_lines(&render_buffer(&form, 160, 50));
    assert!(
        wide[1].starts_with("┏ pets/get  GET /pets/{petId} ") && wide[1].ends_with('┓'),
        "{}",
        wide[1]
    );
    assert!(!wide[1].contains("┏ Operations"), "{}", wide[1]);
}

#[test]
fn response_shape_includes_optional_properties_in_the_detail_pane() {
    for (width, height) in [(100, 30), (120, 40), (160, 50)] {
        let rendered = buffer_text(&render_buffer(&fixtures::response_shape(), width, height));
        assert!(
            rendered.contains("Response  200 application/json"),
            "{rendered}"
        );
        assert!(rendered.contains("name"), "{rendered}");
        assert!(rendered.contains("integer"), "{rendered}");
    }
}

#[test]
fn detail_shape_is_bounded_after_warnings_and_points_to_describe() {
    let mut operation = fixtures::response_shape()
        .selected_operation()
        .unwrap()
        .clone();
    operation.deprecated = true;
    operation.unsupported = vec!["cookie parameter 'session'".into()];
    let response = operation.response.as_mut().unwrap();
    response.schema.type_name = "object".into();
    response.schema.properties = (0..20)
        .map(|index| Property {
            name: format!("field{index}"),
            required: false,
            schema: Schema {
                type_name: "string".into(),
                ..Schema::default()
            },
        })
        .collect();

    let entries = super::detail_entries(Some(&operation), 12);
    let note = entries
        .iter()
        .position(|(label, _)| label == "Note")
        .unwrap();
    let shape = entries
        .iter()
        .position(|(label, _)| label == "Shape")
        .unwrap();
    assert!(note < shape);
    assert!(entries.iter().any(|(_, span)| {
        span.content
            .contains("more lines; --describe shows the whole shape")
    }));
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
fn editors_draw_the_cursor_between_characters_and_keep_long_input_in_view() {
    for (width, height) in SIZES {
        for (model, expected) in [
            (fixtures::form_middle(), "ab日▏本語cd"),
            (fixtures::jq_input_middle(), ".t▏ags[]"),
        ] {
            let text = buffer_text(&render_buffer(&model, width, height));
            assert!(text.contains(expected), "{width}x{height}: {text}");
        }
        let text = buffer_text(&render_buffer(&fixtures::jq_input_long(), width, height));
        let row = text.lines().find(|line| line.contains("Filter")).unwrap();
        assert!(
            row.contains('▏') && row.contains('…'),
            "{width}x{height}: {row}"
        );
    }
}

#[test]
fn jq_candidates_show_the_distinguishing_key_and_type_after_a_deep_path() {
    for (width, height) in SIZES {
        let text = buffer_text(&render_buffer(
            &fixtures::jq_input_completing(),
            width,
            height,
        ));
        for (name, kind) in [("accountId", "integer"), ("accountName", "string")] {
            assert!(
                text.lines()
                    .any(|line| line.contains(name) && line.contains(kind)),
                "{width}x{height}: {text}"
            );
        }
        assert!(
            text.lines()
                .any(|line| line.contains("enabled") && line.contains("boolean")),
            "{text}"
        );
    }
}
