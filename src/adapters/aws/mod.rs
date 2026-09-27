//! AWS services module
//!
//! This module provides AWS service integrations including STS and federation.

pub mod config_builder;
pub mod db_iam_token;
pub mod federation;
pub mod s3_browse;
pub mod s3_data;
pub mod s3_object;
pub mod secret_store;
pub mod ssm_tunnel;
pub mod sts;

use std::time::Duration;

use aws_credential_types::Credentials as SdkCredentials;
use aws_sdk_ssm::config::{BehaviorVersion, Region, retry::RetryConfig, timeout::TimeoutConfig};

use crate::domain::types::Credentials;

/// Role credentials kurama already holds, in the SDK's type. Every client is
/// built from these; none reads a default credential chain.
pub(crate) fn sdk_credentials(credentials: &Credentials) -> SdkCredentials {
    SdkCredentials::new(
        credentials.access_key_id(),
        credentials.secret_access_key(),
        credentials.session_token().map(str::to_owned),
        None,
        "kurama",
    )
}

/// The SSM client a parameter read and a tunnel share: no retries, one
/// 30-second deadline per operation, and the fake endpoint under test.
pub(crate) fn ssm_client(credentials: &Credentials, region: &str) -> aws_sdk_ssm::Client {
    let config = aws_sdk_ssm::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .credentials_provider(sdk_credentials(credentials))
        .region(Region::new(region.to_owned()))
        .retry_config(RetryConfig::disabled())
        .timeout_config(
            TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(30))
                .build(),
        );
    #[cfg(feature = "test-fakes")]
    let config = match std::env::var("KURAMA_TEST_SSM_ENDPOINT") {
        Ok(endpoint) => config.endpoint_url(endpoint),
        Err(_) => config,
    };
    aws_sdk_ssm::Client::from_conf(config.build())
}
