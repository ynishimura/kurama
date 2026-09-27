//! The cache of API descriptions fetched from a URL:
//! `~/.cache/kurama/openapi/<key>.json` holds the URL, the validators the
//! server sent (`ETag`, `Last-Modified`) and the fetch time; `<key>.body`
//! holds the document as received. Nothing else: no credential, no token,
//! no header of the request that fetched it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::adapters::profile_lock::ProfileLock;

/// One cached document with what is needed to revalidate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedSpec {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub fetched_at: DateTime<Utc>,
    pub body: Vec<u8>,
}

impl CachedSpec {
    /// A future timestamp is not trusted; a zero interval always revalidates.
    pub fn is_fresh(&self, now: DateTime<Utc>, interval: std::time::Duration) -> bool {
        (now - self.fetched_at)
            .to_std()
            .is_ok_and(|age| age < interval)
    }
}

#[derive(Serialize, Deserialize)]
struct Metadata {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    fetched_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SpecCache {
    dir: PathBuf,
}

impl SpecCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The cached document for `url`, `None` when there is none or the
    /// entry cannot be read (it is fetched again then).
    pub fn load(&self, url: &str) -> Option<CachedSpec> {
        self.read(url).ok().flatten()
    }

    /// The cached document for `url`: `None` when nothing was stored, an
    /// error naming the file when what is stored cannot be read back.
    pub fn read(&self, url: &str) -> Result<Option<CachedSpec>, String> {
        let (metadata_path, body_path) = self.paths(url);
        if !metadata_path.exists() {
            return Ok(None);
        }
        let metadata = read_metadata(&metadata_path)
            .map_err(|error| format!("{}: {error}", metadata_path.display()))?;
        if metadata.url != url {
            return Err(format!(
                "{}: holds the description of another URL",
                metadata_path.display()
            ));
        }
        let body = std::fs::read(&body_path)
            .map_err(|error| format!("read {}: {error}", body_path.display()))?;
        Ok(Some(CachedSpec {
            url: metadata.url,
            etag: metadata.etag,
            last_modified: metadata.last_modified,
            fetched_at: metadata.fetched_at,
            body,
        }))
    }

    /// Serialize writers and replace each file whole, so readers never see partial bytes.
    #[cfg(test)]
    pub fn store(&self, entry: &CachedSpec) -> Result<(), String> {
        self.store_with_wait(entry, || {})
    }

    /// The async caller's blocking-safe entry point. The callback runs only
    /// when another process already owns the cache lock.
    pub async fn store_async<F>(&self, entry: &CachedSpec, on_wait: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let cache = self.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || cache.store_with_wait(&entry, on_wait))
            .await
            .map_err(|error| format!("cache lock worker failed: {error}"))?
    }

    fn store_with_wait(&self, entry: &CachedSpec, on_wait: impl FnOnce()) -> Result<(), String> {
        let _lock = self.lock_entry(&entry.url, on_wait)?;
        let (metadata_path, body_path) = self.paths(&entry.url);
        replace_file(&body_path, &entry.body)?;
        let metadata = Metadata {
            url: entry.url.clone(),
            etag: entry.etag.clone(),
            last_modified: entry.last_modified.clone(),
            fetched_at: entry.fetched_at,
        };
        write_metadata(&metadata_path, &metadata)
    }

    /// Renew only the generation the server validated; a concurrent refresh wins.
    /// A 304 must never write the response body from an older in-memory copy.
    #[cfg(test)]
    pub fn update_fetched_at(
        &self,
        expected: &CachedSpec,
        fetched_at: DateTime<Utc>,
    ) -> Result<(), String> {
        self.update_fetched_at_with_wait(expected, fetched_at, || {})
    }

    /// Async counterpart that keeps the synchronous lock off the runtime thread.
    pub async fn update_fetched_at_async<F>(
        &self,
        expected: &CachedSpec,
        fetched_at: DateTime<Utc>,
        on_wait: F,
    ) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let cache = self.clone();
        let expected = expected.clone();
        tokio::task::spawn_blocking(move || {
            cache.update_fetched_at_with_wait(&expected, fetched_at, on_wait)
        })
        .await
        .map_err(|error| format!("cache lock worker failed: {error}"))?
    }

    fn update_fetched_at_with_wait(
        &self,
        expected: &CachedSpec,
        fetched_at: DateTime<Utc>,
        on_wait: impl FnOnce(),
    ) -> Result<(), String> {
        let _lock = self.lock_entry(&expected.url, on_wait)?;
        let (path, _) = self.paths(&expected.url);
        let mut current = read_metadata(&path)?;
        if current.url != expected.url
            || current.etag != expected.etag
            || current.last_modified != expected.last_modified
            || current.fetched_at != expected.fetched_at
        {
            return Ok(());
        }
        current.fetched_at = fetched_at;
        write_metadata(&path, &current)
    }

    fn lock_entry(&self, url: &str, on_wait: impl FnOnce()) -> Result<ProfileLock, String> {
        ProfileLock::acquire(&self.dir, &cache_key(url), on_wait)
            .map_err(|error| format!("lock cache {}: {error}", self.dir.display()))
    }

    /// The metadata and the body file of `url`.
    pub fn paths(&self, url: &str) -> (PathBuf, PathBuf) {
        let key = cache_key(url);
        (
            self.dir.join(format!("{key}.json")),
            self.dir.join(format!("{key}.body")),
        )
    }
}

fn read_metadata(path: &Path) -> Result<Metadata, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn write_metadata(path: &Path, metadata: &Metadata) -> Result<(), String> {
    replace_file(
        path,
        &serde_json::to_vec_pretty(metadata).map_err(|error| error.to_string())?,
    )
}

fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("rename {}: {error}", path.display()))
}

/// The first 16 hex digits of the URL's SHA-256: a stable file name.
pub fn cache_key(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_rejects_future_timestamps_and_expires_at_the_interval() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        for (age, expected) in [
            (-1, false),
            (0, true),
            (3599, true),
            (3600, false),
            (3601, false),
        ] {
            let cached = CachedSpec {
                url: "https://example.com/spec.json".into(),
                etag: None,
                last_modified: None,
                body: Vec::new(),
                fetched_at: now - chrono::Duration::seconds(age),
            };
            assert_eq!(
                cached.is_fresh(now, std::time::Duration::from_secs(3600)),
                expected,
                "age={age}"
            );
            assert!(
                !cached.is_fresh(now, std::time::Duration::ZERO),
                "zero interval, age={age}"
            );
        }
    }

    #[test]
    fn a_stored_document_is_loaded_back_with_its_validators() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SpecCache::new(dir.path().join("openapi"));
        let url = "https://example.com/openapi.json";
        assert!(cache.load(url).is_none());
        let entry = CachedSpec {
            url: url.into(),
            etag: Some("\"abc\"".into()),
            last_modified: Some("Wed, 17 Sep 2026 00:00:00 GMT".into()),
            fetched_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            body: b"{\"openapi\":\"3.0.0\"}".to_vec(),
        };
        cache.store(&entry).unwrap();
        assert_eq!(cache.load(url), Some(entry.clone()));
        assert!(cache.load("https://example.com/other.json").is_none());
        let (metadata, body) = cache.paths(url);
        assert_eq!(
            metadata.file_name().unwrap(),
            format!("{}.json", cache_key(url)).as_str()
        );
        assert_eq!(std::fs::read(body).unwrap(), entry.body);
        let metadata = std::fs::read_to_string(metadata).unwrap();
        assert!(metadata.contains("\"etag\": \"\\\"abc\\\"\""), "{metadata}");
        let leftovers = std::fs::read_dir(dir.path().join("openapi"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "temporary files are renamed away");
    }

    #[test]
    fn a_corrupt_entry_is_treated_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SpecCache::new(dir.path().to_path_buf());
        let url = "https://example.com/openapi.yaml";
        let (metadata, body) = cache.paths(url);
        std::fs::write(&metadata, "not json").unwrap();
        std::fs::write(&body, "x").unwrap();
        assert!(cache.load(url).is_none());
        std::fs::write(
            &metadata,
            "{\"url\":\"https://elsewhere\",\"fetched_at\":\"2026-01-01T00:00:00Z\"}",
        )
        .unwrap();
        assert!(cache.load(url).is_none(), "a key collision is not trusted");
        let error = cache.read(url).unwrap_err();
        assert!(error.contains(&metadata.display().to_string()), "{error}");
        std::fs::write(&metadata, "not json").unwrap();
        let error = cache.read(url).unwrap_err();
        assert!(
            error.starts_with(&metadata.display().to_string()),
            "{error}"
        );
        assert_eq!(cache.read("https://example.com/other.yaml"), Ok(None));
    }

    #[test]
    fn revalidation_updates_only_matching_metadata_and_never_the_body() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SpecCache::new(dir.path().to_path_buf());
        let original = CachedSpec {
            url: "https://example.com/openapi.json".into(),
            etag: Some("v1".into()),
            last_modified: Some("old date".into()),
            fetched_at: DateTime::from_timestamp(1, 0).unwrap(),
            body: b"original".to_vec(),
        };
        cache.store(&original).unwrap();
        let (metadata, body) = cache.paths(&original.url);
        let body_time = std::fs::metadata(&body).unwrap().modified().unwrap();
        let renewed = DateTime::from_timestamp(4, 0).unwrap();
        cache.update_fetched_at(&original, renewed).unwrap();
        assert_eq!(cache.load(&original.url).unwrap().fetched_at, renewed);
        assert_eq!(std::fs::read(&body).unwrap(), original.body);
        assert_eq!(
            std::fs::metadata(&body).unwrap().modified().unwrap(),
            body_time
        );

        for (field, value) in [
            ("url", "https://another.example/spec"),
            ("etag", "v2"),
            ("last_modified", "new date"),
            ("fetched_at", "1970-01-01T00:00:03Z"),
        ] {
            cache.store(&original).unwrap();
            let mut changed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
            changed[field] = serde_json::Value::String(value.into());
            let bytes = serde_json::to_vec(&changed).unwrap();
            std::fs::write(&metadata, &bytes).unwrap();
            cache.update_fetched_at(&original, renewed).unwrap();
            assert_eq!(
                std::fs::read(&metadata).unwrap(),
                bytes,
                "{field} differs from the validated generation"
            );
        }
    }

    #[test]
    fn keys_are_stable_and_distinct() {
        assert_eq!(cache_key("https://a/x").len(), 16);
        assert_eq!(cache_key("https://a/x"), cache_key("https://a/x"));
        assert_ne!(cache_key("https://a/x"), cache_key("https://a/y"));
    }
}
