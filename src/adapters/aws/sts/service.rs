//! AWS STS service wrapper
//!
//! This module provides a simple wrapper around the AWS STS client.
//! Implements the `StsOperations` port for dependency injection.

use crate::adapters::aws::config_builder::AwsConfigBuilder;
use crate::adapters::config::Config;
use crate::adapters::error::CoreError;
use crate::domain::functions::error_mapping::{StsErrorKind, classify_sts_error};
use crate::domain::types::Profile;
use crate::ports::sts::{AssumeRoleRequest, StsCredentials, StsError, StsOperations};
use async_trait::async_trait;
use aws_sdk_sts::{
    Client as StsClient,
    error::{ProvideErrorMetadata, SdkError},
    types::{Credentials as SdkCredentials, PolicyDescriptorType},
};
use tracing::{debug, info, warn};

/// Map AWS SDK errors to CoreError with consistent formatting
/// This is a pure function that handles error classification and message building
fn map_sdk_error_to_core_error<E, R>(
    error: &SdkError<E, R>,
    resource: &str,
    operation: &str,
) -> CoreError
where
    E: ProvideErrorMetadata + std::fmt::Display,
    R: std::fmt::Debug,
{
    match error {
        SdkError::ServiceError(service_err) => {
            let err = service_err.err();
            warn!(
                error_code = ?err.code(),
                error_message = ?err.message(),
                "AWS STS service error"
            );
            CoreError::aws_sdk(format!(
                "Failed to {} '{}': {}: {}",
                operation,
                resource,
                err.code().unwrap_or("Unknown"),
                err.message().unwrap_or("No error message")
            ))
        }
        SdkError::TimeoutError(_) => {
            warn!("{} request timed out", operation);
            CoreError::aws_sdk(format!(
                "Request timeout while {} '{}'",
                operation, resource
            ))
        }
        SdkError::DispatchFailure(dispatch_err) => {
            warn!(error = ?dispatch_err, "Network/dispatch failure");
            CoreError::aws_sdk(format!(
                "Network error while {} '{}': {}",
                operation, resource, error
            ))
        }
        _ => CoreError::aws_sdk(format!("Failed to {} '{}': {}", operation, resource, error)),
    }
}

/// Simple AWS STS service wrapper
pub struct StsService {
    client: StsClient,
    /// The provider the client signs with, kept to hand its keys out: the
    /// STS client's own configuration does not give it back.
    keys: Option<aws_credential_types::provider::SharedCredentialsProvider>,
}

impl StsService {
    /// An STS client for the SDK configuration of the selected `profile`: the
    /// 1Password credentials provider for it when the configuration enables
    /// it, and otherwise its `source_profile`, whose keys and region sign.
    /// The selected profile itself is not handed to the SDK without 1Password:
    /// the SDK would assume its role on its own before kurama does. A
    /// profile with no `role_arn` (an IAM user) has no role for the SDK to
    /// assume, so its own keys sign.
    pub async fn from_config(kurama_config: &Config, profile: &Profile) -> Self {
        let mut builder = AwsConfigBuilder::new();
        if kurama_config.onepassword.enabled {
            builder = builder
                .with_onepassword(kurama_config.onepassword.clone())
                .with_profile(profile.name());
        } else if let Some(source) = profile.source_profile() {
            builder = builder.with_profile(source);
        } else if !profile.can_assume_role() {
            builder = builder.with_profile(profile.name());
        }
        let config = builder.build().await;
        debug!(
            region = ?config.region().map(|r| r.as_ref()),
            endpoint_url = ?config.endpoint_url(),
            onepassword = kurama_config.onepassword.enabled,
            "STS client configured"
        );
        Self {
            client: StsClient::new(&config),
            keys: config.credentials_provider(),
        }
    }

    /// Assume role using AWS STS with policy ARNs
    pub async fn assume_role_with_policy_arns(
        &self,
        role_arn: &str,
        session_name: &str,
        duration: Option<u64>,
        mfa_serial: Option<String>,
        mfa_token: Option<String>,
        policy_arns: Option<Vec<String>>,
    ) -> Result<SdkCredentials, CoreError> {
        info!(
            role_arn = %role_arn,
            session_name = %session_name,
            duration_seconds = ?duration,
            mfa_required = mfa_serial.is_some(),
            policy_arns_count = policy_arns.as_ref().map(|v| v.len()).unwrap_or(0),
            "Starting AssumeRole request"
        );

        let mut request = self
            .client
            .assume_role()
            .role_arn(role_arn)
            .role_session_name(session_name);

        if let Some(duration) = duration {
            debug!("Setting duration_seconds: {}", duration);
            request = request.duration_seconds(duration as i32);
        }

        if let Some(serial) = mfa_serial {
            if let Some(token) = mfa_token {
                debug!(mfa_serial = %serial, "MFA authentication configured");
                request = request.serial_number(serial).token_code(token);
            } else {
                warn!(mfa_serial = %serial, "MFA token required but not provided");
                return Err(CoreError::mfa_required(serial));
            }
        }

        // Add policy ARNs if provided (for readonly mode)
        if let Some(ref arns) = policy_arns {
            debug!(policy_arns = ?arns, "Adding policy ARNs for session restriction");
            for arn in arns {
                request = request.policy_arns(PolicyDescriptorType::builder().arn(arn).build());
            }
        }

        debug!("Sending AssumeRole request to AWS STS...");

        let response = request.send().await.map_err(|e| {
            warn!(
                role_arn = %role_arn,
                error_type = %std::any::type_name_of_val(&e),
                "AssumeRole request failed"
            );
            map_sdk_error_to_core_error(&e, role_arn, "assume role")
        })?;

        let credentials = response.credentials().cloned().ok_or_else(|| {
            warn!("AssumeRole succeeded but no credentials returned");
            CoreError::other("No credentials returned from STS")
        })?;

        info!(
            role_arn = %role_arn,
            session_name = %session_name,
            expiration = ?credentials.expiration(),
            "AssumeRole succeeded"
        );

        Ok(credentials)
    }
}

/// Implementation of StsOperations port for StsService
///
/// This allows StsService to be used through the port abstraction,
/// enabling dependency injection and easier testing.
#[async_trait]
impl StsOperations for StsService {
    async fn get_session_token(
        &self,
        request: crate::ports::sts::GetSessionTokenRequest,
    ) -> Result<StsCredentials, StsError> {
        let response = self
            .client
            .get_session_token()
            .serial_number(request.mfa_serial)
            .token_code(request.mfa_token)
            .duration_seconds(request.duration_seconds as i32)
            .send()
            .await
            .map_err(|error| {
                self.map_core_error_to_sts_error(map_sdk_error_to_core_error(
                    &error,
                    "MFA session",
                    "get session token",
                ))
            })?;
        response
            .credentials()
            .map(convert_credentials)
            .ok_or_else(|| StsError::ServiceError("No credentials returned from STS".into()))
    }

    async fn assume_role(&self, request: AssumeRoleRequest) -> Result<StsCredentials, StsError> {
        // Override the signer for this call only; GetSessionToken keeps the default provider.
        let session_service = request.source_credentials.as_ref().map(|source| {
            let credentials = aws_credential_types::Credentials::new(
                source.access_key_id.clone(),
                source.secret_access_key.clone(),
                Some(source.session_token.clone()),
                None,
                "kurama-session",
            );
            StsService {
                client: StsClient::from_conf(
                    self.client
                        .config()
                        .to_builder()
                        .credentials_provider(credentials)
                        .build(),
                ),
                keys: None,
            }
        });
        let (serial, token) = if session_service.is_some() {
            (None, None)
        } else {
            (request.mfa_serial, request.mfa_token)
        };
        let credentials = session_service
            .as_ref()
            .unwrap_or(self)
            .assume_role_with_policy_arns(
                &request.role_arn,
                &request.session_name,
                request.duration_seconds,
                serial,
                token,
                request.policy_arns,
            )
            .await
            .map_err(|e| self.map_core_error_to_sts_error(e))?;

        Ok(convert_credentials(&credentials))
    }

    async fn read_signing_keys(&self) -> Result<crate::domain::Credentials, StsError> {
        use aws_credential_types::provider::ProvideCredentials;
        let provider = self.keys.as_ref().ok_or_else(|| {
            StsError::ServiceError("no AWS credentials are configured for the profile".into())
        })?;
        let keys = provider.provide_credentials().await.map_err(|error| {
            StsError::ServiceError(format!(
                "the profile's AWS credentials could not be read: {}",
                aws_sdk_sts::error::DisplayErrorContext(&error)
            ))
        })?;
        Ok(crate::domain::Credentials::new(
            keys.access_key_id().to_string(),
            keys.secret_access_key().to_string(),
            keys.session_token().map(str::to_string),
            keys.expiry().map(chrono::DateTime::<chrono::Utc>::from),
        ))
    }
}

fn convert_credentials(credentials: &SdkCredentials) -> StsCredentials {
    let expiration = credentials.expiration();
    StsCredentials {
        access_key_id: credentials.access_key_id().to_string(),
        secret_access_key: credentials.secret_access_key().to_string(),
        session_token: credentials.session_token().to_string(),
        expiration: chrono::DateTime::from_timestamp(expiration.secs(), expiration.subsec_nanos()),
    }
}

impl StsService {
    /// Map CoreError to StsError for the port interface
    fn map_core_error_to_sts_error(&self, error: CoreError) -> StsError {
        map_core_error_to_sts_error_impl(error)
    }
}

/// Internal function for mapping CoreError to StsError (allows testing without StsService instance)
/// Uses pure functions from domain layer for error classification
fn map_core_error_to_sts_error_impl(error: CoreError) -> StsError {
    let error_str = error.to_string();
    let error_kind = classify_sts_error(&error_str);

    match error_kind {
        StsErrorKind::MfaRequired => StsError::MfaRequired,
        StsErrorKind::InvalidMfaToken => StsError::InvalidMfaToken,
        StsErrorKind::AccessDenied => StsError::AccessDenied(error_str),
        StsErrorKind::RoleNotFound => StsError::RoleNotFound(error_str),
        StsErrorKind::InvalidCredentials => StsError::InvalidCredentials,
        StsErrorKind::ServiceError => StsError::ServiceError(error_str),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod error_mapping_tests {
        use super::*;

        #[test]
        fn maps_mfa_required_error() {
            let error = CoreError::mfa_required("arn:aws:iam::123456789012:mfa/user");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::MfaRequired));
        }

        #[test]
        fn maps_invalid_mfa_token_error() {
            let error = CoreError::other("Invalid MFA token provided");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::InvalidMfaToken));
        }

        #[test]
        fn maps_access_denied_error() {
            let error = CoreError::aws_sdk("AccessDenied: User is not authorized");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::AccessDenied(_)));
        }

        #[test]
        fn maps_role_not_found_error() {
            let error = CoreError::other("Role not found: TestRole");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::RoleNotFound(_)));
        }

        #[test]
        fn maps_invalid_credentials_error() {
            let error = CoreError::aws_sdk("InvalidClientTokenId: The security token is invalid");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::InvalidCredentials));
        }

        #[test]
        fn maps_signature_mismatch_to_invalid_credentials() {
            let error = CoreError::aws_sdk("SignatureDoesNotMatch: Credential mismatch");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::InvalidCredentials));
        }

        #[test]
        fn maps_generic_error_to_service_error() {
            let error = CoreError::other("Something went wrong");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::ServiceError(_)));
        }

        #[test]
        fn maps_no_such_entity_to_role_not_found() {
            let error = CoreError::aws_sdk("NoSuchEntity: Role does not exist");
            let sts_error = map_core_error_to_sts_error_impl(error);
            assert!(matches!(sts_error, StsError::RoleNotFound(_)));
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod session_tests;
