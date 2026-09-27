//! CLI effect types ("Effects as Data").
//!
//! Command handlers return a list of effects describing *what* to do; the
//! executor in `super::executor` performs the I/O. The planning functions are
//! pure and unit-tested without mocks.
//!
//! ```ignore
//! fn plan_profile_command(config: &ProfileCommandConfig) -> Vec<ProfileEffect> {
//!     vec![
//!         ProfileEffect::info("Loading profile..."),
//!         ProfileEffect::LoadProfiles,
//!         ProfileEffect::SelectProfile { profile_name: "prod".into() },
//!         ProfileEffect::AssumeRole { readonly: false },
//!         ProfileEffect::OutputCredentials { output_format: OutputFormat::Shell },
//!     ]
//! }
//! ```

use crate::domain::{Credentials, OutputFormat, Profile};

/// Effects of `env`, `exec` and `console`, in execution order:
/// LoadProfiles -> SelectProfile -> AssumeRole ->
/// OutputCredentials + PrintSuccess | ExecCommand | OpenAwsConsole.
#[derive(Debug, Clone)]
pub enum ProfileEffect {
    /// Log a message at info level (to tracing)
    Log { message: String },

    /// Print a progress line for the user on stderr
    PrintMessage { message: String },

    /// Load all available profiles from AWS config
    LoadProfiles,

    /// Select a specific profile from loaded profiles
    SelectProfile { profile_name: String },

    /// Execute the AssumeRole workflow via Runtime
    AssumeRole { readonly: bool },

    /// Output credentials in the specified format
    OutputCredentials { output_format: OutputFormat },

    /// Replace kurama with the command, role credentials in its environment
    ExecCommand { command: Vec<String> },

    /// Open AWS console in browser
    OpenAwsConsole,

    /// Print success message
    PrintSuccess { profile_name: String },
}

/// State accumulated while executing profile effects.
#[derive(Debug, Clone, Default)]
pub struct ProfileEffectContext {
    /// All loaded profiles (set by LoadProfiles effect)
    pub profiles: Vec<Profile>,

    /// Selected profile (set by SelectProfile effect)
    pub selected_profile: Option<Profile>,

    /// Retrieved credentials (set by AssumeRole effect)
    pub credentials: Option<Credentials>,

    /// Session name (set by AssumeRole effect)
    pub session_name: Option<String>,

    /// Whether the readonly policy was attached (set by AssumeRole effect);
    /// the export script marks the session with `AWS_READONLY_SESSION`.
    pub readonly: bool,
}

impl ProfileEffectContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn profile(&self) -> Option<&Profile> {
        self.selected_profile.as_ref()
    }
}

/// Effects of the unset command
#[derive(Debug, Clone)]
pub enum UnsetEffect {
    /// Log a message at info level
    Log { message: String },

    /// Output the unset script; `token_vars` are the variables of the
    /// `[auth.*]` sources (`token_managed_vars`)
    OutputUnsetScript { token_vars: Vec<String> },

    /// Print success message
    PrintSuccess,
}

impl ProfileEffect {
    pub fn info(message: impl Into<String>) -> Self {
        Self::Log {
            message: message.into(),
        }
    }

    pub fn print_stderr(message: impl Into<String>) -> Self {
        Self::PrintMessage {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_effect_builders() {
        assert!(matches!(
            ProfileEffect::info("test info"),
            ProfileEffect::Log { message } if message == "test info"
        ));
        assert!(matches!(
            ProfileEffect::print_stderr("user message"),
            ProfileEffect::PrintMessage { message } if message == "user message"
        ));
    }

    #[test]
    fn test_profile_effect_context_default() {
        let ctx = ProfileEffectContext::default();
        assert!(ctx.profiles.is_empty());
        assert!(ctx.selected_profile.is_none());
        assert!(ctx.credentials.is_none());
        assert!(!ctx.readonly);
    }

    #[test]
    fn test_profile_effect_context_profile() {
        let mut ctx = ProfileEffectContext::default();
        assert!(ctx.profile().is_none());

        ctx.selected_profile = Some(Profile::new("test"));
        assert_eq!(ctx.profile().unwrap().name(), "test");
    }
}
