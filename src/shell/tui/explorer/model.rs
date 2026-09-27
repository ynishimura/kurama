//! The explorer's model: the operation list with its search and selection,
//! the form of one operation and the result of a call, and what the view
//! reads off them. No I/O, no clock.

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use std::sync::Arc;

use super::history::{HistoryEntry, HistoryModal};
use super::jq_input::JqInputModel;
use super::json_tree::TreeState;
use crate::domain::functions::api_request::reason_phrase;
use crate::domain::functions::operation_command::{cli_command, is_json};
use crate::domain::functions::operation_lookup::search_operation_indices;
use crate::domain::types::ApiHeaders;
use crate::domain::types::api_spec::{ApiSpec, Operation};
use crate::shell::tui::components::LineInput;

/// What the explorer knows about the `[api.*]` profile it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSummary {
    pub name: String,
    pub base_url: String,
    pub auth: Option<String>,
    pub aws_profile: Option<String>,
    /// `[api.*] headers`, sent with every request over the operation's own.
    pub headers: ApiHeaders,
}

#[derive(Debug, Clone)]
pub struct ExplorerModel {
    pub api: ApiSummary,
    /// `None` until loaded (or after a failed load).
    pub spec: Option<Arc<ApiSpec>>,
    pub loading: bool,
    /// Indices into `spec.operations` that match the search.
    pub filtered: Vec<usize>,
    /// Index into `filtered`.
    pub selected: usize,
    pub search_query: LineInput,
    pub searching: bool,
    pub screen: Screen,
    pub modal: Option<ExplorerModal>,
    /// One line in the header until the next key.
    pub notice: Option<String>,
    pub should_exit: bool,
    pub jq_history: Vec<String>,
    /// The request history of this API as read at start, oldest first, plus
    /// what was sent or saved since.
    pub history: Vec<HistoryEntry>,
    /// The form to open once the description is loaded (from the home
    /// screen's palette).
    pub start: Option<HistoryEntry>,
    /// The clock as of the last tick, for the time the token has left.
    pub now: DateTime<Utc>,
    /// When the stored token of the API's OAuth source ends.
    pub token_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    List,
    Form(FormModel),
    Result(Box<ResultModel>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerModal {
    Help,
    Error(String),
    Processing(String),
    /// The filter being typed.
    JqInput(JqInputModel),
    /// Sent requests and favorites.
    History(HistoryModal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormModel {
    /// Index into `spec.operations`.
    pub operation: usize,
    /// One value per parameter, in the operation's order.
    pub values: Vec<LineInput>,
    /// `values.len()` is the body row.
    pub focus: usize,
    /// The request body text; `None` when the operation takes none.
    pub body: Option<String>,
    /// The last refused input.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultModel {
    /// The form to go back to.
    pub form: FormModel,
    pub params: Vec<(String, String)>,
    pub body_sent: Option<String>,
    pub status: u16,
    pub elapsed_ms: u128,
    pub headers: Vec<(String, String)>,
    pub raw_body: Vec<u8>,
    pub parsed_body: Option<Arc<serde_json::Value>>,
    /// The body pretty-printed when it is JSON, as received otherwise.
    pub body: String,
    pub show_headers: bool,
    pub scroll: usize,
    pub jq: Option<String>,
    pub jq_output: Option<Result<Vec<String>, String>>,
    /// The body as a JSON tree (`t`); `None` shows the text.
    pub tree: Option<TreeState>,
}

impl ExplorerModel {
    pub fn new(api: ApiSummary) -> Self {
        Self {
            api,
            spec: None,
            loading: true,
            filtered: Vec::new(),
            selected: 0,
            search_query: LineInput::default(),
            searching: false,
            screen: Screen::List,
            modal: None,
            notice: None,
            should_exit: false,
            jq_history: Vec::new(),
            history: Vec::new(),
            start: None,
            now: DateTime::UNIX_EPOCH,
            token_expires_at: None,
        }
    }

    pub fn operation(&self, index: usize) -> Option<&Operation> {
        self.spec.as_ref()?.operations.get(index)
    }

    /// The operation under the cursor of the list.
    pub fn selected_operation(&self) -> Option<&Operation> {
        self.operation(*self.filtered.get(self.selected)?)
    }

    pub(super) fn apply_search(&mut self) {
        self.filtered = self
            .spec
            .as_ref()
            .map(|spec| search_operation_indices(spec, self.search_query.as_str()))
            .unwrap_or_default();
        self.selected = 0;
    }
}

/// The empty form of an operation: no values, and a JSON body starting from
/// its skeleton (any other body is typed in the editor).
pub fn new_form(index: usize, operation: &Operation) -> FormModel {
    FormModel {
        operation: index,
        values: vec![LineInput::default(); operation.parameters.len()],
        focus: 0,
        body: operation.request_body.as_ref().map(|body| {
            if is_json(&body.content_type) {
                serde_json::to_string_pretty(&body.schema.skeleton()).unwrap_or_default()
            } else {
                String::new()
            }
        }),
        error: None,
    }
}

/// The `-P name=value` pairs of a form: the non-empty values.
pub fn form_params(operation: &Operation, form: &FormModel) -> Vec<(String, String)> {
    operation
        .parameters
        .iter()
        .zip(&form.values)
        .filter(|(_, value)| !value.is_empty())
        .map(|(parameter, value)| (parameter.name.clone(), value.as_str().to_owned()))
        .collect()
}

/// The lines the result pane shows: the headers when asked for, then the
/// jq output (or its error, or that it is still running) or the body. Borrowed where possible: a large
/// body is not copied for every key press.
pub fn result_lines(result: &ResultModel) -> Vec<Cow<'_, str>> {
    let mut lines: Vec<Cow<'_, str>> = Vec::new();
    if result.show_headers {
        for (name, value) in &result.headers {
            lines.push(Cow::Owned(format!("{name}: {value}")));
        }
        lines.push(Cow::Borrowed(""));
    }
    match &result.jq_output {
        Some(Ok(output)) => lines.extend(output.iter().map(|line| Cow::Borrowed(line.as_str()))),
        Some(Err(error)) => lines.push(Cow::Owned(format!("jq: {error}"))),
        // A filter with no output yet is still running.
        None => match &result.jq {
            Some(filter) => lines.push(Cow::Owned(format!("jq: running {filter}…"))),
            None => lines.extend(result.body.lines().map(Cow::Borrowed)),
        },
    }
    lines
}

/// The body text a form sends and copies: `None` when there is none or it
/// was emptied in the editor.
pub(super) fn form_body(form: &FormModel) -> Option<&str> {
    form.body.as_deref().filter(|text| !text.is_empty())
}

/// The CLI command that repeats the result's call.
pub fn result_command(api: &str, operation: &Operation, result: &ResultModel) -> String {
    cli_command(
        api,
        operation,
        &result.params,
        result.body_sent.as_deref(),
        result.jq.as_deref(),
    )
}

/// The reason phrase for the result title.
pub fn status_text(status: u16) -> String {
    let reason = reason_phrase(status);
    if reason.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status} {reason}")
    }
}
