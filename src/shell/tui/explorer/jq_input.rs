//! jq editing modes: completion, response-shaped examples and session history.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use crate::domain::functions::jq_completion::{self, Candidate, Example};
use crate::domain::types::json_shape::JsonShape;
use crate::shell::tui::components::LineInput;

pub const PREVIEW_LIMIT: usize = 1024 * 1024;
pub const PREVIEW_TIMEOUT: Duration = Duration::from_millis(250);
pub const PREVIEW_TIMEOUT_MESSAGE: &str = "Preview stopped: the filter takes too long.";
/// How long an applied filter may run before the result says it stopped.
/// The filter runs off the event loop meanwhile, so keys still act.
pub const APPLY_TIMEOUT: Duration = Duration::from_secs(5);
pub const APPLY_TIMEOUT_MESSAGE: &str = "the filter ran longer than 5 seconds and was stopped";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Pending,
    Output(Result<Vec<String>, String>),
    Unavailable(&'static str),
}

/// The list under the filter. Completion and examples are alternatives --
/// the view shows one or neither -- so they are one value. Closing the list
/// is then a single assignment: the two fields it replaces could go out of
/// step, and typing cleared only the candidates, so Enter afterwards still
/// took the highlighted example and threw away what had just been typed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JqPanel {
    #[default]
    None,
    Candidates {
        items: Vec<Candidate>,
        selected: usize,
    },
    Examples {
        items: Vec<Example>,
        selected: usize,
    },
}

impl JqPanel {
    pub fn choice_count(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Candidates { items, .. } => items.len(),
            Self::Examples { items, .. } => items.len(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.choice_count() > 0
    }

    fn selected_mut(&mut self) -> Option<&mut usize> {
        match self {
            Self::None => None,
            Self::Candidates { selected, .. } | Self::Examples { selected, .. } => Some(selected),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JqInputModel {
    pub input: LineInput,
    pub panel: JqPanel,
    pub preview: Preview,
    shape: Arc<JsonShape>,
    preview_body: Option<Arc<Value>>,
    history_index: Option<usize>,
    draft: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Stay,
    Changed,
    Close,
    Apply(String),
}

impl JqInputModel {
    pub fn new(filter: String, body: Option<Arc<Value>>, bytes: usize) -> Self {
        Self {
            input: filter.into(),
            panel: JqPanel::None,
            preview: if bytes > PREVIEW_LIMIT {
                Preview::Unavailable("Preview disabled: response exceeds 1 MiB.")
            } else if body.is_none() {
                Preview::Unavailable("Preview unavailable: response is not JSON.")
            } else {
                Preview::Pending
            },
            shape: Arc::new(if bytes > PREVIEW_LIMIT {
                JsonShape::Unknown
            } else {
                body.as_deref()
                    .map_or(JsonShape::Unknown, JsonShape::from_value)
            }),
            preview_body: body,
            history_index: None,
            draft: String::new(),
        }
    }

    pub fn as_str(&self) -> &str {
        self.input.as_str()
    }

    pub fn preview_filter(&self) -> Option<String> {
        if matches!(self.preview, Preview::Unavailable(_)) {
            return None;
        }
        let filter = self.as_str().trim();
        Some(if filter.is_empty() {
            ".".into()
        } else {
            filter.into()
        })
    }

    pub fn preview_body(&self) -> Option<Arc<Value>> {
        self.preview_body.clone()
    }

    pub fn handle_key(&mut self, key: KeyEvent, history: &[String]) -> InputAction {
        match key.code {
            KeyCode::Esc if self.panel.is_open() => self.panel = JqPanel::None,
            KeyCode::Esc => return InputAction::Close,
            KeyCode::Tab => {
                if let JqPanel::Candidates { items, selected } = &mut self.panel {
                    *selected = (*selected + 1) % items.len();
                } else {
                    let items =
                        jq_completion::complete(&self.shape, self.as_str(), self.input.cursor())
                            .candidates;
                    if items.len() == 1 {
                        let only = items.into_iter().next().expect("one candidate");
                        return self.insert(&only);
                    }
                    self.panel = if items.is_empty() {
                        JqPanel::None
                    } else {
                        JqPanel::Candidates { items, selected: 0 }
                    };
                }
            }
            KeyCode::F(1) => {
                self.panel = JqPanel::Examples {
                    items: jq_completion::examples(&self.shape),
                    selected: 0,
                };
            }
            KeyCode::Up | KeyCode::Down if self.panel.is_open() => {
                let count = self.panel.choice_count();
                if let Some(selected) = self.panel.selected_mut() {
                    *selected = move_selection(*selected, count, key.code);
                }
            }
            KeyCode::Enter => match std::mem::take(&mut self.panel) {
                JqPanel::None => return InputAction::Apply(self.as_str().trim().into()),
                JqPanel::Candidates { items, selected } => return self.insert(&items[selected]),
                JqPanel::Examples { items, selected } => {
                    self.input = items[selected].filter.clone().into();
                    return self.changed();
                }
            },
            KeyCode::Up | KeyCode::Down => {
                return self.recall(history, key.code);
            }
            _ => {
                let cursor = self.input.cursor();
                let changed = self.input.handle_key(key);
                if changed || cursor != self.input.cursor() {
                    // One assignment, so neither list can survive an edit.
                    self.panel = JqPanel::None;
                }
                if changed {
                    return self.changed();
                }
            }
        }
        InputAction::Stay
    }

    fn insert(&mut self, candidate: &Candidate) -> InputAction {
        let (text, cursor) = jq_completion::apply(self.as_str(), self.input.cursor(), candidate);
        self.input = LineInput::with_cursor(text, cursor);
        self.panel = JqPanel::None;
        self.changed()
    }

    fn changed(&mut self) -> InputAction {
        self.history_index = None;
        if !matches!(self.preview, Preview::Unavailable(_)) {
            self.preview = Preview::Pending;
        }
        InputAction::Changed
    }

    fn recall(&mut self, history: &[String], code: KeyCode) -> InputAction {
        if history.is_empty() {
            return InputAction::Stay;
        }
        let index = match (code, self.history_index) {
            (KeyCode::Up, None) => {
                self.draft = self.as_str().into();
                Some(history.len() - 1)
            }
            (KeyCode::Up, Some(index)) => Some(index.saturating_sub(1)),
            (KeyCode::Down, Some(index)) if index + 1 < history.len() => Some(index + 1),
            (KeyCode::Down, Some(_)) => None,
            _ => return InputAction::Stay,
        };
        self.input = index
            .map_or_else(|| self.draft.clone(), |index| history[index].clone())
            .into();
        let action = self.changed();
        self.history_index = index;
        action
    }
}

#[cfg(test)]
#[path = "jq_input_tests.rs"]
mod tests;

fn move_selection(current: usize, length: usize, code: KeyCode) -> usize {
    if code == KeyCode::Up {
        current.saturating_sub(1)
    } else {
        (current + 1).min(length - 1)
    }
}
