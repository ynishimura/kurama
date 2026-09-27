//! Secret resolver port: turns a `SecretRef` (an `op://` or `aws-*://`
//! reference, or a literal) into the secret value when a token request or a
//! database connection needs it.

use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::types::{AwsSecretStore, SecretFailure, SecretRef};

/// A secret that could not be read. One type with a typed `failure`, because
/// every caller wants the same two things out of it -- what to print and what
/// to exit with -- and a message is no way to tell a refusal from an
/// unreachable service.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct SecretError {
    pub failure: SecretFailure,
    pub message: String,
    /// The typed chain of a failure the resolver did not classify itself.
    /// Getting the credentials an `aws-*://` reference needs is the AssumeRole
    /// path, whose MFA, STS and 1Password errors already carry codes of their
    /// own, and flattening them here would report a wrong TOTP as a malformed
    /// reference. `ErrorCode::classify` reads this before `failure`.
    /// `anyhow::Error` is no `std::error::Error`, so this is no `#[source]`.
    pub cause: Option<Arc<anyhow::Error>>,
}

impl SecretError {
    /// The secret backend could not answer (1Password not signed in, item
    /// missing, CLI absent); a person has to act.
    pub fn needs_a_person(message: impl Into<String>) -> Self {
        Self::new(SecretFailure::NeedsAPerson, message)
    }

    /// The reference names something unusable, or the value is not what the
    /// reference says it is.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(SecretFailure::Invalid, message)
    }

    /// The store answered and refused; `not_found` when it said the name
    /// does not exist.
    pub fn rejected(store: AwsSecretStore, not_found: bool, message: impl Into<String>) -> Self {
        Self::new(SecretFailure::Rejected { store, not_found }, message)
    }

    /// The store could not be reached, or its SDK failed.
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::new(SecretFailure::Unreachable, message)
    }

    /// A failure this resolver did not classify, such as getting the
    /// credentials an `aws-*://` reference needs: a wrong TOTP is exit 3 and a
    /// role without `sts:AssumeRole` is exit 4, and neither is this reference
    /// being wrong. `failure` is what is left when the chain classifies as
    /// nothing.
    ///
    /// The message is built here, from the same error that is kept, because
    /// that is what makes flattening a chain into text and losing it two
    /// things nobody can do separately.
    pub fn unclassified(
        failure: SecretFailure,
        context: impl std::fmt::Display,
        cause: anyhow::Error,
    ) -> Self {
        // One line: the message is one `error[CODE]:` line and what to do
        // about it belongs in the hint.
        let message = format!("{context}: {}", format!("{cause:#}").replace('\n', " "));
        Self {
            failure,
            message,
            cause: Some(Arc::new(cause)),
        }
    }

    fn new(failure: SecretFailure, message: impl Into<String>) -> Self {
        Self {
            failure,
            message: message.into(),
            cause: None,
        }
    }
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, secret: &SecretRef) -> Result<String, SecretError>;
}
