//! Every key config.toml reads, with the kind of value it takes and the values an enum key accepts, derived from the config types.
//!
//! `known_keys.rs` derives the key names by parsing a sample into [`Config`]
//! and serializing it back. This module walks the same document and then
//! asks the types one more question per key: what happens when the value is
//! one they cannot take. A string where an enum sits answers with the
//! variants it accepts, a string where a number sits answers with the number
//! type, and a `<bogus>://` where a secret reference sits answers that the
//! scheme is unknown. So a field added to a type, or a variant added to an
//! enum, is in the inventory without anyone listing it. An optional table
//! the sample leaves out has no keys to walk, so the inventory stops and asks
//! for it in `SAMPLE` rather than list it without them.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::Config;
use super::known_keys::{ANY_NAME, SAMPLE, Section};

/// Where the entries come from, for the report that prints them.
pub const SOURCE: &str = "src/adapters/config/known_keys.rs: SAMPLE parsed into Config and \
                          serialized back (sections and keys); each key then probed with a \
                          value its type cannot take (kind and enum values)";

/// The value a probe writes where the type expects something else.
const PROBE: &str = "kurama-inventory-probe";

/// One key of one section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigKey {
    /// `[auth.*]`, `[[data.*.sources]]`, `the top level`.
    pub section: String,
    pub key: String,
    /// `string`, `secret`, `enum`, `integer`, `float`, `boolean`, `array`,
    /// `table` or `tables`.
    pub kind: String,
    /// The values an `enum` key accepts; empty for every other kind.
    pub values: Vec<String>,
}

/// Every key of config.toml.
pub fn config_keys() -> Vec<ConfigKey> {
    keys_of::<Config>(SAMPLE)
}

/// Every key `T` reads, from a sample that `T` parses.
fn keys_of<T: Serialize + DeserializeOwned>(sample: &str) -> Vec<ConfigKey> {
    let parsed: T = toml::from_str(sample).expect("the sample parses");
    let value = serde_json::to_value(&parsed).expect("the configuration serializes");
    let mut keys = Vec::new();
    walk::<T>(
        sample,
        &value,
        &Section::default(),
        &mut Vec::new(),
        &mut keys,
    );
    keys
}

/// One step into the document: a key, or the first element of an array.
#[derive(Debug, Clone)]
enum Step {
    Key(String),
    First,
}

fn walk<T: DeserializeOwned>(
    sample: &str,
    value: &Value,
    section: &Section,
    path: &mut Vec<Step>,
    keys: &mut Vec<ConfigKey>,
) {
    let Value::Object(table) = value else {
        return;
    };
    for (key, value) in table {
        path.push(Step::Key(key.clone()));
        if key != ANY_NAME {
            let (kind, values) = kind_of::<T>(sample, value, path);
            assert!(
                !(value.is_null() && kind == "table"),
                "{} {key}: an optional table absent from the sample hides its keys; \
                 add it to known_keys::SAMPLE",
                section.label()
            );
            keys.push(ConfigKey {
                section: section.label(),
                key: key.clone(),
                kind,
                values,
            });
        }
        match value {
            Value::Object(_) => walk::<T>(sample, value, &section.child(key), path, keys),
            Value::Array(tables) if tables.iter().any(Value::is_object) => {
                path.push(Step::First);
                for table in tables {
                    walk::<T>(sample, table, &section.child(key).tables(), path, keys);
                }
                path.pop();
            }
            _ => {}
        }
        path.pop();
    }
}

/// The kind of the value at `path`, and the variants when it is an enum.
fn kind_of<T: DeserializeOwned>(
    sample: &str,
    value: &Value,
    path: &[Step],
) -> (String, Vec<String>) {
    match value {
        Value::Object(_) => ("table".into(), Vec::new()),
        Value::Array(items) if items.iter().any(Value::is_object) => ("tables".into(), Vec::new()),
        Value::Array(_) => ("array".into(), Vec::new()),
        Value::Bool(_) => ("boolean".into(), Vec::new()),
        Value::Number(number) if number.is_f64() => ("float".into(), Vec::new()),
        Value::Number(_) => ("integer".into(), Vec::new()),
        Value::String(_) | Value::Null => probed_kind::<T>(sample, path),
    }
}

/// What the type says about a string it cannot take.
fn probed_kind<T: DeserializeOwned>(sample: &str, path: &[Step]) -> (String, Vec<String>) {
    match probe::<T>(sample, path, PROBE) {
        Ok(()) => {
            // A string it takes: a plain one, or a secret reference whose
            // scheme is checked.
            let kind = match probe::<T>(sample, path, "bogus://x") {
                Err(message) if message.contains("secret reference scheme") => "secret",
                _ => "string",
            };
            (kind.into(), Vec::new())
        }
        Err(message) if message.contains("unknown variant") => {
            ("enum".into(), expected_variants(&message))
        }
        Err(message) => (kind_named_by(&message).into(), Vec::new()),
    }
}

/// The type name serde's `invalid type: string, expected ...` message carries.
fn kind_named_by(message: &str) -> &'static str {
    let expected = message
        .split("expected")
        .nth(1)
        .unwrap_or_default()
        .trim_start();
    if expected.starts_with('u') || expected.starts_with('i') || expected.contains("integer") {
        "integer"
    } else if expected.starts_with('f') {
        "float"
    } else if expected.contains("boolean") {
        "boolean"
    } else if expected.contains("sequence") {
        "array"
    } else if expected.contains("map") || expected.contains("struct") {
        "table"
    } else {
        panic!("serde's message names no kind the inventory knows: {message}")
    }
}

/// The backticked names after `expected` in serde's `unknown variant` message.
fn expected_variants(message: &str) -> Vec<String> {
    let Some((_, rest)) = message.split_once("expected") else {
        return Vec::new();
    };
    rest.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Parse the sample again with `value` written at `path`.
fn probe<T: DeserializeOwned>(sample: &str, path: &[Step], value: &str) -> Result<(), String> {
    let mut document = toml::Value::Table(toml::from_str(sample).expect("the sample parses"));
    set(&mut document, path, toml::Value::String(value.to_owned()));
    let text = toml::to_string(&document).expect("the probed sample serializes");
    toml::from_str::<T>(&text)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn set(node: &mut toml::Value, path: &[Step], value: toml::Value) {
    match path.split_first() {
        None => *node = value,
        Some((Step::Key(key), rest)) => {
            if !node.is_table() {
                *node = toml::Value::Table(toml::Table::new());
            }
            let table = node.as_table_mut().expect("made a table");
            let child = table
                .entry(key.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            set(child, rest, value);
        }
        Some((Step::First, rest)) => {
            if !node.is_array() {
                *node = toml::Value::Array(Vec::new());
            }
            let array = node.as_array_mut().expect("made an array");
            if array.is_empty() {
                array.push(toml::Value::Table(toml::Table::new()));
            }
            set(&mut array[0], rest, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn find<'a>(keys: &'a [ConfigKey], section: &str, key: &str) -> &'a ConfigKey {
        keys.iter()
            .find(|entry| entry.section == section && entry.key == key)
            .unwrap_or_else(|| panic!("{section} {key} is not in the inventory"))
    }

    #[rstest::rstest]
    #[case("the top level", "api", "table", &[])]
    #[case("[core]", "log_level", "string", &[])]
    #[case("[api.*]", "base_url", "string", &[])]
    #[case("[auth.*]", "kind", "enum", &["oauth", "token"])]
    #[case("[auth.*]", "grant_type", "enum", &["authorization_code", "device_code", "client_credentials"])]
    #[case("[auth.*]", "client_secret", "secret", &[])]
    #[case("[auth.*]", "token", "secret", &[])]
    #[case("[auth.*]", "scopes", "array", &[])]
    #[case("[auth.*]", "redirect_port", "integer", &[])]
    #[case("[openapi]", "revalidate_after", "integer", &[])]
    #[case("[db.*]", "engine", "enum", &["sqlite", "postgresql", "mysql"])]
    #[case("[db.*]", "allow_write", "boolean", &[])]
    #[case("[db.*]", "port", "integer", &[])]
    #[case("[db.*]", "password", "secret", &[])]
    #[case("[db.*]", "tls", "enum", &["verify-full", "verify-ca", "disable"])]
    #[case("[db.*.tunnel]", "kind", "enum", &["ssm"])]
    #[case("[data.*]", "sources", "tables", &[])]
    #[case("[[data.*.sources]]", "format", "enum", &["csv", "parquet", "jsonl"])]
    fn every_kind_is_read_off_the_types(
        #[case] section: &str,
        #[case] key: &str,
        #[case] kind: &str,
        #[case] values: &[&str],
    ) {
        let keys = config_keys();
        let entry = find(&keys, section, key);
        assert_eq!(entry.kind, kind, "{entry:?}");
        assert_eq!(entry.values, values, "{entry:?}");
    }

    #[test]
    fn every_key_known_keys_reads_is_listed_with_a_kind() {
        let keys = config_keys();
        assert!(keys.len() > 60, "{}", keys.len());
        for entry in &keys {
            assert_eq!(
                entry.kind == "enum",
                !entry.values.is_empty(),
                "{entry:?}: only an enum lists values"
            );
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Mode {
        Fast,
        Slow,
    }

    #[derive(Serialize, Deserialize)]
    struct Before {
        mode: Mode,
        #[serde(default)]
        retries: u8,
    }

    #[derive(Serialize, Deserialize)]
    struct After {
        mode: Mode,
        #[serde(default)]
        retries: u8,
        #[serde(default)]
        verbose: bool,
    }

    /// The property the module exists for: a field added to a type is a key
    /// in the inventory, with its kind, and nobody listed it.
    #[test]
    fn a_field_added_to_a_type_is_listed() {
        let before = keys_of::<Before>("mode = \"fast\"\n");
        let after = keys_of::<After>("mode = \"fast\"\n");
        let names = |keys: &[ConfigKey]| -> Vec<String> {
            keys.iter().map(|entry| entry.key.clone()).collect()
        };
        assert_eq!(names(&before), ["mode", "retries"]);
        assert_eq!(names(&after), ["mode", "retries", "verbose"]);
        assert_eq!(find(&after, "the top level", "verbose").kind, "boolean");
        assert_eq!(find(&after, "the top level", "retries").kind, "integer");
        let mode = find(&after, "the top level", "mode");
        assert_eq!(mode.kind, "enum");
        assert_eq!(mode.values, ["fast", "slow"]);
    }

    #[derive(Serialize, Deserialize)]
    struct Inner {
        mode: Mode,
    }

    #[derive(Serialize, Deserialize)]
    struct WithOptionalTable {
        mode: Mode,
        #[serde(default)]
        inner: Option<Inner>,
    }

    /// An optional table the sample leaves out serializes as null, so the
    /// walk cannot enter it and its keys and enum values would be missing
    /// without a word.
    #[test]
    #[should_panic(expected = "add it to known_keys::SAMPLE")]
    fn an_optional_table_missing_from_the_sample_stops_the_inventory() {
        keys_of::<WithOptionalTable>("mode = \"fast\"\n");
    }

    #[test]
    fn an_optional_table_in_the_sample_lists_its_keys() {
        let keys = keys_of::<WithOptionalTable>("mode = \"fast\"\n[inner]\nmode = \"slow\"\n");
        let inner = find(&keys, "[inner]", "mode");
        assert_eq!(inner.values, ["fast", "slow"]);
    }

    /// A message none of the kinds matches would otherwise be called a
    /// string, and an enum behind it would lose its values.
    #[test]
    #[should_panic(expected = "names no kind")]
    fn a_serde_message_that_names_no_kind_stops_the_inventory() {
        kind_named_by("invalid value: string \"x\", expected something new");
    }

    #[test]
    fn serde_messages_name_the_kind() {
        assert_eq!(
            kind_named_by("invalid type: string \"x\", expected u16"),
            "integer"
        );
        assert_eq!(
            kind_named_by("invalid type: string \"x\", expected f64"),
            "float"
        );
        assert_eq!(
            kind_named_by("invalid type: string \"x\", expected a boolean"),
            "boolean"
        );
        assert_eq!(
            kind_named_by("invalid type: string \"x\", expected a sequence"),
            "array"
        );
        assert_eq!(
            kind_named_by("invalid type: string \"x\", expected struct Foo"),
            "table"
        );
        assert_eq!(
            expected_variants("unknown variant `x`, expected one of `a`, `b`, `c`"),
            ["a", "b", "c"]
        );
        assert_eq!(
            expected_variants("unknown variant `x`, expected `a` or `b`"),
            ["a", "b"]
        );
    }
}
