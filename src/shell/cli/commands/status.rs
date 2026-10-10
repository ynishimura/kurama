//! `kurama status [PROFILE] [--json]`: every credential source and API with
//! its state, and what the shell holds.
//!
//! AWS profiles show their MFA session, `[auth.*]` sources of kind `oauth`
//! their stored token, `[api.*]` profiles the state of the source they use.
//! A source of kind `token` keeps its credential in a secret store, which
//! nothing here reads, so its state is `not_checked`. Reads
//! `~/.aws/config`, `config.toml`, the session cache, the token store,
//! `KURAMA_AWS` and `KURAMA_AUTH`; never calls AWS, 1Password or a token
//! endpoint. The table is plain text so agents and pipes can read it.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::warn;

use super::source::check_name_collisions;
use crate::adapters::config::{ApiProfile, Config};
use crate::adapters::keychain::log_denied;
use crate::adapters::profile::load_profiles;
use crate::adapters::session_cache::create_session_cache;
use crate::adapters::token_store::create_token_store;
use crate::domain::Profile;
use crate::domain::functions::auth_status::{
    AuthDetail, AuthStatus, TokenLookup, TokenState, auth_status, describe_token,
};
use crate::domain::functions::export::{ACTIVE_AUTH_VAR, ACTIVE_AWS_PROFILE_VAR};
use crate::domain::functions::profile_status::{
    ProfileStatus, SessionLookup, SessionState, StatusInputs, describe_session, profile_status,
};
use crate::domain::types::AuthSource;
use crate::ports::{SessionCacheError, TokenStore, TokenStoreError};
use crate::shell::cli::executor::CliExecutorError;

/// One row of `kurama status`.
#[derive(Debug, Clone)]
pub enum StatusRow {
    Aws(ProfileStatus),
    Auth(AuthStatus),
    Api {
        profile: Box<ApiProfile>,
        /// The state of the source the API uses, when it has one.
        auth: Option<AuthStatus>,
    },
    Data(super::data_contract::DataStatusRow),
    Db(super::db_contract::DbStatusRow),
    S3(super::s3_status::S3StatusRow),
}

impl StatusRow {
    /// What the `KIND` column and the JSON `kind` field say, and what `--only`
    /// compares against: one name per row, derived here so a filter cannot
    /// disagree with what the row prints.
    pub fn kind(&self) -> StatusRowKind {
        match self {
            Self::Aws(_) => StatusRowKind::Aws,
            Self::Auth(_) => StatusRowKind::Auth,
            Self::Api { .. } => StatusRowKind::Api,
            Self::Data(_) => StatusRowKind::Data,
            Self::Db(_) => StatusRowKind::Db,
            Self::S3(_) => StatusRowKind::S3,
        }
    }
}

/// The kind of one status row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusRowKind {
    Aws,
    Auth,
    Api,
    Data,
    Db,
    S3,
}

impl StatusRowKind {
    /// What `--only` accepts. `db` is not among them: plain `kurama status`
    /// builds no database row, so selecting it could only ever print a header.
    /// `kurama status --kind db` is what lists databases.
    pub const SELECTABLE: [Self; 5] = [Self::Aws, Self::Auth, Self::Api, Self::Data, Self::S3];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Auth => "auth",
            Self::Api => "api",
            Self::Data => "data",
            Self::Db => "db",
            Self::S3 => "s3",
        }
    }

    /// The values `--only` offers, in the order the rows themselves come out.
    pub fn selectable_values() -> Vec<&'static str> {
        Self::SELECTABLE.iter().map(|kind| kind.as_str()).collect()
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::SELECTABLE
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

pub async fn handle_status_command(
    profile: Option<&str>,
    only: Option<StatusRowKind>,
    json: bool,
    config: &Config,
) -> Result<()> {
    let mut profiles = load_profiles().await?;
    check_name_collisions(&profiles, config)?;
    let mut clients = config.auth_sources();
    let mut apis = config.api_profiles();
    if let Some(name) = profile {
        apis.retain(|a| a.name == name);
        profiles.retain(|p| {
            p.name() == name
                || apis
                    .iter()
                    .any(|a| a.aws_profile.as_deref() == Some(p.name()))
        });
        // The source a selected API uses is read too, so the API row
        // carries its real state; only the auth rows shown are filtered.
        clients.retain(|c| {
            c.name() == name || apis.iter().any(|a| a.auth.as_deref() == Some(c.name()))
        });
        if profiles.is_empty()
            && clients.is_empty()
            && apis.is_empty()
            && !config.data.contains_key(name)
            && !config.s3.contains_key(name)
        {
            return Err(CliExecutorError::ProfileNotFound(name.to_string()).into());
        }
    }
    profiles.sort_by(|a, b| a.name().cmp(b.name()));

    // A store is read only for the rows `--only` keeps: the session cache for
    // the aws rows, the token store for the auth rows and the api rows that
    // carry their source's state.
    let shows = |kind| only.is_none_or(|only| only == kind);
    if !shows(StatusRowKind::Aws) {
        profiles.clear();
    }
    if !shows(StatusRowKind::Auth) && !shows(StatusRowKind::Api) {
        clients.clear();
    }

    let now = Utc::now();
    let mut rows: Vec<StatusRow> = gather_profile_statuses(&profiles, config, now)
        .await
        .into_iter()
        .map(StatusRow::Aws)
        .collect();
    let mut auth_statuses =
        gather_auth_statuses(&clients, create_token_store().as_ref(), now).await;
    let api_rows: Vec<StatusRow> = apis
        .into_iter()
        .map(|api| {
            let auth = api
                .auth
                .as_ref()
                .and_then(|name| auth_statuses.get(name).cloned());
            StatusRow::Api {
                profile: Box::new(api),
                auth,
            }
        })
        .collect();
    if let Some(name) = profile {
        auth_statuses.retain(|source, _| source == name);
    }
    rows.extend(auth_statuses.into_values().map(StatusRow::Auth));
    rows.extend(api_rows);

    rows.extend(
        super::data_contract::status_rows(config, profile)
            .into_iter()
            .map(StatusRow::Data),
    );
    rows.extend(
        super::s3_status::status_rows(config, profile)
            .into_iter()
            .map(StatusRow::S3),
    );
    if let Some(only) = only {
        rows.retain(|row| row.kind() == only);
    }
    print_rows(&rows, now, json);
    Ok(())
}

/// The rows as the JSON document or as the table, on stdout.
pub fn print_rows(rows: &[StatusRow], now: DateTime<Utc>, json: bool) {
    let output = if json {
        render_status_json(rows)
    } else {
        render_status_table(rows, now)
    };
    print!("{output}");
}

/// Status of each AWS profile from the session cache and `KURAMA_AWS`. A
/// session that cannot be read is reported as unreadable instead of failing.
pub async fn gather_profile_statuses(
    profiles: &[Profile],
    config: &Config,
    now: DateTime<Utc>,
) -> Vec<ProfileStatus> {
    let session_cache_enabled = config.aws.session_cache.enabled;
    let mut sessions = BTreeMap::new();
    if session_cache_enabled {
        let cache = create_session_cache(true);
        let serials: BTreeSet<&str> = profiles
            .iter()
            .filter_map(Profile::mfa_serial_raw)
            .collect();
        for serial in serials {
            let lookup = match cache.load(serial).await {
                Ok(Some(session)) => SessionLookup::Cached {
                    expires_at: session.expiration,
                },
                Ok(None) => continue,
                Err(SessionCacheError::Denied(denied)) => {
                    log_denied(&format!("MFA session {serial}"), denied);
                    SessionLookup::Unreadable
                }
                Err(error) => {
                    warn!(serial, "Failed to read the cached MFA session: {error}");
                    SessionLookup::Unreadable
                }
            };
            sessions.insert(serial.to_string(), lookup);
        }
    }
    let active_profile = std::env::var(ACTIVE_AWS_PROFILE_VAR).ok();
    let inputs = StatusInputs {
        sessions: &sessions,
        active_profile: active_profile.as_deref(),
        session_cache_enabled,
        mfa_provider_enabled: config.onepassword.enabled,
        now,
    };
    profiles
        .iter()
        .map(|profile| profile_status(profile, &inputs))
        .collect()
}

/// Status of each `[auth.*]` source from the token store and `KURAMA_AUTH`,
/// by name. A token that cannot be read is reported as unreadable.
pub async fn gather_auth_statuses(
    clients: &[AuthSource],
    store: &dyn TokenStore,
    now: DateTime<Utc>,
) -> BTreeMap<String, AuthStatus> {
    let active_auth = std::env::var(ACTIVE_AUTH_VAR).ok();
    let mut statuses = BTreeMap::new();
    for client in clients {
        // Only a grant writes a token store entry, so a `kind = "token"` or
        // `kind = "secrets"` source is not looked up: the store answers
        // nothing for it, and asking would be a keychain read for an answer
        // that is already known.
        let lookup = match client {
            AuthSource::Token(_) | AuthSource::Secrets(_) => None,
            AuthSource::OAuth(_) => match store.load(client.name()).await {
                Ok(Some(token)) => Some(TokenLookup::Stored {
                    expires_at: token.expires_at,
                    refreshable: token.refresh_token.is_some(),
                }),
                Ok(None) => None,
                Err(TokenStoreError::Denied(denied)) => {
                    log_denied(&format!("token of [auth.{}]", client.name()), denied);
                    Some(TokenLookup::Unreadable)
                }
                Err(error) => {
                    warn!(
                        name = client.name(),
                        "Failed to read the stored token: {error}"
                    );
                    Some(TokenLookup::Unreadable)
                }
            },
        };
        statuses.insert(
            client.name().to_string(),
            auth_status(client, lookup.as_ref(), active_auth.as_deref(), now),
        );
    }
    statuses
}

/// Aligned plain-text table; `*` marks what the shell holds.
pub fn render_status_table(rows: &[StatusRow], now: DateTime<Utc>) -> String {
    let header = [
        "KIND".to_string(),
        "PROFILE".to_string(),
        "SESSION".to_string(),
        "DETAIL".to_string(),
    ];
    let cells: Vec<(bool, [String; 4])> = rows
        .iter()
        .map(|row| {
            let kind = row.kind().as_str().to_string();
            match row {
                StatusRow::Aws(status) => (
                    status.active,
                    [
                        kind.clone(),
                        status.name.clone(),
                        describe_session(&status.session, now),
                        status
                            .role_arn
                            .clone()
                            .unwrap_or_else(|| "iam user (no role_arn)".to_string()),
                    ],
                ),
                StatusRow::Auth(status) => (
                    status.active,
                    [
                        kind.clone(),
                        status.name.clone(),
                        describe_token(&status.token, now),
                        describe_auth_source(status),
                    ],
                ),
                StatusRow::Api { profile, auth } => (
                    false,
                    [
                        kind.clone(),
                        profile.name.clone(),
                        auth.as_ref().map_or_else(
                            || "-".to_string(),
                            |auth| describe_token(&auth.token, now),
                        ),
                        match (&profile.auth, &profile.aws_profile) {
                            (Some(name), _) => format!("{} auth={name}", profile.base_url),
                            (None, Some(name)) => {
                                format!("{} aws_profile={name}", profile.base_url)
                            }
                            (None, None) => profile.base_url.clone(),
                        },
                    ],
                ),
                StatusRow::Data(status) => (
                    false,
                    [
                        kind.clone(),
                        status.name.clone(),
                        "not_checked".to_string(),
                        format!("duckdb sources={}", status.sources.len()),
                    ],
                ),
                StatusRow::S3(status) => (
                    false,
                    [
                        kind.clone(),
                        status.name.clone(),
                        "not_checked".to_string(),
                        match status.start() {
                            Some(start) => format!("{start} aws_profile={}", status.aws_profile),
                            None => format!("aws_profile={}", status.aws_profile),
                        },
                    ],
                ),
                StatusRow::Db(status) => (
                    false,
                    [
                        kind.clone(),
                        status.name.clone(),
                        "unknown".to_string(),
                        match &status.host {
                            Some(host) => format!(
                                "{} {host} allow_write={}",
                                status.engine.as_str(),
                                status.allow_write
                            ),
                            None => format!(
                                "{} allow_write={}",
                                status.engine.as_str(),
                                status.allow_write
                            ),
                        },
                    ],
                ),
            }
        })
        .collect();
    let widths: Vec<usize> = (0..3)
        .map(|column| {
            cells
                .iter()
                .map(|(_, cells)| cells[column].len())
                .chain(std::iter::once(header[column].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    std::iter::once((false, header))
        .chain(cells)
        .map(|(active, cells)| {
            let line = format!(
                "{} {:<w0$}  {:<w1$}  {:<w2$}  {}",
                if active { '*' } else { ' ' },
                cells[0],
                cells[1],
                cells[2],
                cells[3],
                w0 = widths[0],
                w1 = widths[1],
                w2 = widths[2],
            );
            format!("{}\n", line.trim_end())
        })
        .collect()
}

/// The DETAIL column of an `auth` row: what the source is, and the one
/// thing that differs between the kinds -- the grant it runs, the header it
/// presents its credential in, or the variables it sets.
fn describe_auth_source(status: &AuthStatus) -> String {
    let kind = status.detail.kind().as_str();
    match &status.detail {
        AuthDetail::OAuth { grant_type } => format!("{kind} {}", grant_type.as_str()),
        AuthDetail::Token {
            header: Some(header),
        } => format!("{kind} header={header}"),
        AuthDetail::Token { header: None } => kind.to_string(),
        AuthDetail::Secrets => format!("{kind} env={}", status.env_vars.join(",")),
    }
}

fn format_time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn auth_json_fields(status: &AuthStatus) -> serde_json::Map<String, serde_json::Value> {
    let expires_at = match &status.token {
        TokenState::Valid {
            expires_at: Some(at),
        } => Some(format_time(*at)),
        _ => None,
    };
    let mut fields = serde_json::Map::new();
    fields.insert("token".into(), status.token.as_str().into());
    fields.insert(
        "logged_in".into(),
        matches!(status.token, TokenState::Valid { .. }).into(),
    );
    fields.insert("expires_at".into(), expires_at.into());
    fields.insert(
        "refreshable".into(),
        matches!(status.token, TokenState::Expired { refreshable: true }).into(),
    );
    fields.insert("needs_human".into(), status.needs_human.into());
    fields
}

/// One JSON array, one object per row; `kind` is `aws`, `auth`, `api` or `data`.
pub fn render_status_json(rows: &[StatusRow]) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let kind = row.kind().as_str();
            match row {
            StatusRow::Aws(status) => {
                let expires_at = match &status.session {
                    SessionState::Valid { expires_at } => Some(format_time(*expires_at)),
                    _ => None,
                };
                serde_json::json!({
                    "name": status.name,
                    "kind": kind,
                    "active": status.active,
                    "auth": status.auth().as_str(),
                    "role_arn": status.role_arn,
                    "region": status.region,
                    "mfa_serial": status.mfa_serial,
                    "session": status.session.as_str(),
                    "logged_in": expires_at.is_some(),
                    "expires_at": expires_at,
                    "needs_human": status.needs_human,
                })
            }
            StatusRow::Auth(status) => {
                // `name` and `kind` first, as for the other kinds.
                let mut fields = serde_json::Map::new();
                fields.insert("name".into(), status.name.clone().into());
                fields.insert("kind".into(), kind.into());
                let (grant_type, header) = match &status.detail {
                    AuthDetail::OAuth { grant_type } => (Some(grant_type.as_str()), None),
                    AuthDetail::Token { header } => (None, header.clone()),
                    AuthDetail::Secrets => (None, None),
                };
                // `env_var` is the one variable of a token; a `secrets`
                // source has several, and `env_vars` lists them for both.
                let env_var = match status.env_vars.as_slice() {
                    [only] if status.detail != AuthDetail::Secrets => Some(only.clone()),
                    _ => None,
                };
                fields.insert("auth_kind".into(), status.detail.kind().as_str().into());
                fields.insert("grant_type".into(), grant_type.into());
                fields.insert("header".into(), header.into());
                fields.insert("env_var".into(), env_var.into());
                fields.insert("env_vars".into(), status.env_vars.clone().into());
                fields.insert("active".into(), status.active.into());
                fields.extend(auth_json_fields(status));
                serde_json::Value::Object(fields)
            }
            StatusRow::Api { profile, auth } => serde_json::json!({
                "name": profile.name,
                "kind": kind,
                "base_url": profile.base_url,
                "description": profile.description,
                "auth": profile.auth,
                "auth_status": auth.as_ref().map(|auth| serde_json::Value::Object(auth_json_fields(auth))),
                "aws_profile": profile.aws_profile,
            }),
            StatusRow::Db(status) => serde_json::json!({
                "kind": kind,
                "name": status.name,
                "engine": status.engine.as_str(),
                "host": status.host,
                "database": status.database,
                "allow_write": status.allow_write,
                // Nothing was opened: a file that exists today may not later.
                "connection_status": "unknown",
                "auth_status": null,
            }),
            // Configuration only: neither the role nor the bucket was reached.
            StatusRow::S3(status) => serde_json::json!({
                "kind": kind,
                "name": status.name,
                "aws_profile": status.aws_profile,
                "region": status.region,
                "bucket": status.bucket,
                "prefix": status.prefix,
                "connection_status": "not_checked",
                "auth_status": null,
            }),
            StatusRow::Data(status) => serde_json::json!({
                "kind": kind,
                "name": status.name,
                "engine": "duckdb",
                "sources": status.sources,
                "source_count": status.sources.len(),
                "aws_profile": status.aws_profile,
                "region": status.region,
                "connection_status": "not_checked",
                "auth_status": null,
            }),
            }
        })
        .collect();
    format!("{}\n", serde_json::Value::Array(items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::GrantType;
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn rows() -> Vec<StatusRow> {
        vec![
            StatusRow::Aws(ProfileStatus {
                name: "dev".into(),
                role_arn: Some("arn:aws:iam::123456789012:role/Dev".into()),
                region: Some("ap-northeast-1".into()),
                mfa_serial: None,
                active: false,
                session: SessionState::NotRequired,
                needs_human: false,
            }),
            StatusRow::Aws(ProfileStatus {
                name: "ops-mfa".into(),
                role_arn: Some("arn:aws:iam::123456789012:role/Ops".into()),
                region: None,
                mfa_serial: Some("arn:aws:iam::123456789012:mfa/agent".into()),
                active: true,
                session: SessionState::Valid {
                    expires_at: now() + Duration::minutes(90),
                },
                needs_human: false,
            }),
            StatusRow::Aws(ProfileStatus {
                name: "default".into(),
                role_arn: None,
                region: None,
                mfa_serial: None,
                active: false,
                session: SessionState::NotRequired,
                needs_human: false,
            }),
        ]
    }

    /// The `KIND` column, the JSON `kind` field and `--only` must all name a
    /// row the same way, so they are one function. A new `StatusRow` variant
    /// stops the build in `kind()` instead of quietly printing nothing.
    #[test]
    fn every_row_names_its_own_kind() {
        let kinds: Vec<&str> = rows_with_auth_and_api()
            .iter()
            .map(|row| row.kind().as_str())
            .collect();
        assert!(kinds.contains(&"aws"), "{kinds:?}");
        assert!(kinds.contains(&"auth"), "{kinds:?}");
        assert!(kinds.contains(&"api"), "{kinds:?}");
        // What the table prints is what the filter compares against.
        let table = render_status_table(&rows_with_auth_and_api(), now());
        for kind in kinds {
            assert!(table.contains(kind), "{kind} missing from\n{table}");
        }
        assert_eq!(
            StatusRowKind::selectable_values(),
            ["aws", "auth", "api", "data", "s3"]
        );
        assert_eq!(StatusRowKind::parse("api"), Some(StatusRowKind::Api));
        // Plain status builds no database row, so selecting one is refused
        // rather than answered with a header and nothing under it.
        assert_eq!(StatusRowKind::parse("db"), None);
        assert_eq!(StatusRowKind::parse("nothing"), None);
    }

    /// `--only api` is what saves an agent from reading forty AWS rows to find
    /// one API.
    #[test]
    fn only_one_kind_keeps_only_that_kind() {
        let all = rows_with_auth_and_api();
        for kind in StatusRowKind::SELECTABLE {
            let kept: Vec<&StatusRow> = all.iter().filter(|row| row.kind() == kind).collect();
            assert!(
                kept.iter().all(|row| row.kind() == kind),
                "{} kept another kind",
                kind.as_str()
            );
            if !matches!(kind, StatusRowKind::Data | StatusRowKind::S3) {
                assert!(!kept.is_empty(), "{} kept nothing", kind.as_str());
            }
        }
        let apis: Vec<&StatusRow> = all
            .iter()
            .filter(|row| row.kind() == StatusRowKind::Api)
            .collect();
        // The three the fixture configures, and nothing else.
        assert_eq!(apis.len(), 3);
        assert!(apis.len() < all.len());
    }

    fn github_status() -> AuthStatus {
        AuthStatus {
            name: "github".into(),
            detail: AuthDetail::OAuth {
                grant_type: GrantType::AuthorizationCode,
            },
            env_vars: vec!["GITHUB_TOKEN".into()],
            token: TokenState::Valid {
                expires_at: Some(now() + Duration::minutes(112)),
            },
            active: true,
            needs_human: false,
        }
    }

    fn issued_status() -> AuthStatus {
        AuthStatus {
            name: "example".into(),
            detail: AuthDetail::Token {
                header: Some("X-API-Key".into()),
            },
            env_vars: vec!["EXAMPLE_TOKEN".into()],
            token: TokenState::NotChecked,
            active: false,
            needs_human: false,
        }
    }

    fn rows_with_auth_and_api() -> Vec<StatusRow> {
        let mut rows = rows();
        rows.push(StatusRow::Auth(github_status()));
        rows.push(StatusRow::Api {
            profile: Box::new(ApiProfile {
                name: "github".into(),
                description: Some("GitHub REST API".into()),
                base_url: "https://api.github.com".into(),
                auth: Some("github".into()),
                aws_profile: None,
                signing: Default::default(),
                spec: None,
                openapi_auth: false,
                headers: Default::default(),
            }),
            auth: Some(github_status()),
        });
        rows.push(StatusRow::Api {
            profile: Box::new(ApiProfile {
                name: "public".into(),
                description: None,
                base_url: "https://example.com".into(),
                auth: None,
                aws_profile: None,
                signing: Default::default(),
                spec: None,
                openapi_auth: false,
                headers: Default::default(),
            }),
            auth: None,
        });
        rows.push(StatusRow::Api {
            profile: Box::new(ApiProfile {
                name: "apigw".into(),
                description: None,
                base_url: "https://abc.execute-api.ap-northeast-1.amazonaws.com".into(),
                auth: None,
                aws_profile: Some("dev".into()),
                signing: Default::default(),
                spec: None,
                openapi_auth: false,
                headers: Default::default(),
            }),
            auth: None,
        });
        rows
    }

    #[test]
    fn table_aligns_columns_and_marks_the_active_profile() {
        let table = render_status_table(&rows(), now());
        assert_eq!(
            table,
            "  KIND  PROFILE  SESSION         DETAIL\n\
             \x20 aws   dev      -               arn:aws:iam::123456789012:role/Dev\n\
             * aws   ops-mfa  valid (1h 30m)  arn:aws:iam::123456789012:role/Ops\n\
             \x20 aws   default  -               iam user (no role_arn)\n"
        );
    }

    #[test]
    fn data_status_uses_the_shared_columns_and_resolves_s3_configuration() {
        let config = Config::parse(
            "[s3.archive]\naws_profile='dev'\nregion='eu-west-1'\n\
             [data.monthly-orders]\ns3_source='archive'\n\
             [[data.monthly-orders.sources]]\nname='orders'\npath='s3://bucket/orders.csv'\n",
        )
        .unwrap();
        let data_rows = super::super::data_contract::status_rows(&config, None);
        let mut rows = rows();
        rows.extend(data_rows.into_iter().map(StatusRow::Data));
        let table = render_status_table(&rows, now());
        let lines: Vec<_> = table.lines().collect();
        let data = lines.last().unwrap();
        assert_eq!(
            data.split_whitespace().collect::<Vec<_>>(),
            [
                "data",
                "monthly-orders",
                "not_checked",
                "duckdb",
                "sources=1"
            ]
        );
        assert_eq!(data.find("monthly-orders"), lines[0].find("PROFILE"));
        assert_eq!(data.find("not_checked"), lines[0].find("SESSION"));
        assert_eq!(data.find("duckdb"), lines[0].find("DETAIL"));
        let json: serde_json::Value = serde_json::from_str(&render_status_json(&rows)).unwrap();
        let data = json.as_array().unwrap().last().unwrap();
        assert_eq!(data["name"], "monthly-orders");
        assert_eq!(data["source_count"], 1);
        assert_eq!(data["sources"][0]["path"], "s3://bucket/orders.csv");
        assert_eq!(data["aws_profile"], "dev");
        assert_eq!(data["region"], "eu-west-1");
        assert_eq!(data["connection_status"], "not_checked");
        assert_eq!(data["auth_status"], serde_json::Value::Null);
    }

    /// A `kind = "token"` source has no grant and no cached token, and its
    /// row says so: `not_checked` rather than `none`, and the header it
    /// presents the credential in rather than a grant it never runs.
    #[test]
    fn a_token_source_row_names_its_header_and_reports_nothing_checked() {
        let rows = vec![StatusRow::Auth(issued_status())];
        let table = render_status_table(&rows, now());
        let line = table.lines().nth(1).unwrap();
        assert_eq!(
            line.split_whitespace().collect::<Vec<_>>(),
            [
                "auth",
                "example",
                "not_checked",
                "token",
                "header=X-API-Key"
            ]
        );
        let json: serde_json::Value = serde_json::from_str(&render_status_json(&rows)).unwrap();
        let row = &json[0];
        assert_eq!(row["kind"], "auth");
        assert_eq!(row["auth_kind"], "token");
        assert!(row["grant_type"].is_null());
        assert_eq!(row["header"], "X-API-Key");
        assert_eq!(row["env_var"], "EXAMPLE_TOKEN");
        assert_eq!(row["token"], "not_checked");
        assert_eq!(row["logged_in"], false);
        assert_eq!(row["needs_human"], false);
        assert!(row["expires_at"].is_null());
        assert_eq!(row["refreshable"], false);
    }

    #[test]
    fn table_lists_auth_sources_and_apis_after_the_aws_profiles() {
        let table = render_status_table(&rows_with_auth_and_api(), now());
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(
            lines[4],
            "* auth  github   valid (1h 52m)  oauth authorization_code"
        );
        assert_eq!(
            lines[5],
            "  api   github   valid (1h 52m)  https://api.github.com auth=github"
        );
        assert_eq!(
            lines[6],
            "  api   public   -               https://example.com"
        );
        assert_eq!(
            lines[7],
            "  api   apigw    -               https://abc.execute-api.ap-northeast-1.amazonaws.com aws_profile=dev"
        );
    }

    #[test]
    fn json_is_one_array_with_session_fields() {
        let json: serde_json::Value = serde_json::from_str(&render_status_json(&rows())).unwrap();
        let ops = &json[1];
        assert_eq!(ops["kind"], "aws");
        assert_eq!(ops["active"], true);
        assert_eq!(ops["session"], "valid");
        assert_eq!(ops["logged_in"], true);
        assert_eq!(ops["expires_at"], "2027-01-15T09:30:00Z");
        assert_eq!(json[0]["logged_in"], false);
        assert!(json[0]["expires_at"].is_null());
        assert!(json[2]["role_arn"].is_null());
        assert_eq!(json[0]["auth"], "role");
        assert_eq!(json[2]["auth"], "iam_user");
    }

    #[test]
    fn json_auth_rows_start_with_name_and_kind() {
        let json = render_status_json(&rows_with_auth_and_api());
        assert!(
            json.contains(
                "{\"name\":\"github\",\"kind\":\"auth\",\"auth_kind\":\"oauth\",\"grant_type\":"
            ),
            "{json}"
        );
    }

    #[test]
    fn json_describes_auth_sources_and_the_auth_state_of_apis() {
        let json: serde_json::Value =
            serde_json::from_str(&render_status_json(&rows_with_auth_and_api())).unwrap();
        let auth = &json[3];
        assert_eq!(auth["kind"], "auth");
        assert_eq!(auth["name"], "github");
        assert_eq!(auth["auth_kind"], "oauth");
        assert_eq!(auth["grant_type"], "authorization_code");
        assert!(auth["header"].is_null());
        assert_eq!(auth["env_var"], "GITHUB_TOKEN");
        assert_eq!(auth["token"], "valid");
        assert_eq!(auth["logged_in"], true);
        assert_eq!(auth["expires_at"], "2027-01-15T09:52:00Z");
        assert_eq!(auth["refreshable"], false);
        assert_eq!(auth["needs_human"], false);
        assert_eq!(auth["active"], true);
        let api = &json[4];
        assert_eq!(api["kind"], "api");
        assert_eq!(api["base_url"], "https://api.github.com");
        assert_eq!(api["description"], "GitHub REST API");
        assert_eq!(api["auth"], "github");
        assert_eq!(api["auth_status"]["logged_in"], true);
        assert!(api["aws_profile"].is_null());
        assert!(json[5]["auth"].is_null());
        assert!(json[5]["auth_status"].is_null());
        assert_eq!(json[6]["aws_profile"], "dev");
        assert!(json[6]["auth"].is_null());
    }

    #[tokio::test]
    async fn auth_statuses_come_from_the_token_store() {
        use crate::adapters::config::AuthToml;
        use crate::domain::types::OAuthToken;
        use crate::ports::token_store::{MockTokenStore, TokenStoreError};
        let client = toml::from_str::<AuthToml>(
            "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://as/token\"\nclient_id = \"id\"\n",
        )
        .unwrap()
        .typed("svc")
        .unwrap();
        let mut store = MockTokenStore::new();
        store
            .expect_load()
            .returning(|_| Ok(Some(OAuthToken::bearer("at"))));
        let statuses = gather_auth_statuses(std::slice::from_ref(&client), &store, now()).await;
        assert_eq!(
            statuses["svc"].token,
            TokenState::Valid { expires_at: None }
        );
        let mut locked = MockTokenStore::new();
        locked
            .expect_load()
            .returning(|_| Err(TokenStoreError::Backend("locked".into())));
        let statuses = gather_auth_statuses(&[client], &locked, now()).await;
        assert_eq!(statuses["svc"].token, TokenState::Unreadable);
    }
}
