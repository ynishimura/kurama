//! Tests for the explorer state transitions and their effects.

use super::*;
use crate::domain::types::ApiHeaders;
use crate::shell::tui::components::LineInput;
use crate::shell::tui::explorer::model::status_text;
use crate::shell::tui::explorer::testing::fixtures::{api, spec};
use crossterm::event::{KeyEventKind, KeyEventState};

fn key(code: KeyCode) -> ExplorerMessage {
    key_with(code, KeyModifiers::NONE)
}

fn key_with(code: KeyCode, modifiers: KeyModifiers) -> ExplorerMessage {
    ExplorerMessage::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

fn loaded() -> ExplorerModel {
    let (model, _) = update(
        ExplorerModel::new(api()),
        ExplorerMessage::SpecLoaded(Ok(spec())),
    );
    model
}

fn press(model: ExplorerModel, code: KeyCode) -> (ExplorerModel, Vec<ExplorerEffect>) {
    update(model, key(code))
}

fn type_text(mut model: ExplorerModel, text: &str) -> ExplorerModel {
    for c in text.chars() {
        model = press(model, KeyCode::Char(c)).0;
    }
    model
}

fn selected_id(model: &ExplorerModel) -> &str {
    model
        .selected_operation()
        .map(|op| op.id.as_str())
        .unwrap_or("")
}

#[test]
fn the_loaded_description_lists_every_operation_and_a_failure_is_a_modal() {
    let model = loaded();
    assert!(!model.loading);
    assert_eq!(model.filtered.len(), 5);
    assert_eq!(selected_id(&model), "pets/list");
    assert!(model.notice.is_none());

    let (failed, _) = update(
        ExplorerModel::new(api()),
        ExplorerMessage::SpecLoaded(Err("API_SPEC_UNAVAILABLE: refused".into())),
    );
    assert!(!failed.loading);
    assert_eq!(
        failed.modal,
        Some(ExplorerModal::Error("API_SPEC_UNAVAILABLE: refused".into()))
    );
    let (back, _) = press(failed, KeyCode::Enter);
    assert!(back.modal.is_none());
    assert!(back.filtered.is_empty());
}

#[test]
fn navigation_search_and_quit_on_the_list() {
    let model = loaded();
    let (model, _) = press(model, KeyCode::Down);
    assert_eq!(selected_id(&model), "pets/create");
    let (model, _) = press(model, KeyCode::End);
    assert_eq!(selected_id(&model), "owners/list-pets");
    let (model, _) = press(model, KeyCode::PageUp);
    assert_eq!(selected_id(&model), "pets/list");
    let (model, _) = press(model, KeyCode::Char('j'));
    let (model, _) = press(model, KeyCode::Char('k'));
    assert_eq!(selected_id(&model), "pets/list");

    let (model, _) = press(model, KeyCode::Char('/'));
    assert!(model.searching);
    let model = type_text(model, "owner");
    assert_eq!(model.filtered.len(), 1);
    assert_eq!(selected_id(&model), "owners/list-pets");
    let (model, _) = press(model, KeyCode::Enter);
    assert!(!model.searching);
    assert_eq!(model.search_query.as_str(), "owner");
    let (model, _) = press(model, KeyCode::Esc);
    assert!(model.search_query.is_empty());
    assert_eq!(model.filtered.len(), 5);

    // While searching, the arrows still move the selection; `j` would type.
    let (model, _) = press(model, KeyCode::Char('/'));
    let (model, _) = press(model, KeyCode::Down);
    assert!(model.searching);
    assert_eq!(model.selected, 1);
    let (model, _) = press(model, KeyCode::Up);
    assert_eq!(model.selected, 0);
    let (model, _) = press(model, KeyCode::Esc);

    let (model, _) = press(model, KeyCode::Char('/'));
    let model = type_text(model, "zzz");
    assert!(model.filtered.is_empty());
    let (model, _) = press(model, KeyCode::Backspace);
    let (model, _) = press(model, KeyCode::Esc);
    assert!(!model.searching && model.search_query.is_empty());

    let (model, effects) = press(model, KeyCode::Char('q'));
    assert!(model.should_exit);
    assert_eq!(effects, [ExplorerEffect::Exit]);
    let (model, effects) = update(
        loaded(),
        key_with(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    assert!(model.should_exit);
    assert_eq!(effects, [ExplorerEffect::Exit]);
}

#[test]
fn copy_and_docs_on_the_list_are_effects() {
    let (model, effects) = press(loaded(), KeyCode::Char('c'));
    assert_eq!(
        effects,
        [ExplorerEffect::CopyToClipboard {
            text: "kurama api pets pets/list".into()
        }]
    );
    let (model, effects) = press(model, KeyCode::Char('o'));
    assert!(effects.is_empty());
    assert_eq!(
        model.notice.as_deref(),
        Some("the operation has no documentation link")
    );
    let (model, _) = press(model, KeyCode::Down);
    let (_, effects) = press(model, KeyCode::Char('o'));
    assert_eq!(
        effects,
        [ExplorerEffect::OpenBrowser {
            url: "https://docs.example.com/pets#create".into()
        }]
    );
    let (model, _) = update(
        loaded(),
        ExplorerMessage::Copied(Err("pbcopy: not found".into())),
    );
    assert_eq!(
        model.notice.as_deref(),
        Some("copy failed: pbcopy: not found")
    );
}

/// The API's `headers` go with a request the explorer sends, over the
/// default `Accept` and over a header parameter of the same name.
#[test]
fn a_sent_request_carries_the_configured_headers() {
    let mut summary = api();
    summary.headers = ApiHeaders::new(
        [
            (
                "Accept".to_string(),
                "application/vnd.pets+json".to_string(),
            ),
            ("X-Trace".to_string(), "configured".to_string()),
        ]
        .into(),
    )
    .unwrap();
    let (model, _) = update(
        ExplorerModel::new(summary),
        ExplorerMessage::SpecLoaded(Ok(spec())),
    );
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Enter);
    let model = type_text(model, "p1");
    let (model, _) = press(model, KeyCode::Tab);
    let model = type_text(model, "typed");
    let (_, effects) = press(model, KeyCode::Enter);
    let ExplorerEffect::SendRequest(request) = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(request.header("accept"), Some("application/vnd.pets+json"));
    assert_eq!(request.header("x-trace"), Some("configured"));
    assert_eq!(
        request
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("x-trace"))
            .count(),
        1
    );
}

#[test]
fn the_form_takes_values_validates_and_sends_the_request() {
    let model = loaded();
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Down);
    assert_eq!(selected_id(&model), "pets/get");
    let (model, _) = press(model, KeyCode::Enter);
    let Screen::Form(form) = &model.screen else {
        panic!("expected the form, got {:?}", model.screen);
    };
    assert_eq!(
        form.values
            .iter()
            .map(LineInput::as_str)
            .collect::<Vec<_>>(),
        ["", ""]
    );
    assert_eq!(form.body, None);
    assert_eq!(form.focus, 0);

    // Send without the required petId: refused, nothing sent.
    let (model, effects) = press(model, KeyCode::Enter);
    assert!(effects.is_empty());
    let Screen::Form(form) = &model.screen else {
        panic!()
    };
    assert_eq!(
        form.error.as_deref(),
        Some("operation pets/get needs -P petId=<value>")
    );

    let model = type_text(model, "p 1");
    let (model, _) = press(model, KeyCode::Tab);
    let model = type_text(model, "t1");
    let (model, _) = press(model, KeyCode::BackTab);
    let Screen::Form(form) = &model.screen else {
        panic!()
    };
    assert_eq!(
        form.values
            .iter()
            .map(LineInput::as_str)
            .collect::<Vec<_>>(),
        ["p 1", "t1"]
    );
    assert_eq!(form.focus, 0);
    assert!(form.error.is_none());

    let (model, effects) = press(model, KeyCode::Enter);
    let ExplorerEffect::SendRequest(request) = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(request.method, "GET");
    assert_eq!(request.url, "https://petstore.example.com/v1/pets/p%201");
    assert_eq!(request.header("X-Trace"), Some("t1"));
    assert_eq!(request.header("accept"), Some("application/json"));
    assert_eq!(
        model.modal,
        Some(ExplorerModal::Processing(
            "GET https://petstore.example.com/v1/pets/p%201".into()
        ))
    );

    let (model, _) = update(
        model,
        ExplorerMessage::ResponseReceived(Ok(ResponseInfo {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: br#"{"path":"/v1/pets/p%201","login":"octocat"}"#.to_vec(),
            elapsed_ms: 12,
        })),
    );
    assert!(model.modal.is_none());
    let Screen::Result(result) = &model.screen else {
        panic!("expected the result, got {:?}", model.screen);
    };
    assert_eq!(result.status, 200);
    assert_eq!(
        result.params,
        [
            ("petId".to_string(), "p 1".to_string()),
            ("X-Trace".to_string(), "t1".to_string())
        ]
    );
    assert_eq!(
        result.body,
        "{\n  \"path\": \"/v1/pets/p%201\",\n  \"login\": \"octocat\"\n}"
    );
    assert_eq!(
        result_command("pets", model.operation(2).unwrap(), result),
        "kurama api pets pets/get -P 'petId=p 1' -P X-Trace=t1"
    );
    assert_eq!(status_text(200), "HTTP 200 OK");
    assert_eq!(status_text(599), "HTTP 599");
}

#[test]
fn control_e_and_its_hint_follow_the_focused_form_row() {
    let mut spec = (*spec()).clone();
    spec.operations[2].request_body = spec.operations[1].request_body.clone();
    let (model, _) = update(
        ExplorerModel::new(api()),
        ExplorerMessage::SpecLoaded(Ok(Arc::new(spec))),
    );
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Enter);
    let model = type_text(model, "42");
    let (model, _) = press(model, KeyCode::Home);
    assert!(!super::super::view::hints(&model).contains(&("^E", "body")));
    let (model, effects) = update(model, key_with(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert!(effects.is_empty());
    let Screen::Form(form) = &model.screen else {
        panic!()
    };
    assert_eq!(form.values[0].cursor(), 2);
    let (model, _) = press(model, KeyCode::Tab);
    let (model, _) = press(model, KeyCode::Tab);
    assert!(super::super::view::hints(&model).contains(&("^E", "body")));
    let (_, effects) = update(model, key_with(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert!(matches!(
        effects.as_slice(),
        [ExplorerEffect::EditBody { .. }]
    ));
}

#[test]
fn jq_input_edits_the_filter_in_the_middle_before_applying() {
    let model = super::super::testing::fixtures::result();
    let (model, _) = press(model, KeyCode::Char('j'));
    let model = type_text(model, ".tagz[]");
    let (model, _) = press(model, KeyCode::Home);
    let (model, _) = press(model, KeyCode::Right);
    let (model, _) = press(model, KeyCode::Right);
    let (model, _) = press(model, KeyCode::Right);
    let (model, _) = press(model, KeyCode::Right);
    let (model, _) = press(model, KeyCode::Delete);
    let model = type_text(model, "s");
    let (_, effects) = press(model, KeyCode::Enter);
    assert!(
        matches!(&effects[..], [ExplorerEffect::ApplyJq { filter, .. }] if filter == ".tags[]")
    );
}

#[test]
fn the_result_scrolls_toggles_headers_filters_with_jq_and_goes_back() {
    let model = loaded();
    let (model, _) = press(model, KeyCode::Enter);
    let (model, _) = press(model, KeyCode::Enter);
    let body: Vec<String> = (1..=30).map(|i| format!("\"line{i}\"")).collect();
    let (model, _) = update(
        model,
        ExplorerMessage::ResponseReceived(Ok(ResponseInfo {
            status: 404,
            headers: vec![("x-a".into(), "1".into())],
            body: format!("[{}]", body.join(",")).into_bytes(),
            elapsed_ms: 3,
        })),
    );
    let (model, _) = press(model, KeyCode::PageDown);
    let (model, _) = press(model, KeyCode::Down);
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert_eq!(result.scroll, 11);
    let (model, _) = press(model, KeyCode::End);
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert_eq!(result.scroll, result_lines(result).len() - 1);
    let (model, _) = press(model, KeyCode::Char('h'));
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert!(result.show_headers && result.scroll == 0);
    assert_eq!(result_lines(result)[0], "x-a: 1");
    assert_eq!(result_lines(result)[1], "");

    let (model, _) = press(model, KeyCode::Char('j'));
    assert!(
        matches!(&model.modal, Some(ExplorerModal::JqInput(input)) if input.as_str().is_empty())
    );
    let model = type_text(model, ".[0]");
    let (model, effects) = press(model, KeyCode::Enter);
    assert!(model.modal.is_none());
    assert!(
        matches!(&effects[0], ExplorerEffect::ApplyJq { filter, body } if filter == ".[0]" && body.starts_with(b"[")),
        "{effects:?}"
    );
    let running = model.clone();
    let (model, _) = update(model, ExplorerMessage::JqApplied(Ok(vec!["line1".into()])));
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert_eq!(result.jq.as_deref(), Some(".[0]"));
    assert_eq!(result_lines(result)[2], "line1");
    let (failed, _) = update(
        running,
        ExplorerMessage::JqApplied(Err("unexpected token".into())),
    );
    let Screen::Result(result) = &failed.screen else {
        panic!()
    };
    assert_eq!(result_lines(result)[2], "jq: unexpected token");
    let (_, effects) = press(model.clone(), KeyCode::Char('c'));
    assert_eq!(
        effects,
        [ExplorerEffect::CopyToClipboard {
            text: "kurama api pets pets/list --jq '.[0]'".into()
        }]
    );

    // An empty filter clears it; Esc keeps the current one.
    let (model, _) = press(model, KeyCode::Char('j'));
    let (model, _) = press(model, KeyCode::Esc);
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert_eq!(result.jq.as_deref(), Some(".[0]"));
    let (model, _) = press(model, KeyCode::Char('j'));
    let (model, _) = press(model, KeyCode::Backspace);
    let (model, _) = press(model, KeyCode::Backspace);
    let (model, _) = press(model, KeyCode::Backspace);
    let (model, _) = press(model, KeyCode::Backspace);
    let (model, effects) = press(model, KeyCode::Enter);
    assert!(effects.is_empty());
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert!(result.jq.is_none() && result.jq_output.is_none());

    let (model, _) = press(model, KeyCode::Esc);
    assert!(matches!(model.screen, Screen::Form(_)));
    let (model, _) = press(model, KeyCode::Esc);
    assert_eq!(model.screen, Screen::List);
}

#[test]
fn the_body_is_edited_and_copied_from_the_form() {
    let model = loaded();
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Enter);
    let Screen::Form(form) = &model.screen else {
        panic!()
    };
    assert_eq!(
        form.body.as_deref(),
        Some("{\n  \"name\": \"\"\n}"),
        "the skeleton holds the required properties only"
    );
    assert_eq!(form.values.len(), 0);
    let (model, effects) = update(model, key_with(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert!(
        matches!(&effects[0], ExplorerEffect::EditBody { text, suffix: "json" } if text.starts_with("{\n"))
    );
    let (model, _) = update(
        model,
        ExplorerMessage::BodyEdited(Ok("{\"name\":\"rex\"}\n".into())),
    );
    let Screen::Form(form) = &model.screen else {
        panic!()
    };
    assert_eq!(form.body.as_deref(), Some("{\"name\":\"rex\"}"));
    assert_eq!(model.notice.as_deref(), Some("body updated"));
    // Without parameters the body row is focused: plain `e` edits too.
    let (model, effects) = press(model, KeyCode::Char('e'));
    assert!(
        matches!(&effects[0], ExplorerEffect::EditBody { text, .. } if text == "{\"name\":\"rex\"}")
    );
    let (model, effects) = update(model, key_with(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(
        effects,
        [ExplorerEffect::CopyToClipboard {
            text: "kurama api pets pets/create -d '{\"name\":\"rex\"}'".into()
        }]
    );
    let (model, effects) = press(model, KeyCode::Enter);
    let ExplorerEffect::SendRequest(request) = &effects[0] else {
        panic!("{effects:?}");
    };
    assert_eq!(request.method, "POST");
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.body.as_deref(), Some(&b"{\"name\":\"rex\"}"[..]));
    let (model, _) = update(
        model,
        ExplorerMessage::ResponseReceived(Err("API_HTTP_ERROR: HTTP 500".into())),
    );
    assert_eq!(
        model.modal,
        Some(ExplorerModal::Error("API_HTTP_ERROR: HTTP 500".into()))
    );
    assert!(matches!(model.screen, Screen::Form(_)));
    let (model, _) = press(model, KeyCode::Enter);

    // A body emptied in the editor is no body: the required-body check
    // refuses the send and the copied command carries no -d.
    let (model, _) = update(model, ExplorerMessage::BodyEdited(Ok("\n".into())));
    let (model, effects) = update(model, key_with(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(
        effects,
        [ExplorerEffect::CopyToClipboard {
            text: "kurama api pets pets/create".into()
        }]
    );
    let (model, effects) = press(model, KeyCode::Enter);
    assert!(effects.is_empty());
    let Screen::Form(form) = &model.screen else {
        panic!()
    };
    assert_eq!(
        form.error.as_deref(),
        Some("operation pets/create needs a request body (-d)")
    );

    // A failing editor is an error modal; the form keeps its body.
    let (model, _) = update(
        model,
        ExplorerMessage::BodyEdited(Err("vi exited with exit status: 1".into())),
    );
    assert_eq!(
        model.modal,
        Some(ExplorerModal::Error("vi exited with exit status: 1".into()))
    );
    assert!(matches!(&model.screen, Screen::Form(form) if form.body.as_deref() == Some("")));
}

#[test]
fn a_non_json_body_starts_empty_and_warnings_are_announced() {
    let mut spec = (*spec()).clone();
    spec.operations[1]
        .request_body
        .as_mut()
        .unwrap()
        .content_type = "text/plain".into();
    spec.warnings = vec!["upload: formData parameter 'file' is not supported".into()];
    let (model, _) = update(
        ExplorerModel::new(api()),
        ExplorerMessage::SpecLoaded(Ok(Arc::new(spec))),
    );
    assert_eq!(
        model.notice.as_deref(),
        Some("1 warning(s) about the description are printed after exit")
    );
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Enter);
    let Screen::Form(form) = &model.screen else {
        panic!()
    };
    assert_eq!(form.body.as_deref(), Some(""));
    let (_, effects) = update(model, key_with(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(
        effects,
        [ExplorerEffect::EditBody {
            text: String::new(),
            suffix: "txt"
        }],
        "a text body is edited as a text file"
    );
}

#[test]
fn help_opens_from_every_screen_and_processing_ignores_keys() {
    let (model, _) = press(loaded(), KeyCode::Char('?'));
    assert_eq!(model.modal, Some(ExplorerModal::Help));
    let (model, _) = press(model, KeyCode::Esc);
    assert!(model.modal.is_none());
    let mut model = model;
    model.modal = Some(ExplorerModal::Processing("GET x".into()));
    let (model, effects) = press(model, KeyCode::Esc);
    assert!(effects.is_empty());
    assert!(matches!(model.modal, Some(ExplorerModal::Processing(_))));
}

#[test]
fn an_applied_filter_shows_it_runs_until_its_lines_arrive() {
    let model = super::super::testing::fixtures::result_jq_running();
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert_eq!(result_lines(result), ["jq: running .tags[]…"]);
    // Keys still act meanwhile.
    let (model, _) = press(model, KeyCode::Char('h'));
    let (model, _) = update(model, ExplorerMessage::JqApplied(Ok(vec!["dog".into()])));
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert!(result.show_headers);
    assert_eq!(result.jq_output, Some(Ok(vec!["dog".into()])));
}

#[test]
fn lines_of_a_filter_that_was_cleared_meanwhile_are_dropped() {
    let model = super::super::testing::fixtures::result_jq_running();
    let (model, _) = press(model, KeyCode::Char('j'));
    let model = (0..".tags[]".len()).fold(model, |model, _| press(model, KeyCode::Backspace).0);
    let (model, _) = press(model, KeyCode::Enter);
    let (model, _) = update(model, ExplorerMessage::JqApplied(Ok(vec!["dog".into()])));
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert_eq!(result.jq, None);
    assert_eq!(result.jq_output, None);
}

#[test]
fn q_in_help_quits_as_the_help_says() {
    let (model, _) = press(loaded(), KeyCode::Char('?'));
    let (model, effects) = press(model, KeyCode::Char('q'));
    assert!(model.should_exit);
    assert!(matches!(effects.as_slice(), [ExplorerEffect::Exit]));
}

#[test]
fn jq_completion_from_the_response_reaches_the_copied_command() {
    let model = super::super::testing::fixtures::result();
    let (model, _) = press(model, KeyCode::Char('j'));
    let model = type_text(model, ".nam");
    let (model, _) = press(model, KeyCode::Tab);
    let Some(ExplorerModal::JqInput(input)) = &model.modal else {
        panic!()
    };
    assert_eq!(input.as_str(), ".name");
    let (model, _) = press(model, KeyCode::Enter);
    let (_, effects) = press(model, KeyCode::Char('c'));
    assert_eq!(
        effects,
        [ExplorerEffect::CopyToClipboard {
            text: "kurama api pets pets/get -P petId=p-1 --jq .name".into()
        }]
    );
}

#[test]
fn opening_and_editing_jq_request_a_preview_but_navigation_does_not() {
    let model = super::super::testing::fixtures::result();
    let (model, effects) = press(model, KeyCode::Char('j'));
    assert_eq!(
        effects.len(),
        1,
        "opening jq should preview the current body"
    );
    let (model, effects) = press(model, KeyCode::Char('.'));
    assert_eq!(effects.len(), 1);
    let (_, effects) = press(model, KeyCode::Left);
    assert!(effects.is_empty());
}

#[test]
fn jq_preview_reuses_the_parsed_body_and_displays_errors_in_the_input() {
    let model = super::super::testing::fixtures::result();
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    let parsed = result.parsed_body.clone().unwrap();
    let (model, effects) = press(model, KeyCode::Char('j'));
    let [ExplorerEffect::PreviewJq { filter, body }] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_eq!(filter, ".");
    assert!(Arc::ptr_eq(&parsed, body));
    let (model, _) = update(
        model,
        ExplorerMessage::JqPreviewed(Err("invalid jq filter".into())),
    );
    let Some(ExplorerModal::JqInput(input)) = &model.modal else {
        panic!()
    };
    assert_eq!(
        input.preview,
        Preview::Output(Err("invalid jq filter".into()))
    );
    assert!(matches!(model.screen, Screen::Result(_)));
}

#[test]
fn oversized_or_non_json_responses_never_request_a_live_preview() {
    for body in [
        serde_json::to_vec(
            &serde_json::json!({"payload": "x".repeat(super::super::jq_input::PREVIEW_LIMIT)}),
        )
        .unwrap(),
        b"not json".to_vec(),
    ] {
        let model = super::super::testing::fixtures::processing();
        let (model, _) = update(
            model,
            ExplorerMessage::ResponseReceived(Ok(ResponseInfo {
                status: 200,
                headers: vec![],
                body,
                elapsed_ms: 1,
            })),
        );
        let (model, effects) = press(model, KeyCode::Char('j'));
        assert!(effects.is_empty());
        let (_, effects) = press(model, KeyCode::Char('.'));
        assert!(effects.is_empty());
    }
}

#[test]
fn jq_preview_passes_the_edited_filter_and_cached_body_to_the_runtime() {
    let model = super::super::testing::fixtures::result();
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    let body = result.parsed_body.clone().unwrap();
    let (model, _) = press(model, KeyCode::Char('j'));
    let model = type_text(model, ".nam");
    let (_, effects) = press(model, KeyCode::Char('e'));
    let [
        ExplorerEffect::PreviewJq {
            filter,
            body: actual,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}")
    };
    assert_eq!(filter, ".name");
    assert!(Arc::ptr_eq(actual, &body));
}

#[test]
fn jq_history_recalls_applied_filters_after_another_response() {
    let model = super::super::testing::fixtures::result();
    let (model, _) = press(model, KeyCode::Char('j'));
    let model = type_text(model, ".name");
    let (model, _) = press(model, KeyCode::Enter);
    let (model, _) = press(model, KeyCode::Char('j'));
    let (model, _) = update(model, key_with(KeyCode::Char('u'), KeyModifiers::CONTROL));
    let model = type_text(model, ".tags[]");
    let (model, _) = press(model, KeyCode::Enter);
    let (model, _) = press(model, KeyCode::Esc);
    let (model, _) = press(model, KeyCode::Enter);
    let (model, _) = update(
        model,
        ExplorerMessage::ResponseReceived(Ok(ResponseInfo {
            status: 200,
            headers: vec![],
            body: br#"{"name":"Milo"}"#.to_vec(),
            elapsed_ms: 1,
        })),
    );
    let (model, _) = press(model, KeyCode::Char('j'));
    let model = type_text(model, ".draft");
    let (model, _) = press(model, KeyCode::Up);
    assert!(
        matches!(&model.modal, Some(ExplorerModal::JqInput(input)) if input.as_str() == ".tags[]")
    );
    let (model, _) = press(model, KeyCode::Up);
    assert!(
        matches!(&model.modal, Some(ExplorerModal::JqInput(input)) if input.as_str() == ".name")
    );
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Down);
    assert!(
        matches!(&model.modal, Some(ExplorerModal::JqInput(input)) if input.as_str() == ".draft")
    );
}

#[test]
fn the_token_is_read_after_the_description_and_after_every_call_only_with_auth() {
    use chrono::{DateTime, Duration};
    let (model, effects) = update(
        crate::shell::tui::explorer::testing::fixtures::loading(),
        ExplorerMessage::SpecLoaded(Ok(spec())),
    );
    assert_eq!(
        effects,
        [ExplorerEffect::ReadTokenExpiry, ExplorerEffect::LoadHistory]
    );
    let (_, effects) = update(
        model.clone(),
        ExplorerMessage::ResponseReceived(Err("boom".into())),
    );
    assert_eq!(effects, [ExplorerEffect::ReadTokenExpiry]);

    let mut without_auth = crate::shell::tui::explorer::testing::fixtures::loading();
    without_auth.api.auth = None;
    let (_, effects) = update(without_auth, ExplorerMessage::SpecLoaded(Ok(spec())));
    assert_eq!(effects, [ExplorerEffect::LoadHistory]);

    let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
    let (model, _) = update(model, ExplorerMessage::Tick(now));
    let (model, effects) = update(
        model,
        ExplorerMessage::TokenRead(Some(now + Duration::minutes(5))),
    );
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(model.now, now);
    assert_eq!(model.token_expires_at, Some(now + Duration::minutes(5)));
}

#[test]
fn a_sent_request_is_appended_to_the_history_and_reopens_as_the_same_form() {
    let model = loaded();
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Down);
    let (model, _) = press(model, KeyCode::Enter);
    let model = type_text(model, "p1");
    let (model, _) = press(model, KeyCode::Tab);
    let model = type_text(model, "t1");
    let sent_form = model.screen.clone();
    let (model, effects) = press(model, KeyCode::Enter);
    let entry = crate::domain::types::request_history::HistoryEntry {
        operation: "pets/get".into(),
        params: vec![
            ("petId".into(), "p1".into()),
            ("X-Trace".into(), "t1".into()),
        ],
        body: None,
        name: None,
    };
    assert!(matches!(effects[0], ExplorerEffect::SendRequest(_)));
    assert_eq!(effects[1], ExplorerEffect::AppendHistory(entry.clone()));
    assert_eq!(model.history, [entry]);

    // Back on the list, h lists it and Enter opens its form again.
    let (model, _) = update(model, ExplorerMessage::ResponseReceived(Err("x".into())));
    let (model, _) = press(model, KeyCode::Esc);
    let (model, _) = press(model, KeyCode::Esc);
    assert!(matches!(model.screen, Screen::List), "{:?}", model.screen);
    let (model, effects) = press(model, KeyCode::Char('h'));
    assert!(effects.is_empty(), "opening the history sends nothing");
    assert!(matches!(model.modal, Some(ExplorerModal::History(_))));
    let (model, effects) = press(model, KeyCode::Enter);
    assert!(effects.is_empty(), "opening an entry sends nothing");
    assert!(model.modal.is_none());
    let (Screen::Form(reopened), Screen::Form(sent)) = (&model.screen, &sent_form) else {
        panic!("{:?}", model.screen);
    };
    assert_eq!(reopened.values, sent.values);
    assert_eq!(reopened.operation, sent.operation);
}

#[test]
fn a_favorite_is_named_appended_and_listed_first() {
    let mut model = loaded();
    let (next, _) = update(
        model,
        ExplorerMessage::HistoryLoaded(vec![crate::domain::types::request_history::HistoryEntry {
            operation: "pets/list".into(),
            params: vec![("limit".into(), "2".into())],
            body: None,
            name: None,
        }]),
    );
    model = next;
    let (model, _) = press(model, KeyCode::Char('h'));
    let (model, _) = press(model, KeyCode::Char('s'));
    let model = type_text(model, "two");
    let (model, effects) = press(model, KeyCode::Enter);
    let [ExplorerEffect::AppendHistory(saved)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(saved.name.as_deref(), Some("two"));
    assert_eq!(model.notice.as_deref(), Some("saved as two"));
    let Some(ExplorerModal::History(history)) = &model.modal else {
        panic!("{:?}", model.modal);
    };
    assert_eq!(history.entries[0], *saved);
    assert_eq!(history.entries.len(), 2);
}

fn result_with_body(body: &[u8]) -> ExplorerModel {
    let (model, _) = press(loaded(), KeyCode::Enter);
    let (model, _) = press(model, KeyCode::Enter);
    update(
        model,
        ExplorerMessage::ResponseReceived(Ok(ResponseInfo {
            status: 200,
            headers: Vec::new(),
            body: body.to_vec(),
            elapsed_ms: 1,
        })),
    )
    .0
}

#[test]
fn t_shows_the_tree_where_y_copies_a_path_and_j_filters_by_it() {
    let model = result_with_body(br#"{"items":[{"name":"Rex"}]}"#);
    let (model, _) = press(model, KeyCode::Char('t'));
    let tree = |model: &ExplorerModel| match &model.screen {
        Screen::Result(result) => result.tree.clone(),
        _ => None,
    };
    assert!(tree(&model).is_some());
    let (back, _) = press(model.clone(), KeyCode::Char('t'));
    assert!(tree(&back).is_none(), "t goes back to the text");
    let mut model = model;
    for code in [
        KeyCode::Down,
        KeyCode::Right,
        KeyCode::Down,
        KeyCode::Right,
        KeyCode::Down,
    ] {
        model = press(model, code).0;
    }
    let (model, effects) = press(model, KeyCode::Char('y'));
    assert_eq!(
        effects,
        [ExplorerEffect::CopyPath {
            path: ".items[0].name".into()
        }]
    );
    let (model, _) = update(
        model,
        ExplorerMessage::PathCopied(Ok(".items[0].name".into())),
    );
    assert_eq!(
        model.notice.as_deref(),
        Some(".items[0].name copied to the clipboard")
    );
    let (model, _) = press(model, KeyCode::Char('j'));
    let Some(ExplorerModal::JqInput(input)) = &model.modal else {
        panic!("{:?}", model.modal);
    };
    assert_eq!(input.as_str(), ".items[0].name");
    let (model, effects) = press(model, KeyCode::Enter);
    assert!(
        matches!(&effects[..], [ExplorerEffect::ApplyJq { filter, .. }] if filter == ".items[0].name")
    );
    // The filter's output is text, so applying it leaves the tree.
    assert!(tree(&model).is_none());
    let Screen::Result(result) = &model.screen else {
        panic!()
    };
    assert_eq!(result.jq.as_deref(), Some(".items[0].name"));
}

#[test]
fn the_tree_is_off_for_a_body_that_is_not_json_or_over_one_mebibyte() {
    let (model, _) = press(result_with_body(b"plain text"), KeyCode::Char('t'));
    assert_eq!(model.notice.as_deref(), Some("the response is not JSON"));
    assert!(matches!(&model.screen, Screen::Result(result) if result.tree.is_none()));

    let big = format!("[\"{}\"]", "x".repeat(PREVIEW_LIMIT));
    let (model, _) = press(result_with_body(big.as_bytes()), KeyCode::Char('t'));
    assert_eq!(
        model.notice.as_deref(),
        Some("the tree is off for a response over 1 MiB")
    );
    assert!(matches!(&model.screen, Screen::Result(result) if result.tree.is_none()));
}

#[test]
fn a_start_entry_opens_its_form_once_the_description_is_loaded() {
    let mut model = crate::shell::tui::explorer::testing::fixtures::loading();
    model.start = Some(crate::domain::types::request_history::HistoryEntry {
        operation: "pets/get".into(),
        params: vec![("petId".into(), "p1".into())],
        body: None,
        name: None,
    });
    let (model, _) = update(model, ExplorerMessage::SpecLoaded(Ok(spec())));
    assert!(model.start.is_none());
    let Screen::Form(form) = &model.screen else {
        panic!("{:?}", model.screen);
    };
    assert_eq!(model.operation(form.operation).unwrap().id, "pets/get");
    assert_eq!(form.values[0].as_str(), "p1");

    let mut gone = crate::shell::tui::explorer::testing::fixtures::loading();
    gone.start = Some(crate::domain::types::request_history::HistoryEntry {
        operation: "gone/away".into(),
        params: Vec::new(),
        body: None,
        name: None,
    });
    let (gone, _) = update(gone, ExplorerMessage::SpecLoaded(Ok(spec())));
    assert!(matches!(gone.screen, Screen::List));
    assert_eq!(
        gone.notice.as_deref(),
        Some("gone/away is no longer in the description")
    );
}
