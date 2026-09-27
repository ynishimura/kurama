//! SigV4 signing on the `aws-sigv4` crate: the request `kurama api` built,
//! plus `authorization`, `x-amz-date` and, with temporary credentials,
//! `x-amz-security-token`. The URL is signed as reqwest sends it. S3 gets
//! its own path settings (no normalization, single percent-encoding).
//! S3 and OpenSearch Serverless require the payload hash header.
//!
//! The other signature is a presigned URL, which is what an IAM database
//! token is. This is the only file that signs: `tests/architecture/` keeps
//! `aws_sigv4` out of every other one.

use aws_sigv4::http_request::{
    PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SignatureLocation,
    SigningSettings, UriPathNormalizationMode, sign,
};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use chrono::{DateTime, Utc};
use std::time::Duration;

use crate::adapters::utils::error_chain::causes;
use crate::domain::functions::signing_target::SigningTarget;
use crate::domain::types::{Credentials, HttpRequest};

/// Headers the signature owns; any the caller set are replaced.
const SIGNATURE_HEADERS: [&str; 3] = ["authorization", "x-amz-date", "x-amz-security-token"];

/// `request` signed for `target` at `now`. Every header on the request is
/// signed except the ones the crate excludes (`authorization`,
/// `user-agent`, `x-amzn-trace-id`, `transfer-encoding`).
pub fn sign_request(
    request: HttpRequest,
    credentials: &Credentials,
    target: &SigningTarget,
    now: DateTime<Utc>,
) -> Result<HttpRequest, String> {
    sign_with(
        request,
        credentials,
        target,
        now,
        signing_settings(&target.service),
    )
}

fn signing_settings(service: &str) -> SigningSettings {
    let mut settings = SigningSettings::default();
    if matches!(service, "s3" | "aoss") {
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    }
    if service == "s3" {
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        settings.percent_encoding_mode = PercentEncodingMode::Single;
    }
    settings
}

fn sign_with(
    mut request: HttpRequest,
    credentials: &Credentials,
    target: &SigningTarget,
    now: DateTime<Utc>,
    settings: SigningSettings,
) -> Result<HttpRequest, String> {
    // reqwest parses and re-serializes the URL (percent-encoding, a
    // trailing slash on an empty path); sign exactly that.
    request.url = url::Url::parse(&request.url)
        .map_err(|error| format!("URL {:?}: {error}", request.url))?
        .to_string();
    request
        .headers
        .retain(|(name, _)| !SIGNATURE_HEADERS.contains(&name.to_ascii_lowercase().as_str()));
    let identity = identity_of(credentials);
    let signed_headers: Vec<(String, String)> = {
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&target.region)
            .name(&target.service)
            .time(now.into())
            .settings(settings)
            .build()
            .map_err(|error| error.to_string())?
            .into();
        let body = SignableBody::Bytes(request.body.as_deref().unwrap_or(&[]));
        let signable = SignableRequest::new(
            &request.method,
            request.url.as_str(),
            request
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            body,
        )
        .map_err(|error| causes(&error))?;
        let (instructions, _signature) = sign(signable, &params)
            .map_err(|error| causes(&error))?
            .into_parts();
        instructions
            .headers()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    };
    request.headers.extend(signed_headers);
    Ok(request)
}

/// The query parameters that presign a GET of `url` for `expires_in`: the
/// `X-Amz-*` pairs, decoded, in the signer's order. `url` is signed as it is.
pub fn presign_query(
    url: &url::Url,
    credentials: &Credentials,
    target: &SigningTarget,
    now: DateTime<Utc>,
    expires_in: Duration,
) -> Result<Vec<(String, String)>, String> {
    let identity = identity_of(credentials);
    let mut settings = SigningSettings::default();
    settings.signature_location = SignatureLocation::QueryParams;
    settings.expires_in = Some(expires_in);
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&target.region)
        .name(&target.service)
        .time(now.into())
        .settings(settings)
        .build()
        .map_err(|error| error.to_string())?
        .into();
    let signable = SignableRequest::new(
        "GET",
        url.as_str(),
        std::iter::empty(),
        SignableBody::Bytes(&[]),
    )
    .map_err(|error| causes(&error))?;
    let (instructions, _signature) = sign(signable, &params)
        .map_err(|error| causes(&error))?
        .into_parts();
    Ok(instructions
        .params()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect())
}

fn identity_of(credentials: &Credentials) -> Identity {
    crate::adapters::aws::sdk_credentials(credentials).into()
}

/// Known-answer tests: vectors of the AWS SigV4 test suite
/// (`aws-signing-test-suite/v4` in the `aws-sigv4` crate), all with the
/// credentials `AKIDEXAMPLE`, region `us-east-1`, service `service` and
/// timestamp 2015-08-30T12:36:00Z.
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    fn permanent() -> Credentials {
        Credentials::new("AKIDEXAMPLE".into(), SECRET.into(), None, None)
    }

    fn target() -> SigningTarget {
        SigningTarget {
            service: "service".into(),
            region: "us-east-1".into(),
        }
    }

    fn when() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap()
    }

    fn authorization(request: &HttpRequest) -> &str {
        request.header("authorization").expect("signed")
    }

    #[test]
    fn get_vanilla() {
        let signed = sign_request(
            HttpRequest::new("GET", "https://example.amazonaws.com/"),
            &permanent(),
            &target(),
            when(),
        )
        .unwrap();
        assert_eq!(
            authorization(&signed),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        assert_eq!(signed.header("x-amz-date"), Some("20150830T123600Z"));
        assert_eq!(signed.header("x-amz-security-token"), None);
        assert_eq!(signed.headers.len(), 2);
    }

    #[test]
    fn get_vanilla_with_session_token() {
        let token = "6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267";
        let temporary = Credentials::new(
            "AKIDEXAMPLE".into(),
            SECRET.into(),
            Some(token.into()),
            None,
        );
        let signed = sign_request(
            HttpRequest::new("GET", "https://example.amazonaws.com/"),
            &temporary,
            &target(),
            when(),
        )
        .unwrap();
        assert_eq!(
            authorization(&signed),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-security-token, \
             Signature=07ec1639c89043aa0e3e2de82b96708f198cceab042d4a97044c66dd9f74e7f8"
        );
        assert_eq!(signed.header("x-amz-security-token"), Some(token));
    }

    #[test]
    fn get_vanilla_query_order_key_case() {
        let signed = sign_request(
            HttpRequest::new(
                "GET",
                "https://example.amazonaws.com/?Param2=value2&Param1=value1",
            ),
            &permanent(),
            &target(),
            when(),
        )
        .unwrap();
        assert!(
            authorization(&signed).ends_with(
                "Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
            ),
            "{}",
            authorization(&signed)
        );
    }

    #[test]
    fn post_x_www_form_urlencoded_with_the_payload_hash_header() {
        let mut settings = SigningSettings::default();
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        let request = HttpRequest::new("POST", "https://example.amazonaws.com/")
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("Content-Length", "13")
            .with_body(b"Param1=value1".to_vec());
        let signed = sign_with(request, &permanent(), &target(), when(), settings).unwrap();
        assert_eq!(
            signed.header("x-amz-content-sha256"),
            Some("9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e")
        );
        assert_eq!(
            authorization(&signed),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=content-length;content-type;host;x-amz-content-sha256;x-amz-date, \
             Signature=d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b"
        );
    }

    #[test]
    fn the_body_is_hashed_into_the_signature() {
        let request = |body: &[u8]| {
            HttpRequest::new("POST", "https://example.amazonaws.com/items")
                .with_header("Content-Type", "application/json")
                .with_body(body.to_vec())
        };
        let a = sign_request(request(b"{\"a\":1}"), &permanent(), &target(), when()).unwrap();
        let b = sign_request(request(b"{\"a\":2}"), &permanent(), &target(), when()).unwrap();
        assert_ne!(authorization(&a), authorization(&b));
        assert_eq!(a.header("x-amz-content-sha256"), None);
    }

    #[test]
    fn aoss_requires_the_payload_hash() {
        let target = SigningTarget {
            service: "aoss".into(),
            region: "us-east-1".into(),
        };
        let signed = sign_request(
            HttpRequest::new("POST", "https://example.amazonaws.com/").with_body(b"data".to_vec()),
            &permanent(),
            &target,
            when(),
        )
        .unwrap();
        assert_eq!(
            signed.header("x-amz-content-sha256"),
            Some("3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7")
        );
    }

    #[test]
    fn s3_canonical_path_settings_are_independently_required() {
        let target = SigningTarget {
            service: "s3".into(),
            region: "us-east-1".into(),
        };
        let request = || HttpRequest::new("GET", "https://bucket.s3.amazonaws.com/a//b%2Fc");
        let signed = sign_request(request(), &permanent(), &target, when()).unwrap();
        assert_eq!(signed.url, "https://bucket.s3.amazonaws.com/a//b%2Fc");
        let mut normalized = signing_settings("s3");
        normalized.uri_path_normalization_mode = UriPathNormalizationMode::Enabled;
        let mut encoded = signing_settings("s3");
        encoded.percent_encoding_mode = PercentEncodingMode::Double;
        for settings in [normalized, encoded] {
            let other = sign_with(request(), &permanent(), &target, when(), settings).unwrap();
            assert_ne!(authorization(&signed), authorization(&other));
        }
    }

    #[test]
    fn s3_adds_the_payload_hash_after_url_parsing() {
        let s3 = SigningTarget {
            service: "s3".into(),
            region: "us-east-1".into(),
        };
        let signed = sign_request(
            HttpRequest::new("PUT", "https://bucket.s3.amazonaws.com/a/../key")
                .with_body(b"data".to_vec()),
            &permanent(),
            &s3,
            when(),
        )
        .unwrap();
        // sha256("data")
        assert_eq!(
            signed.header("x-amz-content-sha256"),
            Some("3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7")
        );
        assert!(
            authorization(&signed).contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date,")
        );
        assert_eq!(signed.url, "https://bucket.s3.amazonaws.com/key");
    }

    #[test]
    fn caller_headers_are_signed_and_stale_signature_headers_are_replaced() {
        let request = HttpRequest::new("GET", "https://example.amazonaws.com/items")
            .with_header("Accept", "application/json")
            .with_header("Authorization", "Bearer old")
            .with_header("X-Amz-Date", "20000101T000000Z")
            .with_header("x-amz-security-token", "old");
        let signed = sign_request(request, &permanent(), &target(), when()).unwrap();
        assert_eq!(
            signed
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .count(),
            1
        );
        assert_eq!(signed.header("x-amz-date"), Some("20150830T123600Z"));
        assert_eq!(signed.header("x-amz-security-token"), None);
        assert!(authorization(&signed).contains("SignedHeaders=accept;host;x-amz-date,"));
    }

    #[test]
    fn the_url_is_signed_as_reqwest_sends_it() {
        let signed = sign_request(
            HttpRequest::new("GET", "https://Example.amazonaws.com/ሴ x"),
            &permanent(),
            &target(),
            when(),
        )
        .unwrap();
        assert_eq!(signed.url, "https://example.amazonaws.com/%E1%88%B4%20x");
        assert!(
            sign_request(
                HttpRequest::new("GET", "not a url"),
                &permanent(),
                &target(),
                when()
            )
            .unwrap_err()
            .contains("URL")
        );
    }
}
