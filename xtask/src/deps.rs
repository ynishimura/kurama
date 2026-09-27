//! `cargo xtask deps`: the features each feature's code imports, against the
//! `depends_on` it declares.
//!
//! `impact` follows `depends_on` to select every scenario a change to shared
//! code goes through, and it was once wrong with every gate green:
//! `local-analytics` read `limits.rs` of `api-explorer` undeclared. This
//! resolves every `crate::` / `super::` / `self::` path of `src/` production
//! code -- `use` trees expanded, `pub use` re-exports followed -- to the file
//! it names, and the file to the features that claim it. An import from a
//! feature into another feature's file has to be in `depends_on`, except from
//! the registration hubs. The test at the bottom holds the repository to it;
//! the command says what to add.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::{FeatureMap, feature_owns_file, load_features, root, rust_files};

const USAGE: &str = "usage: cargo xtask deps [--json]";

/// The features every other feature registers with: the dispatch table, the
/// clap definition and the error classification name every command and every
/// error type, while their own scenarios run none of that code. An import
/// from one of their files is a registration, and the edge `impact` follows
/// is the one each feature declares to them.
pub(crate) const REGISTRATION_HUBS: [&str; 2] = ["cli-entry", "errors"];

/// One import that lands in a feature the importing one does not declare.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct Undeclared {
    /// The features that claim the importing file.
    pub(crate) from: Vec<String>,
    /// The features that claim the imported file.
    pub(crate) to: Vec<String>,
    /// The first place it was seen: `<file> names <path> (<target file>)`.
    pub(crate) evidence: String,
}

#[derive(Debug, serde::Serialize)]
struct Report {
    undeclared: Vec<Undeclared>,
    /// Declared edges no import of the feature's files crosses. Not wrong: a
    /// feature can run another's code through the dispatch table, which is a
    /// hub. Listed so a stale declaration can be noticed.
    declared_without_import: BTreeMap<String, Vec<String>>,
}

pub fn deps(args: &[String]) -> Result<(), String> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => return Err(USAGE.to_string()),
    };
    let features = load_features()?;
    let (undeclared, crossed) = crossings(&features);
    let declared_without_import = features
        .iter()
        .map(|(name, feature)| {
            let unused: Vec<String> = feature
                .depends_on
                .iter()
                .filter(|dependency| !crossed.contains(&(name.clone(), (*dependency).clone())))
                .cloned()
                .collect();
            (name.clone(), unused)
        })
        .filter(|(_, unused)| !unused.is_empty())
        .collect();
    let report = Report {
        undeclared,
        declared_without_import,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        print!("{}", render(&report));
    }
    if report.undeclared.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} import(s) cross into a feature `depends_on` does not name",
            report.undeclared.len()
        ))
    }
}

fn render(report: &Report) -> String {
    let mut out = format!("undeclared ({})\n", report.undeclared.len());
    for entry in &report.undeclared {
        out.push_str(&format!(
            "  [{}] -> [{}]  {}\n      add \"{}\" to depends_on in .agent/features/{}.toml\n",
            entry.from.join(", "),
            entry.to.join(", "),
            entry.evidence,
            entry.to[0],
            entry.from[0]
        ));
    }
    out.push_str(&format!(
        "\ndeclared without an import ({}; not wrong when the code runs through a hub)\n",
        report.declared_without_import.len()
    ));
    for (name, unused) in &report.declared_without_import {
        out.push_str(&format!("  [{name}] {}\n", unused.join(", ")));
    }
    out
}

/// The imports that cross into an undeclared feature, one per pair of
/// feature sets, and every `(feature, feature)` pair some import crosses.
pub(crate) fn crossings(features: &FeatureMap) -> (Vec<Undeclared>, BTreeSet<(String, String)>) {
    let owners = |file: &str| -> Vec<&String> {
        features
            .iter()
            .filter(|(_, feature)| feature_owns_file(feature, file))
            .map(|(name, _)| name)
            .collect()
    };
    let mut undeclared: BTreeMap<(Vec<String>, Vec<String>), String> = BTreeMap::new();
    let mut crossed: BTreeSet<(String, String)> = BTreeSet::new();
    for path in rust_files(&root().join("src")) {
        let Some(code) = production_code(&path) else {
            continue;
        };
        let file = path.strip_prefix(root()).unwrap().display().to_string();
        let mine = owners(&file);
        if mine.is_empty() {
            continue;
        }
        let module = module_of(&file);
        for named in named_paths(&code) {
            let Some(target) = module_file(&named, &module, 0) else {
                continue;
            };
            if target == file {
                continue;
            }
            let theirs = owners(&target);
            for a in &mine {
                for b in &theirs {
                    if a != b {
                        crossed.insert(((*a).clone(), (*b).clone()));
                    }
                }
            }
            let hub = mine
                .iter()
                .all(|name| REGISTRATION_HUBS.contains(&name.as_str()));
            let declared = hub
                || theirs.is_empty()
                || mine.iter().any(|a| theirs.contains(a))
                || mine
                    .iter()
                    .any(|a| theirs.iter().any(|b| features[*a].depends_on.contains(b)));
            if declared {
                continue;
            }
            let names = |list: &[&String]| list.iter().map(|name| name.to_string()).collect();
            undeclared
                .entry((names(&mine), names(&theirs)))
                .or_insert_with(|| format!("{file} names `{named}` ({target})"));
        }
    }
    (
        undeclared
            .into_iter()
            .map(|((from, to), evidence)| Undeclared { from, to, evidence })
            .collect(),
        crossed,
    )
}

/// Every `src/` file, with the `src/` files that import it: the reverse of
/// the import graph, tests included, because a test in the importing file is
/// one that can notice the change. `impact` walks it from a changed file to
/// every file whose code can run the change.
pub(crate) fn importers() -> BTreeMap<String, BTreeSet<String>> {
    let mut importers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in rust_files(&root().join("src")) {
        let Ok(code) = std::fs::read_to_string(&path) else {
            continue;
        };
        let file = path.strip_prefix(root()).unwrap().display().to_string();
        // A `<name>_tests.rs` is the `mod tests` of the file that includes it.
        let module = match crate::including_module(&file) {
            Some(parent) => {
                let mut module = module_of(&parent);
                module.push("tests".to_string());
                module
            }
            None => module_of(&file),
        };
        for named in named_paths(&code) {
            if let Some(target) = module_file(&named, &module, 0)
                && target != file
            {
                importers.entry(target).or_default().insert(file.clone());
            }
        }
    }
    importers
}

/// Source text before the test module; test-only files are skipped.
fn production_code(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if name == "tests.rs" || name.ends_with("_tests.rs") {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let cut = test_module_start(&content).unwrap_or(content.len());
    Some(content[..cut].to_string())
}

/// Where `#[cfg(test)] mod ...` begins; a `#[cfg(test)]` on a method in the
/// middle of a file is not the end of the production code.
fn test_module_start(content: &str) -> Option<usize> {
    let mut offset = 0;
    while let Some(found) = content[offset..].find("#[cfg(test)]") {
        let start = offset + found;
        if content[start..]
            .lines()
            .skip(1)
            .take(3)
            .any(|line| line.trim_start().starts_with("mod "))
        {
            return Some(start);
        }
        offset = start + "#[cfg(test)]".len();
    }
    None
}

/// The module path of a file: `src/a/b.rs` is `a::b`, `src/a/mod.rs` is `a`,
/// and the crate root is empty.
fn module_of(file: &str) -> Vec<String> {
    let stem = file
        .strip_prefix("src/")
        .and_then(|rest| rest.strip_suffix(".rs"))
        .unwrap_or_default();
    let stem = stem.strip_suffix("/mod").unwrap_or(stem);
    if stem == "lib" || stem == "main" {
        return Vec::new();
    }
    stem.split('/').map(str::to_string).collect()
}

/// The paths a source names: every `use` tree expanded (`a::{b::c, d}` is
/// `a::b::c` and `a::d`), plus every `crate::` / `super::` / `self::` path
/// written inline. Comments and string literals are dropped first, so a
/// path in prose is not an import.
fn named_paths(code: &str) -> Vec<String> {
    let code = without_comments_and_strings(code);
    let mut paths = Vec::new();
    let mut rest = String::new();
    let mut offset = 0;
    while let Some(found) = code[offset..].find("use ") {
        let start = offset + found;
        let at_word_start = start == 0
            || !code[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let Some(end) = code[start..].find(';') else {
            break;
        };
        if at_word_start {
            rest.push_str(&code[offset..start]);
            let tree: String = code[start + "use ".len()..start + end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            paths.extend(expand_use_tree(&tree));
        } else {
            rest.push_str(&code[offset..start + end]);
        }
        offset = start + end;
    }
    rest.push_str(&code[offset..]);
    for prefix in ["crate::", "super::", "self::", "kurama::"] {
        let mut from = 0;
        while let Some(found) = rest[from..].find(prefix) {
            let start = from + found;
            let preceded_by_word = rest[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':');
            let end = rest[start..]
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                .map_or(rest.len(), |length| start + length);
            if !preceded_by_word {
                paths.push(rest[start..end].trim_end_matches(':').to_string());
            }
            from = end;
        }
    }
    paths
}

/// `a::{b::c, d as e}` -> `a::b::c`, `a::d`. A `*` names the module itself.
fn expand_use_tree(tree: &str) -> Vec<String> {
    let tree = tree.trim();
    if let Some(inner) = tree.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        let mut parts = Vec::new();
        let mut depth = 0;
        let mut current = String::new();
        for character in inner.chars() {
            match character {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(std::mem::take(&mut current));
                    continue;
                }
                _ => {}
            }
            current.push(character);
        }
        parts.push(current);
        return parts
            .iter()
            .filter(|part| !part.trim().is_empty())
            .flat_map(|part| expand_use_tree(part))
            .collect();
    }
    if let Some((head, rest)) = tree.split_once('{') {
        let head = head.trim();
        return expand_use_tree(&format!("{{{rest}"))
            .into_iter()
            .map(|path| format!("{head}{path}"))
            .collect();
    }
    let path = tree.split(" as ").next().unwrap_or(tree).trim();
    let path = path.strip_suffix("::*").unwrap_or(path);
    let path = path
        .strip_prefix("pub(crate) ")
        .or_else(|| path.strip_prefix("pub "))
        .unwrap_or(path);
    vec![path.to_string()]
}

/// The source with its comments and string literals blanked, so that a path
/// in a doc comment or a message is not read as an import.
fn without_comments_and_strings(code: &str) -> String {
    let characters: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(code.len());
    let mut index = 0;
    while index < characters.len() {
        let (c, next) = (characters[index], characters.get(index + 1).copied());
        if c == '/' && next == Some('/') {
            while index < characters.len() && characters[index] != '\n' {
                index += 1;
            }
        } else if c == '/' && next == Some('*') {
            index += 2;
            while index + 1 < characters.len()
                && !(characters[index] == '*' && characters[index + 1] == '/')
            {
                index += 1;
            }
            index += 2;
        } else if c == '"' {
            index += 1;
            while index < characters.len() && characters[index] != '"' {
                if characters[index] == '\\' {
                    index += 1;
                }
                index += 1;
            }
            index += 1;
            out.push_str("\"\"");
        } else if c == 'r' && (next == Some('"') || next == Some('#')) && {
            let hashes = characters[index + 1..]
                .iter()
                .take_while(|c| **c == '#')
                .count();
            characters.get(index + 1 + hashes) == Some(&'"')
        } {
            let hashes = characters[index + 1..]
                .iter()
                .take_while(|c| **c == '#')
                .count();
            let closing: String = std::iter::once('"')
                .chain("#".repeat(hashes).chars())
                .collect();
            let start = index + 2 + hashes;
            let remainder: String = characters[start..].iter().collect();
            let length = remainder
                .find(&closing)
                .map_or(remainder.len(), |at| at + closing.len());
            index = start + length;
            out.push_str("\"\"");
        } else if c == '\''
            && (next == Some('\\') && characters.get(index + 3) == Some(&'\'')
                || next.is_some() && characters.get(index + 2) == Some(&'\''))
        {
            // A char literal; a lifetime is a quote followed by a word.
            index += if next == Some('\\') { 4 } else { 3 };
            out.push_str("' '");
        } else {
            out.push(c);
            index += 1;
        }
    }
    out
}

/// The file a module path names, from the module the path is written in.
/// `crate::a::b::Item` is the longest prefix that is a file, `src/a/b.rs` or
/// `src/a/b/mod.rs`; a segment left over is looked up among that file's
/// `pub use` re-exports, so `crate::domain::types::Credentials` lands in
/// `credentials.rs` and not in the `mod.rs` that re-exports it. A path into
/// another crate is `None`.
fn module_file(path: &str, module: &[String], depth: usize) -> Option<String> {
    let mut segments: Vec<String> = path
        .split("::")
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();
    let first = segments.first()?.clone();
    let mut base: Vec<String> = match first.as_str() {
        "crate" | "kurama" => Vec::new(),
        "self" => module.to_vec(),
        "super" => {
            let mut base = module.to_vec();
            while segments.first().is_some_and(|s| s == "super") {
                base.pop();
                segments.remove(0);
            }
            base
        }
        _ => {
            // A bare path is a child module of this one, or another crate.
            let child = module_dir(module).join(&first);
            if child.with_extension("rs").is_file() || child.join("mod.rs").is_file() {
                module.to_vec()
            } else {
                return None;
            }
        }
    };
    if matches!(first.as_str(), "crate" | "kurama" | "self") {
        segments.remove(0);
    }
    base.extend(segments);
    // A module is snake_case; an item (`Credentials`, `INPUT_LIST`) is not,
    // and the file system here is case-insensitive.
    let modules = base
        .iter()
        .take_while(|segment| {
            segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
        .count();
    for length in (0..=modules).rev() {
        let candidate = if length == 0 {
            String::from("src/lib.rs")
        } else {
            format!("src/{}.rs", base[..length].join("/"))
        };
        let directory = if length == 0 {
            String::new()
        } else {
            format!("src/{}/mod.rs", base[..length].join("/"))
        };
        let file = [candidate, directory]
            .into_iter()
            .find(|file| !file.is_empty() && root().join(file).is_file());
        let Some(file) = file else {
            continue;
        };
        if let Some(item) = base.get(length)
            && depth < 3
            && let Some(reexport) = reexported_path(&file, item)
        {
            let tail = base[length + 1..].join("::");
            let full = if tail.is_empty() {
                reexport
            } else {
                format!("{reexport}::{tail}")
            };
            if let Some(found) = module_file(&full, &module_of(&file), depth + 1) {
                return Some(found);
            }
        }
        return Some(file);
    }
    None
}

/// The directory a module's child modules live in.
fn module_dir(module: &[String]) -> PathBuf {
    let mut dir = root().join("src");
    for segment in module {
        dir = dir.join(segment);
    }
    dir
}

/// The path `pub use` gives `item` in `file`, when it re-exports one.
fn reexported_path(file: &str, item: &str) -> Option<String> {
    let code = production_code(&root().join(file))?;
    let code = without_comments_and_strings(&code);
    let mut offset = 0;
    while let Some(found) = code[offset..].find("pub use ") {
        let start = offset + found;
        let end = code[start..].find(';')?;
        let tree: String = code[start + "pub use ".len()..start + end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for path in expand_use_tree(&tree) {
            if path.rsplit("::").next() == Some(item) {
                return Some(path);
            }
        }
        offset = start + end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule, held against this repository: an import once
    /// crossed into `api-explorer` undeclared and every gate stayed green.
    #[test]
    fn every_crossed_feature_boundary_is_declared() {
        let (undeclared, _) = crossings(&load_features().unwrap());

        assert!(
            undeclared.is_empty(),
            "an import crosses a feature boundary that `depends_on` does not declare, so \
             `cargo xtask impact` would not select the importing feature when the imported \
             code changes; `cargo xtask deps` says what to add:\n{:#?}",
            undeclared
        );
    }

    /// Dropping the edge that was once missed is caught, through a nested `use`.
    #[test]
    fn dropping_a_declared_edge_is_reported_with_the_import() {
        let mut features = load_features().unwrap();
        features
            .get_mut("local-analytics")
            .unwrap()
            .depends_on
            .retain(|name| name != "api-explorer");

        let (undeclared, _) = crossings(&features);

        assert_eq!(undeclared.len(), 1, "{undeclared:#?}");
        assert_eq!(undeclared[0].from, ["local-analytics"]);
        assert_eq!(undeclared[0].to, ["api-explorer"]);
        assert!(undeclared[0].evidence.contains("limits"), "{undeclared:#?}");
        let text = render(&Report {
            undeclared,
            declared_without_import: BTreeMap::new(),
        });
        assert!(
            text.contains(
                "add \"api-explorer\" to depends_on in .agent/features/local-analytics.toml"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_use_tree_expands_to_one_path_per_leaf() {
        assert_eq!(
            expand_use_tree(
                "crate::domain::{functions::parquet_advice, types::{dataset::{DataError, Foo}, limits::INPUT_LIST}}"
            ),
            [
                "crate::domain::functions::parquet_advice",
                "crate::domain::types::dataset::DataError",
                "crate::domain::types::dataset::Foo",
                "crate::domain::types::limits::INPUT_LIST",
            ]
        );
        assert_eq!(
            expand_use_tree("super::executor::Error as E"),
            ["super::executor::Error"]
        );
        assert_eq!(expand_use_tree("super::*"), ["super"]);
    }

    #[test]
    fn a_path_in_a_comment_or_a_string_is_not_an_import() {
        let code = "//! see crate::domain::types::limits\n\
                    use crate::domain::functions::export; // crate::shell::x\n\
                    fn f() { let s = \"crate::adapters::y\"; let c = '\"'; crate::console::progress(s) }\n";

        assert_eq!(
            named_paths(code),
            [
                "crate::domain::functions::export",
                "crate::console::progress"
            ]
        );
    }

    /// The re-export is what makes `crate::domain::types::Credentials` the code
    /// of `assume-role` rather than of whoever claims `types/mod.rs`.
    #[test]
    fn a_re_exported_item_resolves_to_the_file_that_defines_it() {
        assert_eq!(
            module_file("crate::domain::types::Credentials", &[], 0).as_deref(),
            Some("src/domain/types/credentials.rs")
        );
        assert_eq!(
            module_file("crate::domain::types::limits::INPUT_LIST", &[], 0).as_deref(),
            Some("src/domain/types/limits.rs")
        );
        assert_eq!(
            module_file(
                "super::limits",
                &["domain".into(), "types".into(), "database".into()],
                0
            )
            .as_deref(),
            Some("src/domain/types/limits.rs")
        );
        assert_eq!(module_file("std::fs", &[], 0), None);
    }
}
