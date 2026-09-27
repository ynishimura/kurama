//! The edits `kurama config set`, `unset` and `remove` make to config.toml's document: each changes only what it names, so every other value, comment and the order stay as saved.
//!
//! An edit works on units -- a fixed table or one entry of a named table, as
//! in [`super::saved`]. A single `set` writes one key inside a unit the file
//! has (a fixed table it omits is created; a new named entry is `config add`'s
//! job); `set --file` replaces whole units, without merging what the input
//! leaves out; `unset` removes keys; `remove` removes units with the comments
//! they own (see [`super::layout`]), and refuses an `[auth.*]` that an API the
//! file keeps still uses. A unit or key the file
//! does not have, a key given twice, or a key given with a table that holds
//! it refuses the whole edit ([`InputError`]). Nothing here reads the result
//! as a configuration: [`super::writer::ConfigFile::validate_edit`] checks it
//! whole, once, after every edit of the command.

use std::path::{Path, PathBuf};

use serde::Serialize;
use toml_edit::{Decor, InlineTable, Item, Key, Table, Value};

use super::diff::line_diff;
use super::input::{InputError, check_input};
use super::layout::{
    clear_positions, first_header, first_header_mut, first_position, next_position, remove_unit,
};
use super::saved::{FIXED, NAMED, SECRET_KEYS, Saved, SyntaxError, is_reference};

/// One unit a save changes, as the `--json` save report lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub section: String,
    /// `add`, `reuse` (an `[auth.*]` `preset add` uses as it is), `set`,
    /// `replace`, `unset` or `remove`.
    pub action: &'static str,
    /// The dotted key a `set` or an `unset` changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Change {
    /// A change of a whole unit.
    pub fn of(section: impl Into<String>, action: &'static str) -> Self {
        Self {
            section: section.into(),
            action,
            key: None,
        }
    }
}

/// config.toml's document, as read and as edited so far.
pub struct Edit {
    path: PathBuf,
    before: Saved,
    after: Saved,
    /// What each edit changed, in order.
    pub changes: Vec<Change>,
}

/// A dotted key, split where its unit ends.
struct Target {
    keys: Vec<Key>,
    /// How many of `keys` name the unit: 1 for a fixed table, 2 for a named
    /// entry.
    depth: usize,
}

impl Target {
    fn parse(key: &str) -> Result<Self, InputError> {
        let not_a_unit = || InputError::NotAUnit {
            key: key.to_owned(),
        };
        let keys = Key::parse(key).map_err(|_| not_a_unit())?;
        let depth = match keys.first().map(Key::get) {
            Some(table) if FIXED.contains(&table) => 1,
            Some(table) if NAMED.contains(&table) && keys.len() >= 2 => 2,
            _ => return Err(not_a_unit()),
        };
        Ok(Self { keys, depth })
    }

    fn unit(&self) -> String {
        dotted(&self.keys[..self.depth])
    }

    fn dotted(&self) -> String {
        dotted(&self.keys)
    }

    fn is_unit(&self) -> bool {
        self.keys.len() == self.depth
    }

    /// Whether this is a key that holds a secret (`auth.<name>.token`).
    fn is_secret(&self) -> bool {
        let [table, _, key] = self.keys.as_slice() else {
            return false;
        };
        SECRET_KEYS.contains(&(table.get(), key.get()))
    }
}

/// Keys written back as a dotted key, quoted where TOML needs it.
fn dotted(keys: &[Key]) -> String {
    keys.iter()
        .map(|key| Key::new(key.get()).display_repr().into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

impl Edit {
    pub(super) fn new(path: &Path, content: &str) -> Result<Self, SyntaxError> {
        let before = Saved::parse(content)?;
        Ok(Self {
            path: path.to_owned(),
            after: before.clone(),
            before,
            changes: Vec::new(),
        })
    }

    /// The document as edited.
    pub fn text(&self) -> String {
        self.after.text()
    }

    /// Whether the edits changed any byte.
    pub fn changed(&self) -> bool {
        self.after.text() != self.before.text()
    }

    /// The units the edits touched, each once.
    pub fn units(&self) -> Vec<String> {
        let mut units: Vec<String> = Vec::new();
        for change in &self.changes {
            if !units.contains(&change.section) {
                units.push(change.section.clone());
            }
        }
        units
    }

    /// The lines that differ, with literal secrets redacted on both sides.
    pub fn diff(&self) -> Vec<String> {
        let (mut before, mut after) = (self.before.clone(), self.after.clone());
        before.redact();
        after.redact();
        line_diff(&before.text(), &after.text())
    }

    /// `config set KEY VALUE`: `value` is TOML, and a secret key takes a
    /// reference only. An existing value keeps its comment.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), InputError> {
        let target = Target::parse(key)?;
        if target.is_unit() {
            return Err(InputError::WholeUnit {
                key: target.dotted(),
            });
        }
        let value: Value = value.parse().map_err(|error: toml_edit::TomlError| {
            InputError::Invalid(format!(
                "the value for {} is not a TOML value ({}); write a string in quotes, as '\"text\"'",
                target.dotted(),
                error.message().trim().replace(['\r', '\n'], " ")
            ))
        })?;
        if target.is_secret() && !is_reference(&Item::Value(value.clone())) {
            return Err(InputError::LiteralSecret {
                keys: vec![target.dotted()],
            });
        }
        let unit = target.unit();
        if target.depth == 2 && self.after.get(&unit).is_none() {
            return Err(self.absent(vec![unit]));
        }
        let end = next_position(self.after.doc.as_item());
        set_item(self.after.doc.as_item_mut(), &target.keys, value, end, 0).map_err(|depth| {
            InputError::Invalid(format!(
                "{} holds a value, not a table, so {} cannot be set",
                dotted(&target.keys[..depth]),
                target.dotted()
            ))
        })?;
        self.changes.push(Change {
            section: unit,
            action: "set",
            key: Some(target.dotted()),
        });
        Ok(())
    }

    /// `config set --file`: each unit of `fragment` replaces the file's
    /// whole, in the place and under the comments the file had it; a fixed
    /// table the file omits goes at the end.
    pub fn replace(&mut self, fragment: &str) -> Result<(), InputError> {
        let input = check_input(fragment)?;
        let mut targets = Vec::new();
        for unit in input.units() {
            targets.push((Target::parse(&unit)?, unit));
        }
        let absent: Vec<String> = targets
            .iter()
            .filter(|(target, unit)| target.depth == 2 && self.after.get(unit).is_none())
            .map(|(_, unit)| unit.clone())
            .collect();
        if !absent.is_empty() {
            return Err(self.absent(absent));
        }
        let end = next_position(self.after.doc.as_item());
        for (target, unit) in targets {
            let mut new = input.get(&unit).expect("a unit of the input").clone();
            clear_positions(&mut new);
            let action = match item_mut(&mut self.after, &target.keys) {
                Some(old) => {
                    // The replacement's first header takes the place and the
                    // comments of the file's first one.
                    let decor = first_header(old).map(|header| header.decor().clone());
                    if let Item::Table(table) = &mut new {
                        table.set_position(first_position(old));
                    }
                    if let (Some(header), Some(decor)) = (first_header_mut(&mut new), decor) {
                        *header.decor_mut() = decor;
                    }
                    if matches!(old, Item::Value(_)) {
                        // An inline entry holds values only.
                        new = new.into_value().map(Item::Value).unwrap_or(Item::None);
                    }
                    *old = new;
                    "replace"
                }
                None => {
                    if let Item::Table(table) = &mut new {
                        table.set_position(Some(end));
                        *table.decor_mut() = Decor::default();
                    }
                    self.after.doc.insert(target.keys[0].get(), new);
                    "add"
                }
            };
            self.changes.push(Change::of(unit, action));
        }
        Ok(())
    }

    /// `config unset KEY...`: each key inside a unit is removed, with the
    /// comment above it.
    pub fn unset(&mut self, keys: &[String]) -> Result<(), InputError> {
        let targets = parse_distinct(keys)?;
        if let Some(target) = targets.iter().find(|target| target.is_unit()) {
            return Err(InputError::WholeUnit {
                key: target.dotted(),
            });
        }
        let missing: Vec<String> = targets
            .iter()
            .map(Target::dotted)
            .filter(|key| self.after.get(key).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(InputError::NoSuchKey {
                path: self.path.clone(),
                key: missing.join(", "),
            });
        }
        for target in targets {
            let (last, parents) = target.keys.split_last().expect("a key");
            item_mut(&mut self.after, parents)
                .and_then(Item::as_table_like_mut)
                .expect("the key was found")
                .remove(last.get());
            self.changes.push(Change {
                section: target.unit(),
                action: "unset",
                key: Some(target.dotted()),
            });
        }
        Ok(())
    }

    /// `config remove SECTION...`: each unit is removed whole, with the
    /// comments above it. An `[auth.*]` an API left in the file still uses
    /// refuses the whole edit.
    pub fn remove(&mut self, units: &[String]) -> Result<(), InputError> {
        let targets = parse_distinct(units)?;
        if let Some(target) = targets.iter().find(|target| !target.is_unit()) {
            return Err(InputError::InsideUnit {
                key: target.dotted(),
                unit: target.unit(),
            });
        }
        let absent: Vec<String> = targets
            .iter()
            .map(Target::unit)
            .filter(|unit| self.after.get(unit).is_none())
            .collect();
        if !absent.is_empty() {
            return Err(self.absent(absent));
        }
        for target in &targets {
            remove_unit(&mut self.after.doc, &target.keys);
            self.changes.push(Change::of(target.unit(), "remove"));
        }
        let removed: Vec<&str> = targets
            .iter()
            .filter(|target| target.keys[0].get() == "auth")
            .map(|target| target.keys[1].get())
            .collect();
        let used = auth_users(&self.after, &removed);
        if !used.is_empty() {
            let mut auths: Vec<String> = Vec::new();
            for (auth, _) in &used {
                let unit = dotted(&[Key::new("auth"), Key::new(*auth)]);
                if !auths.contains(&unit) {
                    auths.push(unit);
                }
            }
            return Err(InputError::StillReferenced {
                auths,
                referrers: used.into_iter().map(|(_, api)| api).collect(),
            });
        }
        Ok(())
    }

    fn absent(&self, units: Vec<String>) -> InputError {
        InputError::Absent {
            path: self.path.clone(),
            units,
        }
    }
}

/// `keys` as targets, refusing one given twice and one inside another.
fn parse_distinct(keys: &[String]) -> Result<Vec<Target>, InputError> {
    let targets = keys
        .iter()
        .map(|key| Target::parse(key))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, first) in targets.iter().enumerate() {
        for second in &targets[index + 1..] {
            let (outer, inner) = if first.keys.len() <= second.keys.len() {
                (first, second)
            } else {
                (second, first)
            };
            let holds = outer
                .keys
                .iter()
                .zip(&inner.keys)
                .all(|(a, b)| a.get() == b.get());
            if holds {
                return Err(InputError::Overlap {
                    outer: outer.dotted(),
                    inner: inner.dotted(),
                });
            }
        }
    }
    Ok(targets)
}

/// The item `keys` names, through tables and inline tables.
fn item_mut<'a>(saved: &'a mut Saved, keys: &[Key]) -> Option<&'a mut Item> {
    keys.iter().try_fold(saved.doc.as_item_mut(), |item, key| {
        item.as_table_like_mut()?.get_mut(key.get())
    })
}

/// Put `value` at `keys` under `item`, creating the tables on the way: a
/// new one inside a dotted table is dotted, inside an inline table inline,
/// and a new top-level one goes at position `end`. `Err(n)` when the first
/// `n` keys name a value.
fn set_item(
    item: &mut Item,
    keys: &[Key],
    mut value: Value,
    end: isize,
    depth: usize,
) -> Result<(), usize> {
    let inline = matches!(item, Item::Value(_));
    let dotted = item.as_table().is_some_and(Table::is_dotted);
    let table = item.as_table_like_mut().ok_or(depth)?;
    let (key, rest) = keys.split_first().expect("a key");
    if rest.is_empty() {
        match table.get_mut(key.get()) {
            Some(Item::Value(old)) => {
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            // A table given a value: written again as a plain key.
            Some(_) => {
                table.remove(key.get());
                table.insert(key.get(), Item::Value(value));
            }
            None => {
                table.insert(key.get(), Item::Value(value));
            }
        }
        return Ok(());
    }
    if table.get(key.get()).is_none() {
        let child = if inline {
            Item::Value(Value::InlineTable(InlineTable::new()))
        } else {
            let mut new = Table::new();
            new.set_implicit(true);
            new.set_dotted(dotted);
            if depth == 0 {
                new.set_position(Some(end));
            }
            Item::Table(new)
        };
        table.insert(key.get(), child);
    }
    let child = table.get_mut(key.get()).expect("inserted above");
    set_item(child, rest, value, end, depth + 1)
}

/// The `[api.*]` entries that use one of `auths`, as `(auth, api.<name>)`:
/// by `auth`, or by the default -- no `auth`, no `aws_profile` and the
/// auth's own name.
fn auth_users<'a>(saved: &Saved, auths: &[&'a str]) -> Vec<(&'a str, String)> {
    let Some(apis) = saved.doc.get("api").and_then(Item::as_table_like) else {
        return Vec::new();
    };
    let mut users = Vec::new();
    for (name, api) in apis.iter() {
        let Some(api) = api.as_table_like() else {
            continue;
        };
        let used = match api.get("auth") {
            Some(auth) => auth.as_str(),
            None if api.get("aws_profile").is_none() => Some(name),
            None => None,
        };
        if let Some(auth) = auths.iter().find(|auth| Some(**auth) == used) {
            users.push((*auth, dotted(&[Key::new("api"), Key::new(name)])));
        }
    }
    users
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
