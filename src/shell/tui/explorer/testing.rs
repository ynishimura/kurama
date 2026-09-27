//! Fixtures for every explorer state and the explorer's UI contract, for
//! the render regression (`view_snapshot_tests.rs`).

use ratatui::buffer::Buffer;

use super::model::{ExplorerModal, ExplorerModel, Screen};
use super::view::{APP_TITLE, hints, render};
use crate::shell::tui::testing::{
    buffer_lines, modal_violations, render_frame, selection_violations,
};
use crate::shell::tui::theme::{self, Tone};

pub fn render_buffer(model: &ExplorerModel, width: u16, height: u16) -> Buffer {
    render_frame(width, height, |frame| render(frame, model))
}

/// Violations of the explorer contract: the title is in the header, the
/// primary hint in the footer, a panel ends above the footer, the selected
/// operation is marked on the list, a modal is inside the viewport in its
/// tone.
pub fn check_contract(model: &ExplorerModel, buffer: &Buffer) -> Vec<String> {
    let lines = buffer_lines(buffer);
    let height = lines.len();
    let mut violations = Vec::new();
    if !lines[0].contains(APP_TITLE) {
        violations.push(format!(
            "header row does not show the title: {:?}",
            lines[0]
        ));
    }
    let footer = &lines[height - 1];
    if let Some((key, action)) = hints(model).first()
        && !footer.contains(&format!("{key} {action}"))
    {
        violations.push(format!(
            "footer lacks the primary hint {key} {action}: {footer:?}"
        ));
    }
    if height >= 4 && !lines[height - 2].contains(theme::BORDER.bottom_left) {
        violations.push(format!(
            "row above the footer is not a panel bottom border: {:?}",
            lines[height - 2]
        ));
    }
    let modal = match &model.modal {
        Some(ExplorerModal::Help) => Some(("Help", Tone::Info)),
        Some(ExplorerModal::Error(_)) => Some(("Error", Tone::Danger)),
        Some(ExplorerModal::Processing(_)) => Some(("Sending", Tone::Info)),
        Some(ExplorerModal::JqInput(_)) => Some(("jq filter", Tone::Info)),
        Some(ExplorerModal::History(_)) => Some((super::history_view::TITLE, Tone::Info)),
        None => None,
    };
    if let Some((title, tone)) = modal {
        violations.extend(modal_violations(buffer, &lines, title, tone));
    } else if matches!(model.screen, Screen::List)
        && !model.loading
        && let Some(operation) = model.selected_operation()
    {
        violations.extend(selection_violations(buffer, &lines, &operation.id));
        violations.extend(detail_clipping_violations(&lines));
    }
    violations
}

/// The detail pane has no scrollbar: whatever does not fit is simply gone.
/// Its last row is therefore the contract -- if a variable-length section
/// above it grows, the warnings and the next step disappear with no sign.
fn detail_clipping_violations(lines: &[String]) -> Vec<String> {
    // "┏ Operations 4/5" is the list; the detail pane's title is bare.
    let detail_title = format!("{} Operation ", theme::BORDER.top_left);
    if !lines.iter().any(|line| line.contains(&detail_title)) {
        return Vec::new();
    }
    if lines.iter().any(|line| line.contains(DETAIL_LAST_ROW)) {
        return Vec::new();
    }
    vec![format!(
        "the detail pane clipped its last row ({DETAIL_LAST_ROW:?}): a section \
         above it is longer than the pane and the rows after it are unreachable"
    )]
}

/// The last row `detail_entries` appends; everything a user must not miss
/// (deprecated, unsupported inputs) is above it.
const DETAIL_LAST_ROW: &str = "Enter opens the form";

/// Every explorer state the view is verified in.
pub mod fixtures {
    use std::sync::Arc;

    use super::*;
    use crate::domain::functions::openapi::normalize_spec;
    use crate::domain::types::api_spec::{ApiSpec, Operation};
    use crate::shell::tui::explorer::messages::{ExplorerMessage, ResponseInfo};
    use crate::shell::tui::explorer::model::{ApiSummary, FormModel};
    use crate::shell::tui::explorer::update::update;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    const PETSTORE: &str = include_str!("../../../../tests/fixtures/openapi/petstore.json");

    pub fn spec() -> Arc<ApiSpec> {
        Arc::new(normalize_spec(&serde_json::from_str(PETSTORE).unwrap()).unwrap())
    }

    pub fn api() -> ApiSummary {
        ApiSummary {
            name: "pets".into(),
            base_url: "https://petstore.example.com/v1".into(),
            auth: Some("pets".into()),
            aws_profile: None,
            headers: Default::default(),
        }
    }

    fn key(model: ExplorerModel, code: KeyCode) -> ExplorerModel {
        update(
            model,
            ExplorerMessage::Key(KeyEvent {
                code,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }),
        )
        .0
    }

    fn keys(mut model: ExplorerModel, codes: &[KeyCode]) -> ExplorerModel {
        for code in codes {
            model = key(model, *code);
        }
        model
    }

    fn type_text(mut model: ExplorerModel, text: &str) -> ExplorerModel {
        for c in text.chars() {
            model = key(model, KeyCode::Char(c));
        }
        model
    }

    pub fn loading() -> ExplorerModel {
        ExplorerModel::new(api())
    }

    pub fn loaded() -> ExplorerModel {
        update(loading(), ExplorerMessage::SpecLoaded(Ok(spec()))).0
    }

    /// The source's stored token has less than 15 minutes left.
    pub fn token_soon() -> ExplorerModel {
        let mut model = loaded();
        model.now = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        model.token_expires_at = Some(model.now + chrono::Duration::minutes(10));
        model
    }

    pub fn response_shape() -> ExplorerModel {
        let document = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/openapi/responses.json"
        ))
        .unwrap();
        let spec = Arc::new(normalize_spec(&document).unwrap());
        update(loading(), ExplorerMessage::SpecLoaded(Ok(spec))).0
    }

    /// A deprecated operation whose response schema is far longer than the
    /// detail pane. Real descriptions reach this size easily; the small
    /// `response_shape` fixture fits by accident and hides clipping.
    pub fn response_shape_large() -> ExplorerModel {
        let properties: serde_json::Map<String, serde_json::Value> = (0..60)
            .map(|index| {
                (
                    format!("field_{index:02}"),
                    serde_json::json!({"type": "string"}),
                )
            })
            .collect();
        let document = serde_json::json!({
            "openapi": "3.0.3",
            "info": {"title": "Response shapes", "version": "1"},
            "paths": {
                "/wide": {
                    "get": {
                        "operationId": "wide/list",
                        "summary": "Every field of a wide record",
                        "deprecated": true,
                        "responses": {
                            "200": {
                                "description": "A wide record",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "properties": properties,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        let spec = Arc::new(normalize_spec(&document).unwrap());
        update(loading(), ExplorerMessage::SpecLoaded(Ok(spec))).0
    }

    /// A later operation selected, one with a body, docs and two scopes.
    pub fn selected() -> ExplorerModel {
        key(loaded(), KeyCode::Down)
    }

    pub fn searching() -> ExplorerModel {
        type_text(key(loaded(), KeyCode::Char('/')), "pets/")
    }

    pub fn empty() -> ExplorerModel {
        type_text(key(loaded(), KeyCode::Char('/')), "zzz")
    }

    /// The description failed to load: the error modal over an empty list.
    pub fn error() -> ExplorerModel {
        update(
            loading(),
            ExplorerMessage::SpecLoaded(Err(
                "API_SPEC_UNAVAILABLE: cannot load the API description https://specs.example.com/openapi.json: request failed: could not connect: connection refused\nhint: check the openapi URL under [api.pets]; `kurama api pets --refresh-spec` fetches it again".into(),
            )),
        )
        .0
    }

    /// The form of `pets/get` with a value typed and the header field focused.
    pub fn form() -> ExplorerModel {
        let model = keys(loaded(), &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        key(type_text(model, "p-1"), KeyCode::Tab)
    }

    /// The form of `pets/create` with its body skeleton.
    pub fn form_body() -> ExplorerModel {
        keys(loaded(), &[KeyCode::Down, KeyCode::Enter])
    }

    /// A refused send: the error line under the fields.
    pub fn form_error() -> ExplorerModel {
        let model = keys(loaded(), &[KeyCode::End, KeyCode::Enter]);
        key(type_text(model, "x"), KeyCode::Enter)
    }

    /// A long body with a refused send: the error stays in view.
    pub fn form_long_body_error() -> ExplorerModel {
        let mut spec = (*spec()).clone();
        let body = spec.operations[1].request_body.as_mut().unwrap();
        body.schema.properties = (1..=30)
            .map(|index| crate::domain::types::api_spec::Property {
                name: format!("field_{index:02}"),
                required: true,
                schema: crate::domain::types::api_spec::Schema {
                    type_name: "string".into(),
                    ..Default::default()
                },
            })
            .collect();
        spec.operations[1]
            .parameters
            .push(crate::domain::types::api_spec::Parameter {
                name: "dry_run".into(),
                location: crate::domain::types::api_spec::ParameterLocation::Query,
                required: true,
                schema: crate::domain::types::api_spec::Schema {
                    type_name: "boolean".into(),
                    ..Default::default()
                },
                description: None,
            });
        let model = update(loading(), ExplorerMessage::SpecLoaded(Ok(Arc::new(spec)))).0;
        keys(model, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter])
    }

    pub fn processing() -> ExplorerModel {
        key(form(), KeyCode::Enter)
    }

    fn response(model: ExplorerModel, status: u16, body: &str) -> ExplorerModel {
        update(
            model,
            ExplorerMessage::ResponseReceived(Ok(ResponseInfo {
                status,
                headers: vec![
                    ("content-type".into(), "application/json".into()),
                    ("x-ratelimit-remaining".into(), "4999".into()),
                ],
                body: body.as_bytes().to_vec(),
                elapsed_ms: 42,
            })),
        )
        .0
    }

    pub fn result() -> ExplorerModel {
        response(
            processing(),
            200,
            r#"{"id":"p-1","name":"Rex","tags":["dog","friendly"],"owner":{"login":"octocat"}}"#,
        )
    }

    pub fn result_headers() -> ExplorerModel {
        key(result(), KeyCode::Char('h'))
    }

    /// A long body scrolled down, with a 404.
    pub fn result_scrolled() -> ExplorerModel {
        let items: Vec<String> = (1..=40)
            .map(|i| format!(r#"{{"id":{i},"name":"pet-{i}"}}"#))
            .collect();
        let model = response(processing(), 404, &format!("[{}]", items.join(",")));
        keys(model, &[KeyCode::PageDown, KeyCode::PageDown])
    }

    /// The result as a tree, `owner` open and its `login` selected.
    pub fn result_tree() -> ExplorerModel {
        let codes = [
            KeyCode::Char('t'),
            KeyCode::End,
            KeyCode::Right,
            KeyCode::Down,
        ];
        keys(result(), &codes)
    }

    /// An array longer than a node lists, open, the selection past the page.
    pub fn result_tree_large() -> ExplorerModel {
        let items: Vec<String> = (0..600)
            .map(|i| format!(r#"{{"id":{i},"name":"pet-{i}"}}"#))
            .collect();
        let model = response(processing(), 200, &format!("[{}]", items.join(",")));
        keys(model, &[KeyCode::Char('t'), KeyCode::End, KeyCode::Up])
    }

    pub fn jq_input() -> ExplorerModel {
        type_text(key(result(), KeyCode::Char('j')), ".tags[]")
    }

    pub fn jq_input_completing() -> ExplorerModel {
        let model = response(
            processing(),
            200,
            r#"{"securityAdminGuardrails":{"organizationConfiguration":{"accounts":[{"accountId":15,"accountName":"fukuda_test_aws_account_2024","enabled":false}]}}}"#,
        );
        key(
            type_text(
                key(model, KeyCode::Char('j')),
                ".securityAdminGuardrails.organizationConfiguration.accounts[].",
            ),
            KeyCode::Tab,
        )
    }

    pub fn jq_input_preview() -> ExplorerModel {
        let model = type_text(key(result(), KeyCode::Char('j')), ".tags[]");
        update(
            model,
            ExplorerMessage::JqPreviewed(Ok(vec!["dog".into(), "friendly".into()])),
        )
        .0
    }

    pub fn jq_input_examples() -> ExplorerModel {
        key(jq_input_preview(), KeyCode::F(1))
    }

    pub fn jq_input_error() -> ExplorerModel {
        let model = type_text(key(result(), KeyCode::Char('j')), ".tags[");
        update(
            model,
            ExplorerMessage::JqPreviewed(Err(
                "invalid jq filter: expected a closing bracket".into()
            )),
        )
        .0
    }

    /// A filter applied and still running.
    pub fn result_jq_running() -> ExplorerModel {
        key(jq_input(), KeyCode::Enter)
    }

    pub fn result_jq() -> ExplorerModel {
        let model = key(jq_input(), KeyCode::Enter);
        update(
            model,
            ExplorerMessage::JqApplied(Ok(vec!["dog".into(), "friendly".into()])),
        )
        .0
    }

    /// A jq filter that failed.
    pub fn result_jq_error() -> ExplorerModel {
        let model = key(jq_input(), KeyCode::Enter);
        update(
            model,
            ExplorerMessage::JqApplied(Err(
                "invalid jq filter \".tags[\": expected a closing bracket at the end of the filter"
                    .into(),
            )),
        )
        .0
    }

    pub fn help() -> ExplorerModel {
        key(loaded(), KeyCode::Char('?'))
    }

    /// Long ids, paths and summaries, on the list and in the form.
    pub fn long_text() -> ExplorerModel {
        let mut spec = (*spec()).clone();
        let mut long = spec.operations[2].clone();
        long.id =
            "repositories/list-collaborators-with-permissions-for-an-organization-project".into();
        long.path = "/organizations/{organization_id}/projects/{project_id}/collaborators/{collaborator_login}/permissions".into();
        long.summary = Some("List the collaborators of an organization project with the permission level each of them has been granted on the project".into());
        long.external_docs = Some("https://docs.example.com/rest/projects/collaborators#list-collaborators-with-permissions".into());
        spec.operations.push(long);
        let model = update(loading(), ExplorerMessage::SpecLoaded(Ok(Arc::new(spec)))).0;
        key(model, KeyCode::End)
    }

    pub fn long_text_form() -> ExplorerModel {
        key(long_text(), KeyCode::Enter)
    }

    /// Full-width text in summaries, tags and the search.
    pub fn unicode() -> ExplorerModel {
        let mut spec = (*spec()).clone();
        let operations: Vec<Operation> = spec
            .operations
            .iter()
            .cloned()
            .map(|mut operation| {
                operation.summary =
                    Some(format!("ペットを{}", operation.summary.unwrap_or_default()));
                operation.tags = vec!["ペット".into(), "店舗".into()];
                operation
            })
            .collect();
        spec.operations = operations;
        let model = update(loading(), ExplorerMessage::SpecLoaded(Ok(Arc::new(spec)))).0;
        type_text(key(model, KeyCode::Char('/')), "ペット")
    }

    pub fn notice() -> ExplorerModel {
        update(
            loaded(),
            ExplorerMessage::Notice("command copied to the clipboard".into()),
        )
        .0
    }

    /// The form of an operation without inputs.
    pub fn form_no_inputs() -> ExplorerModel {
        let model = key(loaded(), KeyCode::Enter);
        let Screen::Form(FormModel { operation: 0, .. }) = &model.screen else {
            panic!("expected the form of pets/list");
        };
        model
    }

    pub fn form_middle() -> ExplorerModel {
        keys(
            type_text(form(), "ab日本語cd"),
            &[
                KeyCode::Home,
                KeyCode::Right,
                KeyCode::Right,
                KeyCode::Right,
            ],
        )
    }

    pub fn jq_input_middle() -> ExplorerModel {
        keys(jq_input(), &[KeyCode::Home, KeyCode::Right, KeyCode::Right])
    }

    pub fn jq_input_long() -> ExplorerModel {
        let text = r#".items[] | select(.name == "日本語の長い名前を表示する") | .name"#;
        let model = type_text(key(result(), KeyCode::Char('j')), text);
        keys(model, &[KeyCode::Left, KeyCode::Left, KeyCode::Left])
    }

    pub fn searching_middle() -> ExplorerModel {
        key(searching(), KeyCode::Left)
    }

    /// A favorite, a request with a body and one with values, the second
    /// selected.
    pub fn history() -> ExplorerModel {
        use crate::domain::types::request_history::HistoryEntry;
        let entry = |operation: &str, params: &[(&str, &str)], body: Option<&str>| HistoryEntry {
            operation: operation.into(),
            params: params
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: body.map(str::to_string),
            name: None,
        };
        let model = update(
            loaded(),
            ExplorerMessage::HistoryLoaded(vec![
                entry("pets/get", &[("petId", "p1"), ("X-Trace", "t1")], None),
                entry("pets/create", &[], Some("{\"name\":\"rex\"}")),
                HistoryEntry {
                    name: Some("all pets".into()),
                    ..entry("pets/list", &[("limit", "100")], None)
                },
            ]),
        )
        .0;
        keys(model, &[KeyCode::Char('h'), KeyCode::Down])
    }

    /// `s` on the selected entry: its name being typed.
    pub fn history_naming() -> ExplorerModel {
        type_text(key(history(), KeyCode::Char('s')), "new pet")
    }

    /// The history of an API nothing was sent to yet.
    pub fn history_empty() -> ExplorerModel {
        key(loaded(), KeyCode::Char('h'))
    }

    pub fn all() -> Vec<(&'static str, ExplorerModel)> {
        vec![
            ("loading", loading()),
            ("loaded", loaded()),
            ("token_soon", token_soon()),
            ("selected", selected()),
            ("response_shape", response_shape()),
            ("response_shape_large", response_shape_large()),
            ("searching", searching()),
            ("searching_middle", searching_middle()),
            ("empty", empty()),
            ("error", error()),
            ("form", form()),
            ("form_middle", form_middle()),
            ("form_body", form_body()),
            ("form_error", form_error()),
            ("form_long_body_error", form_long_body_error()),
            ("form_no_inputs", form_no_inputs()),
            ("processing", processing()),
            ("result", result()),
            ("result_headers", result_headers()),
            ("result_scrolled", result_scrolled()),
            ("jq_input", jq_input()),
            ("jq_input_completing", jq_input_completing()),
            ("jq_input_preview", jq_input_preview()),
            ("jq_input_examples", jq_input_examples()),
            ("jq_input_error", jq_input_error()),
            ("jq_input_middle", jq_input_middle()),
            ("jq_input_long", jq_input_long()),
            ("result_jq_running", result_jq_running()),
            ("result_jq", result_jq()),
            ("result_jq_error", result_jq_error()),
            ("help", help()),
            ("long_text", long_text()),
            ("long_text_form", long_text_form()),
            ("unicode", unicode()),
            ("notice", notice()),
            ("result_tree", result_tree()),
            ("result_tree_large", result_tree_large()),
            ("history", history()),
            ("history_naming", history_naming()),
            ("history_empty", history_empty()),
        ]
    }
}
