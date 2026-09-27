use super::*;
use crate::ports::sts::{GetSessionTokenRequest, SourceCredentials};
use aws_credential_types::Credentials;
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};
use aws_smithy_types::body::SdkBody;

fn service(operation: &str, error: Option<(&str, &str)>) -> (StsService, CaptureRequestReceiver) {
    let (status, body) = if let Some((code, message)) = error {
        (
            403,
            format!(
                "<ErrorResponse><Error><Type>Sender</Type><Code>{code}</Code><Message>{message}</Message></Error><RequestId>test</RequestId></ErrorResponse>"
            ),
        )
    } else {
        (
            200,
            format!(
                "<{operation}Response xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><{operation}Result><Credentials><AccessKeyId>ASIARESULT</AccessKeyId><SecretAccessKey>result-secret</SecretAccessKey><SessionToken>result-token</SessionToken><Expiration>2030-01-01T12:00:00Z</Expiration></Credentials></{operation}Result><ResponseMetadata><RequestId>test</RequestId></ResponseMetadata></{operation}Response>"
            ),
        )
    };
    let (client, receiver) = capture_request(Some(
        http::Response::builder()
            .status(status)
            .body(SdkBody::from(body))
            .unwrap(),
    ));
    let config = aws_sdk_sts::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_sts::config::Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            "AKIADEFAULT",
            "default-secret",
            None,
            None,
            "test",
        ))
        .http_client(client)
        .retry_config(aws_sdk_sts::config::retry::RetryConfig::disabled())
        .build();
    (
        StsService {
            client: StsClient::from_conf(config),
        },
        receiver,
    )
}

#[tokio::test]
async fn session_signer_omits_mfa_and_preserves_role_options() {
    let (service, captured) = service("AssumeRole", None);
    let mut request =
        AssumeRoleRequest::new("arn:aws:iam::123456789012:role/TestRole", "custom-session")
            .with_duration(7200)
            .with_mfa("arn:aws:iam::123456789012:mfa/user", "123456")
            .with_policy_arns(vec!["arn:aws:iam::aws:policy/ReadOnlyAccess".into()]);
    request.source_credentials = Some(SourceCredentials {
        access_key_id: "ASIASIGNER".into(),
        secret_access_key: "signer-secret".into(),
        session_token: "signer-token".into(),
    });
    let response = StsOperations::assume_role(&service, request).await.unwrap();
    assert_eq!(response.access_key_id, "ASIARESULT");
    let request = captured.expect_request();
    assert!(
        request
            .headers()
            .get("authorization")
            .unwrap()
            .contains("Credential=ASIASIGNER/")
    );
    assert_eq!(
        request.headers().get("x-amz-security-token"),
        Some("signer-token")
    );
    let body = std::str::from_utf8(request.body().bytes().unwrap()).unwrap();
    assert!(!body.contains("TokenCode") && !body.contains("SerialNumber"));
    assert!(
        body.contains("DurationSeconds=7200") && body.contains("RoleSessionName=custom-session")
    );
    assert!(body.contains("ReadOnlyAccess"));
}

#[tokio::test]
async fn get_session_token_uses_default_signer_and_requested_duration() {
    let (service, captured) = service("GetSessionToken", None);
    let response = service
        .get_session_token(GetSessionTokenRequest {
            mfa_serial: "arn:aws:iam::123456789012:mfa/user".into(),
            mfa_token: "123456".into(),
            duration_seconds: 129600,
        })
        .await
        .unwrap();
    assert_eq!(response.access_key_id, "ASIARESULT");
    assert_eq!(response.secret_access_key, "result-secret");
    assert_eq!(response.session_token, "result-token");
    assert_eq!(
        response.expiration.unwrap().to_rfc3339(),
        "2030-01-01T12:00:00+00:00"
    );
    let request = captured.expect_request();
    assert!(
        request
            .headers()
            .get("authorization")
            .unwrap()
            .contains("Credential=AKIADEFAULT/")
    );
    assert!(request.headers().get("x-amz-security-token").is_none());
    let body = std::str::from_utf8(request.body().bytes().unwrap()).unwrap();
    assert!(body.contains("Action=GetSessionToken") && body.contains("TokenCode=123456"));
    assert!(body.contains("SerialNumber=") && body.contains("DurationSeconds=129600"));
}

#[rstest::rstest]
#[case("AssumeRole")]
#[case("GetSessionToken")]
#[tokio::test]
async fn consumed_totp_is_classified_from_aws_error_message(#[case] operation: &str) {
    let (service, _) = service(
        operation,
        Some((
            "AccessDenied",
            "MultiFactorAuthentication failed with invalid MFA one time pass code",
        )),
    );
    let result = if operation == "AssumeRole" {
        StsOperations::assume_role(
            &service,
            AssumeRoleRequest::new("arn:aws:iam::123456789012:role/TestRole", "test")
                .with_mfa("serial", "123456"),
        )
        .await
    } else {
        service
            .get_session_token(GetSessionTokenRequest {
                mfa_serial: "serial".into(),
                mfa_token: "123456".into(),
                duration_seconds: 43200,
            })
            .await
    };
    assert!(
        matches!(result, Err(StsError::InvalidMfaToken)),
        "{result:?}"
    );
}
