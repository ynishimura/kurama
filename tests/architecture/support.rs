//! What every rule reads: the file walk, the production part of a file, the
//! feature map and the scenario sources.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub(crate) fn rust_files(dir: &Path) -> Vec<PathBuf> {
    if dir.is_file() {
        return vec![dir.to_path_buf()];
    }
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Source text before the first `#[cfg(test)]`; test-only files are skipped.
pub(crate) fn production_code(path: &Path) -> Option<String> {
    let name = path.file_name().unwrap().to_string_lossy();
    if name == "tests.rs" || name.ends_with("_tests.rs") {
        return None;
    }
    let content = std::fs::read_to_string(path).unwrap();
    // Cut at the trailing test module, not at the first `#[cfg(test)]`: that
    // attribute also marks test-only methods in the middle of a file, and
    // stopping there left the production code after them unchecked.
    let cut = test_module_start(&content).unwrap_or(content.len());
    Some(content[..cut].to_string())
}

/// Where `#[cfg(test)] mod ...` begins, allowing attributes between the two.
pub(crate) fn test_module_start(content: &str) -> Option<usize> {
    let mut offset = 0;
    while let Some(found) = content[offset..].find("#[cfg(test)]") {
        let start = offset + found;
        let declares_a_module = content[start..]
            .lines()
            .skip(1)
            .take(3)
            .any(|line| line.trim_start().starts_with("mod "));
        if declares_a_module {
            return Some(start);
        }
        offset = start + "#[cfg(test)]".len();
    }
    None
}

/// The paths the production code under `dir` names that one of `forbidden`
/// covers (`syntax::names`), read as syntax: test-only items are skipped, a
/// comment or a string is not a path, and a nested, aliased or multi-line
/// `use` is the path it brings in.
pub(crate) fn violations(dir: &str, forbidden: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for path in rust_files(&root().join(dir)) {
        if production_code(&path).is_none() {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        found.extend(
            violations_in(&source, forbidden)
                .into_iter()
                .map(|(line, named, token)| {
                    format!(
                        "{}:{line}: `{token}` in `{named}`",
                        path.strip_prefix(root()).unwrap().display()
                    )
                }),
        );
    }
    found
}

/// `(line, path, forbidden entry)` for each path of `source` that is
/// forbidden.
pub(crate) fn violations_in(source: &str, forbidden: &[&str]) -> Vec<(usize, String, String)> {
    let reading = crate::syntax::read_source(source);
    let mut found = Vec::new();
    for named in &reading.paths {
        if let Some(token) = forbidden
            .iter()
            .find(|token| crate::syntax::names(named, token))
        {
            found.push((named.line, named.text(), token.to_string()));
        }
    }
    found.sort();
    found.dedup();
    found
}

#[derive(serde::Deserialize)]
pub(crate) struct Feature {
    pub(crate) entry: String,
    #[serde(default)]
    pub(crate) docs: Vec<String>,
    pub(crate) files: Vec<String>,
    #[serde(default)]
    pub(crate) scenarios: Vec<String>,
    #[serde(default)]
    pub(crate) depends_on: Vec<String>,
}

/// The feature map: `.agent/features/<feature>.toml`, one table per file.
/// `feature_files_declare_the_feature_they_are_named_after` is what fails when
/// a file and its table disagree, which is also how a merge that kept both
/// sides of one feature shows up; two files for one feature cannot happen
/// while every file is named after the table it holds.
pub(crate) fn feature_map() -> BTreeMap<String, Feature> {
    let mut features = BTreeMap::new();
    for (path, content) in feature_files() {
        let parsed: BTreeMap<String, Feature> =
            toml::from_str(&content).unwrap_or_else(|e| panic!("{path} parses: {e}"));
        features.extend(parsed);
    }
    assert!(!features.is_empty(), ".agent/features/ holds feature files");
    features
}

/// The feature files with their repository-relative paths, sorted.
pub(crate) fn feature_files() -> Vec<(String, String)> {
    let directory = root().join(".agent/features");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&directory)
        .expect(".agent/features/ exists")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .map(|path| {
            let name = format!(
                ".agent/features/{}",
                path.file_name().expect("a file").to_string_lossy()
            );
            (name, std::fs::read_to_string(&path).expect("readable"))
        })
        .collect();
    files.sort();
    files
}

/// The scenario sources: one file per feature under `tests/scenarios/`, with
/// `main.rs` holding only the module list, and every case declared under
/// `tests/cases/<feature>/` (a `.toml` file is one scenario). Every check
/// that scans them reads this, so a new feature file cannot be missed by one
/// of them.
pub(crate) fn scenario_sources() -> Vec<(String, String)> {
    let directory = root().join("tests/scenarios");
    let mut sources: Vec<(String, String)> = std::fs::read_dir(&directory)
        .expect("tests/scenarios/ exists")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| path.file_name().is_some_and(|name| name != "main.rs"))
        .map(|path| {
            let name = format!(
                "tests/scenarios/{}",
                path.file_name().expect("a file").to_string_lossy()
            );
            (name, std::fs::read_to_string(&path).expect("readable"))
        })
        .collect();
    assert!(
        sources.len() > 1,
        "tests/scenarios/ holds one file per feature; found {}",
        sources.len()
    );
    sources.extend(case_files().into_iter().map(|(path, _)| {
        let content = std::fs::read_to_string(root().join(&path)).expect("readable");
        (path, content)
    }));
    sources.sort();
    sources
}

/// Every case file as `(tests/cases/<feature>/<id>.toml, feature)`.
fn case_files() -> Vec<(String, String)> {
    let directory = root().join("tests/cases");
    let mut files = Vec::new();
    for feature in std::fs::read_dir(&directory).into_iter().flatten() {
        let feature = feature.expect("readable directory entry").path();
        if !feature.is_dir() {
            continue;
        }
        let name = feature.file_name().expect("a directory").to_string_lossy();
        for case in std::fs::read_dir(&feature).expect("readable directory") {
            let case = case.expect("readable directory entry").path();
            if case.extension().is_some_and(|ext| ext == "toml") {
                files.push((
                    format!(
                        "tests/cases/{name}/{}",
                        case.file_name().expect("a file").to_string_lossy()
                    ),
                    name.to_string(),
                ));
            }
        }
    }
    files.sort();
    files
}

/// Every case by name and the feature whose directory holds it: a case is
/// claimed by where it lives, not by a list.
pub(crate) fn case_claims() -> BTreeMap<String, String> {
    case_files()
        .into_iter()
        .map(|(path, feature)| {
            let name = path
                .rsplit('/')
                .next()
                .and_then(|file| file.strip_suffix(".toml"))
                .expect("a case file")
                .to_string();
            (name, feature)
        })
        .collect()
}

/// A scenario's name: a Rust function under `tests/scenarios/`, or a case
/// file under `tests/cases/`.
pub(crate) fn scenario_names() -> BTreeSet<String> {
    scenario_sources()
        .iter()
        .filter(|(file, _)| file.ends_with(".rs"))
        .flat_map(|(_, content)| {
            content
                .lines()
                .filter_map(|line| line.strip_prefix("fn "))
                .filter_map(|line| line.split('(').next())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .chain(case_claims().into_keys())
        .collect()
}

/// The agent contract as `kurama agent` prints it: the sections of
/// `docs/agents/kurama/`, which `src/shell/cli/commands/agent.rs` concatenates.
/// Checks here search the whole page, so reading the files in name order is
/// enough; `every_guide_section_is_printed` is what pins the order itself.
pub(crate) fn agent_guide_sections() -> Vec<(String, String)> {
    let directory = root().join("docs/agents/kurama");
    let mut sections: Vec<(String, String)> = std::fs::read_dir(&directory)
        .expect("docs/agents/kurama/ exists")
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("a file")
                .to_string_lossy()
                .to_string();
            (name, std::fs::read_to_string(&path).expect("readable"))
        })
        .collect();
    sections.sort();
    sections
}

pub(crate) fn agent_guide() -> String {
    agent_guide_sections()
        .into_iter()
        .map(|(_, content)| content)
        .collect()
}

pub(crate) fn matches_entry(entry: &str, file: &str) -> bool {
    match entry.strip_suffix('/') {
        Some(prefix) => file.starts_with(prefix) && file[prefix.len()..].starts_with('/'),
        None => entry == file,
    }
}

pub(crate) fn location(path: &Path, number: usize, line: &str) -> String {
    format!(
        "{}:{}: `{}`",
        path.strip_prefix(root()).unwrap().display(),
        number + 1,
        line.trim()
    )
}

pub(crate) fn contains_word(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(start, _)| {
        !text[..start].chars().next_back().is_some_and(is_ident)
            && !text[start + word.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
    })
}

/// An allowlist entry: the path it allows, why, and the test that exercises
/// what the path is allowed to do (resolved as a `Held by` cell is).
pub(crate) type Allowed = (&'static str, &'static str, &'static str);

pub(crate) fn allows(list: &[Allowed], path: &str) -> bool {
    list.iter().any(|(allowed, _, _)| *allowed == path)
}

/// A violating fixture's verdict: the rule's detector refused it. The panic
/// message is what `cargo xtask architecture-audit --run-fixtures` reads to
/// tell a miss of this rule from a fixture that failed for another reason.
pub(crate) fn assert_detected(rule: &str, detected: bool, fixture: &str) {
    assert!(detected, "{rule}: missed a violation: {fixture}");
}

/// A passing fixture's verdict: the rule's detector allowed it.
pub(crate) fn assert_allowed(rule: &str, allowed: bool, fixture: &str) {
    assert!(allowed, "{rule}: refused allowed code: {fixture}");
}
