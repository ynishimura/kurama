//! Error handling module
//!
//! This module provides unified error types and handling utilities.

use thiserror::Error;

/// Core error type that unifies all layer-specific errors
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("MFA required for profile: {0}")]
    MfaRequired(String),

    #[error("Internal error: {0}")]
    Internal(String),
    #[error("{0}")]
    Other(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    JsonParse(#[from] serde_json::Error),

    #[error(transparent)]
    TomlParse(#[from] toml::de::Error),

    #[error("AWS SDK error: {0}")]
    AwsSdk(String),

    /// An unknown key that this kurama reads in other sections: misplaced, or
    /// written for a newer kurama. Only the key and section names are kept.
    #[error("Configuration error: {message}")]
    KeyOfAnotherSection {
        message: String,
        key: String,
        sections: Vec<String>,
    },
}

impl CoreError {
    /// Create a new configuration error
    pub fn config<S: Into<String>>(msg: S) -> Self {
        Self::Configuration(msg.into())
    }

    /// Create a new other error
    pub fn other<S: Into<String>>(msg: S) -> Self {
        Self::Other(msg.into())
    }

    /// Create an MFA required error
    pub fn mfa_required<S: Into<String>>(profile: S) -> Self {
        Self::MfaRequired(profile.into())
    }

    /// Create an AWS SDK error
    pub fn aws_sdk<S: Into<String>>(msg: S) -> Self {
        Self::AwsSdk(msg.into())
    }

    /// Create a OnePassword error
    pub fn onepassword_error<S: Into<String>>(msg: S) -> Self {
        Self::other(format!("OnePassword error: {}", msg.into()))
    }
}

/// Result type alias using CoreError
pub type CoreResult<T> = std::result::Result<T, CoreError>;
