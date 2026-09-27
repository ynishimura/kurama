//! Bounded S3 input enumeration with explicit credentials; never use a default AWS chain.
use crate::adapters::data_inputs::validate_resolved_path;
use crate::domain::types::{
    Credentials,
    dataset::{DataError, DataLimits, DataSource, InputFile, InvalidInput},
};
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Region, retry::RetryConfig, timeout::TimeoutConfig},
    error::{ProvideErrorMetadata, SdkError},
};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::task::JoinSet;

const S3_GLOB: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};
const FINAL_HEAD_CONCURRENCY: usize = 16;

/// One S3 client per region, and where each bucket is read: the AWS profile's
/// region says where to call STS, not where a bucket lives.
pub struct BucketClients {
    credentials: Credentials,
    timeout: u64,
    /// Where a bucket's first request is signed.
    start: String,
    /// A region the request named is used as is; a starting region is a guess
    /// the bucket corrects once.
    correctable: bool,
    /// Bucket to the region it is read in.
    regions: BTreeMap<String, String>,
    /// Region to the client that signs for it.
    clients: BTreeMap<String, Client>,
    /// Every request sent through these clients: each HEAD and LIST, the retry
    /// after a region redirect, and the final HEADs of the change check.
    requests: Arc<AtomicU64>,
    /// Unit tests drive the redirect over a fake transport.
    #[cfg(test)]
    http: Option<aws_smithy_runtime_api::client::http::SharedHttpClient>,
}

impl BucketClients {
    pub fn new(credentials: &Credentials, start: &str, correctable: bool, timeout: u64) -> Self {
        Self {
            credentials: credentials.clone(),
            timeout,
            start: start.to_owned(),
            correctable,
            regions: BTreeMap::new(),
            clients: BTreeMap::new(),
            requests: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            http: None,
        }
    }

    /// Where each bucket touched so far is read.
    pub fn regions(&self) -> &BTreeMap<String, String> {
        &self.regions
    }

    /// How many S3 requests have been sent, redirect retries included.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// Sign from now on with `credentials`. The clients signed with other
    /// credentials are dropped; where each bucket lives is kept.
    pub fn renew(&mut self, credentials: &Credentials) {
        if self.credentials != *credentials {
            self.credentials = credentials.clone();
            self.clients.clear();
        }
    }

    /// The client for `bucket` and the region it signs for, recording it.
    fn client(&mut self, bucket: &str) -> (Client, String) {
        let start = &self.start;
        let region = self
            .regions
            .entry(bucket.to_owned())
            .or_insert_with(|| start.clone())
            .clone();
        (self.client_in(&region), region)
    }

    fn client_in(&mut self, region: &str) -> Client {
        if let Some(client) = self.clients.get(region) {
            return client.clone();
        }
        #[cfg(test)]
        let built = match &self.http {
            Some(http) => test_client(http.clone(), region),
            None => client(&self.credentials, region, self.timeout),
        };
        #[cfg(not(test))]
        let built = client(&self.credentials, region, self.timeout);
        self.clients.insert(region.to_owned(), built.clone());
        built
    }

    /// Run one S3 call for `bucket` and, when S3 answers with the bucket's own
    /// region, retry it there once. Later calls for that bucket start in the
    /// region S3 named.
    pub async fn call<T, E, F, Fut>(&mut self, bucket: &str, operation: F) -> Result<T, DataError>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = Result<T, SdkError<E>>>,
        E: ProvideErrorMetadata,
    {
        let (client, signed) = self.client(bucket);
        self.requests.fetch_add(1, Ordering::Relaxed);
        let error = match operation(client).await {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let SdkError::ServiceError(service) = &error else {
            return Err(s3_error(error, &signed));
        };
        let Some(region) = redirect_region(service.raw(), &signed) else {
            return Err(s3_error(error, &signed));
        };
        if !self.correctable {
            return Err(DataError::S3OtherRegion {
                region: signed,
                bucket_region: region,
            });
        }
        tracing::debug!(bucket, signed, region, "S3 named the bucket's own region");
        let client = self.client_in(&region);
        self.regions.insert(bucket.to_owned(), region.clone());
        self.requests.fetch_add(1, Ordering::Relaxed);
        operation(client)
            .await
            .map_err(|error| s3_error(error, &region))
    }
}

impl BucketClients {
    /// Run one call that names no bucket (`ListBuckets`), signed for the
    /// starting region: there is no bucket whose region could correct it.
    pub async fn call_account<T, E, F, Fut>(&mut self, operation: F) -> Result<T, DataError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: Future<Output = Result<T, SdkError<E>>>,
        E: ProvideErrorMetadata,
    {
        let region = self.start.clone();
        let client = self.client_in(&region);
        self.requests.fetch_add(1, Ordering::Relaxed);
        operation(client)
            .await
            .map_err(|error| s3_error(error, &region))
    }
}

/// The region to retry a rejected request in: S3 returns the bucket's own
/// region when the request was signed for another one.
fn redirect_region(
    response: &aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    signed: &str,
) -> Option<String> {
    response
        .headers()
        .get("x-amz-bucket-region")
        .filter(|region| *region != signed)
        .map(str::to_owned)
}

fn client(credentials: &Credentials, region: &str, timeout: u64) -> Client {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .credentials_provider(super::sdk_credentials(credentials))
        .region(Region::new(region.to_owned()))
        .retry_config(RetryConfig::disabled())
        .timeout_config(
            TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(timeout))
                .build(),
        );
    #[cfg(feature = "test-fakes")]
    let config = if let Ok(endpoint) = std::env::var("KURAMA_TEST_S3_ENDPOINT") {
        config.endpoint_url(endpoint).force_path_style(true)
    } else {
        config
    };
    Client::from_conf(config.build())
}

#[cfg(test)]
fn test_client(
    http: aws_smithy_runtime_api::client::http::SharedHttpClient,
    region: &str,
) -> Client {
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.to_owned()))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "TEST", "SECRET", None, None, "test",
            ))
            .retry_config(RetryConfig::disabled())
            .http_client(http)
            .build(),
    )
}

#[cfg(test)]
impl BucketClients {
    /// Clients whose transport is the test's, not the network.
    pub(crate) fn for_tests(
        http: impl aws_smithy_runtime_api::client::http::HttpClient + 'static,
        start: &str,
        correctable: bool,
    ) -> Self {
        let credentials = Credentials::new("TEST".into(), "SECRET".into(), None, None);
        let mut clients = Self::new(&credentials, start, correctable, 30);
        clients.http = Some(aws_smithy_runtime_api::client::http::SharedHttpClient::new(
            http,
        ));
        clients
    }
}

/// Split without URL/path normalization: %, +, spaces, .. and repeated / are keys.
pub fn split_uri(uri: &str) -> Result<(&str, &str), DataError> {
    let (bucket, key) = uri
        .strip_prefix("s3://")
        .and_then(|s| s.split_once('/'))
        .ok_or(InvalidInput::InvalidS3Uri)?;
    if bucket.is_empty()
        || key.is_empty()
        || !bucket
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
    {
        return Err(InvalidInput::InvalidS3BucketOrKey.into());
    }
    Ok((bucket, key))
}

pub async fn head(
    clients: &mut BucketClients,
    source: &str,
    uri: &str,
) -> Result<InputFile, DataError> {
    let (bucket, key) = split_uri(uri)?;
    validate_resolved_path(uri)?;
    let (owned_bucket, owned_key) = (bucket.to_owned(), key.to_owned());
    let object = clients
        .call(bucket, move |client| {
            let (bucket, key) = (owned_bucket.clone(), owned_key.clone());
            async move { client.head_object().bucket(bucket).key(key).send().await }
        })
        .await?;
    Ok(head_input(source, uri, &object))
}

/// The same HEAD in the region the bucket was already read in.
async fn head_again(
    client: &Client,
    region: &str,
    source: &str,
    uri: &str,
) -> Result<InputFile, DataError> {
    let (bucket, key) = split_uri(uri)?;
    let object = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|error| s3_error(error, region))?;
    Ok(head_input(source, uri, &object))
}

fn head_input(
    source: &str,
    uri: &str,
    object: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
) -> InputFile {
    InputFile {
        source: source.into(),
        uri: uri.into(),
        size: object.content_length().unwrap_or(0).max(0) as u64,
        etag: object.e_tag().map(str::to_owned),
        version_id: object.version_id().map(str::to_owned),
        modified: object.last_modified().map(ToString::to_string),
    }
}

pub async fn resolve(
    clients: &mut BucketClients,
    source: &DataSource,
    limits: &DataLimits,
    examined: &mut usize,
) -> Result<Vec<InputFile>, DataError> {
    let (bucket, key) = split_uri(&source.path)?;
    let first_glob = key.find(['*', '?', '[']);
    if first_glob.is_none() {
        *examined += 1;
        if *examined > limits.max_source_objects {
            return Err(InvalidInput::SourceObjectLimit.into());
        }
        return Ok(vec![head(clients, &source.name, &source.path).await?]);
    }
    let prefix = &key[..first_glob.unwrap()];
    let pattern = glob::Pattern::new(key).map_err(|_| InvalidInput::InvalidGlob {
        source_name: source.name.clone(),
        pattern: source.path.clone(),
    })?;
    let mut token = None;
    let mut files = vec![];
    loop {
        let left = limits.max_source_objects.saturating_sub(*examined);
        if left == 0 {
            return Err(InvalidInput::SourceObjectLimit.into());
        }
        let page = (
            bucket.to_owned(),
            prefix.to_owned(),
            left.min(1000) as i32,
            token,
        );
        let response = clients
            .call(bucket, move |client| {
                let (bucket, prefix, max_keys, token) =
                    (page.0.clone(), page.1.clone(), page.2, page.3.clone());
                async move {
                    client
                        .list_objects_v2()
                        .bucket(bucket)
                        .prefix(prefix)
                        .max_keys(max_keys)
                        .set_continuation_token(token)
                        .send()
                        .await
                }
            })
            .await?;
        for object in response.contents() {
            *examined += 1;
            if *examined > limits.max_source_objects {
                return Err(InvalidInput::SourceObjectLimit.into());
            }
            if let Some(key) = object
                .key()
                .filter(|key| pattern.matches_with(key, S3_GLOB))
            {
                let uri = format!("s3://{bucket}/{key}");
                validate_resolved_path(&uri)?;
                files.push(InputFile {
                    source: source.name.clone(),
                    uri,
                    size: object.size().unwrap_or(0).max(0) as u64,
                    etag: object.e_tag().map(str::to_owned),
                    version_id: None,
                    modified: object.last_modified().map(ToString::to_string),
                });
            }
        }
        if !response.is_truncated().unwrap_or(false) {
            break;
        }
        token = response.next_continuation_token().map(str::to_owned);
        if token.is_none() {
            return Err(DataError::S3Failed);
        }
    }
    if files.is_empty() {
        return Err(InvalidInput::NoMatchingInputs {
            source_name: source.name.clone(),
            pattern: source.path.clone(),
        }
        .into());
    }
    Ok(files)
}

pub async fn verify_unchanged(
    clients: &mut BucketClients,
    files: &[InputFile],
) -> Result<(), DataError> {
    let mut tasks = JoinSet::new();
    let mut pending = files.iter().filter(|file| file.uri.starts_with("s3://"));
    loop {
        while tasks.len() < FINAL_HEAD_CONCURRENCY {
            let Some(input) = pending.next() else { break };
            let (bucket, _) = split_uri(&input.uri)?;
            let ((client, region), input) = (clients.client(bucket), input.clone());
            let requests = clients.requests.clone();
            tasks.spawn(async move {
                requests.fetch_add(1, Ordering::Relaxed);
                let now = head_again(&client, &region, &input.source, &input.uri).await?;
                if now.size != input.size
                    || now.etag != input.etag
                    || input
                        .version_id
                        .as_ref()
                        .is_some_and(|v| now.version_id.as_ref() != Some(v))
                {
                    return Err(DataError::Changed);
                }
                Ok(())
            });
        }
        match tasks.join_next().await {
            Some(result) => result.map_err(|_| DataError::S3Failed)??,
            None => return Ok(()),
        }
    }
}
/// What a failed call signed for `region` means. A 301 is where the bucket
/// is, never a permission: a HEAD's 301 has no body, so its status is all
/// there is to read.
fn s3_error<E: ProvideErrorMetadata>(error: SdkError<E>, region: &str) -> DataError {
    match error {
        SdkError::ServiceError(error) if error.raw().status().as_u16() == 301 => {
            DataError::S3Redirected {
                region: region.to_owned(),
            }
        }
        SdkError::ServiceError(error) => DataError::S3Rejected(
            error
                .err()
                .code()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("HTTP {}", error.raw().status().as_u16())),
        ),
        error => {
            let kind = match error {
                SdkError::TimeoutError(_) => "timeout",
                SdkError::DispatchFailure(_) => "dispatch",
                SdkError::ResponseError(_) => "response",
                SdkError::ConstructionFailure(_) => "construction",
                _ => "unknown",
            };
            tracing::warn!(kind, "S3 data request failed");
            DataError::S3Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_http_client::test_util::infallible_client_fn;
    use aws_smithy_runtime_api::client::{
        http::{
            HttpClient, HttpConnector, HttpConnectorFuture, SharedHttpConnector, http_client_fn,
        },
        orchestrator::{HttpRequest, HttpResponse},
    };
    use aws_smithy_types::body::SdkBody;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn test_clients(http: impl HttpClient + 'static) -> BucketClients {
        BucketClients::for_tests(http, "ap-northeast-1", true)
    }

    #[test]
    fn s3_keys_are_not_normalized() {
        for key in ["a//../b", "a+b%20c", "日本語 file.csv"] {
            let uri = format!("s3://bucket/{key}");
            assert_eq!(split_uri(&uri).unwrap(), ("bucket", key));
        }
    }

    #[tokio::test]
    async fn data_s3_globs_match_local_separator_rules() {
        let mut clients = test_clients(infallible_client_fn(|request| {
            assert_eq!(request.uri().path(), "/");
            http::Response::builder().status(200).body(
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>month/direct.parquet</Key><Size>1</Size></Contents><Contents><Key>month/nested/part.parquet</Key><Size>1</Size></Contents><Contents><Key>month/.hidden.parquet</Key><Size>1</Size></Contents><Contents><Key>month/UPPER.PARQUET</Key><Size>1</Size></Contents></ListBucketResult>"
            ).unwrap()
        }));
        for (pattern, expected) in [
            (
                "month/*.parquet",
                vec!["month/direct.parquet", "month/.hidden.parquet"],
            ),
            (
                "month/**/*.parquet",
                vec![
                    "month/direct.parquet",
                    "month/nested/part.parquet",
                    "month/.hidden.parquet",
                ],
            ),
        ] {
            let source = serde_json::from_value(
                serde_json::json!({"name":"orders","path":format!("s3://bucket/{pattern}")}),
            )
            .unwrap();
            let files = resolve(&mut clients, &source, &DataLimits::default(), &mut 0)
                .await
                .unwrap();
            assert_eq!(
                files
                    .iter()
                    .map(|file| file.uri.as_str())
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|key| format!("s3://bucket/{key}"))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[tokio::test]
    async fn data_s3_bodyless_head_rejection_keeps_http_status() {
        let mut clients = test_clients(infallible_client_fn(|_| {
            http::Response::builder().status(403).body("").unwrap()
        }));
        let error = head(&mut clients, "orders", "s3://bucket/denied.csv")
            .await
            .unwrap_err();
        assert!(
            matches!(error, DataError::S3Rejected(ref code) if code == "HTTP 403"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn data_s3_a_redirect_that_names_no_region_names_the_signed_one() {
        for correctable in [true, false] {
            let mut clients = BucketClients::for_tests(
                infallible_client_fn(|_| http::Response::builder().status(301).body("").unwrap()),
                "ap-northeast-1",
                correctable,
            );
            let error = head(&mut clients, "orders", "s3://west/orders.parquet")
                .await
                .unwrap_err();
            assert!(
                matches!(error, DataError::S3Redirected { ref region } if region == "ap-northeast-1"),
                "{error}"
            );
            assert_eq!(clients.requests(), 1);
        }
    }

    #[tokio::test]
    async fn data_s3_final_heads_reject_changed_metadata() {
        let mut clients = test_clients(infallible_client_fn(|_| {
            http::Response::builder()
                .status(200)
                .header("content-length", "2")
                .header("etag", "\"etag\"")
                .header("x-amz-version-id", "v2")
                .body("")
                .unwrap()
        }));
        for (size, etag, version, changed) in [
            (1, "\"etag\"", Some("v2"), true),
            (2, "\"old\"", Some("v2"), true),
            (2, "\"etag\"", Some("v1"), true),
            (2, "\"etag\"", Some("v2"), false),
            (2, "\"etag\"", None, false),
        ] {
            let input = InputFile {
                source: "orders".into(),
                uri: "s3://bucket/orders.csv".into(),
                size,
                etag: Some(etag.into()),
                version_id: version.map(str::to_owned),
                modified: None,
            };
            let result = verify_unchanged(&mut clients, &[input]).await;
            if changed {
                assert!(matches!(result, Err(DataError::Changed)), "{result:?}");
            } else {
                result.unwrap();
            }
        }
    }

    #[derive(Debug, Default)]
    struct HeadCounts {
        active: AtomicUsize,
        peak: AtomicUsize,
        completed: AtomicUsize,
    }

    #[derive(Debug)]
    struct DelayedHeads(Arc<HeadCounts>);

    impl HttpConnector for DelayedHeads {
        fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
            assert_eq!(request.method(), "HEAD");
            let counts = self.0.clone();
            HttpConnectorFuture::new(async move {
                let active = counts.active.fetch_add(1, Ordering::SeqCst) + 1;
                counts.peak.fetch_max(active, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(1)).await;
                counts.active.fetch_sub(1, Ordering::SeqCst);
                counts.completed.fetch_add(1, Ordering::SeqCst);
                Ok(HttpResponse::try_from(
                    http::Response::builder()
                        .status(200)
                        .header("content-length", "1")
                        .body(SdkBody::empty())
                        .unwrap(),
                )
                .unwrap())
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn data_s3_final_heads_have_bounded_concurrency() {
        let counts = Arc::new(HeadCounts::default());
        let observed = counts.clone();
        let mut clients = test_clients(http_client_fn(move |_, _| {
            SharedHttpConnector::new(DelayedHeads(observed.clone()))
        }));
        let mut files: Vec<_> = (0..40)
            .map(|index| InputFile {
                source: "orders".into(),
                uri: format!("s3://bucket/{index}.csv"),
                size: 1,
                etag: None,
                version_id: None,
                modified: None,
            })
            .collect();
        files.push(InputFile {
            source: "local".into(),
            uri: "/tmp/orders.csv".into(),
            size: 1,
            etag: None,
            version_id: None,
            modified: None,
        });
        verify_unchanged(&mut clients, &files).await.unwrap();
        assert_eq!(counts.completed.load(Ordering::SeqCst), 40);
        assert_eq!(counts.peak.load(Ordering::SeqCst), 16);
    }

    /// A fake bucket that answers only requests signed for its own region, the
    /// way S3 answers a request routed to the wrong one.
    #[derive(Debug)]
    struct RegionalBucket {
        region: &'static str,
        signed: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl HttpConnector for RegionalBucket {
        fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
            // "AWS4-HMAC-SHA256 Credential=<key>/<date>/<region>/s3/aws4_request, ..."
            let signed = request
                .headers()
                .get("authorization")
                .and_then(|value| value.split('/').nth(2))
                .unwrap_or_default()
                .to_owned();
            self.signed.lock().unwrap().push(signed.clone());
            let region = self.region;
            HttpConnectorFuture::new(async move {
                let response = if signed == region {
                    http::Response::builder()
                        .status(200)
                        .header("content-length", "7")
                        .body(SdkBody::empty())
                } else {
                    http::Response::builder()
                        .status(301)
                        .header("x-amz-bucket-region", region)
                        .body(SdkBody::empty())
                };
                Ok(HttpResponse::try_from(response.unwrap()).unwrap())
            })
        }
    }

    fn regional_bucket(
        signed: &Arc<std::sync::Mutex<Vec<String>>>,
    ) -> impl HttpClient + 'static + use<> {
        let signed = signed.clone();
        http_client_fn(move |_, _| {
            SharedHttpConnector::new(RegionalBucket {
                region: "us-west-2",
                signed: signed.clone(),
            })
        })
    }

    #[tokio::test]
    async fn data_s3_reads_a_bucket_in_the_region_s3_names() {
        let signed = Arc::new(std::sync::Mutex::new(vec![]));
        let mut clients =
            BucketClients::for_tests(regional_bucket(&signed), "ap-northeast-1", true);
        let file = head(&mut clients, "orders", "s3://west/orders.parquet")
            .await
            .unwrap();
        assert_eq!(file.size, 7);
        assert_eq!(clients.regions()["west"], "us-west-2");
        head(&mut clients, "orders", "s3://west/orders.parquet")
            .await
            .unwrap();
        // The rejected request is retried once; the next call starts where the
        // bucket lives.
        assert_eq!(
            *signed.lock().unwrap(),
            ["ap-northeast-1", "us-west-2", "us-west-2"]
        );
    }

    #[tokio::test]
    async fn data_s3_uses_a_named_region_as_it_is() {
        let signed = Arc::new(std::sync::Mutex::new(vec![]));
        let mut clients =
            BucketClients::for_tests(regional_bucket(&signed), "ap-northeast-1", false);
        let error = head(&mut clients, "orders", "s3://west/orders.parquet")
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                DataError::S3OtherRegion { ref region, ref bucket_region }
                    if region == "ap-northeast-1" && bucket_region == "us-west-2"
            ),
            "{error}"
        );
        assert_eq!(*signed.lock().unwrap(), ["ap-northeast-1"]);
        assert_eq!(clients.regions()["west"], "ap-northeast-1");
    }

    /// Renewed credentials drop the clients signed with the old ones and keep
    /// where each bucket lives; the same credentials keep the clients.
    #[test]
    fn s3_renewed_credentials_rebuild_the_clients_and_keep_the_regions() {
        let old = Credentials::new("OLD".into(), "SECRET".into(), None, None);
        let mut clients = BucketClients::new(&old, "ap-northeast-1", true, 30);
        let _ = clients.client("west");
        clients.regions.insert("west".into(), "us-west-2".into());
        clients.renew(&old);
        assert_eq!(clients.clients.len(), 1);
        let new = Credentials::new("NEW".into(), "SECRET".into(), None, None);
        clients.renew(&new);
        assert!(clients.clients.is_empty());
        assert_eq!(clients.credentials.access_key_id(), "NEW");
        assert_eq!(clients.regions()["west"], "us-west-2");
    }
}
