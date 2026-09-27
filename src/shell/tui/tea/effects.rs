//! TUI Effects - Side effects as data
//!
//! Effects represent all side effects that need to be performed.
//! They are returned by the pure `update` function and executed by the runtime.

/// All possible effects in the TUI application
#[derive(Clone)]
pub enum TuiEffect {
    /// Load profiles from disk
    LoadProfiles,

    /// Assume role for a profile
    AssumeRole(AssumeRoleEffect),

    /// Generate console URL
    GenerateConsoleUrl(ConsoleUrlEffect),

    /// Open URL in browser
    OpenBrowser { url: String },

    /// Copy an example command
    CopyToClipboard { text: String },

    /// Read the palette's operations (descriptions on disk only) and the
    /// explorer's history
    LoadPaletteItems,

    /// Write credentials to environment
    WriteCredentials(WriteCredentialsEffect),

    /// Show notification
    ShowNotification(NotificationEffect),

    /// Log a debug message
    Log(LogEffect),

    /// Exit the application
    Exit,
}

impl std::fmt::Debug for TuiEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadProfiles => f.write_str("LoadProfiles"),
            Self::LoadPaletteItems => f.write_str("LoadPaletteItems"),
            Self::AssumeRole(effect) => f.debug_tuple("AssumeRole").field(effect).finish(),
            Self::GenerateConsoleUrl(effect) => {
                f.debug_tuple("GenerateConsoleUrl").field(effect).finish()
            }
            Self::OpenBrowser { url: _ } => f
                .debug_struct("OpenBrowser")
                .field("url", &"[REDACTED]")
                .finish(),
            Self::CopyToClipboard { text } => f
                .debug_struct("CopyToClipboard")
                .field("text", text)
                .finish(),
            Self::WriteCredentials(effect) => {
                f.debug_tuple("WriteCredentials").field(effect).finish()
            }
            Self::ShowNotification(effect) => {
                f.debug_tuple("ShowNotification").field(effect).finish()
            }
            Self::Log(effect) => f.debug_tuple("Log").field(effect).finish(),
            Self::Exit => f.write_str("Exit"),
        }
    }
}

/// Effect for assuming a role
#[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct AssumeRoleEffect {
    pub profile_name: String,
    pub mfa_token: Option<String>,
    #[zeroize(skip)]
    pub readonly: bool,
}

impl std::fmt::Debug for AssumeRoleEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssumeRoleEffect")
            .field("profile_name", &self.profile_name)
            .field("mfa_token", &self.mfa_token.as_ref().map(|_| "[REDACTED]"))
            .field("readonly", &self.readonly)
            .finish()
    }
}

/// Effect for generating console URL
#[derive(Debug, Clone)]
pub struct ConsoleUrlEffect {
    pub credentials: CredentialsForConsole,
}

/// Credentials needed for console URL generation
#[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct CredentialsForConsole {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl std::fmt::Debug for CredentialsForConsole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialsForConsole")
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .field("session_token", &"[REDACTED]")
            .finish()
    }
}

/// Effect for writing credentials
#[derive(Clone)]
pub struct WriteCredentialsEffect {
    pub profile_name: String,
    pub credentials: CredentialsToWrite,
    pub readonly: bool,
}

impl std::fmt::Debug for WriteCredentialsEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteCredentialsEffect")
            .field("profile_name", &self.profile_name)
            .field("credentials", &self.credentials)
            .field("readonly", &self.readonly)
            .finish()
    }
}

/// Credentials to write
#[derive(Clone)]
pub struct CredentialsToWrite {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: Option<String>,
    pub expiration: Option<chrono::DateTime<chrono::Utc>>,
}

impl zeroize::Zeroize for CredentialsToWrite {
    fn zeroize(&mut self) {
        self.access_key_id.zeroize();
        self.secret_access_key.zeroize();
        self.session_token.zeroize();
    }
}

impl zeroize::ZeroizeOnDrop for CredentialsToWrite {}

impl std::fmt::Debug for CredentialsToWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialsToWrite")
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .field("session_token", &"[REDACTED]")
            .field("region", &self.region)
            .field("expiration", &self.expiration)
            .finish()
    }
}

/// Effect for showing notification
#[derive(Debug, Clone)]
pub struct NotificationEffect {
    pub message: String,
}

/// Effect for logging a debug message
#[derive(Debug, Clone)]
pub struct LogEffect {
    pub message: String,
}

impl TuiEffect {
    /// Create a debug log effect
    pub fn debug(message: impl Into<String>) -> Self {
        TuiEffect::Log(LogEffect {
            message: message.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_bearing_effects_debug_are_redacted() {
        let effects = [
            TuiEffect::AssumeRole(AssumeRoleEffect {
                profile_name: "prod".into(),
                mfa_token: Some("mfa-token".into()),
                readonly: false,
            }),
            TuiEffect::WriteCredentials(WriteCredentialsEffect {
                profile_name: "prod".into(),
                credentials: CredentialsToWrite {
                    access_key_id: "ASIA123".into(),
                    secret_access_key: "secret-access-key".into(),
                    session_token: Some("session-token".into()),
                    region: None,
                    expiration: None,
                },
                readonly: false,
            }),
            TuiEffect::GenerateConsoleUrl(ConsoleUrlEffect {
                credentials: CredentialsForConsole {
                    access_key_id: "ASIA123".into(),
                    secret_access_key: "secret-access-key".into(),
                    session_token: Some("session-token".into()),
                },
            }),
            TuiEffect::OpenBrowser {
                url: "https://signin.aws.amazon.com/federation?SigninToken=session-token".into(),
            },
        ];

        for effect in effects {
            let debug = format!("{effect:?}");
            assert!(!debug.contains("mfa-token"));
            assert!(!debug.contains("secret-access-key"));
            assert!(!debug.contains("session-token"));
        }
    }
}
