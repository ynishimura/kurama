//! One object's metadata (HeadObject) or a bounded, conditional read of its bytes (GetObject), through the role's clients.
use super::s3_data::BucketClients;
use crate::domain::types::{dataset::DataError, s3_browse::S3Error, s3_object::S3ObjectHead};

pub async fn head_object(
    clients: &mut BucketClients,
    bucket: &str,
    key: &str,
) -> Result<S3ObjectHead, S3Error> {
    let request = (bucket.to_owned(), key.to_owned());
    let object = clients
        .call(bucket, move |client| {
            let (bucket, key) = request.clone();
            async move { client.head_object().bucket(bucket).key(key).send().await }
        })
        .await
        .map_err(read_error)?;
    Ok(S3ObjectHead {
        key: key.to_owned(),
        size: object.content_length().unwrap_or(0).max(0) as u64,
        content_type: object.content_type().map(str::to_owned),
        content_encoding: object.content_encoding().map(str::to_owned),
        last_modified: object.last_modified().map(ToString::to_string),
        etag: object.e_tag().map(str::to_owned),
        storage_class: object
            .storage_class()
            .map_or("STANDARD", |class| class.as_str())
            .to_owned(),
        restore: object.restore().map(str::to_owned),
        archive_status: object
            .archive_status()
            .map(|status| status.as_str().to_owned()),
    })
}

/// The bytes `start..end` of an object, and never more than `end - start`
/// of them whatever S3 sends, read only while its ETag is `etag`: an object
/// that changed since it was listed or read is `S3Error::Changed`, not other
/// bytes under the same name.
pub async fn get_range(
    clients: &mut BucketClients,
    bucket: &str,
    key: &str,
    (start, end): (u64, u64),
    etag: Option<&str>,
) -> Result<Vec<u8>, S3Error> {
    let request = (
        bucket.to_owned(),
        key.to_owned(),
        format!("bytes={start}-{}", end - 1),
        etag.map(str::to_owned),
    );
    let response = clients
        .call(bucket, move |client| {
            let (bucket, key, range, etag) = request.clone();
            async move {
                client
                    .get_object()
                    .bucket(bucket)
                    .key(key)
                    .range(range)
                    .set_if_match(etag)
                    .send()
                    .await
            }
        })
        .await
        .map_err(read_error)?;
    let limit = usize::try_from(end - start).unwrap_or(usize::MAX);
    let mut body = response.body;
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        let Some(chunk) = body.next().await else {
            break;
        };
        let chunk = chunk.map_err(|_| S3Error::Failed)?;
        let take = chunk.len().min(limit - bytes.len());
        bytes.extend_from_slice(&chunk[..take]);
    }
    Ok(bytes)
}

/// The shared clients speak the analysis's failures; a read names its own.
fn read_error(error: DataError) -> S3Error {
    match error {
        DataError::S3Rejected(code) if code == "PreconditionFailed" => S3Error::Changed,
        DataError::S3Rejected(code) => S3Error::ReadRejected(code),
        DataError::S3Redirected { region } => S3Error::Redirected { region },
        _ => S3Error::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_http_client::test_util::infallible_client_fn;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, Option<String>, Option<String>)>>>;

    /// S3 over a fake transport that records each request's path, Range and
    /// If-Match, and answers with `status` and `body`.
    fn clients(status: u16, body: &'static str, seen: Seen) -> BucketClients {
        BucketClients::for_tests(
            infallible_client_fn(move |request| {
                let header = |name: &str| {
                    request
                        .headers()
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned)
                };
                seen.lock().unwrap().push((
                    request.uri().to_string(),
                    header("range"),
                    header("if-match"),
                ));
                http::Response::builder()
                    .status(status)
                    .header("etag", "\"e1\"")
                    .header("content-length", body.len().to_string())
                    .header("x-amz-storage-class", "GLACIER")
                    .header("x-amz-restore", "ongoing-request=\"true\"")
                    .body(body)
                    .unwrap()
            }),
            "ap-northeast-1",
            true,
        )
    }

    #[tokio::test]
    async fn s3_get_range_asks_for_the_range_and_the_etag_and_keeps_to_the_range() {
        let seen = Seen::default();
        let mut clients = clients(206, "0123456789", seen.clone());
        let bytes = get_range(&mut clients, "bucket", "a b.json", (4, 8), Some("\"e1\""))
            .await
            .unwrap();
        // A server that sends more than the range asked for is cut to it.
        assert_eq!(bytes, b"0123");
        let (uri, range, if_match) = seen.lock().unwrap()[0].clone();
        assert!(uri.contains("/a%20b.json"), "{uri}");
        assert_eq!(range.as_deref(), Some("bytes=4-7"));
        assert_eq!(if_match.as_deref(), Some("\"e1\""));
    }

    #[tokio::test]
    async fn s3_head_object_reports_what_s3_answers() {
        let mut clients = clients(200, "", Seen::default());
        let head = head_object(&mut clients, "bucket", "k").await.unwrap();
        assert_eq!(head.etag.as_deref(), Some("\"e1\""));
        assert_eq!(head.storage_class, "GLACIER");
        assert!(head.archived());
    }

    #[tokio::test]
    async fn s3_a_read_refused_for_its_etag_is_a_change_not_a_denial() {
        for (status, body, changed) in [
            (412, "<Error><Code>PreconditionFailed</Code></Error>", true),
            (403, "<Error><Code>AccessDenied</Code></Error>", false),
        ] {
            let mut clients = clients(status, body, Seen::default());
            let error = get_range(&mut clients, "bucket", "k", (0, 4), Some("\"old\""))
                .await
                .unwrap_err();
            match error {
                S3Error::Changed => assert!(changed),
                S3Error::ReadRejected(code) => {
                    assert!(!changed);
                    assert_eq!(code, "AccessDenied");
                }
                other => panic!("{other}"),
            }
        }
    }
}
