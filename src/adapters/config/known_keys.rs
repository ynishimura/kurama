//! Which sections of config.toml read a key, derived from the config types.
//!
//! An unknown key is either misplaced or written for a newer kurama. Only the
//! key names this binary knows can tell those apart from a typo, so they are
//! read off the types themselves: a sample with one entry for every named
//! table is parsed into [`Config`], serialized back, and every key of every
//! table is recorded with the section it sits in. A field added to a type is
//! then a key this module knows without anyone listing it.

use super::Config;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The name every named table of the sample has; it is not a key.
pub(super) const ANY_NAME: &str = "*";

/// Each named table, and each optional table inside one, written once with
/// only its required keys: the rest come from the types' defaults.
/// `inventory.rs` reads the same sample to list every key with its kind.
pub(super) const SAMPLE: &str = r#"
[auth."*"]

[api."*"]
base_url = ""

[api."*".agent]

[data."*"]

[[data."*".sources]]
name = ""
path = ""

[db."*"]
engine = "sqlite"
max_affected_rows = 1

[db."*".iam]
aws_profile = ""

[db."*".tunnel]
kind = "ssm"
aws_profile = ""

[s3."*"]
aws_profile = ""

[obsidian]
vault = ""
allow_paths = ["x/"]
"#;

/// The sections that read `key`, as they are written in config.toml
/// (`[auth.*]`, `[[data.*.sources]]`, `the top level`); empty when no
/// section does.
pub fn sections_reading(key: &str) -> Vec<String> {
    let config: Config = toml::from_str(SAMPLE).expect("the sample parses");
    let value = serde_json::to_value(&config).expect("the configuration serializes");
    let mut keys = BTreeMap::new();
    record_keys(&value, &Section::default(), &mut keys);
    keys.remove(key).unwrap_or_default().into_iter().collect()
}

/// Where a key sits: the tables above it, and whether the innermost is an
/// array of tables (`[[data.*.sources]]`).
#[derive(Default)]
pub(super) struct Section {
    path: Vec<String>,
    array_of_tables: bool,
}

impl Section {
    pub(super) fn label(&self) -> String {
        match (self.path.is_empty(), self.array_of_tables) {
            (true, _) => "the top level".to_owned(),
            (false, false) => format!("[{}]", self.path.join(".")),
            (false, true) => format!("[[{}]]", self.path.join(".")),
        }
    }

    pub(super) fn child(&self, key: &str) -> Self {
        let mut path = self.path.clone();
        path.push(key.to_owned());
        Self {
            path,
            array_of_tables: false,
        }
    }

    /// The same tables, entered through an array of tables.
    pub(super) fn tables(&self) -> Self {
        Self {
            path: self.path.clone(),
            array_of_tables: true,
        }
    }
}

fn record_keys(value: &Value, section: &Section, keys: &mut BTreeMap<String, BTreeSet<String>>) {
    match value {
        Value::Object(table) => {
            for (key, value) in table {
                if key != ANY_NAME {
                    keys.entry(key.clone()).or_default().insert(section.label());
                }
                record_keys(value, &section.child(key), keys);
            }
        }
        Value::Array(tables) => {
            let section = section.tables();
            for table in tables {
                record_keys(table, &section, keys);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case("api", "the top level")]
    #[case("log_level", "[core]")]
    #[case("session_cache", "[aws]")]
    #[case("template", "[aws.session_name]")]
    #[case("duration", "[aws.session_cache]")]
    #[case("vault", "[onepassword]")]
    #[case("access_key_id", "[onepassword.field_names]")]
    #[case("revalidate_after", "[openapi]")]
    #[case("grant_type", "[auth.*]")]
    #[case("openapi_auth", "[api.*]")]
    #[case("threads", "[data.*]")]
    #[case("hive_partitioning", "[[data.*.sources]]")]
    #[case("allow_write", "[db.*]")]
    #[case("max_affected_rows", "[db.*]")]
    #[case("region", "[db.*.iam]")]
    #[case("instance_name", "[db.*.tunnel]")]
    #[case("aws_profile", "[s3.*]")]
    #[case("exec_readonly", "[agent]")]
    #[case("deny_paths", "[api.*.agent]")]
    #[case("max_bytes", "[audit]")]
    fn every_section_is_read_off_the_types(#[case] key: &str, #[case] section: &str) {
        assert!(
            sections_reading(key).iter().any(|found| found == section),
            "{key}: {:?}",
            sections_reading(key)
        );
    }

    #[test]
    fn a_key_of_two_sections_names_both() {
        assert_eq!(
            sections_reading("format"),
            ["[[data.*.sources]]", "[auth.*]"]
        );
    }

    #[test]
    fn a_key_of_no_section_names_none() {
        assert!(sections_reading("mfa").is_empty());
        assert!(sections_reading(ANY_NAME).is_empty());
    }
}
