//! Read a secret AWS holds: Secrets Manager `GetSecretValue` and Parameter
//! Store `GetParameter`, with the role credentials of an AWS profile.
//!
//! Both are read with one call each and nothing is written anywhere: the value
//! goes back to the caller and no further.

use aws_sdk_secretsmanager::config::{
    BehaviorVersion, Region, retry::RetryConfig, timeout::TimeoutConfig,
};
use aws_sdk_secretsmanager::error::ProvideErrorMetadata;
use std::time::Duration;
use tracing::debug;

use crate::adapters::utils::error_chain::causes;
use crate::domain::types::{AwsSecretStore, Credentials, Secret};
use crate::ports::SecretError;

/// How long one read may take end to end.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Read the value of one secret. `id` is a name or an ARN for Secrets Manager
/// and a parameter name for Parameter Store; a `SecureString` is always
/// decrypted, because a caller that asked for the secret asked for its value.
pub async fn read(
    store: AwsSecretStore,
    credentials: &Credentials,
    region: &str,
    id: &str,
) -> Result<Secret, SecretError> {
    debug!(store = store.scheme(), region, "Reading a secret from AWS");
    match store {
        AwsSecretStore::SecretsManager => secrets_manager(credentials, region, id).await,
        AwsSecretStore::ParameterStore => parameter_store(credentials, region, id).await,
    }
    .map(Secret::new)
}

async fn secrets_manager(
    credentials: &Credentials,
    region: &str,
    id: &str,
) -> Result<String, SecretError> {
    let config = aws_sdk_secretsmanager::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .credentials_provider(super::sdk_credentials(credentials))
        .region(Region::new(region.to_owned()))
        .retry_config(RetryConfig::disabled())
        .timeout_config(
            TimeoutConfig::builder()
                .operation_timeout(OPERATION_TIMEOUT)
                .build(),
        );
    #[cfg(feature = "test-fakes")]
    let config = match std::env::var("KURAMA_TEST_SECRETSMANAGER_ENDPOINT") {
        Ok(endpoint) => config.endpoint_url(endpoint),
        Err(_) => config,
    };
    let secret = aws_sdk_secretsmanager::Client::from_conf(config.build())
        .get_secret_value()
        .secret_id(id)
        .send()
        .await
        .map_err(|error| failure(AwsSecretStore::SecretsManager, "GetSecretValue", error))?;
    // A binary secret is bytes nobody can put in a password field; saying so
    // is better than handing over base64 that would fail at the server.
    secret.secret_string().map(str::to_owned).ok_or_else(|| {
        SecretError::invalid(format!(
            "the secret {id} holds SecretBinary, which kurama does not read"
        ))
    })
}

async fn parameter_store(
    credentials: &Credentials,
    region: &str,
    name: &str,
) -> Result<String, SecretError> {
    let answer = super::ssm_client(credentials, region)
        .get_parameter()
        .name(name)
        .with_decryption(true)
        .send()
        .await
        .map_err(|error| failure(AwsSecretStore::ParameterStore, "GetParameter", error))?;
    answer
        .parameter()
        .and_then(|parameter| parameter.value())
        .map(str::to_owned)
        .ok_or_else(|| SecretError::invalid(format!("the parameter {name} has no value")))
}

/// A service that answered refused; anything else never reached it. The two
/// need different things done about them, so they are different failures: a
/// refusal is a permission or a name to fix, and no answer is a network or an
/// endpoint, which retrying can get past.
///
/// Whether the service answered is `as_service_error`, not the presence of an
/// error code: a modeled error carries its code only once a response has been
/// deserialized into it, so reading the code would call an answer no answer.
fn failure<E, R>(
    store: AwsSecretStore,
    operation: &str,
    error: aws_sdk_ssm::error::SdkError<E, R>,
) -> SecretError
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug,
{
    let Some(refusal) = error.as_service_error() else {
        return SecretError::unreachable(format!("{operation}: {}", causes(&error)));
    };
    let named = match (refusal.code(), refusal.message()) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_owned(),
        (None, Some(message)) => message.to_owned(),
        (None, None) => causes(refusal),
    };
    let not_found = refusal.code() == Some(store.not_found_code());
    SecretError::rejected(store, not_found, format!("{operation} {named}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::SecretFailure;
    use aws_sdk_ssm::error::SdkError;
    use aws_sdk_ssm::operation::get_parameter::GetParameterError;
    use aws_sdk_ssm::types::error::ParameterNotFound;

    /// A service that answered is a permission or a name to fix (exit 4); a
    /// call that never reached one is a network to retry (exit 1). Telling
    /// them apart by the answer, not by the message, is what keeps the exit
    /// code honest.
    #[test]
    fn an_answer_is_a_refusal_and_no_answer_is_an_unreachable_store() {
        let refused: SdkError<GetParameterError, ()> = SdkError::service_error(
            GetParameterError::ParameterNotFound(
                ParameterNotFound::builder()
                    .message("Parameter /app/db not found.")
                    .build(),
            ),
            (),
        );
        let error = failure(AwsSecretStore::ParameterStore, "GetParameter", refused);
        assert_eq!(
            error.failure,
            SecretFailure::Rejected {
                store: AwsSecretStore::ParameterStore,
                not_found: false,
            }
        );
        assert!(error.to_string().contains("GetParameter"), "{error}");
        assert!(error.to_string().contains("/app/db"), "{error}");

        // The arm a refusal deserialized from a response really takes: AWS
        // fills the code and the message, and both are the only thing that
        // says what the service actually refused. The case above leaves `meta`
        // empty, so it exercises a different arm than production ever does.
        let denied: SdkError<GetParameterError, ()> = SdkError::service_error(
            GetParameterError::ParameterNotFound(
                ParameterNotFound::builder()
                    .message("is not authorized to perform ssm:GetParameter")
                    .meta(
                        aws_smithy_types::error::ErrorMetadata::builder()
                            .code("AccessDeniedException")
                            .message("is not authorized to perform ssm:GetParameter")
                            .build(),
                    )
                    .build(),
            ),
            (),
        );
        let error = failure(AwsSecretStore::ParameterStore, "GetParameter", denied);
        assert_eq!(
            error.failure,
            SecretFailure::Rejected {
                store: AwsSecretStore::ParameterStore,
                not_found: false,
            }
        );
        assert!(
            error.to_string().contains("AccessDeniedException"),
            "{error}"
        );
        assert!(error.to_string().contains("is not authorized"), "{error}");

        // The code a store answers for a name that does not exist is what
        // `not_found` reads; the other store's code is not it.
        for (store, code, not_found) in [
            (AwsSecretStore::ParameterStore, "ParameterNotFound", true),
            (
                AwsSecretStore::SecretsManager,
                "ResourceNotFoundException",
                true,
            ),
            (AwsSecretStore::SecretsManager, "ParameterNotFound", false),
        ] {
            let missing: SdkError<GetParameterError, ()> = SdkError::service_error(
                GetParameterError::ParameterNotFound(
                    ParameterNotFound::builder()
                        .meta(
                            aws_smithy_types::error::ErrorMetadata::builder()
                                .code(code)
                                .build(),
                        )
                        .build(),
                ),
                (),
            );
            let error = failure(store, "GetParameter", missing);
            assert_eq!(
                error.failure,
                SecretFailure::Rejected { store, not_found },
                "{code}"
            );
        }

        let silent: SdkError<GetParameterError, ()> = SdkError::timeout_error("no answer in 30s");
        let error = failure(AwsSecretStore::SecretsManager, "GetSecretValue", silent);
        assert_eq!(error.failure, SecretFailure::Unreachable);
        assert!(error.to_string().contains("GetSecretValue"), "{error}");
    }
}
