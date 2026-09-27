//! Profile manager - Functional profile loading
//!
//! This module provides profile management functionality following functional programming principles:
//! - Pure functions for profile operations
//! - No mutable state
//! - Separation of I/O from pure logic
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Pure Functions                            │
//! │                                                              │
//! │  find_profile(profiles, name) -> Option<Profile>             │
//! └─────────────────────────────────────────────────────────────┘
//!                           ▲
//!                           │
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    I/O Functions                             │
//! │                                                              │
//! │  load_profiles() -> Result<Vec<Profile>>                     │
//! │  (Uses AwsConfigLoader internally)                           │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use crate::adapters::profile::loader::AwsConfigLoader;
use crate::domain::Profile;
use anyhow::Result;
use tracing::debug;

// =============================================================================
// Pure Functions (No I/O, No State)
// =============================================================================

/// Find a profile by name (pure function)
///
/// # Arguments
/// * `profiles` - Slice of profiles to search
/// * `name` - Profile name to find
///
/// # Returns
/// Cloned profile if found, None otherwise
pub fn find_profile(profiles: &[Profile], name: &str) -> Option<Profile> {
    profiles.iter().find(|p| p.name() == name).cloned()
}

/// Create a default profile (pure function)
pub fn create_default_profile() -> Profile {
    Profile::new("default")
}

// =============================================================================
// I/O Functions (Side Effects)
// =============================================================================

/// Load all profiles from AWS config files
///
/// This is the main I/O function that reads from the file system.
/// It returns an immutable collection of profiles.
///
/// # Returns
/// * `Ok(profiles)` - Vector of loaded profiles
/// * `Err(error)` - Failed to load profiles
pub async fn load_profiles() -> Result<Vec<Profile>> {
    // The loader reads the file AWS_CONFIG_FILE names, wherever it is, and
    // finds nothing when there is no file: ~/.aws is not asked about.
    let loader = AwsConfigLoader::new()?;
    let profiles_map = loader.load_profiles().await?;
    let profiles: Vec<Profile> = profiles_map.into_values().collect();

    if profiles.is_empty() {
        debug!("No profiles found, returning default profile");
        Ok(vec![create_default_profile()])
    } else {
        debug!("Loaded {} profiles", profiles.len());
        Ok(profiles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_profiles() -> Vec<Profile> {
        vec![
            Profile::new("default"),
            Profile::new("dev").with_role_arn_raw("arn:aws:iam::123:role/Dev"),
            Profile::new("prod").with_role_arn_raw("arn:aws:iam::456:role/Prod"),
        ]
    }

    #[test]
    fn test_find_profile() {
        let profiles = test_profiles();

        let found = find_profile(&profiles, "dev");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "dev");

        let not_found = find_profile(&profiles, "nonexistent");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_create_default_profile() {
        let profile = create_default_profile();
        assert_eq!(profile.name(), "default");
    }
}
