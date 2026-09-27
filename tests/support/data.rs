//! S3 HTTP fixture: exact object keys, per-bucket regions, signed HEAD/Range GET, paginated LIST and service rejection.
use kurama::adapters::duckdb::connection::{Cancellation, Connection};
use kurama::domain::types::dataset::sql_string;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::path_regex};

pub async fn mount(server: &MockServer) {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(
        &[
            ("threads", "1".into()),
            ("autoload_known_extensions", "false".into()),
        ],
        Arc::new(Cancellation::default()),
    )
    .unwrap();
    let mut objects = BTreeMap::new();
    for (index, key, values) in [
        (
            0,
            "orders.parquet",
            "('001',40.25::DECIMAL(18,2)),('002',50.50),('001',6.25)",
        ),
        (1, "month/part.parquet", "('001',7.50::DECIMAL(18,2))"),
        (
            2,
            "month/nested/part.parquet",
            "('002',11.25::DECIMAL(18,2))",
        ),
    ] {
        let path = directory.path().join(format!("{index}.parquet"));
        connection.execute(&format!("COPY (SELECT * FROM (VALUES {values}) t(customer_id,amount)) TO {} (FORMAT PARQUET)", sql_string(&path.to_string_lossy())), "fixture").unwrap();
        objects.insert(
            key,
            FixtureObject {
                key,
                body: std::fs::read(path).unwrap(),
            },
        );
    }
    // One object big enough that the engine reads its footer separately from
    // its data. Every other Parquet fixture here fits in a single range, which
    // makes the bytes transferred equal to the object size and leaves a check
    // on `bytes_transferred` unable to tell a measurement from a copy of it.
    let path = directory.path().join("wide.parquet");
    connection.execute(&format!("COPY (SELECT i::BIGINT AS amount, repeat(md5(i::VARCHAR), 8) AS body_text FROM range(20000) t(i)) TO {} (FORMAT PARQUET, ROW_GROUP_SIZE 2048)", sql_string(&path.to_string_lossy())), "fixture").unwrap();
    objects.insert(
        "wide/data.parquet",
        FixtureObject {
            key: "wide/data.parquet",
            body: std::fs::read(path).unwrap(),
        },
    );
    for (encoded_key, key, body) in [
        (
            "events.jsonl",
            "events.jsonl",
            include_bytes!("../fixtures/data/events.jsonl").as_slice(),
        ),
        (
            "changed.csv",
            "changed.csv",
            include_bytes!("../fixtures/data/orders.csv").as_slice(),
        ),
        (
            "%E6%97%A5%E6%9C%AC%E8%AA%9E%20%2B%25//orders.csv",
            "日本語 +%//orders.csv",
            include_bytes!("../fixtures/data/orders.csv").as_slice(),
        ),
        (
            "many/1.csv",
            "many/1.csv",
            b"customer_id,amount\n001,101.00\n".as_slice(),
        ),
        (
            "many/2.csv",
            "many/2.csv",
            b"customer_id,amount\n002,202.00\n".as_slice(),
        ),
    ] {
        objects.insert(
            encoded_key,
            FixtureObject {
                key,
                body: body.to_vec(),
            },
        );
    }
    // A bucket that lives in another region than every AWS profile's.
    let west = BTreeMap::from([(
        "orders.parquet",
        FixtureObject {
            key: "orders.parquet",
            body: objects["orders.parquet"].body.clone(),
        },
    )]);
    Mock::given(path_regex(
        "^/(?:data-bucket|west-bucket|moved-bucket)(?:/.*)?$",
    ))
    .respond_with(S3Endpoint {
        buckets: BTreeMap::from([
            (
                "data-bucket",
                Bucket {
                    region: "ap-northeast-1",
                    names_its_region: true,
                    objects,
                },
            ),
            (
                "west-bucket",
                Bucket {
                    region: WEST_REGION,
                    names_its_region: true,
                    objects: west,
                },
            ),
            // A bucket whose 301 does not say where it lives.
            (
                "moved-bucket",
                Bucket {
                    region: WEST_REGION,
                    names_its_region: false,
                    objects: BTreeMap::new(),
                },
            ),
        ]),
        changed_heads: AtomicUsize::new(0),
    })
    .mount(server)
    .await;
}

/// Where `west-bucket` lives: not the region of any AWS profile the scenarios configure.
const WEST_REGION: &str = "us-west-2";

struct FixtureObject {
    key: &'static str,
    body: Vec<u8>,
}

struct Bucket {
    /// The only region this bucket answers requests signed for.
    region: &'static str,
    /// Whether its 301 carries `x-amz-bucket-region`.
    names_its_region: bool,
    /// Only the exact encoded key is accepted; no decoding or path normalization.
    objects: BTreeMap<&'static str, FixtureObject>,
}

struct S3Endpoint {
    buckets: BTreeMap<&'static str, Bucket>,
    changed_heads: AtomicUsize,
}

impl Respond for S3Endpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path();
        let Some(signature) = super::decode_sigv4(request).filter(|signature| {
            signature.valid && signature.security_token.as_deref() == Some(super::RESULT_TOKEN)
        }) else {
            return s3_rejection(request, 403, "InvalidSignature");
        };
        let (name, key) = match path.trim_start_matches('/').split_once('/') {
            Some((name, key)) => (name, key),
            None => (path.trim_start_matches('/'), ""),
        };
        let Some(bucket) = self.buckets.get(name) else {
            return s3_rejection(request, 404, "NoSuchBucket");
        };
        // S3 answers a request signed for another region with the bucket's own.
        if signature.region != bucket.region {
            let redirect = s3_rejection(request, 301, "PermanentRedirect");
            return if bucket.names_its_region {
                redirect.insert_header("x-amz-bucket-region", bucket.region)
            } else {
                redirect
            };
        }
        if path == "/data-bucket/denied.csv" {
            return s3_rejection(request, 403, "AccessDenied");
        }
        if request
            .url
            .query_pairs()
            .any(|(name, value)| name == "list-type" && value == "2")
        {
            if request.method != "GET" || !key.is_empty() {
                return s3_rejection(request, 400, "InvalidRequest");
            }
            return self.list_objects(request, bucket);
        }
        let Some((encoded_key, object)) = bucket.objects.get_key_value(key) else {
            return s3_rejection(request, 404, "NoSuchKey");
        };
        let body = &object.body;
        let changed = object.key == "changed.csv"
            && request.method == "HEAD"
            && request
                .headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("aws-sdk-rust/"))
            && self.changed_heads.fetch_add(1, Ordering::SeqCst) > 0;
        let etag = if changed {
            "\"changed\"".into()
        } else {
            object_etag(encoded_key)
        };
        if request.method == "HEAD" {
            return ResponseTemplate::new(200)
                .insert_header("Content-Length", body.len().to_string())
                .insert_header("ETag", etag)
                .insert_header("x-amz-version-id", "v1");
        }
        if request.method != "GET" {
            return s3_rejection(request, 405, "MethodNotAllowed");
        }
        if let Some(range) = request
            .headers
            .get("range")
            .and_then(|value| value.to_str().ok())
        {
            let Some((start, end)) = range
                .strip_prefix("bytes=")
                .and_then(|value| value.split_once('-'))
            else {
                return ResponseTemplate::new(416);
            };
            let Ok(start) = start.parse::<usize>() else {
                return ResponseTemplate::new(416);
            };
            let end = if end.is_empty() {
                body.len() - 1
            } else {
                let Ok(end) = end.parse::<usize>() else {
                    return ResponseTemplate::new(416);
                };
                end.min(body.len() - 1)
            };
            if start > end {
                return ResponseTemplate::new(416);
            }
            return ResponseTemplate::new(206)
                .insert_header("ETag", etag)
                .insert_header(
                    "Content-Range",
                    format!("bytes {start}-{end}/{}", body.len()),
                )
                .set_body_bytes(body[start..=end].to_vec());
        }
        ResponseTemplate::new(200)
            .insert_header("ETag", etag)
            .set_body_bytes(body.clone())
    }
}

impl S3Endpoint {
    fn list_objects(&self, request: &Request, bucket: &Bucket) -> ResponseTemplate {
        let query: BTreeMap<_, _> = request.url.query_pairs().collect();
        let prefix = query
            .get("prefix")
            .map(|value| value.as_ref())
            .unwrap_or("");
        let max_keys = match query.get("max-keys") {
            Some(value) => match value.parse::<usize>() {
                Ok(value) if value <= 1000 => value,
                _ => return s3_rejection(request, 400, "InvalidArgument"),
            },
            None => 1000,
        };
        let start = match query.get("continuation-token") {
            Some(value) => match value
                .strip_prefix("offset-")
                .and_then(|value| value.parse::<usize>().ok())
            {
                Some(value) => value,
                None => return s3_rejection(request, 400, "InvalidArgument"),
            },
            None => 0,
        };
        let mut matching: Vec<_> = bucket
            .objects
            .iter()
            .filter(|(_, object)| object.key.starts_with(prefix))
            .collect();
        matching.sort_by_key(|(_, object)| object.key);
        if start > matching.len() {
            return s3_rejection(request, 400, "InvalidArgument");
        }
        let end = (start + max_keys).min(matching.len());
        let truncated = max_keys > 0 && end < matching.len();
        let mut xml = format!(
            "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Prefix>{}</Prefix><MaxKeys>{max_keys}</MaxKeys><KeyCount>{}</KeyCount><IsTruncated>{truncated}</IsTruncated>",
            xml_text(prefix),
            end - start
        );
        if truncated {
            xml.push_str(&format!(
                "<NextContinuationToken>offset-{end}</NextContinuationToken>"
            ));
        }
        for (encoded_key, object) in &matching[start..end] {
            xml.push_str(&format!(
                "<Contents><Key>{}</Key><Size>{}</Size><ETag>{}</ETag></Contents>",
                xml_text(object.key),
                object.body.len(),
                xml_text(&object_etag(encoded_key))
            ));
        }
        xml.push_str("</ListBucketResult>");
        ResponseTemplate::new(200).set_body_string(xml)
    }
}

fn object_etag(encoded_key: &str) -> String {
    format!("\"fixture-{encoded_key}\"")
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn s3_rejection(request: &Request, status: u16, code: &str) -> ResponseTemplate {
    let response = ResponseTemplate::new(status);
    if request.method == "HEAD" {
        response
    } else {
        response.set_body_string(format!("<Error><Code>{code}</Code></Error>"))
    }
}
