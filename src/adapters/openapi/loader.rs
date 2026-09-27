//! Local API description files, the default cache location, and document normalization.
//! These operations have no HTTP client or credential dependencies.

use std::path::PathBuf;

use anyhow::Result;
use thiserror::Error;

use super::SpecCache;
use super::parse_document;
use crate::adapters::config::{ApiDescription, SpecSource};
use crate::adapters::utils::path;
use crate::domain::functions::discovery::normalize_discovery;
use crate::domain::functions::graphql::normalize_graphql;
use crate::domain::functions::openapi::normalize_spec;
use crate::domain::types::api_spec::{ApiSpec, SpecFormat};

#[derive(Debug, Error)]
pub enum FileReadError {
    #[error(transparent)]
    Home(#[from] anyhow::Error),
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Read a local description, expanding `~` before opening the file.
/// Returning the expanded path lets the caller identify the actual source.
pub fn read_file(location: &str) -> Result<(PathBuf, Vec<u8>), FileReadError> {
    let expanded = expand_home(location)?;
    let bytes = std::fs::read(&expanded).map_err(|source| FileReadError::Read {
        path: expanded.clone(),
        source,
    })?;
    Ok((expanded, bytes))
}

/// One conversion for files, cached documents, and downloaded bytes, by the
/// format the configuration names.
pub fn parse_spec(format: &SpecFormat, bytes: &[u8]) -> Result<ApiSpec, String> {
    let document = parse_document(bytes)?;
    match format {
        SpecFormat::OpenApi => normalize_spec(&document),
        SpecFormat::Discovery => normalize_discovery(&document),
        SpecFormat::GraphQl(endpoint) => normalize_graphql(&document, &endpoint.path),
    }
}

/// The bytes of a description without the network: a file as it is, a URL
/// from the cache however old; `None` when neither is there.
pub fn read_offline(spec: &ApiDescription) -> Option<Vec<u8>> {
    match &spec.source {
        SpecSource::File(path) => Some(read_file(path).ok()?.1),
        SpecSource::Url(url) => Some(SpecCache::new(default_cache_dir().ok()?).load(url)?.body),
    }
}

/// [`read_offline`], parsed; one that does not parse is `None` too.
pub fn read_offline_spec(spec: &ApiDescription) -> Option<ApiSpec> {
    parse_spec(&spec.format, &read_offline(spec)?).ok()
}

/// The shared cache location; obtaining it opens no files or clients.
pub fn default_cache_dir() -> Result<PathBuf> {
    Ok(path::get_home_dir()?
        .join(".cache")
        .join("kurama")
        .join("openapi"))
}

/// `~` and `~/...` under the home directory.
fn expand_home(location: &str) -> Result<PathBuf> {
    if location == "~" {
        return Ok(path::get_home_dir()?);
    }
    match location.strip_prefix("~/") {
        Some(rest) => Ok(path::get_home_dir()?.join(rest)),
        None => Ok(PathBuf::from(location)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_documents_are_read_and_normalized_without_a_runtime() {
        let dir = tempfile::tempdir().unwrap();
        for (name, body, title, count) in [
            (
                "spec.json",
                include_bytes!("../../../tests/fixtures/openapi/petstore.json").as_slice(),
                "Petstore",
                5,
            ),
            (
                "spec.yaml",
                include_bytes!("../../../tests/fixtures/openapi/petstore.yaml").as_slice(),
                "Petstore YAML",
                3,
            ),
        ] {
            let file = dir.path().join(name);
            std::fs::write(&file, body).unwrap();
            let (expanded, bytes) = read_file(file.to_str().unwrap()).unwrap();
            assert_eq!(expanded, file);
            let spec = parse_spec(&SpecFormat::OpenApi, &bytes).unwrap();
            assert_eq!(spec.title, title);
            assert_eq!(spec.operations.len(), count);
        }
        let missing = dir.path().join("missing.json");
        let error = read_file(missing.to_str().unwrap()).unwrap_err();
        assert!(matches!(error, FileReadError::Read { path, source }
            if path == missing && source.kind() == std::io::ErrorKind::NotFound));
        assert!(
            parse_spec(&SpecFormat::OpenApi, br#"{"openapi":"3.0.0"}"#)
                .unwrap_err()
                .contains("no `paths`")
        );
        assert!(
            parse_spec(&SpecFormat::OpenApi, b"{broken")
                .unwrap_err()
                .starts_with("JSON:")
        );
    }

    #[test]
    fn the_default_cache_is_under_the_same_home_as_local_files() {
        assert_eq!(
            default_cache_dir().unwrap(),
            expand_home("~/.cache/kurama/openapi").unwrap()
        );
    }
    #[test]
    fn tilde_expands_to_the_home_directory() {
        let home = path::get_home_dir().unwrap();
        assert_eq!(
            expand_home("~/specs/x.yaml").unwrap(),
            home.join("specs/x.yaml")
        );
        assert_eq!(expand_home("~").unwrap(), home);
        assert_eq!(
            expand_home("/abs/x.json").unwrap(),
            PathBuf::from("/abs/x.json")
        );
        assert_eq!(
            expand_home("relative.json").unwrap(),
            PathBuf::from("relative.json")
        );
    }
}
