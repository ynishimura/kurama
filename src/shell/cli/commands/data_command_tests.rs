//! Data CLI shorthand, S3 URL normalization and request validation tests.
use super::*;
#[test]
fn data_shorthand_accepts_paths_and_optional_table_names() {
    for input in [
        "events.jsonl",
        "./input",
        "/tmp/input",
        "s3://bucket/key.parquet",
    ] {
        let matches = command().try_get_matches_from(["data", input]).unwrap();
        let command = DataCommand::parse(&matches);
        let (request, url_region) = command.read_request().unwrap();
        assert!(command.workspace.is_none());
        assert_eq!(url_region, None);
        assert!(matches!(request, DataRequest::Preview(_)));
        assert_eq!(request.args().from.as_deref(), Some(input));
    }
    for operation in ["--describe", "--preview", "--summary"] {
        let matches = command()
            .try_get_matches_from(["data", "lake", operation, "--json"])
            .unwrap();
        let command = DataCommand::parse(&matches);
        assert_eq!(command.workspace.as_deref(), Some("lake"));
        assert!(command.read_request().unwrap().0.args().table.is_none());
    }
    let matches = command().try_get_matches_from(["data", "lake"]).unwrap();
    assert!(DataCommand::parse(&matches).read_request().is_err());
}

#[test]
fn data_https_normalization_removes_credentials_and_preserves_object_keys() {
    for (input, region) in [
        (
            "https://bucket.s3.amazonaws.com/a/../%E6%97%A5%E6%9C%AC%E8%AA%9E+%25//data.parquet?Signature=FAKE_SECRET#fragment",
            None,
        ),
        (
            "https://bucket.s3.ap-northeast-1.amazonaws.com/a/../%E6%97%A5%E6%9C%AC%E8%AA%9E+%25//data.parquet?X-Amz-Signature=FAKE_SECRET",
            Some("ap-northeast-1"),
        ),
        (
            "https://s3.us-west-2.amazonaws.com/bucket/a/../%E6%97%A5%E6%9C%AC%E8%AA%9E+%25//data.parquet?AWSAccessKeyId=FAKE_SECRET",
            Some("us-west-2"),
        ),
    ] {
        let matches = command()
            .try_get_matches_from(["data", "--from", input, "--preview", "data"])
            .unwrap();
        let (request, url_region) = DataCommand::parse(&matches).read_request().unwrap();
        assert_eq!(
            request.args().from.as_deref(),
            Some("s3://bucket/a/../日本語+%//data.parquet")
        );
        assert_eq!(url_region.as_deref(), region, "{input}");
    }
    for input in [
        "https://bucket.s3.amazonaws.com.evil.test/data.parquet?Signature=FAKE_SECRET",
        "https://user:FAKE_SECRET@bucket.s3.amazonaws.com/data.parquet",
        "https://bucket.s3.amazonaws.com:444/data.parquet?Signature=FAKE_SECRET",
        "https://bucket.s3.amazonaws.com/%2A.parquet?Signature=FAKE_SECRET",
        "https://bucket.s3.amazonaws.com/data.parquet?versionId=old&Signature=FAKE_SECRET",
        "https://bucket.s3.amazonaws.com?Signature=FAKE_SECRET/data.parquet",
        "https://bucket.s3.amazonaws.com#FAKE_SECRET/data.parquet",
    ] {
        let matches = command()
            .try_get_matches_from(["data", "--from", input, "--preview", "data"])
            .unwrap();
        let error = DataCommand::parse(&matches)
            .read_request()
            .err()
            .expect("URL must be rejected");
        assert!(!error.to_string().contains("FAKE_SECRET"));
    }
}

#[test]
fn data_request_conflicts_with_cli_operation_arguments() {
    assert!(
        command()
            .try_get_matches_from(["data", "--threads", "4294967296"])
            .is_err()
    );
    for tail in [
        ["--query", "SELECT 1"],
        ["--from", "a.csv"],
        ["--threads", "2"],
    ] {
        assert!(
            command()
                .try_get_matches_from(["data", "--request", "-"].into_iter().chain(tail))
                .is_err()
        );
    }
    assert!(
        command()
            .try_get_matches_from(["data", "lake", "--request", "-", "--json"])
            .is_ok()
    );
}
#[test]
fn data_request_rejects_unknown_keys_and_multiple_operations() {
    for text in [
        r#"{"operation":"query","args":{"sql":"SELECT 1","secret":"oops"}}"#,
        r#"{"operation":"query","args":{"sql":"SELECT 1"},"extra":1}"#,
    ] {
        assert!(serde_json::from_str::<DataRequest>(text).is_err());
    }
    assert!(
        command()
            .try_get_matches_from(["data", "--tables", "--query", "SELECT 1"])
            .is_err()
    );
}
