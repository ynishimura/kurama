//! Regression tests for jq completion and input behavior.

use super::*;
use crossterm::event::KeyModifiers;
use serde_json::json;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn input(text: &str) -> JqInputModel {
    JqInputModel::new(
        text.into(),
        Some(std::sync::Arc::new(json!({
            "content": [{"enabled":true,"name":"Rex"}], "count": 1
        }))),
        64,
    )
}

#[test]
fn jq_input_tab_inserts_one_candidate_and_enter_selects_from_many() {
    let mut model = input(".cou");
    assert_eq!(
        model.handle_key(key(KeyCode::Tab), &[]),
        InputAction::Changed
    );
    assert_eq!(model.as_str(), ".count");
    assert_eq!(model.panel, JqPanel::None);
    model = input(".co");
    assert_eq!(model.handle_key(key(KeyCode::Tab), &[]), InputAction::Stay);
    assert_eq!(model.panel.choice_count(), 2);
    model.handle_key(key(KeyCode::Tab), &[]);
    assert!(matches!(
        model.panel,
        JqPanel::Candidates { selected: 1, .. }
    ));
    model.handle_key(key(KeyCode::Up), &[]);
    assert!(matches!(
        model.panel,
        JqPanel::Candidates { selected: 0, .. }
    ));
    model.handle_key(key(KeyCode::Down), &[]);
    assert_eq!(
        model.handle_key(key(KeyCode::Enter), &[]),
        InputAction::Changed
    );
    assert_eq!(model.as_str(), ".count");
    assert_eq!(
        model.handle_key(key(KeyCode::Enter), &[]),
        InputAction::Apply(".count".into())
    );
}

#[test]
fn jq_input_esc_closes_the_open_list_then_the_modal() {
    let mut model = input(".");
    model.handle_key(key(KeyCode::F(1)), &[]);
    assert!(matches!(model.panel, JqPanel::Examples { .. }));
    // Tab replaces the examples with candidates instead of stacking them:
    // one list is open at a time, so one Esc closes what is on screen.
    model.handle_key(key(KeyCode::Tab), &[]);
    assert!(matches!(model.panel, JqPanel::Candidates { .. }));
    assert_eq!(model.handle_key(key(KeyCode::Esc), &[]), InputAction::Stay);
    assert_eq!(model.panel, JqPanel::None);
    assert_eq!(model.handle_key(key(KeyCode::Esc), &[]), InputAction::Close);
}

#[test]
fn jq_input_examples_insert_without_applying_and_history_restores_the_draft() {
    let mut model = input(".co");
    model.handle_key(key(KeyCode::F(1)), &[]);
    model.handle_key(key(KeyCode::Down), &[]);
    assert_eq!(
        model.handle_key(key(KeyCode::Enter), &[]),
        InputAction::Changed
    );
    assert_eq!(model.as_str(), ".content[]");
    assert_eq!(model.panel, JqPanel::None);
    let history = vec![".count".into(), ".content | length".into()];
    model.handle_key(key(KeyCode::Up), &history);
    assert_eq!(model.as_str(), ".content | length");
    model.handle_key(key(KeyCode::Up), &history);
    assert_eq!(model.as_str(), ".count");
    model.handle_key(key(KeyCode::Down), &history);
    model.handle_key(key(KeyCode::Down), &history);
    assert_eq!(model.as_str(), ".content[]");
}

#[test]
fn jq_input_preview_requires_a_small_json_body_and_only_changes_for_edits() {
    let mut model = input(".count");
    model.preview = Preview::Output(Ok(vec!["1".into()]));
    assert_eq!(model.handle_key(key(KeyCode::Left), &[]), InputAction::Stay);
    assert_eq!(model.preview, Preview::Output(Ok(vec!["1".into()])));
    model.handle_key(key(KeyCode::Delete), &[]);
    assert_eq!(model.preview, Preview::Pending);
    let body = json!({"id": 1});
    assert!(
        JqInputModel::new(
            "".into(),
            Some(std::sync::Arc::new(body.clone())),
            PREVIEW_LIMIT
        )
        .preview_filter()
        .is_some()
    );
    assert!(
        JqInputModel::new(
            "".into(),
            Some(std::sync::Arc::new(body)),
            PREVIEW_LIMIT + 1
        )
        .preview_filter()
        .is_none()
    );
    assert!(
        JqInputModel::new("".into(), None, 5)
            .preview_filter()
            .is_none()
    );
}

#[test]
fn jq_input_f1_and_tab_replace_each_other_so_one_list_is_open() {
    let mut model = input(".");
    model.handle_key(key(KeyCode::Tab), &[]);
    assert!(matches!(model.panel, JqPanel::Candidates { .. }));
    model.handle_key(key(KeyCode::F(1)), &[]);
    assert!(matches!(model.panel, JqPanel::Examples { .. }));
    model.handle_key(key(KeyCode::Tab), &[]);
    assert!(matches!(model.panel, JqPanel::Candidates { .. }));
    model.handle_key(key(KeyCode::F(1)), &[]);
    assert!(matches!(model.panel, JqPanel::Examples { .. }));
}

#[test]
fn jq_input_typing_closes_examples_before_enter_applies_the_filter() {
    let mut model = input(".co");
    model.handle_key(key(KeyCode::F(1)), &[]);
    assert!(matches!(model.panel, JqPanel::Examples { .. }));
    assert_eq!(
        model.handle_key(key(KeyCode::Char('u')), &[]),
        InputAction::Changed
    );
    assert_eq!(model.as_str(), ".cou");
    assert_eq!(model.panel, JqPanel::None);
    assert_eq!(
        model.handle_key(key(KeyCode::Enter), &[]),
        InputAction::Apply(".cou".into())
    );
}
