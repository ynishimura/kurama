//! `[aws]`: the session name template kurama gives AssumeRole, validated
//! while the file is read, and the MFA session cache.

use crate::domain::functions::{SessionNameConfig, validate_session_name_template};
use serde::{Deserialize, Serialize};

use super::default_true;

/// AWS settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AwsConfig {
    /// Validated while the file is read: an unknown placeholder in the
    /// template is a `CONFIG_INVALID` error, and no later step re-validates.
    #[serde(
        default,
        deserialize_with = "deserialize_session_name",
        serialize_with = "serialize_session_name"
    )]
    pub session_name: SessionNameConfig,
    #[serde(default)]
    pub session_cache: SessionCacheConfig,
}

fn deserialize_session_name<'de, D>(
    deserializer: D,
) -> std::result::Result<SessionNameConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let toml = SessionNameToml::deserialize(deserializer)?;
    validate_session_name_template(&toml.template).map_err(|reason| {
        serde::de::Error::custom(format!(
            "Invalid [aws.session_name] template '{}': {reason}",
            toml.template
        ))
    })?;
    Ok(SessionNameConfig::new()
        .with_template(toml.template)
        .with_prefix(toml.prefix)
        .with_readonly_indicator(toml.readonly_indicator))
}

fn serialize_session_name<S>(
    config: &SessionNameConfig,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    SessionNameToml {
        template: config.template.clone(),
        prefix: config.prefix.clone(),
        readonly_indicator: config.readonly_indicator.clone(),
    }
    .serialize(serializer)
}

/// The TOML shape of `[aws.session_name]`; it becomes a `SessionNameConfig`
/// while the file is read.
///
/// # Example config.toml
/// ```toml
/// [aws.session_name]
/// template = "{prefix}-{readonly}-{profile}"  # default
/// prefix = "kurama"                           # default
/// readonly_indicator = "ro"                   # default
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionNameToml {
    /// Template with {prefix}, {profile}, {readonly}, {role}, {account}
    /// placeholders
    #[serde(default = "default_session_name_template")]
    pub template: String,
    /// Value for the {prefix} placeholder
    #[serde(default = "default_session_name_prefix")]
    pub prefix: String,
    /// Value for the {readonly} placeholder in readonly mode
    #[serde(default = "default_session_name_readonly_indicator")]
    pub readonly_indicator: String,
}

impl Default for SessionNameToml {
    fn default() -> Self {
        let domain = SessionNameConfig::new();
        Self {
            template: domain.template,
            prefix: domain.prefix,
            readonly_indicator: domain.readonly_indicator,
        }
    }
}

fn default_session_name_template() -> String {
    SessionNameConfig::new().template
}

fn default_session_name_prefix() -> String {
    SessionNameConfig::new().prefix
}

fn default_session_name_readonly_indicator() -> String {
    SessionNameConfig::new().readonly_indicator
}

/// Settings for MFA-authenticated IAM-user sessions (independent of role duration).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCacheConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_session_cache_duration")]
    pub duration: u64,
}

const fn default_session_cache_duration() -> u64 {
    43200
}

impl Default for SessionCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            duration: default_session_cache_duration(),
        }
    }
}
