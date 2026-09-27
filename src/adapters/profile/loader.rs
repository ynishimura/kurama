//! AWS config file loader
//!
//! This module handles loading of ~/.aws/config file.
//!
//! # Architecture
//!
//! This module separates I/O from pure logic:
//! - **I/O**: Reading files from disk (this module)
//! - **Pure**: Parsing and transformation (see `parser` module)
//!
//! ```text
//! AwsConfigLoader (I/O)
//!       │
//!       ▼ read_config_file()
//! String (file content)
//!       │
//!       ▼ parser::parse_aws_config() (Pure)
//! HashMap<String, Profile>
//! ```

use super::parser;
use crate::adapters::utils::path;
use crate::domain::Profile;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

/// AWS configuration loader for ~/.aws/config
///
/// This struct handles file I/O operations only.
/// Parsing logic is delegated to pure functions in the `parser` module.
///
/// Note: ~/.aws/credentials is NOT used - credentials are retrieved from 1Password
pub struct AwsConfigLoader {
    aws_dir: PathBuf,
}

impl AwsConfigLoader {
    /// Create a new AWS config loader
    pub fn new() -> Result<Self> {
        let aws_dir = path::get_aws_config_dir()?;
        Ok(Self { aws_dir })
    }

    /// Create a loader with a custom AWS directory (for testing)
    #[cfg(test)]
    pub fn with_dir(aws_dir: PathBuf) -> Self {
        Self { aws_dir }
    }

    /// Load AWS profiles from ~/.aws/config only
    ///
    /// This is the main entry point that:
    /// 1. Reads the config file (I/O)
    /// 2. Parses content into profiles (pure function)
    /// 3. Applies default region (pure function)
    pub async fn load_profiles(&self) -> Result<HashMap<String, Profile>> {
        debug!("Loading AWS profiles from ~/.aws/config");

        // I/O: Read file content
        let content = match self.read_config_file().await? {
            Some(content) => content,
            None => {
                debug!("No config file found, returning empty profiles");
                return Ok(HashMap::new());
            }
        };

        // Pure: Parse and transform using parser module
        let profiles = parser::parse_aws_config(&content);

        // Log results
        debug!("Loaded {} profiles", profiles.len());
        for (name, profile) in &profiles {
            debug!(
                "  Profile '{}': region={:?}, can_assume_role={}",
                name,
                profile.region_raw(),
                profile.can_assume_role()
            );
        }

        Ok(profiles)
    }

    // =========================================================================
    // Private I/O Methods
    // =========================================================================

    /// Read the config file content
    ///
    /// Returns `Ok(None)` if file doesn't exist.
    async fn read_config_file(&self) -> Result<Option<String>> {
        let config_path = self.resolve_config_path();

        if !config_path.exists() {
            debug!("AWS config file not found: {:?}", config_path);
            return Ok(None);
        }

        debug!("Reading AWS config file: {:?}", config_path);
        let content = tokio::fs::read_to_string(&config_path).await?;
        debug!("Read {} bytes from config file", content.len());

        Ok(Some(content))
    }

    /// Resolve the config file path
    ///
    /// Priority: AWS_CONFIG_FILE env var > default path
    pub(crate) fn resolve_config_path(&self) -> PathBuf {
        self.config_source().0
    }

    /// The config file and whether `AWS_CONFIG_FILE` named it.
    pub(crate) fn config_source(&self) -> (PathBuf, bool) {
        if let Ok(custom_path) = std::env::var("AWS_CONFIG_FILE") {
            debug!("Using custom AWS config path: {}", custom_path);
            (PathBuf::from(custom_path), true)
        } else {
            (self.aws_dir.join("config"), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;

    // Note: Parser unit tests are in parser.rs
    // This module only tests I/O integration

    #[test]
    #[serial_test::serial]
    fn test_resolve_config_path_default() {
        let loader = AwsConfigLoader::with_dir(PathBuf::from("/home/user/.aws"));
        // Without AWS_CONFIG_FILE env var, uses default
        test_env::remove("AWS_CONFIG_FILE");
        let path = loader.resolve_config_path();
        assert_eq!(path, PathBuf::from("/home/user/.aws/config"));
        assert!(!loader.config_source().1);
    }

    #[test]
    #[serial_test::serial]
    fn aws_config_file_names_the_file_and_says_so() {
        let loader = AwsConfigLoader::with_dir(PathBuf::from("/home/user/.aws"));
        let previous = std::env::var_os("AWS_CONFIG_FILE");
        test_env::set_or_remove("AWS_CONFIG_FILE", Some("/elsewhere/aws.ini"));
        let source = loader.config_source();
        test_env::set_or_remove("AWS_CONFIG_FILE", previous.as_ref());
        assert_eq!(source, (PathBuf::from("/elsewhere/aws.ini"), true));
    }
}
