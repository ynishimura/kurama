//! State and keys of the activity monitor (`kurama audit --watch`): the audit log's entries, newest first, the selected one, what a key does, and the command that makes a call again. Pure: the runtime reads the log and runs the copy.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::domain::functions::operation_command::shell_quote;
use crate::domain::types::audit::AuditEntry;
use crate::shell::tui::components::list_navigation::moved_selection;

/// Which side has the keys; on a compact terminal it is the side on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    List,
    Detail,
}

#[derive(Debug, Clone, Default)]
pub struct ActivityModel {
    /// Every entry of the log, newest first.
    pub entries: Vec<AuditEntry>,
    /// The selected entry. At 0 it follows the newest one as calls arrive;
    /// anywhere else it stays on the entry it selects.
    pub selected: usize,
    /// The log has been read once.
    pub loaded: bool,
    /// Why the last read failed; the entries of the read before stay.
    pub read_error: Option<String>,
    pub focus: Focus,
    /// One line in the header until the next key.
    pub notice: Option<String>,
    /// `[api.<name>] base_url`'s path per API, for the command that makes an
    /// `api` call again.
    pub base_paths: BTreeMap<String, String>,
    pub should_exit: bool,
}

#[derive(Debug)]
pub enum ActivityMessage {
    Key(KeyEvent),
    /// The log as read, oldest first.
    Read(Result<Vec<AuditEntry>, String>),
    /// The command copied, or why it was not.
    Copied(Result<String, String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ActivityEffect {
    CopyToClipboard { text: String },
}

impl ActivityModel {
    pub fn selected_entry(&self) -> Option<&AuditEntry> {
        self.entries.get(self.selected)
    }

    /// The command a person runs to make the selected call again.
    pub fn rerun(&self, entry: &AuditEntry) -> Option<String> {
        rerun_command(
            entry,
            self.base_paths.get(&entry.target).map(String::as_str),
        )
    }
}

/// The command a person runs to make the same call again, or `None` for a
/// call that cannot be named from its entry (an `api` call that failed
/// before it had a request, a `data` run). The entry keeps no query, body,
/// argument or SQL, so the command has none either: `api` names the method
/// and the path under the API's `base_url` (`base_path` is that URL's path;
/// the entry's path is the whole URL's), `exec` the program, `db` the
/// database. It carries no `--confirm`: a person's run is not an agent's.
pub fn rerun_command(entry: &AuditEntry, base_path: Option<&str>) -> Option<String> {
    let target = shell_quote(&entry.target);
    match entry.command.as_str() {
        "api" => {
            let (method, path) = (entry.method.as_deref()?, entry.path.as_deref()?);
            let base = base_path.unwrap_or_default().trim_end_matches('/');
            let path = match path.strip_prefix(base) {
                Some("") => "/",
                Some(rest) if rest.starts_with('/') => rest,
                _ => path,
            };
            let method = if method == "GET" {
                String::new()
            } else {
                format!(" -X {}", shell_quote(method))
            };
            Some(format!("kurama api {target}{method} {}", shell_quote(path)))
        }
        "exec" => Some(format!(
            "kurama exec {target} -- {}",
            shell_quote(entry.program.as_deref()?)
        )),
        "db" => Some(format!("kurama db {target}")),
        _ => None,
    }
}

pub fn update(model: &mut ActivityModel, message: ActivityMessage) -> Vec<ActivityEffect> {
    match message {
        ActivityMessage::Key(key) => update_key(model, key),
        ActivityMessage::Read(Ok(mut entries)) => {
            entries.reverse();
            let kept = model.selected_entry().filter(|_| model.selected > 0);
            model.selected = match kept {
                Some(kept) => entries
                    .iter()
                    .position(|entry| entry == kept)
                    .unwrap_or(model.selected),
                None => 0,
            }
            .min(entries.len().saturating_sub(1));
            model.entries = entries;
            model.loaded = true;
            model.read_error = None;
            Vec::new()
        }
        ActivityMessage::Read(Err(error)) => {
            model.loaded = true;
            model.read_error = Some(error);
            Vec::new()
        }
        ActivityMessage::Copied(result) => {
            model.notice = Some(match result {
                Ok(text) => format!("Copied: {text}"),
                Err(error) => format!("Could not copy: {error}"),
            });
            Vec::new()
        }
    }
}

fn update_key(model: &mut ActivityModel, key: KeyEvent) -> Vec<ActivityEffect> {
    model.notice = None;
    if key.code == KeyCode::Char('q')
        || (key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL)
    {
        model.should_exit = true;
        return Vec::new();
    }
    if let Some(index) = moved_selection(key.code, model.selected, model.entries.len()) {
        model.selected = index;
        return Vec::new();
    }
    match key.code {
        KeyCode::Enter if model.selected_entry().is_some() => model.focus = Focus::Detail,
        KeyCode::Esc => model.focus = Focus::List,
        KeyCode::Char('y') => {
            if let Some(entry) = model.selected_entry() {
                match model.rerun(entry) {
                    Some(text) => return vec![ActivityEffect::CopyToClipboard { text }],
                    None => {
                        model.notice =
                            Some("This entry does not name a call to make again".to_owned())
                    }
                }
            }
        }
        _ => {}
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn entry(second: i64, path: &str) -> AuditEntry {
        AuditEntry {
            time: chrono::DateTime::from_timestamp(1_800_000_000 + second, 0).unwrap(),
            command: "api".into(),
            target: "pets".into(),
            agent: true,
            method: Some("GET".into()),
            path: Some(path.into()),
            status: Some(200),
            program: None,
            sql_sha256: None,
            exit_code: Some(0),
            error_code: None,
            duration_ms: 5,
        }
    }

    fn key(code: KeyCode) -> ActivityMessage {
        ActivityMessage::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn paths(model: &ActivityModel) -> Vec<&str> {
        model
            .entries
            .iter()
            .map(|entry| entry.path.as_deref().unwrap())
            .collect()
    }

    #[test]
    fn the_newest_call_is_first_and_selected_as_calls_arrive() {
        let mut model = ActivityModel::default();
        update(
            &mut model,
            ActivityMessage::Read(Ok(vec![entry(1, "/a"), entry(2, "/b")])),
        );
        assert!(model.loaded);
        assert_eq!(paths(&model), ["/b", "/a"]);
        assert_eq!(model.selected, 0);
        update(
            &mut model,
            ActivityMessage::Read(Ok(vec![entry(1, "/a"), entry(2, "/b"), entry(3, "/c")])),
        );
        assert_eq!(model.selected_entry().unwrap().path.as_deref(), Some("/c"));
    }

    #[test]
    fn a_selection_below_the_newest_stays_on_its_entry() {
        let mut model = ActivityModel::default();
        update(
            &mut model,
            ActivityMessage::Read(Ok(vec![entry(1, "/a"), entry(2, "/b")])),
        );
        update(&mut model, key(KeyCode::Down));
        assert_eq!(model.selected_entry().unwrap().path.as_deref(), Some("/a"));
        update(
            &mut model,
            ActivityMessage::Read(Ok(vec![entry(1, "/a"), entry(2, "/b"), entry(3, "/c")])),
        );
        assert_eq!(model.selected, 2);
        assert_eq!(model.selected_entry().unwrap().path.as_deref(), Some("/a"));
        // A rotation dropped it: the selection stays in the list.
        update(&mut model, ActivityMessage::Read(Ok(vec![entry(3, "/c")])));
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn a_failed_read_keeps_the_entries_it_had() {
        let mut model = ActivityModel::default();
        update(&mut model, ActivityMessage::Read(Ok(vec![entry(1, "/a")])));
        update(&mut model, ActivityMessage::Read(Err("denied".into())));
        assert_eq!(model.read_error.as_deref(), Some("denied"));
        assert_eq!(paths(&model), ["/a"]);
        update(&mut model, ActivityMessage::Read(Ok(vec![entry(1, "/a")])));
        assert_eq!(model.read_error, None);
    }

    #[test]
    fn y_copies_the_command_under_the_apis_base_path() {
        let mut model = ActivityModel {
            base_paths: [("pets".to_owned(), "/v1".to_owned())].into(),
            ..Default::default()
        };
        update(
            &mut model,
            ActivityMessage::Read(Ok(vec![entry(1, "/v1/pets")])),
        );
        assert_eq!(
            update(&mut model, key(KeyCode::Char('y'))),
            [ActivityEffect::CopyToClipboard {
                text: "kurama api pets /pets".into()
            }]
        );
        update(
            &mut model,
            ActivityMessage::Copied(Ok("kurama api pets /pets".into())),
        );
        assert_eq!(
            model.notice.as_deref(),
            Some("Copied: kurama api pets /pets")
        );
        update(&mut model, key(KeyCode::Down));
        assert_eq!(model.notice, None, "a key clears the notice");
        let no_request = AuditEntry {
            method: None,
            path: None,
            ..entry(2, "/")
        };
        update(&mut model, ActivityMessage::Read(Ok(vec![no_request])));
        assert!(update(&mut model, key(KeyCode::Char('y'))).is_empty());
        assert!(model.notice.is_some());
    }

    #[test]
    fn enter_shows_the_details_esc_the_list_and_q_quits() {
        let mut model = ActivityModel::default();
        update(&mut model, key(KeyCode::Enter));
        assert_eq!(model.focus, Focus::List, "nothing to show");
        update(&mut model, ActivityMessage::Read(Ok(vec![entry(1, "/a")])));
        update(&mut model, key(KeyCode::Enter));
        assert_eq!(model.focus, Focus::Detail);
        update(&mut model, key(KeyCode::Esc));
        assert_eq!(model.focus, Focus::List);
        assert!(!model.should_exit);
        update(&mut model, key(KeyCode::Char('q')));
        assert!(model.should_exit);
    }

    #[rstest::rstest]
    #[case("GET", "/api/pets/1", Some("/api"), "kurama api pets /pets/1")]
    #[case("GET", "/api/pets/1", Some("/api/"), "kurama api pets /pets/1")]
    #[case(
        "DELETE",
        "/api/pets/1",
        Some("/api"),
        "kurama api pets -X DELETE /pets/1"
    )]
    #[case("GET", "/v2/pets", None, "kurama api pets /v2/pets")]
    #[case("GET", "/v2/pets", Some("/"), "kurama api pets /v2/pets")]
    // A base path that is only a prefix of the first segment is not cut.
    #[case("GET", "/apis/x", Some("/api"), "kurama api pets /apis/x")]
    #[case("GET", "/api", Some("/api"), "kurama api pets /")]
    #[case("GET", "/it's", None, "kurama api pets '/it'\\''s'")]
    fn an_api_call_is_made_again_under_its_base_url(
        #[case] method: &str,
        #[case] path: &str,
        #[case] base_path: Option<&str>,
        #[case] command: &str,
    ) {
        let entry = AuditEntry {
            method: Some(method.to_owned()),
            ..entry(1, path)
        };
        assert_eq!(rerun_command(&entry, base_path).as_deref(), Some(command));
    }

    #[test]
    fn exec_and_db_are_made_again_without_what_the_entry_does_not_keep() {
        let exec = AuditEntry {
            command: "exec".into(),
            target: "dev".into(),
            method: None,
            path: None,
            program: Some("aws".into()),
            ..entry(1, "/")
        };
        assert_eq!(
            rerun_command(&exec, None).as_deref(),
            Some("kurama exec dev -- aws")
        );
        let db = AuditEntry {
            command: "db".into(),
            target: "my app".into(),
            method: None,
            path: None,
            ..entry(1, "/")
        };
        assert_eq!(
            rerun_command(&db, None).as_deref(),
            Some("kurama db 'my app'")
        );
        let no_request = AuditEntry {
            method: None,
            path: None,
            ..entry(1, "/")
        };
        assert_eq!(rerun_command(&no_request, None), None);
        let data = AuditEntry {
            command: "data".into(),
            ..entry(1, "/")
        };
        assert_eq!(rerun_command(&data, None), None);
    }
}
