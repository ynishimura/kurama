//! TEA Runtime - Effect execution and main loop
//!
//! The runtime is responsible for:
//! - Executing effects returned by the update function
//! - Converting external events (I/O) into messages
//! - Running the main application loop

use super::effects::{
    AssumeRoleEffect, ConsoleUrlEffect, LogEffect, NotificationEffect, TuiEffect,
    WriteCredentialsEffect,
};
use super::messages::{
    AssumeRoleError, AssumeRoleResult, CredentialsInfo, ProfileRow, ProfilesLoadedResult,
    TuiMessage,
};
use super::palette::{MAX_ITEMS, PaletteItem, PaletteTarget};
use super::sources::{SourceList, SourceRow, Sources, Tab};
use super::update::{Handoff, TuiModel, tea_update};
use crate::adapters::aws::federation::FederationService;
use crate::adapters::browser::open_url;
use crate::adapters::clipboard::copy_to_clipboard;
use crate::adapters::config::Config;
use crate::adapters::env_script::output_shell_script;
use crate::adapters::error::CoreError;
use crate::adapters::openapi::read_offline_spec;
use crate::adapters::profile::{find_profile, load_profiles};
use crate::adapters::request_history::{history_file, read_history};
use crate::adapters::token_store::create_token_store;
use crate::domain::functions::export::generate_export_script;
use crate::domain::types::request_history::HistoryEntry;
use crate::domain::{Credentials, Profile};
use crate::shell::cli::commands::status::{gather_auth_statuses, gather_profile_statuses};
use crate::shell::runtime::Runtime;
use crate::shell::tui::event::{Event, EventHandler};
use crate::shell::tui::explorer::history::listed;
use crate::shell::tui::session::TuiApplication;
use crate::shell::tui::terminal::Terminal;

/// TEA Runtime for executing the TUI application
pub struct TeaRuntime {
    /// Current model state
    model: TuiModel,
    /// Configuration used to create a profile-specific Runtime on demand.
    config: Config,
    /// Credential output is held until the alternate screen is restored.
    pending_output: Option<String>,
}

impl TeaRuntime {
    /// Create a TUI runtime that builds the AWS Runtime for each selected
    /// profile. This preserves source-profile-specific credential providers.
    pub fn with_config(config: Config) -> Self {
        Self {
            model: TuiModel::new(false, false),
            config,
            pending_output: None,
        }
    }

    /// Open on `tab` instead of AWS: the tab a handoff left from.
    pub fn on_tab(mut self, tab: Tab) -> Self {
        self.model.tab = tab;
        self
    }

    /// What to run before the screen opens again, and the tab to open it on.
    pub fn handoff(&self) -> Option<(Handoff, Tab)> {
        self.model
            .handoff
            .clone()
            .map(|handoff| (handoff, self.model.tab))
    }

    /// Check if the application should exit
    pub fn should_exit(&self) -> bool {
        self.model.should_exit
    }

    /// Start the TEA application by executing its initial profile-load effect.
    pub async fn start(&mut self) -> Result<(), CoreError> {
        self.dispatch(TuiMessage::Tick(chrono::Utc::now())).await?;
        self.execute_effect(TuiEffect::LoadProfiles).await
    }

    /// Write credential output after the terminal has been restored.
    pub fn flush_output(&mut self) -> Result<(), CoreError> {
        let Some(output) = self.pending_output.take() else {
            return Ok(());
        };
        output_shell_script(&output)
            .map_err(|error| CoreError::Other(format!("Failed to output credentials: {error}")))
    }

    /// Dispatch a message and execute resulting effects
    pub async fn dispatch(&mut self, msg: TuiMessage) -> Result<(), CoreError> {
        let effects = self.update(msg);
        self.execute_effects(effects).await
    }

    /// Apply the pure update and return the effects it planned.
    fn update(&mut self, msg: TuiMessage) -> Vec<TuiEffect> {
        let result = tea_update(self.model.clone(), msg);
        self.model = result.model;
        result.effects
    }

    async fn execute_effects(&mut self, effects: Vec<TuiEffect>) -> Result<(), CoreError> {
        for effect in effects {
            self.execute_effect(effect).await?;
        }
        Ok(())
    }

    /// Execute a single effect
    async fn execute_effect(&mut self, effect: TuiEffect) -> Result<(), CoreError> {
        match effect {
            TuiEffect::LoadProfiles => {
                let result = self.load_profiles_impl().await;
                // The S3 tab shows each connection's AWS profile as this
                // tab does.
                let sessions: std::collections::HashMap<String, String> = result
                    .iter()
                    .flatten()
                    .map(|row| (row.name.clone(), row.session.clone()))
                    .collect();
                let msg = TuiMessage::ProfilesLoaded(ProfilesLoadedResult { profiles: result });
                Box::pin(self.dispatch(msg)).await?;
                let sources = load_sources(&self.config, &sessions, chrono::Utc::now()).await;
                Box::pin(self.dispatch(TuiMessage::SourcesLoaded(sources))).await
            }

            TuiEffect::AssumeRole(effect) => {
                let result = self.assume_role(effect.clone()).await;
                let msg = TuiMessage::AssumeRoleCompleted(AssumeRoleResult {
                    profile_name: effect.profile_name.clone(),
                    result,
                });
                Box::pin(self.dispatch(msg)).await
            }

            TuiEffect::GenerateConsoleUrl(effect) => {
                let result = self.generate_console_url(effect).await;
                let msg =
                    TuiMessage::ConsoleUrlGenerated(super::messages::ConsoleUrlResult { result });
                Box::pin(self.dispatch(msg)).await
            }

            TuiEffect::OpenBrowser { url } => {
                let result = open_url(&url).map_err(|error| error.to_string());
                match result {
                    Ok(()) => tracing::info!("AWS console opened in browser"),
                    // Held while the TUI owns the terminal, printed on stderr
                    // after it is restored, where the URL can be copied whole.
                    Err(_) => crate::console::progress!("# Console URL: {url}"),
                }
                let msg = TuiMessage::BrowserOpened(super::messages::BrowserOpenResult { result });
                Box::pin(self.dispatch(msg)).await
            }

            TuiEffect::CopyToClipboard { text } => {
                let result = copy_to_clipboard(&text).map_err(|error| error.to_string());
                Box::pin(self.dispatch(TuiMessage::Copied(result))).await
            }

            TuiEffect::LoadPaletteItems => {
                let items = palette_items(&self.config);
                Box::pin(self.dispatch(TuiMessage::PaletteItemsLoaded(items))).await
            }

            TuiEffect::WriteCredentials(effect) => {
                self.pending_output = Some(self.credential_output(effect));
                Ok(())
            }

            TuiEffect::ShowNotification(effect) => {
                self.show_notification(effect);
                Ok(())
            }

            TuiEffect::Log(effect) => {
                self.log(effect);
                Ok(())
            }

            TuiEffect::Exit => {
                // Exit is handled by the main loop checking should_exit
                Ok(())
            }
        }
    }

    /// Profiles sorted by name, with the same session state as `kurama status`.
    async fn load_profiles_impl(&self) -> Result<Vec<ProfileRow>, String> {
        let mut profiles = load_profiles()
            .await
            .map_err(|e| format!("Failed to load profiles: {}", e))?;
        profiles.sort_by(|a, b| a.name().cmp(b.name()));

        let now = chrono::Utc::now();
        let statuses = gather_profile_statuses(&profiles, &self.config, now).await;
        Ok(statuses
            .iter()
            .map(|status| ProfileRow::from_status(status, now))
            .collect())
    }

    fn credential_output(&self, effect: WriteCredentialsEffect) -> String {
        let credentials_to_write = &effect.credentials;
        let credentials = Credentials::new(
            credentials_to_write.access_key_id.clone(),
            credentials_to_write.secret_access_key.clone(),
            credentials_to_write.session_token.clone(),
            credentials_to_write.expiration,
        );
        let profile = match credentials_to_write.region.clone() {
            Some(region) => Profile::new(effect.profile_name.clone()).with_region_raw(region),
            None => Profile::new(effect.profile_name.clone()),
        };

        generate_export_script(&credentials, &profile, effect.readonly)
    }

    async fn generate_console_url(&self, effect: ConsoleUrlEffect) -> Result<String, String> {
        let session_token = effect
            .credentials
            .session_token
            .as_deref()
            .ok_or_else(|| "Session token is required for console access".to_string())?;
        FederationService::new()
            .console_url(
                &effect.credentials.access_key_id,
                &effect.credentials.secret_access_key,
                session_token,
            )
            .await
            .map_err(|error| format!("Failed to get federation token: {error}"))
    }

    /// Assume role for a profile
    async fn assume_role(
        &self,
        effect: AssumeRoleEffect,
    ) -> Result<CredentialsInfo, AssumeRoleError> {
        tracing::debug!(
            "Assuming role for profile: {}, mfa: {:?}, readonly: {}",
            effect.profile_name,
            effect.mfa_token.is_some(),
            effect.readonly
        );

        // Load profiles before creating a Runtime so the selected profile can
        // provide the correct source credentials to STS.
        let profiles = load_profiles().await.map_err(|e| AssumeRoleError::Failed {
            message: format!("Failed to load profiles: {}", e),
        })?;

        let profile = find_profile(&profiles, &effect.profile_name).ok_or_else(|| {
            AssumeRoleError::Failed {
                message: format!("Profile '{}' not found", effect.profile_name),
            }
        })?;

        let runtime = Runtime::from_config(self.config.clone(), &profile).await;
        assume_role_with_profile(&runtime, profile, effect).await
    }

    /// Show a notification
    fn show_notification(&self, effect: NotificationEffect) {
        tracing::info!("{}", effect.message);
    }

    /// Log a message
    fn log(&self, effect: LogEffect) {
        tracing::debug!("{}", effect.message);
    }
}

#[async_trait::async_trait]
impl TuiApplication for TeaRuntime {
    fn should_exit(&self) -> bool {
        self.should_exit()
    }

    fn draw(&self, terminal: &mut Terminal) -> Result<(), CoreError> {
        terminal.draw(|frame| super::view::render(frame, &self.model))
    }

    async fn initialize(
        &mut self,
        _terminal: &mut Terminal,
        _events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        self.start().await
    }

    async fn handle_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal,
        _events: &mut EventHandler,
    ) -> Result<(), CoreError> {
        let message = match event {
            Event::Key(key) => TuiMessage::Key(key),
            Event::Resize => TuiMessage::Resize,
            Event::Tick => TuiMessage::Tick(chrono::Utc::now()),
        };
        let effects = self.update(message);
        // Show the new state before its effects run, so the Processing modal
        // is on screen while STS is called.
        self.draw(terminal)?;
        self.execute_effects(effects).await?;
        self.draw(terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_effect() -> WriteCredentialsEffect {
        WriteCredentialsEffect {
            profile_name: "prod".into(),
            credentials: super::super::effects::CredentialsToWrite {
                access_key_id: "ASIA123".into(),
                secret_access_key: "secret".into(),
                session_token: Some("token".into()),
                region: Some("ap-northeast-1".into()),
                expiration: None,
            },
            readonly: true,
        }
    }

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_runtime_creation() {
        let runtime = TeaRuntime::with_config(Config::default());
        assert!(!runtime.should_exit());
        assert!(!runtime.model.mode.readonly);
    }

    #[tokio::test]
    async fn test_dispatch_quit() {
        let mut runtime = TeaRuntime::with_config(Config::default());
        let quit = key(crossterm::event::KeyCode::Char('q'));
        runtime.dispatch(TuiMessage::Key(quit)).await.unwrap();
        assert!(runtime.should_exit());
    }

    #[tokio::test]
    async fn test_dispatch_key_toggle_readonly() {
        let mut runtime = TeaRuntime::with_config(Config::default());
        assert!(!runtime.model.mode.readonly);

        let toggle = key(crossterm::event::KeyCode::Char('r'));
        runtime.dispatch(TuiMessage::Key(toggle)).await.unwrap();
        assert!(runtime.model.mode.readonly);
    }

    #[test]
    fn credential_output_matches_cli_shell_contract() {
        let runtime = TeaRuntime::with_config(Config::default());
        let output = runtime.credential_output(write_effect());

        assert!(output.contains("export AWS_ACCESS_KEY_ID='ASIA123'"));
        assert!(output.contains("export AWS_SECRET_ACCESS_KEY='secret'"));
        assert!(output.contains("export AWS_REGION='ap-northeast-1'"));
        assert!(output.contains("export AWS_READONLY_SESSION='true'"));
    }
}

/// The Auth, API, DB and Data rows, from what `kurama status` reads: the
/// configuration and the token store; nothing is called. A configuration
/// that cannot list a kind leaves that tab empty: the AWS tab is still
/// usable, and `kurama config check` is where the reason is reported.
/// The operations of every `[api.*]` description already on disk (a file,
/// or a URL's cached copy however old; nothing is fetched) and the
/// explorer's history of each, at most `MAX_ITEMS`.
fn palette_items(config: &Config) -> Vec<PaletteItem> {
    let mut items = Vec::new();
    for api in config.api_profiles() {
        if let Ok(path) = history_file(&api.name) {
            for entry in listed(&read_history(&path).unwrap_or_default()) {
                let mut detail = api.name.clone();
                for (name, value) in &entry.params {
                    detail.push_str(&format!("  {name}={value}"));
                }
                items.push(PaletteItem::new(
                    "history",
                    entry
                        .name
                        .clone()
                        .unwrap_or_else(|| entry.operation.clone()),
                    detail,
                    PaletteTarget::Explore {
                        api: api.name.clone(),
                        entry: Some(entry),
                    },
                ));
            }
        }
        let Some(spec) = api.spec.as_ref().and_then(read_offline_spec) else {
            continue;
        };
        for operation in &spec.operations {
            items.push(PaletteItem::new(
                "op",
                operation.id.clone(),
                format!("{}  {}", api.name, operation.label()),
                PaletteTarget::Explore {
                    api: api.name.clone(),
                    entry: Some(HistoryEntry {
                        operation: operation.id.clone(),
                        params: Vec::new(),
                        body: None,
                        name: None,
                    }),
                },
            ));
        }
    }
    items.truncate(MAX_ITEMS);
    items
}

async fn load_sources(
    config: &Config,
    sessions: &std::collections::HashMap<String, String>,
    now: chrono::DateTime<chrono::Utc>,
) -> Sources {
    let clients = config.auth_sources();
    let apis = config.api_profiles();
    let auth = gather_auth_statuses(&clients, create_token_store().as_ref(), now).await;
    let list = |rows: Vec<SourceRow>| SourceList { rows, selected: 0 };
    Sources {
        api: list(
            apis.iter()
                .map(|api| {
                    let status = api.auth.as_ref().and_then(|name| auth.get(name));
                    SourceRow::from_api(api, status, now)
                })
                .collect(),
        ),
        auth: list(
            auth.values()
                .map(|status| SourceRow::from_auth(status, now))
                .collect(),
        ),
        db: list(
            crate::shell::cli::commands::db_contract::status_rows(config, None)
                .iter()
                .map(SourceRow::from_db)
                .collect(),
        ),
        data: list(
            crate::shell::cli::commands::data_contract::status_rows(config, None)
                .iter()
                .map(SourceRow::from_data)
                .collect(),
        ),
        s3: list(
            crate::shell::cli::commands::s3_status::status_rows(config, None)
                .iter()
                .map(|row| {
                    SourceRow::from_s3(row, sessions.get(&row.aws_profile).map(String::as_str))
                })
                .collect(),
        ),
    }
}

async fn assume_role_with_profile(
    runtime: &Runtime,
    profile: crate::domain::Profile,
    effect: AssumeRoleEffect,
) -> Result<CredentialsInfo, AssumeRoleError> {
    use crate::shell::executor::{ExecutorError, assume_role_for_profile};
    let region = profile.region_raw().map(str::to_string);

    match assume_role_for_profile(runtime, profile, effect.readonly, effect.mfa_token.clone()).await
    {
        Ok(output) => {
            let credentials = output.credentials;
            tracing::info!(
                "Successfully assumed role for profile: {}",
                effect.profile_name
            );
            Ok(CredentialsInfo {
                access_key_id: credentials.access_key_id().to_string(),
                credentials,
                region,
            })
        }
        Err(ExecutorError::MfaRequired { serial }) => Err(AssumeRoleError::MfaRequired { serial }),
        Err(error) => {
            tracing::warn!("Failed to assume role: {error}");
            Err(AssumeRoleError::Failed {
                message: error.to_string(),
            })
        }
    }
}

#[cfg(test)]
#[path = "runtime_session_tests.rs"]
mod session_tests;
