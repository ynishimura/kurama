//! config.toml as it is saved, read for its syntax only: the units it holds, their keys, and their values with literal secrets redacted.
//!
//! A unit is what `kurama config add` adds whole and refuses to add twice: a
//! fixed table (`core`, `aws`, `onepassword`, `openapi`) or one entry of a
//! named table (`auth.github`, `api.github`, `data.lake`, `db.app`,
//! `s3.assets`). Nothing here reads the file into [`super::Config`], so a file
//! that is valid TOML but not a valid configuration can still be shown.

use std::str::FromStr;

use serde_json::{Map, Value as Json};
use toml_edit::{DocumentMut, Item, Key, Table, TableLike, Value};

use super::line_of;
use crate::adapters::error::CoreError;
use crate::domain::types::SecretRef;

/// The top-level tables that are units whole.
pub const FIXED: [&str; 8] = [
    "core",
    "aws",
    "onepassword",
    "openapi",
    "agent",
    "audit",
    "mcp",
    "obsidian",
];

/// The top-level tables whose entries are units of their own.
pub const NAMED: [&str; 5] = ["auth", "api", "data", "db", "s3"];

/// Where an entry of a named table keeps a secret: under one key, or as
/// every value of a table (`[auth.<name>.env]`, one reference per variable).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SecretSlot {
    Key(&'static str),
    EveryValueOf(&'static str),
}

/// The secret slots, under the named table they belong to. A literal value
/// there is the secret itself.
pub(super) const SECRET_SLOTS: [(&str, SecretSlot); 4] = [
    ("auth", SecretSlot::Key("client_secret")),
    ("auth", SecretSlot::Key("token")),
    ("auth", SecretSlot::EveryValueOf("env")),
    ("db", SecretSlot::Key("password")),
];

/// What a literal secret is shown as.
pub const REDACTED: &str = "<redacted>";

/// A parsed config.toml, or a piece of one, with its comments and order.
#[derive(Clone)]
pub struct Saved {
    pub(super) doc: DocumentMut,
}

/// Text that is not TOML: the line and the parser's message, never the text
/// of the line.
#[derive(Debug)]
pub struct SyntaxError {
    line: Option<usize>,
    message: String,
}

impl SyntaxError {
    /// One line naming `name` (`config.toml`, `the input`) and the line.
    pub fn describe(&self, name: &str) -> String {
        match self.line {
            Some(line) => format!("{name} line {line}: {}", self.message),
            None => format!("{name}: {}", self.message),
        }
    }
}

impl From<SyntaxError> for CoreError {
    /// A syntax error of config.toml itself.
    fn from(error: SyntaxError) -> Self {
        CoreError::config(error.describe("config.toml"))
    }
}

impl Saved {
    pub fn parse(content: &str) -> Result<Self, SyntaxError> {
        DocumentMut::from_str(content)
            .map(|doc| Self { doc })
            .map_err(|error| SyntaxError {
                line: line_of(content, error.span()),
                message: error.message().replace(['\r', '\n'], " "),
            })
    }

    /// Every unit, grouped by top-level table in the order the file first
    /// names each.
    pub fn units(&self) -> Vec<String> {
        let mut units = Vec::new();
        for (key, item) in self.doc.iter() {
            match item.as_table_like() {
                Some(entries) if NAMED.contains(&key) => units.extend(
                    entries
                        .iter()
                        .map(|(name, _)| format!("{key}.{}", Key::new(name).display_repr())),
                ),
                _ => units.push(Key::new(key).display_repr().into_owned()),
            }
        }
        units
    }

    /// The leaf keys under `unit`, dotted and relative to it.
    pub fn keys(&self, unit: &str) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(table) = self.get(unit).and_then(Item::as_table_like) {
            leaves("", table, &mut keys);
        }
        keys
    }

    /// The item a dotted key names (`auth.github`, `aws.session_cache.duration`).
    pub fn get(&self, path: &str) -> Option<&Item> {
        let keys = Key::parse(path).ok()?;
        keys.iter()
            .try_fold(self.doc.as_item(), |item, key| item.get(key.get()))
    }

    /// Whether the document holds a key-value pair of its own, outside every
    /// table: appended after a file, it would land in the file's last table.
    pub fn has_top_level_values(&self) -> bool {
        self.doc.iter().any(|(_, item)| match item {
            Item::Table(table) => table.is_dotted(),
            Item::ArrayOfTables(_) => false,
            _ => true,
        })
    }

    /// Every secret that is anything but a secret reference -- the secret
    /// itself, a number, a table -- as `auth.<name>.client_secret` or
    /// `auth.<name>.env.<VAR>`.
    pub fn literal_secrets(&self) -> Vec<String> {
        let mut found = Vec::new();
        self.clone().visit_secrets(|path, item| {
            if !is_reference(item) {
                found.push(path);
            }
        });
        found
    }

    /// Replace every secret that is no reference with [`REDACTED`], keeping
    /// a value's comments.
    pub fn redact(&mut self) {
        self.visit_secrets(|_, item| {
            if is_reference(item) {
                return;
            }
            let mut redacted = Value::from(REDACTED);
            if let Some(value) = item.as_value() {
                *redacted.decor_mut() = value.decor().clone();
            }
            *item = Item::Value(redacted);
        });
    }

    /// Call `visit` with the dotted path and the item of every secret the
    /// [`SECRET_SLOTS`] name. A slot table that is not a table is visited
    /// whole, so a literal written in its place is found too.
    fn visit_secrets(&mut self, mut visit: impl FnMut(String, &mut Item)) {
        for (kind, slot) in SECRET_SLOTS {
            let Some(entries) = self.doc.get_mut(kind).and_then(Item::as_table_like_mut) else {
                continue;
            };
            for (name, entry) in entries.iter_mut() {
                let (SecretSlot::Key(key) | SecretSlot::EveryValueOf(key)) = slot;
                let Some(item) = entry
                    .as_table_like_mut()
                    .and_then(|entry| entry.get_mut(key))
                else {
                    continue;
                };
                let path = format!("{kind}.{}.{key}", Key::new(name.get()).display_repr());
                match (slot, item.as_table_like_mut()) {
                    (SecretSlot::EveryValueOf(_), Some(table)) => {
                        for (var, value) in table.iter_mut() {
                            visit(
                                format!("{path}.{}", Key::new(var.get()).display_repr()),
                                value,
                            );
                        }
                    }
                    _ => visit(path, item),
                }
            }
        }
    }

    /// The whole document as TOML, as it was written.
    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// The item at `path` as TOML: a value as itself (`"https://x"`), a table
    /// under its own header.
    pub fn text_of(&self, path: &str) -> Option<String> {
        let item = self.get(path)?;
        if let Item::Value(value) = item {
            return Some(format!("{}\n", value.clone().decorated("", "")));
        }
        let keys = Key::parse(path).ok()?;
        let (last, parents) = keys.split_last()?;
        let mut out = DocumentMut::new();
        let mut table = out.as_table_mut();
        for key in parents {
            table = table
                .entry(key.get())
                .or_insert_with(|| {
                    let mut implicit = Table::new();
                    implicit.set_implicit(true);
                    Item::Table(implicit)
                })
                .as_table_mut()?;
        }
        table.insert(last.get(), item.clone());
        Some(out.to_string().trim_start().to_owned())
    }

    /// The whole document as JSON.
    pub fn json(&self) -> Json {
        table_json(self.doc.as_table())
    }

    /// The item at `path` as JSON.
    pub fn json_of(&self, path: &str) -> Option<Json> {
        self.get(path).map(item_json)
    }
}

/// A string that parses as a reference to a secret store.
pub(super) fn is_reference(item: &Item) -> bool {
    item.as_str()
        .is_some_and(|text| SecretRef::parse(text).is_ok_and(|secret| secret.is_reference()))
}

fn leaves(prefix: &str, table: &dyn TableLike, out: &mut Vec<String>) {
    for (key, item) in table.iter() {
        let key = Key::new(key).display_repr().into_owned();
        let path = if prefix.is_empty() {
            key
        } else {
            format!("{prefix}.{key}")
        };
        match item.as_table_like() {
            Some(inner) => leaves(&path, inner, out),
            None => out.push(path),
        }
    }
}

fn table_json(table: &dyn TableLike) -> Json {
    Json::Object(
        table
            .iter()
            .map(|(key, item)| (key.to_owned(), item_json(item)))
            .collect::<Map<_, _>>(),
    )
}

fn item_json(item: &Item) -> Json {
    match item {
        Item::None => Json::Null,
        Item::Value(value) => value_json(value),
        Item::Table(table) => table_json(table),
        Item::ArrayOfTables(tables) => Json::Array(
            tables
                .iter()
                .map(|table| table_json(table as &dyn TableLike))
                .collect(),
        ),
    }
}

fn value_json(value: &Value) -> Json {
    match value {
        Value::String(text) => Json::from(text.value().as_str()),
        Value::Integer(number) => Json::from(*number.value()),
        Value::Float(number) => Json::from(*number.value()),
        Value::Boolean(flag) => Json::from(*flag.value()),
        Value::Datetime(datetime) => Json::from(datetime.value().to_string()),
        Value::Array(values) => Json::Array(values.iter().map(value_json).collect()),
        Value::InlineTable(table) => table_json(table),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# my settings\n[core]\nlog_level = \"debug\"\n\n\
        [aws.session_cache]\nduration = 3600 # an hour\n\n\
        [auth.svc]\nkind = \"oauth\"\ngrant_type = \"client_credentials\"\n\
        token_url = \"https://as/token\"\nclient_id = \"id\"\n\
        client_secret = \"hunter2-literal\" # keep\n\n\
        [auth.gh]\nkind = \"token\"\ntoken = \"op://Agent/gh/credential\"\n\n\
        [api.gh]\nbase_url = \"https://api.github.com\"\nheaders = { Accept = \"application/json\" }\n\n\
        [db.app]\nengine = \"postgres\"\npassword = \"plain\"\n";

    fn saved() -> Saved {
        Saved::parse(FILE).unwrap()
    }

    #[test]
    fn the_units_are_the_fixed_tables_and_each_named_entry() {
        assert_eq!(
            saved().units(),
            ["core", "aws", "auth.svc", "auth.gh", "api.gh", "db.app"]
        );
        assert_eq!(saved().keys("aws"), ["session_cache.duration"]);
        assert_eq!(saved().keys("api.gh"), ["base_url", "headers.Accept"]);
        assert!(saved().keys("api.none").is_empty());
    }

    #[test]
    fn literal_secrets_are_found_and_redacted_in_place() {
        let mut saved = saved();
        assert_eq!(
            saved.literal_secrets(),
            ["auth.svc.client_secret", "db.app.password"]
        );
        saved.redact();
        let text = saved.text();
        assert!(!text.contains("hunter2-literal"), "{text}");
        assert!(!text.contains("\"plain\""), "{text}");
        assert!(
            text.contains("client_secret = \"<redacted>\" # keep\n"),
            "{text}"
        );
        assert!(text.contains("token = \"op://Agent/gh/credential\""));
        assert!(text.starts_with("# my settings\n[core]\n"), "{text}");
        assert_eq!(
            saved.json_of("auth.svc.client_secret"),
            Some(Json::from(REDACTED))
        );
    }

    /// Every value of `[auth.<name>.env]` is a secret of its own: a literal
    /// is found and redacted by its variable, a reference is kept, and a
    /// literal written in place of the table is found whole.
    #[test]
    fn every_value_of_a_secrets_table_is_a_secret() {
        let mut saved = Saved::parse(
            "[auth.site]\nkind = \"secrets\"\n[auth.site.env]\nSITE_USER = \"op://Agent/site/username\"\n\
             SITE_PASS = \"hunter2-literal\" # keep\n[auth.flat]\nkind = \"secrets\"\nenv = \"plain-literal\"\n",
        )
        .unwrap();
        assert_eq!(
            saved.literal_secrets(),
            ["auth.site.env.SITE_PASS", "auth.flat.env"]
        );
        saved.redact();
        let text = saved.text();
        assert!(
            !text.contains("hunter2-literal") && !text.contains("plain-literal"),
            "{text}"
        );
        assert!(text.contains("SITE_PASS = \"<redacted>\" # keep"), "{text}");
        assert!(
            text.contains("SITE_USER = \"op://Agent/site/username\""),
            "{text}"
        );
    }

    /// A number or a table where a reference belongs is not a reference
    /// either: it is found and redacted like a string, so no message or
    /// output repeats it.
    #[test]
    fn a_secret_key_holding_no_reference_is_a_literal_whatever_its_type() {
        let mut saved = Saved::parse(
            "[auth.n]\ntoken = 987654321\n[auth.t]\ntoken = { a = \"b\" }\n\
             [auth.u]\ntoken = \"aws-secret://dev/x\"\n[db.p]\npassword = 12345678 # pin\n",
        )
        .unwrap();
        assert_eq!(
            saved.literal_secrets(),
            [
                "auth.n.token",
                "auth.t.token",
                "auth.u.token",
                "db.p.password"
            ]
        );
        saved.redact();
        let text = saved.text();
        assert!(
            !text.contains("987654321") && !text.contains("12345678"),
            "{text}"
        );
        assert!(text.contains("password = \"<redacted>\" # pin"), "{text}");
    }

    #[test]
    fn an_unredacted_file_prints_back_byte_for_byte() {
        assert_eq!(saved().text(), FILE);
    }

    #[test]
    fn a_path_prints_as_a_value_or_as_a_table_under_its_header() {
        let saved = saved();
        assert_eq!(
            saved.text_of("aws.session_cache.duration").unwrap(),
            "3600\n"
        );
        assert_eq!(
            saved.text_of("auth.gh").unwrap(),
            "[auth.gh]\nkind = \"token\"\ntoken = \"op://Agent/gh/credential\"\n"
        );
        assert_eq!(saved.text_of("auth.none"), None);
        assert_eq!(
            saved.json_of("api.gh").unwrap(),
            serde_json::json!({"base_url": "https://api.github.com", "headers": {"Accept": "application/json"}})
        );
    }

    #[test]
    fn a_syntax_error_names_the_line_and_not_its_text() {
        let error = Saved::parse("[auth.x]\ntoken = \"secret-in-a-broken-line\n")
            .err()
            .unwrap()
            .describe("the input");
        assert!(error.contains("the input line 2:"), "{error}");
        assert!(!error.contains("secret-in-a-broken-line"), "{error}");
    }

    #[test]
    fn a_value_outside_every_table_is_found() {
        let top = |text| Saved::parse(text).unwrap().has_top_level_values();
        assert!(top("auth.x.kind = \"token\"\n"));
        assert!(top("log = 1\n[core]\n"));
        assert!(!top(
            "[auth.x]\nkind = \"token\"\n[[data.w.sources]]\nname = \"a\"\n"
        ));
    }
}
