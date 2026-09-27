//! Explorer state and its pure transitions.
//!
//! Screens: the operation list (with search and the detail of the selected
//! operation), the form of one operation (parameter values and the request
//! body), and the result of a call (status, headers, body, a jq filter).
//! Help, an error, the progress of a call and the jq input are modals over
//! them. No I/O, no clock: the runtime runs the effects.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::effects::ExplorerEffect;
use super::history::{HistoryAction, HistoryEntry, HistoryModal, entry_for, form_from};
use super::jq_input::{InputAction, JqInputModel, PREVIEW_LIMIT, Preview};
use super::json_tree::{TreeState, handle_tree_key, selected_path};
use super::messages::{ExplorerMessage, ResponseInfo};
use super::model::{
    ExplorerModal, ExplorerModel, ResultModel, Screen, form_body, form_params, new_form,
    result_command, result_lines,
};
use crate::domain::functions::api_request::{
    body_as_json, indent_json, with_default_headers, with_headers,
};
use crate::domain::functions::operation_command::{cli_command, example_command, is_json};
use crate::domain::functions::operation_request::build_operation_request;
use crate::shell::tui::components::line_input::typed;
use crate::shell::tui::components::list_navigation::{PAGE, moved_selection};

/// A source's token can be read once the description is loaded and after
/// every call; an API without `auth` has none.
fn read_token_expiry(model: &ExplorerModel) -> Vec<ExplorerEffect> {
    if model.api.auth.is_some() {
        vec![ExplorerEffect::ReadTokenExpiry]
    } else {
        Vec::new()
    }
}

pub fn update(
    mut model: ExplorerModel,
    message: ExplorerMessage,
) -> (ExplorerModel, Vec<ExplorerEffect>) {
    let effects = match message {
        ExplorerMessage::Key(key) => on_key(&mut model, key),
        ExplorerMessage::Resize => Vec::new(),
        ExplorerMessage::Tick(now) => {
            model.now = now;
            Vec::new()
        }
        ExplorerMessage::TokenRead(expires_at) => {
            model.token_expires_at = expires_at;
            Vec::new()
        }
        ExplorerMessage::SpecLoaded(result) => {
            model.loading = false;
            let mut effects = read_token_expiry(&model);
            match result {
                Ok(spec) => {
                    effects.push(ExplorerEffect::LoadHistory);
                    let warnings = spec.warnings.len();
                    model.spec = Some(spec);
                    model.apply_search();
                    if let Some(start) = model.start.take() {
                        open_entry(&mut model, &start);
                    }
                    if warnings > 0 {
                        model.notice = Some(format!(
                            "{warnings} warning(s) about the description are printed after exit"
                        ));
                    }
                }
                Err(message) => model.modal = Some(ExplorerModal::Error(message)),
            }
            effects
        }
        ExplorerMessage::ResponseReceived(result) => {
            model.modal = None;
            match result {
                Ok(info) => on_response(&mut model, info),
                Err(message) => model.modal = Some(ExplorerModal::Error(message)),
            }
            // The call may have refreshed or replaced the token.
            read_token_expiry(&model)
        }
        ExplorerMessage::BodyEdited(result) => {
            match (result, &mut model.screen) {
                (Ok(text), Screen::Form(form)) => {
                    form.body = Some(text.trim_end().to_string());
                    form.error = None;
                    model.notice = Some("body updated".into());
                }
                (Err(message), _) => model.modal = Some(ExplorerModal::Error(message)),
                _ => {}
            }
            Vec::new()
        }
        ExplorerMessage::Copied(result) => {
            model.notice = Some(match result {
                Ok(()) => "command copied to the clipboard".into(),
                Err(message) => format!("copy failed: {message}"),
            });
            Vec::new()
        }
        ExplorerMessage::PathCopied(result) => {
            model.notice = Some(match result {
                Ok(path) => format!("{path} copied to the clipboard"),
                Err(message) => format!("copy failed: {message}"),
            });
            Vec::new()
        }
        ExplorerMessage::JqPreviewed(result) => {
            if let Some(ExplorerModal::JqInput(input)) = &mut model.modal {
                input.preview = Preview::Output(result);
            }
            Vec::new()
        }
        ExplorerMessage::JqPreviewUnavailable(message) => {
            if let Some(ExplorerModal::JqInput(input)) = &mut model.modal {
                input.preview = Preview::Unavailable(message);
            }
            Vec::new()
        }
        ExplorerMessage::JqApplied(result) => {
            // Lines for a filter that was cleared meanwhile, or for an
            // earlier result, belong to no filter on screen.
            if let Screen::Result(current) = &mut model.screen
                && current.jq.is_some()
                && current.jq_output.is_none()
            {
                current.jq_output = Some(result);
                current.scroll = 0;
            }
            Vec::new()
        }
        ExplorerMessage::HistoryLoaded(entries) => {
            model.history = entries;
            Vec::new()
        }
        ExplorerMessage::Notice(text) => {
            model.notice = Some(text);
            Vec::new()
        }
    };
    (model, effects)
}

fn on_response(model: &mut ExplorerModel, info: ResponseInfo) {
    let Screen::Form(form) = &model.screen else {
        return;
    };
    let Some(operation) = model.operation(form.operation) else {
        return;
    };
    let parsed_body = body_as_json(&info.body).map(Arc::new);
    // The screen and `kurama api` on a terminal lay a body out the same way,
    // and neither shows a number the server did not send.
    let body =
        indent_json(&info.body).unwrap_or_else(|| String::from_utf8_lossy(&info.body).into_owned());
    model.screen = Screen::Result(Box::new(ResultModel {
        params: form_params(operation, form),
        body_sent: form_body(form).map(str::to_string),
        form: form.clone(),
        status: info.status,
        elapsed_ms: info.elapsed_ms,
        headers: info.headers,
        raw_body: info.body,
        parsed_body,
        body,
        show_headers: false,
        scroll: 0,
        jq: None,
        jq_output: None,
        tree: None,
    }));
}

fn on_key(model: &mut ExplorerModel, key: KeyEvent) -> Vec<ExplorerEffect> {
    model.notice = None;
    if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
        return quit(model);
    }
    if let Some(modal) = model.modal.clone() {
        return on_modal_key(model, modal, key);
    }
    match &model.screen {
        Screen::List => on_list_key(model, key),
        Screen::Form(_) => on_form_key(model, key),
        Screen::Result(_) => on_result_key(model, key),
    }
}

fn quit(model: &mut ExplorerModel) -> Vec<ExplorerEffect> {
    model.should_exit = true;
    vec![ExplorerEffect::Exit]
}

fn on_list_key(model: &mut ExplorerModel, key: KeyEvent) -> Vec<ExplorerEffect> {
    if model.searching {
        return on_search_key(model, key);
    }
    match key.code {
        KeyCode::Char('q') => return quit(model),
        code if let Some(index) = moved_selection(code, model.selected, model.filtered.len()) => {
            model.selected = index;
        }
        KeyCode::Enter => {
            if let Some(index) = model.filtered.get(model.selected).copied()
                && let Some(operation) = model.operation(index)
            {
                model.screen = Screen::Form(new_form(index, operation));
            }
        }
        KeyCode::Char('/') => model.searching = true,
        KeyCode::Char('h') => {
            model.modal = Some(ExplorerModal::History(HistoryModal::new(&model.history)));
        }
        KeyCode::Char('?') | KeyCode::F(1) => model.modal = Some(ExplorerModal::Help),
        KeyCode::Char('c') | KeyCode::Char('Y') => match model.selected_operation() {
            Some(operation) => {
                return vec![ExplorerEffect::CopyToClipboard {
                    text: example_command(&model.api.name, operation),
                }];
            }
            None => model.notice = Some("no operation selected".into()),
        },
        KeyCode::Char('o') => match model
            .selected_operation()
            .and_then(|op| op.external_docs.clone())
        {
            Some(url) => return vec![ExplorerEffect::OpenBrowser { url }],
            None => model.notice = Some("the operation has no documentation link".into()),
        },
        KeyCode::Esc if !model.search_query.is_empty() => {
            model.search_query.clear();
            model.apply_search();
        }
        _ => {}
    }
    Vec::new()
}

fn on_search_key(model: &mut ExplorerModel, key: KeyEvent) -> Vec<ExplorerEffect> {
    match key.code {
        KeyCode::Esc => {
            model.searching = false;
            model.search_query.clear();
            model.apply_search();
        }
        KeyCode::Enter => model.searching = false,
        KeyCode::Up | KeyCode::Down => {
            model.selected =
                moved_selection(key.code, model.selected, model.filtered.len()).unwrap_or(0);
        }
        _ => {
            if model.search_query.handle_key(key) {
                model.apply_search();
            }
        }
    }
    Vec::new()
}

enum FormAction {
    Nothing,
    Back,
    Send,
    Edit,
    Copy,
    Help,
}

fn on_form_key(model: &mut ExplorerModel, key: KeyEvent) -> Vec<ExplorerEffect> {
    let Some(spec) = model.spec.clone() else {
        return Vec::new();
    };
    let Screen::Form(form) = &mut model.screen else {
        return Vec::new();
    };
    let operation = &spec.operations[form.operation];
    let rows = form.values.len() + usize::from(form.body.is_some());
    let on_body = form.focus >= form.values.len();
    let action = match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => FormAction::Back,
        (KeyCode::Enter, _) => FormAction::Send,
        (KeyCode::Char('e'), KeyModifiers::CONTROL) if on_body => FormAction::Edit,
        (KeyCode::Char('e'), _) if on_body && typed(key) => FormAction::Edit,
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => FormAction::Copy,
        (KeyCode::F(1), _) => FormAction::Help,
        (KeyCode::Tab, _) | (KeyCode::Down, _) => {
            if rows > 0 {
                form.focus = (form.focus + 1) % rows;
            }
            FormAction::Nothing
        }
        (KeyCode::BackTab, _) | (KeyCode::Up, _) => {
            if rows > 0 {
                form.focus = (form.focus + rows - 1) % rows;
            }
            FormAction::Nothing
        }
        _ => {
            if let Some(value) = form.values.get_mut(form.focus)
                && value.handle_key(key)
            {
                form.error = None;
            }
            FormAction::Nothing
        }
    };
    match action {
        FormAction::Nothing => Vec::new(),
        FormAction::Back => {
            model.screen = Screen::List;
            Vec::new()
        }
        FormAction::Help => {
            model.modal = Some(ExplorerModal::Help);
            Vec::new()
        }
        FormAction::Edit => match &form.body {
            Some(text) => {
                let json = operation
                    .request_body
                    .as_ref()
                    .is_some_and(|body| is_json(&body.content_type));
                vec![ExplorerEffect::EditBody {
                    text: text.clone(),
                    suffix: if json { "json" } else { "txt" },
                }]
            }
            None => {
                model.notice = Some("the operation takes no request body".into());
                Vec::new()
            }
        },
        FormAction::Copy => {
            let params = form_params(operation, form);
            vec![ExplorerEffect::CopyToClipboard {
                text: cli_command(&model.api.name, operation, &params, form_body(form), None),
            }]
        }
        FormAction::Send => {
            let params = form_params(operation, form);
            let body = form_body(form).map(|text| text.as_bytes().to_vec());
            match build_operation_request(&model.api.base_url, operation, &params, body) {
                Ok(request) => {
                    let request =
                        with_default_headers(with_headers(request, model.api.headers.iter()));
                    form.error = None;
                    model.modal = Some(ExplorerModal::Processing(format!(
                        "{} {}",
                        request.method, request.url
                    )));
                    let entry = entry_for(operation, &params, form_body(form));
                    model.history.push(entry.clone());
                    vec![
                        ExplorerEffect::SendRequest(request),
                        ExplorerEffect::AppendHistory(entry),
                    ]
                }
                Err(error) => {
                    form.error = Some(error.to_string());
                    Vec::new()
                }
            }
        }
    }
}

fn on_result_key(model: &mut ExplorerModel, key: KeyEvent) -> Vec<ExplorerEffect> {
    let Screen::Result(result) = &mut model.screen else {
        return Vec::new();
    };
    if let (Some(tree), Some(body)) = (&mut result.tree, &result.parsed_body) {
        match key.code {
            KeyCode::Char('t') => result.tree = None,
            KeyCode::Char('y') => {
                if let Some(path) = selected_path(tree, body) {
                    return vec![ExplorerEffect::CopyPath { path }];
                }
            }
            KeyCode::Char('j') => {
                let path = selected_path(tree, body).unwrap_or_else(|| ".".into());
                let input =
                    JqInputModel::new(path, result.parsed_body.clone(), result.raw_body.len());
                let effects = preview_effect(&input);
                model.modal = Some(ExplorerModal::JqInput(input));
                return effects;
            }
            code if handle_tree_key(tree, code, body) => {}
            _ => return on_result_text_key(model, key),
        }
        return Vec::new();
    }
    on_result_text_key(model, key)
}

/// The result's keys shared by the text and the tree, and the text's own.
fn on_result_text_key(model: &mut ExplorerModel, key: KeyEvent) -> Vec<ExplorerEffect> {
    let Some(spec) = model.spec.clone() else {
        return Vec::new();
    };
    let Screen::Result(result) = &mut model.screen else {
        return Vec::new();
    };
    let operation = &spec.operations[result.form.operation];
    let last = result_lines(result).len().saturating_sub(1);
    match key.code {
        KeyCode::Char('q') => return quit(model),
        KeyCode::Esc | KeyCode::Backspace => {
            model.screen = Screen::Form(result.form.clone());
        }
        KeyCode::Up => result.scroll = result.scroll.saturating_sub(1),
        KeyCode::Down => result.scroll = (result.scroll + 1).min(last),
        KeyCode::PageUp => result.scroll = result.scroll.saturating_sub(PAGE),
        KeyCode::PageDown => result.scroll = (result.scroll + PAGE).min(last),
        KeyCode::Home => result.scroll = 0,
        KeyCode::End => result.scroll = last,
        KeyCode::Char('h') => {
            result.show_headers = !result.show_headers;
            result.scroll = 0;
        }
        KeyCode::Char('t') => {
            if result.raw_body.len() > PREVIEW_LIMIT {
                model.notice = Some("the tree is off for a response over 1 MiB".into());
            } else if result.parsed_body.is_none() {
                model.notice = Some("the response is not JSON".into());
            } else {
                result.tree = Some(TreeState::default());
            }
        }
        KeyCode::Char('j') => {
            let input = JqInputModel::new(
                result.jq.clone().unwrap_or_default(),
                result.parsed_body.clone(),
                result.raw_body.len(),
            );
            let effects = preview_effect(&input);
            model.modal = Some(ExplorerModal::JqInput(input));
            return effects;
        }
        KeyCode::Char('c') | KeyCode::Char('Y') => {
            return vec![ExplorerEffect::CopyToClipboard {
                text: result_command(&model.api.name, operation, result),
            }];
        }
        KeyCode::Char('?') | KeyCode::F(1) => model.modal = Some(ExplorerModal::Help),
        _ => {}
    }
    Vec::new()
}

fn on_modal_key(
    model: &mut ExplorerModel,
    modal: ExplorerModal,
    key: KeyEvent,
) -> Vec<ExplorerEffect> {
    match modal {
        ExplorerModal::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::F(1)
            ) {
                model.modal = None;
            } else if key.code == KeyCode::Char('q') {
                // The help lists q as quit, so it does what it says.
                return quit(model);
            }
        }
        ExplorerModal::Error(_) => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                model.modal = None;
            } else if key.code == KeyCode::Char('q') {
                return quit(model);
            }
        }
        ExplorerModal::Processing(_) => {}
        ExplorerModal::History(mut history) => match history.handle_key(key) {
            HistoryAction::Nothing => model.modal = Some(ExplorerModal::History(history)),
            HistoryAction::Close => model.modal = None,
            HistoryAction::Quit => return quit(model),
            HistoryAction::Open(entry) => {
                model.modal = None;
                open_entry(model, &entry);
            }
            HistoryAction::Save(favorite) => {
                model.notice = Some(format!(
                    "saved as {}",
                    favorite.name.as_deref().unwrap_or_default()
                ));
                model.history.push(favorite.clone());
                model.modal = Some(ExplorerModal::History(HistoryModal::new(&model.history)));
                return vec![ExplorerEffect::AppendHistory(favorite)];
            }
        },
        ExplorerModal::JqInput(mut input) => match input.handle_key(key, &model.jq_history) {
            InputAction::Close => model.modal = None,
            InputAction::Apply(filter) => {
                model.modal = None;
                if let Screen::Result(result) = &mut model.screen {
                    // The filter's output is text: an applied path leaves the tree.
                    result.tree = None;
                    result.scroll = 0;
                    result.jq_output = None;
                    result.jq = (!filter.is_empty()).then(|| filter.clone());
                    if !filter.is_empty() {
                        if model.jq_history.last() != Some(&filter) {
                            model.jq_history.push(filter.clone());
                        }
                        return vec![ExplorerEffect::ApplyJq {
                            filter,
                            body: result.raw_body.clone(),
                        }];
                    }
                }
            }
            action => {
                let effects = match (&action, &model.screen) {
                    (InputAction::Changed, Screen::Result(_)) => preview_effect(&input),
                    _ => Vec::new(),
                };
                model.modal = Some(ExplorerModal::JqInput(input));
                return effects;
            }
        },
    }
    Vec::new()
}

/// The form of a history entry (or of an operation with no values).
fn open_entry(model: &mut ExplorerModel, entry: &HistoryEntry) {
    match model.spec.as_ref().and_then(|spec| form_from(spec, entry)) {
        Some(form) => model.screen = Screen::Form(form),
        None => {
            model.notice = Some(format!(
                "{} is no longer in the description",
                entry.operation
            ));
        }
    }
}

fn preview_effect(input: &JqInputModel) -> Vec<ExplorerEffect> {
    match (input.preview_filter(), input.preview_body()) {
        (Some(filter), Some(body)) => vec![ExplorerEffect::PreviewJq { filter, body }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
