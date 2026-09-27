//! TEA update function - Pure state transitions
//!
//! The update function is the core of TEA architecture.
//! It takes the current state and a message, and returns the new state
//! along with any effects that need to be executed.
//!
//! This function is completely pure - no I/O, no side effects.

use super::effects::{
    AssumeRoleEffect, ConsoleUrlEffect, CredentialsForConsole, CredentialsToWrite,
    NotificationEffect, TuiEffect, WriteCredentialsEffect,
};
use super::messages::{
    AssumeRoleError, AssumeRoleResult, ProfileRow, ProfilesLoadedResult, TuiMessage,
};
use super::palette::{Palette, PaletteItem, model_items, update_palette_key};
use super::sources::{Sources, Tab, update_source_key};
use crate::domain::types::request_history::HistoryEntry;
use crate::shell::tui::components::LineInput;
use crate::shell::tui::components::list_navigation::moved_selection;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyModifiers};

/// Length of a TOTP code.
pub const MFA_CODE_LEN: usize = 6;

/// TUI Model - Application state
#[derive(Debug, Clone)]
pub struct TuiModel {
    /// Current screen/view state
    pub screen: Screen,
    /// Profile data
    pub profiles: ProfileListModel,
    /// MFA input state
    pub mfa_input: MfaInputModel,
    /// Application mode settings
    pub mode: AppMode,
    /// Current profile search query.
    pub search_query: LineInput,
    /// Whether profile-list keys edit the search query.
    pub searching: bool,
    /// Whether the app should exit
    pub should_exit: bool,
    /// The clock as of the last tick, for the time a session has left.
    pub now: DateTime<Utc>,
    /// The tab on screen.
    pub tab: Tab,
    /// The rows of every tab but AWS.
    pub sources: Sources,
    /// One line in the header until the next key.
    pub notice: Option<String>,
    /// What to run once the screen is left, before it opens again.
    pub handoff: Option<Handoff>,
    /// The command palette, while it is open.
    pub palette: Option<Palette>,
    /// The operations and history the runtime read for the palette; `None`
    /// until the palette is first opened.
    pub palette_items: Option<Vec<PaletteItem>>,
}

/// A command the home screen leaves to, and opens again after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// `kurama <args>` (`login`, the API or database explorer).
    Run(Vec<String>),
    /// `kurama api <API>` with the form of `entry` open.
    Explore { api: String, entry: HistoryEntry },
}

/// Current screen being displayed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    ProfileList,
    MfaInput,
    Processing { message: String },
    Success(SuccessModel),
    Error(ErrorModel),
    Help,
}

/// Profile list model
#[derive(Debug, Clone, Default)]
pub struct ProfileListModel {
    /// All available profiles, sorted by name
    pub all_profiles: Vec<ProfileRow>,
    /// Filtered profiles (based on search)
    pub filtered_profiles: Vec<ProfileRow>,
    /// Currently selected index
    pub selected_index: usize,
    /// Loading state
    pub loading: bool,
}

/// MFA input model
#[derive(Clone, Default)]
pub struct MfaInputModel {
    /// Current MFA input value
    pub value: String,
    /// MFA serial number
    pub serial: String,
    /// Profile name requiring MFA
    pub profile: String,
}

impl std::fmt::Debug for MfaInputModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MfaInputModel")
            .field("value", &(!self.value.is_empty()).then_some("[REDACTED]"))
            .field("serial", &self.serial)
            .field("profile", &self.profile)
            .finish()
    }
}

/// Success model
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuccessModel {
    pub profile: String,
    pub access_key_id: String,
}

/// Error model
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorModel {
    pub message: String,
    /// The browser did not open the console: its URL is printed on quit.
    pub console_url_on_quit: bool,
}

/// Application mode settings
#[derive(Debug, Clone, Default)]
pub struct AppMode {
    pub readonly: bool,
    pub console_launch: bool,
}

impl TuiModel {
    /// Create a new TUI model with initial state
    pub fn new(readonly: bool, console_launch: bool) -> Self {
        Self {
            screen: Screen::ProfileList,
            profiles: ProfileListModel {
                loading: true,
                ..Default::default()
            },
            mfa_input: MfaInputModel::default(),
            mode: AppMode {
                readonly,
                console_launch,
            },
            search_query: LineInput::default(),
            searching: false,
            should_exit: false,
            now: DateTime::UNIX_EPOCH,
            tab: Tab::Aws,
            sources: Sources::default(),
            notice: None,
            handoff: None,
            palette: None,
            palette_items: None,
        }
    }

    /// Get the currently selected profile name
    pub fn selected_profile(&self) -> Option<&str> {
        self.profiles
            .filtered_profiles
            .get(self.profiles.selected_index)
            .map(|row| row.name.as_str())
    }
}

/// Update result containing new model and effects
pub struct UpdateResult {
    pub model: TuiModel,
    pub effects: Vec<TuiEffect>,
}

impl UpdateResult {
    /// Create a result with no effects
    pub fn no_effect(model: TuiModel) -> Self {
        Self {
            model,
            effects: vec![],
        }
    }

    /// Create a result with effects
    pub fn with_effects(model: TuiModel, effects: Vec<TuiEffect>) -> Self {
        Self { model, effects }
    }

    /// Create a result with a single effect
    pub fn with_effect(model: TuiModel, effect: TuiEffect) -> Self {
        Self {
            model,
            effects: vec![effect],
        }
    }
}

/// Pure update function - the heart of TEA
///
/// This function takes the current model and a message, and returns
/// a new model along with any effects that need to be executed.
///
/// # Purity
/// This function is completely pure:
/// - No I/O operations
/// - No side effects
/// - Deterministic output for the same inputs
pub fn tea_update(model: TuiModel, msg: TuiMessage) -> UpdateResult {
    match msg {
        TuiMessage::Key(key) => update_key(model, key),
        TuiMessage::Resize => UpdateResult::no_effect(model),
        TuiMessage::Tick(now) => {
            let mut model = model;
            model.now = now;
            UpdateResult::no_effect(model)
        }
        TuiMessage::ProfilesLoaded(result) => update_profiles_loaded(model, result),
        TuiMessage::AssumeRoleCompleted(result) => update_assume_role_completed(model, result),
        TuiMessage::ConsoleUrlGenerated(result) => update_console_url_generated(model, result),
        TuiMessage::BrowserOpened(result) => update_browser_opened(model, result),
        TuiMessage::SourcesLoaded(sources) => {
            let mut model = model;
            model.sources = sources;
            UpdateResult::no_effect(model)
        }
        TuiMessage::PaletteItemsLoaded(items) => {
            let mut model = model;
            if let Some(palette) = &mut model.palette {
                palette.extend(items.clone());
            }
            model.palette_items = Some(items);
            UpdateResult::no_effect(model)
        }
        TuiMessage::Copied(result) => {
            let mut model = model;
            model.notice = Some(match result {
                Ok(()) => "command copied to the clipboard".into(),
                Err(error) => format!("copy failed: {error}"),
            });
            UpdateResult::no_effect(model)
        }
    }
}

/// Handle quit message
fn update_quit(mut model: TuiModel) -> UpdateResult {
    model.should_exit = true;
    UpdateResult::with_effect(model, TuiEffect::Exit)
}

/// Handle key events based on current screen
fn update_key(mut model: TuiModel, key: crossterm::event::KeyEvent) -> UpdateResult {
    model.notice = None;
    if model.palette.is_some() {
        if (key.code, key.modifiers) == (KeyCode::Char('c'), KeyModifiers::CONTROL) {
            return update_quit(model);
        }
        let (model, left) = update_palette_key(model, key);
        return if left && model.should_exit {
            UpdateResult::with_effect(model, TuiEffect::Exit)
        } else {
            UpdateResult::no_effect(model)
        };
    }
    let palette_key = matches!(
        (key.code, key.modifiers),
        (KeyCode::Char('k'), KeyModifiers::CONTROL) | (KeyCode::Char(':'), _)
    );
    if palette_key && model.screen == Screen::ProfileList && !model.searching {
        return open_palette(model);
    }
    // Global quit shortcuts
    let typing_q =
        model.screen == Screen::ProfileList && model.searching && key.code == KeyCode::Char('q');
    if is_quit_key(&key) && !typing_q {
        return update_quit(model);
    }

    if model.screen == Screen::ProfileList && !model.searching {
        if let Some(tab) = model.tab.for_key(key.code) {
            model.tab = tab;
            return UpdateResult::no_effect(model);
        }
        if model.tab != Tab::Aws {
            return update_source_key(model, key);
        }
    }
    match model.screen {
        Screen::ProfileList => update_profile_list_key(model, key),
        Screen::MfaInput => update_mfa_input_key(model, key),
        Screen::Help => update_help_key(model, key),
        Screen::Error(_) => update_error_key(model, key),
        Screen::Success(_) => update_success_key(model, key),
        Screen::Processing { .. } => UpdateResult::no_effect(model),
    }
}

/// Open the palette on what the model holds; the operations and history
/// are read once, the first time.
fn open_palette(mut model: TuiModel) -> UpdateResult {
    let mut items = model_items(&model);
    items.extend(model.palette_items.clone().unwrap_or_default());
    model.palette = Some(Palette::new(items));
    if model.palette_items.is_none() {
        UpdateResult::with_effect(model, TuiEffect::LoadPaletteItems)
    } else {
        UpdateResult::no_effect(model)
    }
}

/// Handle key events in profile list screen
fn update_profile_list_key(mut model: TuiModel, key: crossterm::event::KeyEvent) -> UpdateResult {
    if model.searching {
        return update_search_key(model, key);
    }

    let total = model.profiles.filtered_profiles.len();

    match key.code {
        // Navigation
        code if let Some(index) = moved_selection(code, model.profiles.selected_index, total) => {
            model.profiles.selected_index = index;
            UpdateResult::no_effect(model)
        }

        // Selection
        KeyCode::Enter => {
            if let Some(profile) = model.selected_profile().map(|s| s.to_string()) {
                model.screen = Screen::Processing {
                    message: format!("Assuming role for profile: {}", profile),
                };
                let effect = TuiEffect::AssumeRole(AssumeRoleEffect {
                    profile_name: profile,
                    mfa_token: None,
                    readonly: model.mode.readonly,
                });
                UpdateResult::with_effect(model, effect)
            } else {
                UpdateResult::no_effect(model)
            }
        }

        // Help
        KeyCode::F(1) | KeyCode::Char('?') => {
            model.screen = Screen::Help;
            UpdateResult::no_effect(model)
        }

        // Search
        KeyCode::Char('/') => {
            model.searching = true;
            UpdateResult::no_effect(model)
        }

        // Toggle readonly mode
        KeyCode::Char('r') => {
            model.mode.readonly = !model.mode.readonly;
            let msg = if model.mode.readonly {
                "Readonly mode enabled"
            } else {
                "Readonly mode disabled"
            };
            UpdateResult::with_effect(model, TuiEffect::debug(msg))
        }

        // Toggle console launch mode
        KeyCode::Char('c') if key.modifiers == KeyModifiers::NONE => {
            model.mode.console_launch = !model.mode.console_launch;
            let msg = if model.mode.console_launch {
                "Console launch enabled"
            } else {
                "Console launch disabled"
            };
            UpdateResult::with_effect(model, TuiEffect::debug(msg))
        }

        _ => UpdateResult::no_effect(model),
    }
}

fn update_search_key(mut model: TuiModel, key: crossterm::event::KeyEvent) -> UpdateResult {
    match key.code {
        KeyCode::Esc => {
            model.searching = false;
            model.search_query.clear();
            apply_search(&mut model);
        }
        KeyCode::Enter => {
            model.searching = false;
        }
        KeyCode::Up | KeyCode::Down => {
            let total = model.profiles.filtered_profiles.len();
            model.profiles.selected_index =
                moved_selection(key.code, model.profiles.selected_index, total).unwrap_or(0);
        }
        _ => {
            if model.search_query.handle_key(key) {
                apply_search(&mut model);
            }
        }
    }
    UpdateResult::no_effect(model)
}

fn apply_search(model: &mut TuiModel) {
    let query = model.search_query.as_str().to_lowercase();
    model.profiles.filtered_profiles = model
        .profiles
        .all_profiles
        .iter()
        .filter(|row| query.is_empty() || row.name.to_lowercase().contains(&query))
        .cloned()
        .collect();
    model.profiles.selected_index = 0;
}

/// Handle key events in MFA input screen
fn update_mfa_input_key(mut model: TuiModel, key: crossterm::event::KeyEvent) -> UpdateResult {
    match key.code {
        KeyCode::Esc => {
            model.screen = Screen::ProfileList;
            model.mfa_input = MfaInputModel::default();
            UpdateResult::no_effect(model)
        }

        KeyCode::Enter => {
            if model.mfa_input.value.len() == MFA_CODE_LEN {
                let profile = model.mfa_input.profile.clone();
                let mfa_token = model.mfa_input.value.clone();
                model.screen = Screen::Processing {
                    message: format!("Verifying MFA for profile: {}", profile),
                };
                let effect = TuiEffect::AssumeRole(AssumeRoleEffect {
                    profile_name: profile,
                    mfa_token: Some(mfa_token),
                    readonly: model.mode.readonly,
                });
                UpdateResult::with_effect(model, effect)
            } else {
                UpdateResult::no_effect(model)
            }
        }

        KeyCode::Char(c) if c.is_ascii_digit() => {
            if model.mfa_input.value.len() < MFA_CODE_LEN {
                model.mfa_input.value.push(c);
            }
            UpdateResult::no_effect(model)
        }

        KeyCode::Backspace => {
            model.mfa_input.value.pop();
            UpdateResult::no_effect(model)
        }

        _ => UpdateResult::no_effect(model),
    }
}

/// Handle key events in help screen
fn update_help_key(mut model: TuiModel, key: crossterm::event::KeyEvent) -> UpdateResult {
    match key.code {
        KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?') | KeyCode::Char('q') => {
            model.screen = Screen::ProfileList;
            UpdateResult::no_effect(model)
        }
        _ => UpdateResult::no_effect(model),
    }
}

/// Handle key events in error screen
fn update_error_key(mut model: TuiModel, key: crossterm::event::KeyEvent) -> UpdateResult {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => {
            model.screen = Screen::ProfileList;
            UpdateResult::no_effect(model)
        }
        _ => UpdateResult::no_effect(model),
    }
}

/// Handle key events in success screen
fn update_success_key(mut model: TuiModel, key: crossterm::event::KeyEvent) -> UpdateResult {
    match key.code {
        KeyCode::Enter => {
            model.should_exit = true;
            UpdateResult::with_effect(model, TuiEffect::Exit)
        }
        KeyCode::Esc => {
            model.screen = Screen::ProfileList;
            UpdateResult::no_effect(model)
        }
        _ => UpdateResult::no_effect(model),
    }
}

/// Handle profiles loaded message
fn update_profiles_loaded(mut model: TuiModel, result: ProfilesLoadedResult) -> UpdateResult {
    model.profiles.loading = false;

    match result.profiles {
        Ok(profiles) => {
            model.search_query.clear();
            model.searching = false;
            model.profiles.all_profiles = profiles.clone();
            model.profiles.filtered_profiles = profiles;
            model.profiles.selected_index = 0;
            let count = model.profiles.all_profiles.len();
            UpdateResult::with_effect(
                model,
                TuiEffect::debug(format!("Loaded {} profiles", count)),
            )
        }
        Err(err) => {
            model.screen = Screen::Error(ErrorModel {
                message: format!("Failed to load profiles: {}", err),
                console_url_on_quit: false,
            });
            UpdateResult::no_effect(model)
        }
    }
}

/// Handle assume role completed message
fn update_assume_role_completed(mut model: TuiModel, result: AssumeRoleResult) -> UpdateResult {
    match result.result {
        Ok(creds) => {
            let profile_name = result.profile_name;
            let credentials = creds.credentials;
            let write_credentials = CredentialsToWrite {
                access_key_id: credentials.access_key_id().to_string(),
                secret_access_key: credentials.secret_access_key().to_string(),
                session_token: credentials.session_token().map(str::to_string),
                region: creds.region.clone(),
                expiration: credentials.expiration(),
            };
            let mut effects = vec![
                TuiEffect::WriteCredentials(WriteCredentialsEffect {
                    profile_name: profile_name.clone(),
                    credentials: write_credentials,
                    readonly: model.mode.readonly,
                }),
                TuiEffect::ShowNotification(NotificationEffect {
                    message: "Role assumed successfully".to_string(),
                }),
            ];
            if model.mode.console_launch {
                effects.push(TuiEffect::GenerateConsoleUrl(ConsoleUrlEffect {
                    credentials: CredentialsForConsole {
                        access_key_id: credentials.access_key_id().to_string(),
                        secret_access_key: credentials.secret_access_key().to_string(),
                        session_token: credentials.session_token().map(str::to_string),
                    },
                }));
            }
            model.screen = Screen::Success(SuccessModel {
                profile: profile_name,
                access_key_id: creds.access_key_id,
            });
            UpdateResult::with_effects(model, effects)
        }
        Err(AssumeRoleError::MfaRequired { serial }) => {
            model.screen = Screen::MfaInput;
            model.mfa_input = MfaInputModel {
                serial: serial.clone(),
                profile: result.profile_name,
                value: String::new(),
            };
            UpdateResult::no_effect(model)
        }
        Err(AssumeRoleError::Failed { message }) => {
            model.screen = Screen::Error(ErrorModel {
                message,
                console_url_on_quit: false,
            });
            UpdateResult::no_effect(model)
        }
    }
}

/// Handle console URL generated message
fn update_console_url_generated(
    mut model: TuiModel,
    result: super::messages::ConsoleUrlResult,
) -> UpdateResult {
    match result.result {
        Ok(url) => UpdateResult::with_effect(model, TuiEffect::OpenBrowser { url }),
        Err(err) => {
            model.screen = Screen::Error(ErrorModel {
                message: format!("Failed to generate console URL: {}", err),
                console_url_on_quit: false,
            });
            UpdateResult::no_effect(model)
        }
    }
}

/// A browser that did not open leaves the URL to the user, printed when the
/// TUI ends; the credentials were already held for the shell.
fn update_browser_opened(
    mut model: TuiModel,
    result: super::messages::BrowserOpenResult,
) -> UpdateResult {
    if let Err(error) = result.result {
        model.screen = Screen::Error(ErrorModel {
            message: format!("Failed to open the browser: {error}"),
            console_url_on_quit: true,
        });
    }
    UpdateResult::no_effect(model)
}

/// Check if key is a quit key
fn is_quit_key(key: &crossterm::event::KeyEvent) -> bool {
    matches!(
        (key.code, key.modifiers),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Char('q'), KeyModifiers::NONE)
    )
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
