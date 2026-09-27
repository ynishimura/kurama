//! Pure functions for AWS federation
//!
//! This module contains pure functions for building federation URLs and credentials.
//! All functions are side-effect free - the actual HTTP calls are handled by ports.
//!
//! ## Architecture
//!
//! ```text
//! Credentials (domain type)
//!       │
//!       ▼ build_session_credentials()
//! SessionCredentials (JSON)
//!       │
//!       ▼ FederationService::console_url (HTTP)
//! SigninToken
//!       │
//!       ▼ build_console_url()
//! Console URL (String)
//! ```

use crate::domain::constants::{DEFAULT_CONSOLE_URL, FEDERATION_ENDPOINT, FEDERATION_ISSUER};
use serde_json::{Value, json};

/// Build the AWS Console login URL
///
/// This is a pure function that constructs the federation URL from a signin token.
///
/// # Arguments
/// * `signin_token` - The signin token received from AWS federation endpoint
/// * `destination` - Optional destination URL (defaults to AWS Console home)
///
/// # Returns
/// The complete federation URL for console login
///
/// # Example
/// ```ignore
/// let url = build_console_url("token123", None);
/// assert!(url.contains("Action=login"));
/// ```
pub fn build_console_url(signin_token: &str, destination: Option<&str>) -> String {
    let destination_url = destination.unwrap_or(DEFAULT_CONSOLE_URL);

    let mut url = url::Url::parse(FEDERATION_ENDPOINT).expect("valid federation endpoint URL");
    url.query_pairs_mut().extend_pairs([
        ("Action", "login"),
        ("Issuer", FEDERATION_ISSUER),
        ("Destination", destination_url),
        ("SigninToken", signin_token),
    ]);
    url.into()
}

/// Build session credentials JSON for federation
///
/// This is a pure function that converts AWS credentials into the JSON format
/// required by the federation endpoint.
///
/// # Arguments
/// * `access_key_id` - AWS access key ID
/// * `secret_access_key` - AWS secret access key
/// * `session_token` - AWS session token
///
/// # Returns
/// JSON value with the session credentials in federation format
///
/// # Example
/// ```ignore
/// let creds = build_session_credentials("AKIA...", "secret", "token");
/// assert_eq!(creds["sessionId"], "AKIA...");
/// ```
pub fn build_session_credentials(
    access_key_id: &str,
    secret_access_key: &str,
    session_token: &str,
) -> Value {
    json!({
        "sessionId": access_key_id,
        "sessionKey": secret_access_key,
        "sessionToken": session_token
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    mod build_console_url_tests {
        use super::*;

        #[rstest::rstest]
        #[case("plain-token", None)]
        #[case(
            "token+with/slashes==",
            Some("https://console.aws.amazon.com/s3?prefix=a+b%20c&region=ap-northeast-1")
        )]
        #[case(
            "token&part=value#fragment",
            Some("https://console.aws.amazon.com/s3?prefix=日本語 folder#objects")
        )]
        fn preserves_console_query_values(#[case] token: &str, #[case] destination: Option<&str>) {
            let url = reqwest::Url::parse(&build_console_url(token, destination)).unwrap();
            let query: Vec<_> = url.query_pairs().collect();

            assert_eq!(
                query,
                vec![
                    ("Action".into(), "login".into()),
                    ("Issuer".into(), FEDERATION_ISSUER.into()),
                    (
                        "Destination".into(),
                        destination.unwrap_or(DEFAULT_CONSOLE_URL).into()
                    ),
                    ("SigninToken".into(), token.into()),
                ]
            );
            assert!(url.fragment().is_none());
        }

        #[test]
        fn builds_url_with_default_destination() {
            let url = build_console_url("test-token", None);

            assert!(url.contains(FEDERATION_ENDPOINT));
            assert!(url.contains("Action=login"));
            assert!(url.contains("SigninToken=test-token"));
            assert!(url.contains("Issuer=kurama"));
            assert!(url.contains("Destination=https%3A%2F%2Fconsole.aws.amazon.com%2F"));
        }

        #[test]
        fn builds_url_with_custom_destination() {
            let destination = "https://console.aws.amazon.com/s3";
            let url = build_console_url("test-token", Some(destination));

            assert!(url.contains("Destination=https%3A%2F%2Fconsole.aws.amazon.com%2Fs3"));
        }

        #[test]
        fn url_encodes_special_characters() {
            let destination = "https://console.aws.amazon.com/s3?prefix=foo&bar=baz";
            let url = build_console_url("token", Some(destination));

            // Should be URL encoded
            assert!(url.contains("%3F")); // ?
            assert!(url.contains("%26")); // &
        }
    }

    mod build_session_credentials_tests {
        use super::*;

        #[test]
        fn builds_correct_json_structure() {
            let creds = build_session_credentials("ACCESS_KEY", "SECRET_KEY", "SESSION_TOKEN");

            assert_eq!(creds["sessionId"], "ACCESS_KEY");
            assert_eq!(creds["sessionKey"], "SECRET_KEY");
            assert_eq!(creds["sessionToken"], "SESSION_TOKEN");
        }

        #[test]
        fn handles_special_characters() {
            let creds =
                build_session_credentials("AKIA+/TEST", "secret+/key=", "token+with/special=chars");

            assert_eq!(creds["sessionId"], "AKIA+/TEST");
            assert_eq!(creds["sessionKey"], "secret+/key=");
            assert_eq!(creds["sessionToken"], "token+with/special=chars");
        }
    }
}
