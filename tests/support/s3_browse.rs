//! S3 fixture for `kurama s3`: exact keys, common prefixes, paginated ListObjectsV2 (one bucket slow after its first page), per-bucket regions and refusals, and HeadObject / ranged, conditional GetObject over object bodies.
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path, path_regex},
};

/// Keys as S3 holds them: `%`, `+`, spaces, `..` and repeated `/` are part
/// of the key, and case matters.
const KEYS: &[&str] = &[
    "other/invoice-999.pdf",
    "reports/2024/a.csv",
    "reports/2024/b.csv",
    "reports/2025/c.csv",
    "reports/Invoice-003.pdf",
    "reports/a b/../c+d%20.csv",
    "reports/invoice-001.pdf",
    "reports/invoice-002.pdf",
    "reports/summary.txt",
    "reports/日本語 +%//x.csv",
];

/// How long `browse-slow` takes to answer any page but its first.
const SLOW_PAGE: std::time::Duration = std::time::Duration::from_secs(30);

/// An object of the fixture: its bytes and what HEAD says about them.
struct Object {
    key: &'static str,
    body: Vec<u8>,
    content_type: Option<&'static str>,
    storage_class: &'static str,
    /// The ETag a listing reports when it is not the object's own: the object
    /// changed after it was listed.
    listed_etag: Option<&'static str>,
}

impl Object {
    fn text(key: &'static str, body: &str) -> Self {
        Self {
            key,
            body: body.as_bytes().to_vec(),
            content_type: None,
            storage_class: "STANDARD",
            listed_etag: None,
        }
    }

    /// The ETag the body has now, derived from it.
    fn etag(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.body.hash(&mut hasher);
        format!("\"{:016x}\"", hasher.finish())
    }
}

fn gzip(text: &str) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

/// The bucket whose objects have contents: text in every format a preview
/// names, a gzip stream, a binary, an empty and an archived object, logs to
/// search, one that changes after it is listed and keys the role may not read.
fn objects() -> Vec<Object> {
    vec![
        Object {
            content_type: Some("application/json"),
            ..Object::text(
                "data/report.json",
                "{\"name\":\"βeta\",\"items\":[1,2,3]}\n",
            )
        },
        Object::text("data/broken.json", "{\"name\": [1, 2"),
        Object::text("data/orders.csv", "id,amount\n1,10\n2,20\n"),
        Object::text("data/empty.txt", ""),
        Object {
            body: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01".to_vec(),
            content_type: Some("image/png"),
            ..Object::text("data/image.png", "")
        },
        Object {
            body: gzip(
                "{\"id\":1,\"msg\":\"start\"}\n{\"id\":2,\"msg\":\"request-id-123 done\"}\n",
            ),
            ..Object::text("data/events.jsonl.gz", "")
        },
        Object {
            storage_class: "GLACIER",
            ..Object::text("archive/old.csv", "a,b\n1,2\n")
        },
        Object::text(
            "logs/a.log",
            "boot\nGET /x request-id-123 200\nGET /y request-id-456 200\n",
        ),
        Object {
            body: gzip("line one\nPOST /z request-id-123 500\n"),
            ..Object::text("logs/b.log.gz", "")
        },
        Object {
            body: b"\0\x01request-id-123\0".to_vec(),
            ..Object::text("logs/c.bin", "")
        },
        Object::text("logs/d.log", "nothing here\n"),
        Object {
            listed_etag: Some("\"listed-before-a-change\""),
            ..Object::text("logs/e.log", "request-id-123 after the change\n")
        },
        Object::text("notes/1.txt", "first note\n"),
        Object::text("notes/2.txt", "second note\n"),
        Object::text("secret/keys.txt", "not for this role\n"),
    ]
}

pub async fn mount(server: &MockServer) {
    let keyed = |keys: &[&'static str]| keys.iter().map(|key| Object::text(key, key)).collect();
    let buckets = BTreeMap::from([
        (
            "browse-bucket",
            Bucket {
                region: "ap-northeast-1",
                objects: keyed(KEYS),
            },
        ),
        // A bucket that lives in another region than every AWS profile's.
        (
            "browse-west",
            Bucket {
                region: "us-west-2",
                objects: keyed(&["reports/west.csv"]),
            },
        ),
        (
            "browse-objects",
            Bucket {
                region: "ap-northeast-1",
                objects: objects(),
            },
        ),
        // Every page after the first answers only after SLOW_PAGE: a search
        // over it is still running when a scenario presses Esc.
        (
            "browse-slow",
            Bucket {
                region: "ap-northeast-1",
                objects: keyed(&[
                    "slow/k1.log",
                    "slow/k2.log",
                    "slow/k3.log",
                    "slow/k4.log",
                    "slow/k5.log",
                    "slow/k6.log",
                ]),
            },
        ),
    ]);
    let buckets = std::sync::Arc::new(buckets);
    Mock::given(path_regex("^/browse-[^/]+/?$"))
        .respond_with(Listing {
            buckets: buckets.clone(),
        })
        .mount(server)
        .await;
    Mock::given(path_regex("^/browse-[^/]+/.+"))
        .respond_with(Objects { buckets })
        .mount(server)
        .await;
    // ListBuckets: this role may open the buckets it knows, not list them.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(|request: &Request| {
            signed(request).unwrap_or_else(|| rejection(403, "AccessDenied"))
        })
        .mount(server)
        .await;
}

struct Bucket {
    /// The only region this bucket answers requests signed for.
    region: &'static str,
    objects: Vec<Object>,
}

type Buckets = std::sync::Arc<BTreeMap<&'static str, Bucket>>;

struct Listing {
    buckets: Buckets,
}

struct Objects {
    buckets: Buckets,
}

/// The rejection a request without the role's valid signature gets, if any.
fn signed(request: &Request) -> Option<ResponseTemplate> {
    match super::decode_sigv4(request) {
        Some(signature)
            if signature.valid
                && signature.security_token.as_deref() == Some(super::RESULT_TOKEN) =>
        {
            None
        }
        _ => Some(rejection(403, "InvalidSignature")),
    }
}

impl Respond for Listing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if let Some(rejected) = signed(request) {
            return rejected;
        }
        let name = request.url.path().trim_matches('/');
        if name == "browse-denied" {
            return rejection(403, "AccessDenied");
        }
        let Some(bucket) = self.buckets.get(name) else {
            return rejection(404, "NoSuchBucket");
        };
        if let Some(redirect) = elsewhere(request, bucket) {
            return redirect;
        }
        let query: BTreeMap<_, _> = request.url.query_pairs().collect();
        if request.method != "GET" || query.get("list-type").map(|v| v.as_ref()) != Some("2") {
            return rejection(400, "InvalidRequest");
        }
        let prefix = query.get("prefix").map(|v| v.as_ref()).unwrap_or("");
        let delimiter = query.get("delimiter").map(|v| v.as_ref());
        let Some(max_keys) = query
            .get("max-keys")
            .map_or(Some(1000), |v| v.parse::<usize>().ok())
            .filter(|keys| (1..=1000).contains(keys))
        else {
            return rejection(400, "InvalidArgument");
        };
        let start = match query.get("continuation-token") {
            None => 0,
            Some(token) => match token.strip_prefix("offset-").and_then(|n| n.parse().ok()) {
                Some(start) => start,
                None => return rejection(400, "InvalidArgument"),
            },
        };
        // What S3 lists: each key under the prefix, or the common prefix it
        // rolls up into, once, in key order.
        let entries: BTreeSet<(String, bool)> = bucket
            .objects
            .iter()
            .filter_map(|object| {
                object
                    .key
                    .strip_prefix(prefix)
                    .map(|rest| (object.key, rest))
            })
            .map(
                |(key, rest)| match delimiter.and_then(|d| rest.find(d).map(|at| at + d.len())) {
                    Some(end) => (format!("{prefix}{}", &rest[..end]), true),
                    None => (key.to_owned(), false),
                },
            )
            .collect();
        let entries: Vec<_> = entries.into_iter().collect();
        if start > entries.len() {
            return rejection(400, "InvalidArgument");
        }
        let end = (start + max_keys).min(entries.len());
        let truncated = end < entries.len();
        let mut xml = format!(
            "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{name}</Name><Prefix>{}</Prefix><KeyCount>{}</KeyCount><MaxKeys>{max_keys}</MaxKeys><IsTruncated>{truncated}</IsTruncated>",
            xml_text(prefix),
            end - start
        );
        if truncated {
            xml.push_str(&format!(
                "<NextContinuationToken>offset-{end}</NextContinuationToken>"
            ));
        }
        for (entry, common) in &entries[start..end] {
            if *common {
                xml.push_str(&format!(
                    "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
                    xml_text(entry)
                ));
            } else {
                let object = bucket
                    .objects
                    .iter()
                    .find(|o| o.key == entry)
                    .expect("a listed key is an object");
                xml.push_str(&format!(
                    "<Contents><Key>{}</Key><Size>{}</Size><ETag>{}</ETag><LastModified>2026-09-01T00:00:00.000Z</LastModified><StorageClass>{}</StorageClass></Contents>",
                    xml_text(entry),
                    object.body.len(),
                    xml_text(&object.listed_etag.map_or_else(|| object.etag(), str::to_owned)),
                    object.storage_class,
                ));
            }
        }
        xml.push_str("</ListBucketResult>");
        let template = ResponseTemplate::new(200).set_body_string(xml);
        if name == "browse-slow" && start > 0 {
            return template.set_delay(SLOW_PAGE);
        }
        template
    }
}

/// The redirect a request signed for another region than the bucket's gets.
fn elsewhere(request: &Request, bucket: &Bucket) -> Option<ResponseTemplate> {
    let region = super::decode_sigv4(request).map(|s| s.region);
    (region.as_deref() != Some(bucket.region)).then(|| {
        rejection(301, "PermanentRedirect").insert_header("x-amz-bucket-region", bucket.region)
    })
}

impl Respond for Objects {
    /// HeadObject and GetObject: a GET honours one `Range: bytes=a-b` and
    /// `If-Match`, the way S3 does; `secret/` is refused to this role.
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if let Some(rejected) = signed(request) {
            return rejected;
        }
        let path = request.url.path().trim_start_matches('/');
        let (name, encoded) = path.split_once('/').expect("the route has a key");
        let key = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .expect("a key is UTF-8");
        let Some(bucket) = self.buckets.get(name) else {
            return rejection(404, "NoSuchBucket");
        };
        if let Some(redirect) = elsewhere(request, bucket) {
            return redirect;
        }
        if key.starts_with("secret/") {
            return rejection(403, "AccessDenied");
        }
        let Some(object) = bucket.objects.iter().find(|o| o.key == key) else {
            return rejection(404, "NoSuchKey");
        };
        let etag = object.etag();
        let size = object.body.len();
        let mut response = ResponseTemplate::new(200)
            .insert_header("etag", etag.as_str())
            .insert_header("last-modified", "Tue, 01 Sep 2026 00:00:00 GMT");
        if let Some(content_type) = object.content_type {
            response = response.insert_header("content-type", content_type);
        }
        if object.storage_class != "STANDARD" {
            response = response.insert_header("x-amz-storage-class", object.storage_class);
        }
        if request.method == "HEAD" {
            return response.insert_header("content-length", size.to_string().as_str());
        }
        if request.method != "GET" {
            return rejection(405, "MethodNotAllowed");
        }
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        if object.storage_class == "GLACIER" {
            return rejection(403, "InvalidObjectState");
        }
        if header("if-match").is_some_and(|wanted| wanted != etag) {
            return rejection(412, "PreconditionFailed");
        }
        let Some(range) = header("range") else {
            return response.set_body_bytes(object.body.clone());
        };
        let Some((start, end)) = range
            .strip_prefix("bytes=")
            .and_then(|r| r.split_once('-'))
            .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)))
        else {
            return rejection(400, "InvalidArgument");
        };
        if start >= size || end < start {
            return rejection(416, "InvalidRange");
        }
        let end = end.min(size - 1);
        ResponseTemplate::new(206)
            .insert_header("etag", etag.as_str())
            .insert_header(
                "content-range",
                format!("bytes {start}-{end}/{size}").as_str(),
            )
            .set_body_bytes(object.body[start..=end].to_vec())
    }
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn rejection(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_string(format!("<Error><Code>{code}</Code></Error>"))
}

/// The `[s3.*]` sections the explorer scenarios open: no start (the bucket
/// list), a bucket root, and the slow bucket two keys a page.
const EXPLORER_CONFIG: &str = "[s3.assets]\naws_profile = \"dev\"\n\n\
     [s3.walk]\naws_profile = \"dev\"\nbucket = \"browse-bucket\"\n\n\
     [s3.slow]\naws_profile = \"dev\"\nbucket = \"browse-slow\"\nprefix = \"slow/\"\npage_size = 2\n";

/// A TUI scenario of the S3 explorer, against this fake.
pub fn explorer_scenario(id: &'static str) -> super::Scenario {
    super::Scenario::tui(id)
        .for_feature("s3-explorer")
        .with_extra_config(EXPLORER_CONFIG)
}

/// The ListObjectsV2 requests among `calls`.
pub fn listings(calls: &[super::ApiCall]) -> usize {
    calls
        .iter()
        .filter(|call| call.path.contains("list-type=2"))
        .count()
}

/// The GetObject requests among `calls`.
pub fn gets(calls: &[super::ApiCall]) -> usize {
    calls
        .iter()
        .filter(|call| call.method == "GET" && call.path.contains("x-id=GetObject"))
        .count()
}
