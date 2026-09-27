//! The explorer's request history: what one line of it holds, which values
//! it leaves out (a secret header or parameter, the response), the order the
//! history modal lists them in, and the modal's keys. No I/O: the runtime
//! reads and appends the file through `adapters::request_history`.

use super::model::{FormModel, new_form};
use crate::domain::types::api_spec::{ApiSpec, Operation, Parameter, ParameterLocation};
use crate::domain::types::http::is_secret_header;
pub use crate::domain::types::request_history::HistoryEntry;
use crate::shell::tui::components::LineInput;
use crate::shell::tui::components::list_navigation::moved_selection;
use crossterm::event::{KeyCode, KeyEvent};

/// Unnamed requests the modal lists; favorites are all listed.
pub const MAX_RECENT: usize = 50;

/// Name fragments of a parameter that carries a credential, compared with
/// `-` and `_` removed and in lower case.
const SECRET_NAME_PARTS: [&str; 10] = [
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "credential",
    "signature",
    "authorization",
    "cookie",
    "sessionid",
];

/// Whether a value of this parameter must not be written down.
pub fn is_secret_parameter(parameter: &Parameter) -> bool {
    let compact: String = parameter
        .name
        .chars()
        .filter(|c| !matches!(c, '-' | '_'))
        .collect::<String>()
        .to_ascii_lowercase();
    (parameter.location == ParameterLocation::Header && is_secret_header(&parameter.name))
        || SECRET_NAME_PARTS.iter().any(|part| compact.contains(part))
}

/// The history line of a request: `params` without the secret ones.
pub fn entry_for(
    operation: &Operation,
    params: &[(String, String)],
    body: Option<&str>,
) -> HistoryEntry {
    let secret = |name: &str| {
        operation
            .parameters
            .iter()
            .any(|parameter| parameter.name == name && is_secret_parameter(parameter))
    };
    HistoryEntry {
        operation: operation.id.clone(),
        params: params
            .iter()
            .filter(|(name, _)| !secret(name))
            .cloned()
            .collect(),
        body: body.map(str::to_string),
        name: None,
    }
}

/// What the modal lists, from the file's lines (oldest first): favorites
/// newest first, one per name, then the latest distinct requests, newest
/// first, at most [`MAX_RECENT`].
pub fn listed(entries: &[HistoryEntry]) -> Vec<HistoryEntry> {
    let mut favorites: Vec<HistoryEntry> = Vec::new();
    let mut recent: Vec<HistoryEntry> = Vec::new();
    for entry in entries.iter().rev() {
        match &entry.name {
            Some(name) => {
                if !favorites
                    .iter()
                    .any(|kept| kept.name.as_ref() == Some(name))
                {
                    favorites.push(entry.clone());
                }
            }
            None => {
                if recent.len() < MAX_RECENT && !recent.contains(entry) {
                    recent.push(entry.clone());
                }
            }
        }
    }
    favorites.extend(recent);
    favorites
}

/// The form an entry opens: its operation with the saved values in their
/// fields. `None` when the description no longer has the operation.
pub fn form_from(spec: &ApiSpec, entry: &HistoryEntry) -> Option<FormModel> {
    let index = spec
        .operations
        .iter()
        .position(|operation| operation.id == entry.operation)?;
    let operation = &spec.operations[index];
    let mut form = new_form(index, operation);
    for (parameter, value) in operation.parameters.iter().zip(&mut form.values) {
        if let Some((_, saved)) = entry
            .params
            .iter()
            .find(|(name, _)| *name == parameter.name)
        {
            *value = LineInput::from(saved.as_str());
        }
    }
    if form.body.is_some()
        && let Some(body) = &entry.body
    {
        form.body = Some(body.clone());
    }
    Some(form)
}

/// The history modal: the listed entries, the selected one, and the name
/// being typed for a favorite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryModal {
    pub entries: Vec<HistoryEntry>,
    pub selected: usize,
    pub naming: Option<LineInput>,
}

pub enum HistoryAction {
    Nothing,
    Close,
    Quit,
    Open(HistoryEntry),
    /// Append this favorite.
    Save(HistoryEntry),
}

impl HistoryModal {
    pub fn new(entries: &[HistoryEntry]) -> Self {
        Self {
            entries: listed(entries),
            selected: 0,
            naming: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> HistoryAction {
        if let Some(name) = &mut self.naming {
            match key.code {
                KeyCode::Esc => self.naming = None,
                KeyCode::Enter => {
                    let name = name.as_str().trim().to_string();
                    let Some(entry) = self.entries.get(self.selected) else {
                        return HistoryAction::Nothing;
                    };
                    if name.is_empty() {
                        return HistoryAction::Nothing;
                    }
                    let favorite = HistoryEntry {
                        name: Some(name),
                        ..entry.clone()
                    };
                    self.naming = None;
                    return HistoryAction::Save(favorite);
                }
                _ => {
                    name.handle_key(key);
                }
            }
            return HistoryAction::Nothing;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') => HistoryAction::Close,
            KeyCode::Char('q') => HistoryAction::Quit,
            KeyCode::Enter => match self.entries.get(self.selected) {
                Some(entry) => HistoryAction::Open(entry.clone()),
                None => HistoryAction::Nothing,
            },
            KeyCode::Char('s') if !self.entries.is_empty() => {
                let current = self.entries[self.selected].name.clone();
                self.naming = Some(LineInput::from(current.unwrap_or_default()));
                HistoryAction::Nothing
            }
            code => {
                if let Some(index) = moved_selection(code, self.selected, self.entries.len()) {
                    self.selected = index;
                }
                HistoryAction::Nothing
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tui::explorer::testing::fixtures::spec;
    use crossterm::event::KeyModifiers;

    fn entry(operation: &str, params: &[(&str, &str)], name: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            operation: operation.into(),
            params: params
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: None,
            name: name.map(str::to_string),
        }
    }

    fn press(modal: &mut HistoryModal, code: KeyCode) -> HistoryAction {
        modal.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn parameter(name: &str, location: ParameterLocation) -> Parameter {
        Parameter {
            name: name.into(),
            location,
            required: false,
            schema: Default::default(),
            description: None,
        }
    }

    #[test]
    fn a_secret_header_or_parameter_is_left_out_and_the_rest_is_kept() {
        for (name, location) in [
            ("X-Api-Key", ParameterLocation::Header),
            ("Authorization", ParameterLocation::Header),
            ("access_token", ParameterLocation::Query),
            ("api-key", ParameterLocation::Query),
            ("client_secret", ParameterLocation::Query),
            ("Password", ParameterLocation::Query),
        ] {
            assert!(
                is_secret_parameter(&parameter(name, location)),
                "{name} is a secret"
            );
        }
        for (name, location) in [
            ("X-Trace", ParameterLocation::Header),
            ("petId", ParameterLocation::Path),
            ("limit", ParameterLocation::Query),
            ("author", ParameterLocation::Query),
        ] {
            assert!(
                !is_secret_parameter(&parameter(name, location)),
                "{name} is not a secret"
            );
        }

        let mut operation = spec().operations[2].clone();
        operation
            .parameters
            .push(parameter("X-Api-Key", ParameterLocation::Header));
        let params = [
            ("X-Trace".to_string(), "t1".to_string()),
            ("petId".to_string(), "p1".to_string()),
            ("X-Api-Key".to_string(), "sekret".to_string()),
        ];
        let saved = entry_for(&operation, &params, Some("{}"));
        assert_eq!(
            saved,
            HistoryEntry {
                operation: "pets/get".into(),
                params: params[..2].to_vec(),
                body: Some("{}".into()),
                name: None,
            }
        );
        let line = serde_json::to_string(&saved).unwrap();
        assert!(!line.contains("sekret"), "{line}");
    }

    #[test]
    fn favorites_come_first_once_per_name_then_distinct_requests_newest_first() {
        let lines = vec![
            entry("pets/list", &[("limit", "1")], None),
            entry("pets/get", &[("petId", "p1")], Some("one pet")),
            entry("pets/list", &[("limit", "2")], None),
            entry("pets/list", &[("limit", "1")], None),
            entry("pets/get", &[("petId", "p2")], Some("one pet")),
        ];
        assert_eq!(
            listed(&lines),
            vec![
                entry("pets/get", &[("petId", "p2")], Some("one pet")),
                entry("pets/list", &[("limit", "1")], None),
                entry("pets/list", &[("limit", "2")], None),
            ]
        );
        let many: Vec<_> = (0..MAX_RECENT + 5)
            .map(|index| entry("pets/list", &[("limit", &index.to_string())], None))
            .collect();
        let shown = listed(&many);
        assert_eq!(shown.len(), MAX_RECENT);
        assert_eq!(shown[0].params[0].1, (MAX_RECENT + 4).to_string());
    }

    #[test]
    fn an_entry_fills_its_form_and_an_unknown_operation_opens_none() {
        let spec = spec();
        let form = form_from(
            &spec,
            &entry("pets/get", &[("petId", "p 1"), ("X-Trace", "t1")], None),
        )
        .unwrap();
        assert_eq!(spec.operations[form.operation].id, "pets/get");
        let values: Vec<&str> = form.values.iter().map(LineInput::as_str).collect();
        assert_eq!(values, ["p 1", "t1"]);

        let create = HistoryEntry {
            body: Some("{\"name\":\"rex\"}".into()),
            ..entry("pets/create", &[], None)
        };
        assert_eq!(
            form_from(&spec, &create).unwrap().body.as_deref(),
            Some("{\"name\":\"rex\"}")
        );
        assert!(form_from(&spec, &entry("gone/away", &[], None)).is_none());
    }

    #[test]
    fn s_names_the_selected_entry_and_enter_saves_it_as_a_favorite() {
        let mut modal = HistoryModal::new(&[
            entry("pets/list", &[], None),
            entry("pets/get", &[("petId", "p1")], None),
        ]);
        assert!(matches!(
            press(&mut modal, KeyCode::Down),
            HistoryAction::Nothing
        ));
        assert!(matches!(
            press(&mut modal, KeyCode::Char('s')),
            HistoryAction::Nothing
        ));
        assert!(modal.naming.is_some());
        assert!(
            matches!(press(&mut modal, KeyCode::Enter), HistoryAction::Nothing),
            "an empty name saves nothing"
        );
        for c in "all".chars() {
            press(&mut modal, KeyCode::Char(c));
        }
        match press(&mut modal, KeyCode::Enter) {
            HistoryAction::Save(saved) => {
                assert_eq!(saved, entry("pets/list", &[], Some("all")));
            }
            _ => panic!("Enter saves the named entry"),
        }
        assert!(modal.naming.is_none());
        match press(&mut modal, KeyCode::Enter) {
            HistoryAction::Open(opened) => assert_eq!(opened.operation, "pets/list"),
            _ => panic!("Enter opens the selected entry"),
        }
        assert!(matches!(
            press(&mut modal, KeyCode::Esc),
            HistoryAction::Close
        ));
        assert!(matches!(
            press(&mut modal, KeyCode::Char('q')),
            HistoryAction::Quit
        ));
    }
}
