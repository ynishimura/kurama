//! One S3 listing page or every bucket, through the role's clients; failures stay failures, never an empty list.
use super::s3_data::BucketClients;
use crate::domain::types::{
    dataset::DataError,
    s3_browse::{S3Bucket, S3Entry, S3Error, S3Object, S3Page},
};

/// One `ListObjectsV2` page under `prefix`, one level deep when `delimited`.
pub async fn list_page(
    clients: &mut BucketClients,
    bucket: &str,
    prefix: &str,
    delimited: bool,
    page_size: u32,
    token: Option<String>,
) -> Result<S3Page, S3Error> {
    let request = (bucket.to_owned(), prefix.to_owned(), token);
    let response = clients
        .call(bucket, move |client| {
            let (bucket, prefix, token) = request.clone();
            async move {
                client
                    .list_objects_v2()
                    .bucket(bucket)
                    .prefix(prefix)
                    .set_delimiter(delimited.then(|| "/".to_owned()))
                    .max_keys(page_size as i32)
                    .set_continuation_token(token)
                    .send()
                    .await
            }
        })
        .await
        .map_err(browse_error)?;
    let mut entries: Vec<S3Entry> = response
        .common_prefixes()
        .iter()
        .filter_map(|prefix| prefix.prefix())
        .map(|prefix| S3Entry::Prefix(prefix.to_owned()))
        .chain(response.contents().iter().filter_map(|object| {
            Some(S3Entry::Object(S3Object {
                key: object.key()?.to_owned(),
                size: object.size().unwrap_or(0).max(0) as u64,
                last_modified: object.last_modified().map(ToString::to_string),
                etag: object.e_tag().map(str::to_owned),
                storage_class: object
                    .storage_class()
                    .map(|class| class.as_str().to_owned()),
            }))
        }))
        .collect();
    // S3 counts both toward one page in key order; the two lists it answers
    // with are merged back into that order, which a resumed run skips into.
    entries.sort_by(|a, b| a.name().cmp(b.name()));
    let next_token = if response.is_truncated().unwrap_or(false) {
        Some(
            response
                .next_continuation_token()
                .ok_or(S3Error::Failed)?
                .to_owned(),
        )
    } else {
        None
    };
    Ok(S3Page {
        entries,
        next_token,
    })
}

/// Every bucket `ListBuckets` answers, page by page.
pub async fn list_buckets(
    clients: &mut BucketClients,
    page_size: u32,
) -> Result<Vec<S3Bucket>, S3Error> {
    let mut buckets = vec![];
    let mut token: Option<String> = None;
    loop {
        let request = token.clone();
        let response = clients
            .call_account(move |client| async move {
                client
                    .list_buckets()
                    .max_buckets(page_size as i32)
                    .set_continuation_token(request)
                    .send()
                    .await
            })
            .await
            .map_err(browse_error)?;
        buckets.extend(response.buckets().iter().filter_map(|bucket| {
            Some(S3Bucket {
                name: bucket.name()?.to_owned(),
                created: bucket.creation_date().map(ToString::to_string),
                region: bucket.bucket_region().map(str::to_owned),
            })
        }));
        match response.continuation_token() {
            Some(next) if !next.is_empty() => token = Some(next.to_owned()),
            _ => return Ok(buckets),
        }
    }
}

/// The shared S3 clients speak the analysis's failures; a browse names its own.
fn browse_error(error: DataError) -> S3Error {
    match error {
        DataError::S3Rejected(code) => S3Error::Rejected(code),
        DataError::S3Redirected { region } => S3Error::Redirected { region },
        _ => S3Error::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_http_client::test_util::infallible_client_fn;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn s3_list_page_merges_prefixes_and_keys_in_key_order() {
        let queries = Arc::new(Mutex::new(vec![]));
        let seen = queries.clone();
        let mut clients = BucketClients::for_tests(
            infallible_client_fn(move |request| {
                seen.lock().unwrap().push(request.uri().to_string());
                http::Response::builder().status(200).body(
                    "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>next+/=</NextContinuationToken>\
                     <Contents><Key>r/b.csv</Key><Size>3</Size></Contents><Contents><Key>r/日本語 +%.csv</Key><Size>4</Size></Contents>\
                     <CommonPrefixes><Prefix>r/a/</Prefix></CommonPrefixes><CommonPrefixes><Prefix>r/c//</Prefix></CommonPrefixes></ListBucketResult>",
                ).unwrap()
            }),
            "ap-northeast-1",
            true,
        );
        let page = list_page(&mut clients, "bucket", "r/", true, 4, Some("t".into()))
            .await
            .unwrap();
        assert_eq!(
            page.entries.iter().map(S3Entry::name).collect::<Vec<_>>(),
            ["r/a/", "r/b.csv", "r/c//", "r/日本語 +%.csv"]
        );
        assert_eq!(page.next_token.as_deref(), Some("next+/="));
        let query = queries.lock().unwrap()[0].clone();
        for part in [
            "delimiter=%2F",
            "max-keys=4",
            "prefix=r%2F",
            "continuation-token=t",
        ] {
            assert!(query.contains(part), "{query}");
        }
    }

    #[tokio::test]
    async fn s3_a_truncated_page_without_a_token_is_a_failure_not_the_end() {
        let mut clients = BucketClients::for_tests(
            infallible_client_fn(|_| {
                http::Response::builder()
                    .status(200)
                    .body("<ListBucketResult><IsTruncated>true</IsTruncated></ListBucketResult>")
                    .unwrap()
            }),
            "ap-northeast-1",
            true,
        );
        let error = list_page(&mut clients, "bucket", "", false, 2, None)
            .await
            .unwrap_err();
        assert!(matches!(error, S3Error::Failed), "{error}");
    }

    #[tokio::test]
    async fn s3_list_buckets_reads_every_page() {
        let queries = Arc::new(Mutex::new(vec![]));
        let seen = queries.clone();
        let mut clients = BucketClients::for_tests(
            infallible_client_fn(move |request| {
                let query = request.uri().to_string();
                seen.lock().unwrap().push(query.clone());
                let body = if query.contains("continuation-token=p2") {
                    "<ListAllMyBucketsResult><Buckets><Bucket><Name>two</Name></Bucket></Buckets></ListAllMyBucketsResult>"
                } else {
                    "<ListAllMyBucketsResult><Buckets><Bucket><Name>one</Name><BucketRegion>us-west-2</BucketRegion></Bucket></Buckets><ContinuationToken>p2</ContinuationToken></ListAllMyBucketsResult>"
                };
                http::Response::builder().status(200).body(body).unwrap()
            }),
            "ap-northeast-1",
            true,
        );
        let buckets = list_buckets(&mut clients, 1).await.unwrap();
        assert_eq!(
            buckets
                .iter()
                .map(|b| (b.name.as_str(), b.region.as_deref()))
                .collect::<Vec<_>>(),
            [("one", Some("us-west-2")), ("two", None)]
        );
        assert_eq!(queries.lock().unwrap().len(), 2);
        assert_eq!(clients.requests(), 2);
    }

    #[tokio::test]
    async fn s3_a_rejected_listing_keeps_the_code_s3_answered() {
        let mut clients = BucketClients::for_tests(
            infallible_client_fn(|_| {
                http::Response::builder()
                    .status(404)
                    .body("<Error><Code>NoSuchBucket</Code></Error>")
                    .unwrap()
            }),
            "ap-northeast-1",
            true,
        );
        let error = list_page(&mut clients, "missing", "", true, 2, None)
            .await
            .unwrap_err();
        assert!(
            matches!(error, S3Error::Rejected(ref code) if code == "NoSuchBucket"),
            "{error}"
        );
    }
}
