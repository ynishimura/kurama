//! Pure parsing functions for AWS config files
//!
//! This module contains pure functions for parsing AWS configuration files.
//! All functions are side-effect free and follow functional programming principles:
//!
//! - **Pure Functions**: Same input always produces same output
//! - **Immutability**: No mutation of input data
//! - **Composability**: Small functions that can be combined
//!
//! # Architecture
//!
//! ```text
//! INI Content (String)
//!       │
//!       ▼ parse_ini_content()
//! IniSections (HashMap<String, HashMap<String, String>>)
//!       │
//!       ▼ parse_aws_config_sections()
//! ParsedAwsConfig { profiles, default_region }
//!       │
//!       ▼ apply_default_region()
//! HashMap<String, Profile> (final result)
//! ```

use crate::domain::Profile;
use crate::domain::types::SessionDuration;
use std::collections::HashMap;
use tracing::debug;

// ============================================================================
// Types
// ============================================================================

/// Raw INI section data: section_name -> (key -> value)
pub type IniSections = HashMap<String, HashMap<String, String>>;

/// Result of parsing AWS config sections
#[derive(Debug, Clone)]
pub struct ParsedAwsConfig {
    /// Parsed profiles indexed by name
    pub profiles: HashMap<String, Profile>,
    /// Default region from [default] section (if present)
    pub default_region: Option<String>,
}

// ============================================================================
// Pure Parsing Functions
// ============================================================================

/// Parse INI content into sections
///
/// This is a pure function that converts raw INI text into structured data.
/// Handles AWS config file format with sections like `[default]` and `[profile name]`.
///
/// # Arguments
/// * `content` - Raw INI file content
///
/// # Returns
/// A HashMap where keys are section names and values are property maps
///
/// # Example
/// ```ignore
/// let content = "[default]\nregion = us-east-1";
/// let sections = parse_ini_content(content);
/// assert_eq!(sections["default"]["region"], "us-east-1");
/// ```
pub fn parse_ini_content(content: &str) -> IniSections {
    let mut sections = IniSections::new();
    let mut current_section = String::new();
    let mut current_properties = HashMap::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Section header: [section_name]
        if let Some(section_name) = parse_section_header(line) {
            // Save previous section
            if !current_section.is_empty() {
                sections.insert(current_section, current_properties);
                current_properties = HashMap::new();
            }
            current_section = section_name;
            continue;
        }

        // Key-value pair: key = value
        if let Some((key, value)) = parse_key_value(line) {
            current_properties.insert(key, value);
        }
    }

    // Save last section
    if !current_section.is_empty() {
        sections.insert(current_section, current_properties);
    }

    sections
}

/// Parse a section header line like `[section_name]`
///
/// # Returns
/// `Some(section_name)` if valid section header, `None` otherwise
fn parse_section_header(line: &str) -> Option<String> {
    if line.starts_with('[') && line.ends_with(']') && line.len() > 2 {
        Some(line[1..line.len() - 1].to_string())
    } else {
        None
    }
}

/// Parse a key-value line like `key = value`
///
/// # Returns
/// `Some((key, value))` if valid, `None` otherwise
fn parse_key_value(line: &str) -> Option<(String, String)> {
    line.find('=').map(|pos| {
        let key = line[..pos].trim().to_string();
        let value = line[pos + 1..].trim().to_string();
        (key, value)
    })
}

/// Parse AWS config sections into Profile objects
///
/// Transforms raw INI sections into typed Profile objects.
/// Recognizes AWS config format:
/// - `[default]` section for default settings
/// - `[profile name]` sections for named profiles
///
/// # Arguments
/// * `sections` - Parsed INI sections from `parse_ini_content`
///
/// # Returns
/// `ParsedAwsConfig` containing profiles and default region
pub fn parse_aws_config_sections(sections: &IniSections) -> ParsedAwsConfig {
    let mut profiles = HashMap::new();
    let mut default_region = None;

    for (section, properties) in sections {
        let (profile_name, is_default) = extract_profile_name(section);

        // Skip non-profile sections (e.g., [sso-session name])
        let Some(name) = profile_name else {
            continue;
        };

        // Extract default region from [default] section
        if is_default && default_region.is_none() {
            default_region = properties.get("region").cloned();
        }

        let profile = build_profile(&name, properties);
        profiles.insert(name, profile);
    }

    ParsedAwsConfig {
        profiles,
        default_region,
    }
}

/// Extract profile name from section header
///
/// # Returns
/// `(Some(name), is_default)` tuple where:
/// - `name` is the profile name
/// - `is_default` is true if this is the [default] section
fn extract_profile_name(section: &str) -> (Option<String>, bool) {
    if section == "default" {
        (Some("default".to_string()), true)
    } else if let Some(name) = section.strip_prefix("profile ") {
        (Some(name.to_string()), false)
    } else {
        // Unknown section type (e.g., sso-session)
        (None, false)
    }
}

/// Build a Profile from properties map
///
/// Pure function that converts key-value pairs into a typed Profile.
fn build_profile(name: &str, properties: &HashMap<String, String>) -> Profile {
    let mut profile = Profile::new(name);

    for (key, value) in properties {
        apply_profile_property(&mut profile, key, value);
    }

    profile
}

/// Apply a single property to a profile
///
/// Validates values where appropriate (e.g., duration_seconds).
fn apply_profile_property(profile: &mut Profile, key: &str, value: &str) {
    match key {
        "role_arn" => profile.set_role_arn(value),
        "source_profile" => profile.set_source_profile(value),
        "mfa_serial" => profile.set_mfa_serial(value),
        "region" => profile.set_region(value),
        "role_session_name" => profile.set_role_session_name(value),
        "duration_seconds" => {
            if let Some(duration) = parse_duration_seconds(value) {
                profile.set_duration_seconds(duration);
            }
        }
        _ => {
            // Unknown properties are silently ignored
            // This allows forward compatibility with new AWS config options
        }
    }
}

/// Parse and validate duration_seconds value
///
/// Returns `None` if parsing fails or value is outside AWS limits (900-43200).
fn parse_duration_seconds(value: &str) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .and_then(|secs| SessionDuration::new(secs).ok())
        .map(|d| d.as_secs())
}

/// Apply default region to profiles that don't have one
///
/// Pure function that returns a new HashMap with defaults applied.
/// Profiles with existing regions are unchanged.
///
/// # Arguments
/// * `profiles` - Original profiles
/// * `default_region` - Default region to apply
///
/// # Returns
/// New HashMap with default region applied where needed
pub fn apply_default_region(
    profiles: HashMap<String, Profile>,
    default_region: Option<&str>,
) -> HashMap<String, Profile> {
    let Some(region) = default_region else {
        return profiles;
    };

    profiles
        .into_iter()
        .map(|(name, profile)| {
            let updated = if profile.has_region() {
                profile
            } else {
                debug!("Applying default region '{}' to profile '{}'", region, name);
                profile.with_region_raw(region)
            };
            (name, updated)
        })
        .collect()
}

// ============================================================================
// Convenience Functions
// ============================================================================

/// Parse AWS config content and return profiles with defaults applied
///
/// This is a convenience function that chains the parsing pipeline:
/// 1. Parse INI content
/// 2. Extract profiles and default region
/// 3. Apply default region to profiles
///
/// # Arguments
/// * `content` - Raw AWS config file content
///
/// # Returns
/// HashMap of profile name to Profile
pub fn parse_aws_config(content: &str) -> HashMap<String, Profile> {
    let sections = parse_ini_content(content);
    let parsed = parse_aws_config_sections(&sections);
    apply_default_region(parsed.profiles, parsed.default_region.as_deref())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_ini_content_tests {
        use super::*;

        #[test]
        fn empty_content() {
            let result = parse_ini_content("");
            assert!(result.is_empty());
        }

        #[test]
        fn single_section() {
            let content = "[default]\nregion = us-east-1";
            let result = parse_ini_content(content);

            assert_eq!(result.len(), 1);
            assert_eq!(result["default"]["region"], "us-east-1");
        }

        #[test]
        fn multiple_sections() {
            let content = r#"
[default]
region = us-east-1

[profile admin]
role_arn = arn:aws:iam::123456789012:role/Admin
source_profile = default
"#;
            let result = parse_ini_content(content);

            assert_eq!(result.len(), 2);
            assert!(result.contains_key("default"));
            assert!(result.contains_key("profile admin"));
        }

        #[test]
        fn ignores_comments() {
            let content = r#"
# Comment line
[default]
region = us-east-1
; Another comment
output = json
"#;
            let result = parse_ini_content(content);

            assert_eq!(result.len(), 1);
            assert_eq!(result["default"].len(), 2);
        }

        #[test]
        fn handles_whitespace() {
            let content = "  [default]  \n  region  =  us-west-2  ";
            let result = parse_ini_content(content);

            assert_eq!(result["default"]["region"], "us-west-2");
        }

        #[test]
        fn handles_equals_in_value() {
            let content = "[default]\nkey = value=with=equals";
            let result = parse_ini_content(content);

            assert_eq!(result["default"]["key"], "value=with=equals");
        }
    }

    mod parse_aws_config_sections_tests {
        use super::*;

        #[test]
        fn extracts_default_region() {
            let content = "[default]\nregion = ap-northeast-1";
            let sections = parse_ini_content(content);
            let result = parse_aws_config_sections(&sections);

            assert_eq!(result.default_region, Some("ap-northeast-1".to_string()));
        }

        #[test]
        fn parses_profile_with_role() {
            let content = r#"
[profile admin]
role_arn = arn:aws:iam::123456789012:role/Admin
source_profile = default
mfa_serial = arn:aws:iam::123456789012:mfa/user
duration_seconds = 3600
"#;
            let sections = parse_ini_content(content);
            let result = parse_aws_config_sections(&sections);

            let admin = &result.profiles["admin"];
            assert!(admin.can_assume_role());
            assert!(admin.requires_mfa());
            assert_eq!(admin.source_profile(), Some("default"));
            assert_eq!(admin.duration_seconds(), Some(3600));
        }

        #[test]
        fn skips_unknown_sections() {
            let content = r#"
[default]
region = us-east-1

[sso-session my-sso]
sso_start_url = https://example.awsapps.com/start
"#;
            let sections = parse_ini_content(content);
            let result = parse_aws_config_sections(&sections);

            assert_eq!(result.profiles.len(), 1);
            assert!(result.profiles.contains_key("default"));
        }

        #[test]
        fn ignores_invalid_duration() {
            let content = "[profile test]\nduration_seconds = 100"; // Below minimum
            let sections = parse_ini_content(content);
            let result = parse_aws_config_sections(&sections);

            assert!(result.profiles["test"].duration_seconds().is_none());
        }
    }

    mod apply_default_region_tests {
        use super::*;

        #[test]
        fn applies_to_profiles_without_region() {
            let mut profiles = HashMap::new();
            profiles.insert("test".to_string(), Profile::new("test"));

            let result = apply_default_region(profiles, Some("us-east-1"));

            assert_eq!(result["test"].region_raw(), Some("us-east-1"));
        }

        #[test]
        fn preserves_existing_region() {
            let mut profiles = HashMap::new();
            profiles.insert(
                "test".to_string(),
                Profile::new("test").with_region_raw("eu-west-1"),
            );

            let result = apply_default_region(profiles, Some("us-east-1"));

            assert_eq!(result["test"].region_raw(), Some("eu-west-1"));
        }

        #[test]
        fn handles_none_default() {
            let mut profiles = HashMap::new();
            profiles.insert("test".to_string(), Profile::new("test"));

            let result = apply_default_region(profiles, None);

            assert!(result["test"].region_raw().is_none());
        }
    }

    mod parse_aws_config_tests {
        use super::*;

        #[test]
        fn full_pipeline() {
            let content = r#"
[default]
region = ap-northeast-1

[profile admin]
role_arn = arn:aws:iam::123456789012:role/Admin
source_profile = default
"#;
            let profiles = parse_aws_config(content);

            assert_eq!(profiles.len(), 2);
            assert_eq!(profiles["default"].region_raw(), Some("ap-northeast-1"));
            assert_eq!(profiles["admin"].region_raw(), Some("ap-northeast-1")); // inherited
            assert!(profiles["admin"].can_assume_role());
        }
    }
}
