//! The home screen's tabs other than AWS: the `[auth.*]`, `[api.*]`,
//! `[db.*]`, `[data.*]` and `[s3.*]` rows, built from the facts `kurama status`
//! reports, and what their keys do. Pure: the runtime gathers the facts.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent};

use super::effects::TuiEffect;
use super::update::{Handoff, Screen, TuiModel, UpdateResult};
use crate::adapters::config::{ApiProfile, SpecSource};
use crate::domain::functions::auth_status::{AuthStatus, TokenState, describe_token};
use crate::domain::types::AuthKind;
use crate::shell::cli::commands::data_contract::DataStatusRow;
use crate::shell::cli::commands::db_contract::DbStatusRow;
use crate::shell::cli::commands::s3_status::S3StatusRow;
use crate::shell::tui::components::list_navigation::moved_selection;

/// One tab of the home screen, in the order `1`-`6` select them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Aws,
    Auth,
    Api,
    Db,
    Data,
    S3,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Aws, Tab::Auth, Tab::Api, Tab::Db, Tab::Data, Tab::S3];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Aws => "AWS",
            Tab::Auth => "Auth",
            Tab::Api => "API",
            Tab::Db => "DB",
            Tab::Data => "Data",
            Tab::S3 => "S3",
        }
    }

    /// The tab a key selects: `1`-`6`, `Tab` for the next, `Shift-Tab` for
    /// the previous.
    pub fn for_key(self, key: KeyCode) -> Option<Tab> {
        let index = Tab::ALL.iter().position(|tab| *tab == self)?;
        let count = Tab::ALL.len();
        match key {
            KeyCode::Char(digit @ '1'..='6') => Some(Tab::ALL[digit as usize - '1' as usize]),
            KeyCode::Tab => Some(Tab::ALL[(index + 1) % count]),
            KeyCode::BackTab => Some(Tab::ALL[(index + count - 1) % count]),
            _ => None,
        }
    }
}

/// What Enter does on a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enter {
    /// Leave the screen and run the `kurama` arguments, then come back.
    Run(Vec<String>),
    /// Copy an example command to the clipboard.
    Copy(String),
    /// Nothing to do; the notice says why.
    Explain(String),
}

/// One row of a source tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    pub name: String,
    /// The KIND column: `auth`, `api`, `db`, `data`, `s3`.
    pub kind: &'static str,
    /// The STATE column, as `kurama status` prints it.
    pub state: String,
    /// The shell holds this source's credential.
    pub active: bool,
    /// Label and value rows of the detail pane.
    pub details: Vec<(&'static str, String)>,
    /// What Enter does, in words, for the detail pane.
    pub next: String,
    pub enter: Enter,
}

/// The rows of one source tab and the selected one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceList {
    pub rows: Vec<SourceRow>,
    pub selected: usize,
}

/// Every source tab's rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sources {
    pub auth: SourceList,
    pub api: SourceList,
    pub db: SourceList,
    pub data: SourceList,
    pub s3: SourceList,
}

impl Sources {
    pub fn list(&self, tab: Tab) -> Option<&SourceList> {
        match tab {
            Tab::Aws => None,
            Tab::Auth => Some(&self.auth),
            Tab::Api => Some(&self.api),
            Tab::Db => Some(&self.db),
            Tab::Data => Some(&self.data),
            Tab::S3 => Some(&self.s3),
        }
    }

    pub(super) fn list_mut(&mut self, tab: Tab) -> Option<&mut SourceList> {
        match tab {
            Tab::Aws => None,
            Tab::Auth => Some(&mut self.auth),
            Tab::Api => Some(&mut self.api),
            Tab::Db => Some(&mut self.db),
            Tab::Data => Some(&mut self.data),
            Tab::S3 => Some(&mut self.s3),
        }
    }
}

fn or_none(value: Option<&str>) -> String {
    value.unwrap_or("-").to_string()
}

fn describe_expiry(token: &TokenState) -> String {
    match token {
        TokenState::Valid {
            expires_at: Some(at),
        } => at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        _ => "-".to_string(),
    }
}

impl SourceRow {
    pub fn from_auth(status: &AuthStatus, now: DateTime<Utc>) -> Self {
        let (grant_or_header, enter, next) = match status.kind {
            AuthKind::OAuth => (
                (
                    "Grant",
                    or_none(status.grant_type.map(|grant| grant.as_str())),
                ),
                Enter::Run(vec!["login".into(), status.name.clone()]),
                format!("Enter runs kurama login {}", status.name),
            ),
            AuthKind::Token => {
                let why = format!(
                    "{} uses a credential issued elsewhere; there is nothing to log in to",
                    status.name
                );
                (
                    ("Header", or_none(status.header.as_deref())),
                    Enter::Explain(why.clone()),
                    why,
                )
            }
        };
        let state = describe_token(&status.token, now);
        Self {
            name: status.name.clone(),
            kind: "auth",
            state: state.clone(),
            active: status.active,
            details: vec![
                ("Source", status.name.clone()),
                ("Kind", status.kind.as_str().to_string()),
                grant_or_header,
                ("Env var", status.env_var.clone()),
                ("Token", state),
                ("Expires", describe_expiry(&status.token)),
                (
                    "Shell",
                    if status.active {
                        "holds its token"
                    } else {
                        "not active"
                    }
                    .into(),
                ),
            ],
            next,
            enter,
        }
    }

    pub fn from_api(profile: &ApiProfile, auth: Option<&AuthStatus>, now: DateTime<Utc>) -> Self {
        let openapi = match profile.spec.as_ref().map(|spec| &spec.source) {
            Some(SpecSource::Url(url)) => Some(url.clone()),
            Some(SpecSource::File(path)) => Some(path.clone()),
            None => None,
        };
        let (enter, next) = if openapi.is_some() {
            (
                Enter::Run(vec!["api".into(), profile.name.clone()]),
                "Enter opens the explorer".to_string(),
            )
        } else {
            let why = format!(
                "[api.{}] has no openapi description to explore",
                profile.name
            );
            (Enter::Explain(why.clone()), why)
        };
        let state = auth.map_or_else(|| "-".to_string(), |auth| describe_token(&auth.token, now));
        Self {
            name: profile.name.clone(),
            kind: "api",
            state: state.clone(),
            active: false,
            details: vec![
                ("API", profile.name.clone()),
                ("Base URL", profile.base_url.clone()),
                ("About", or_none(profile.description.as_deref())),
                ("Auth", or_none(profile.auth.as_deref())),
                ("Token", state),
                ("AWS profile", or_none(profile.aws_profile.as_deref())),
                ("OpenAPI", or_none(openapi.as_deref())),
            ],
            next,
            enter,
        }
    }

    pub fn from_db(status: &DbStatusRow) -> Self {
        Self {
            name: status.name.clone(),
            kind: "db",
            state: "unknown".into(),
            active: false,
            details: vec![
                ("Database", status.name.clone()),
                ("Engine", status.engine.as_str().to_string()),
                ("Name", status.database.clone()),
                ("Host", or_none(status.host.as_deref())),
                ("Allow write", status.allow_write.to_string()),
            ],
            next: "Enter opens the database explorer".into(),
            enter: Enter::Run(vec!["db".into(), status.name.clone()]),
        }
    }

    pub fn from_data(status: &DataStatusRow) -> Self {
        let example = format!("kurama data {} --tables", status.name);
        Self {
            name: status.name.clone(),
            kind: "data",
            state: "not_checked".into(),
            active: false,
            details: vec![
                ("Workspace", status.name.clone()),
                ("Engine", "duckdb".into()),
                ("Sources", status.sources.len().to_string()),
                ("AWS profile", or_none(status.aws_profile.as_deref())),
                ("Region", or_none(status.region.as_deref())),
            ],
            next: format!("Enter copies {example}"),
            enter: Enter::Copy(example),
        }
    }

    /// `session` is the SESSION the AWS tab shows for the connection's
    /// `aws_profile`, or `None` when `~/.aws/config` has no such profile.
    pub fn from_s3(status: &S3StatusRow, session: Option<&str>) -> Self {
        let state = session.unwrap_or("unknown profile").to_string();
        Self {
            name: status.name.clone(),
            kind: "s3",
            state: state.clone(),
            active: false,
            details: vec![
                ("Connection", status.name.clone()),
                ("AWS profile", status.aws_profile.clone()),
                ("Session", state),
                ("Region", or_none(status.region.as_deref())),
                (
                    "Start",
                    status
                        .start()
                        .unwrap_or_else(|| "the bucket list".to_string()),
                ),
            ],
            next: "Enter opens the S3 explorer".into(),
            enter: Enter::Run(vec!["s3".into(), status.name.clone()]),
        }
    }
}

/// Keys on a source tab: move, Enter, help. Everything else does nothing.
pub fn update_source_key(mut model: TuiModel, key: KeyEvent) -> UpdateResult {
    let tab = model.tab;
    let Some(list) = model.sources.list_mut(tab) else {
        return UpdateResult::no_effect(model);
    };
    if let Some(index) = moved_selection(key.code, list.selected, list.rows.len()) {
        list.selected = index;
        return UpdateResult::no_effect(model);
    }
    match key.code {
        KeyCode::Enter => {
            let Some(row) = list.rows.get(list.selected) else {
                return UpdateResult::no_effect(model);
            };
            match row.enter.clone() {
                Enter::Run(args) => {
                    model.handoff = Some(Handoff::Run(args));
                    model.should_exit = true;
                    UpdateResult::with_effect(model, TuiEffect::Exit)
                }
                Enter::Copy(text) => {
                    UpdateResult::with_effect(model, TuiEffect::CopyToClipboard { text })
                }
                Enter::Explain(why) => {
                    model.notice = Some(why);
                    UpdateResult::no_effect(model)
                }
            }
        }
        KeyCode::F(1) | KeyCode::Char('?') => {
            model.screen = Screen::Help;
            UpdateResult::no_effect(model)
        }
        _ => UpdateResult::no_effect(model),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::DbEngine;

    fn api(openapi: Option<SpecSource>) -> ApiProfile {
        ApiProfile {
            name: "pets".into(),
            description: None,
            base_url: "https://pets.example".into(),
            auth: None,
            aws_profile: None,
            signing: Default::default(),
            spec: openapi.map(|source| crate::adapters::config::ApiDescription {
                format: crate::domain::types::api_spec::SpecFormat::OpenApi,
                source,
            }),
            openapi_auth: false,
            headers: Default::default(),
        }
    }

    #[test]
    fn enter_follows_what_each_source_can_do() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let described =
            SourceRow::from_api(&api(Some(SpecSource::File("/a.json".into()))), None, now);
        assert_eq!(
            described.enter,
            Enter::Run(vec!["api".into(), "pets".into()])
        );
        assert_eq!(described.state, "-");
        let bare = SourceRow::from_api(&api(None), None, now);
        assert!(matches!(bare.enter, Enter::Explain(_)), "{:?}", bare.enter);

        let db = SourceRow::from_db(&DbStatusRow {
            name: "app".into(),
            engine: DbEngine::Sqlite,
            database: "/tmp/app.db".into(),
            host: None,
            allow_write: false,
        });
        assert_eq!(db.enter, Enter::Run(vec!["db".into(), "app".into()]));
        assert_eq!(db.state, "unknown");

        let data = SourceRow::from_data(&DataStatusRow {
            name: "logs".into(),
            sources: Vec::new(),
            aws_profile: None,
            region: None,
        });
        assert_eq!(data.enter, Enter::Copy("kurama data logs --tables".into()));
        assert_eq!(data.state, "not_checked");

        let connection = S3StatusRow {
            name: "assets".into(),
            aws_profile: "dev".into(),
            region: None,
            bucket: None,
            prefix: None,
        };
        let s3 = SourceRow::from_s3(&connection, Some("valid (11h 59m)"));
        assert_eq!(s3.enter, Enter::Run(vec!["s3".into(), "assets".into()]));
        // The session of its AWS profile, as the AWS tab prints it.
        assert_eq!((s3.kind, s3.state.as_str()), ("s3", "valid (11h 59m)"));
        assert!(s3.details.contains(&("Session", "valid (11h 59m)".into())));
        let unknown = SourceRow::from_s3(&connection, None);
        assert_eq!(unknown.state, "unknown profile");
    }
}
