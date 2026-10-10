//! The feature map: `.agent/features/<feature>.toml`, one table per file,
//! with the cases under `tests/cases/<feature>/` added to its scenarios.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::{cases, root, verify_real};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct Feature {
    pub(crate) summary: String,
    pub(crate) entry: String,
    /// Documents to read before the files: the narration of the feature's
    /// flow, which `map` prints first.
    #[serde(default)]
    pub(crate) docs: Vec<String>,
    /// File paths, or directory prefixes ending in `/`.
    pub(crate) files: Vec<String>,
    /// Filters for tests whose path implies no module prefix: an
    /// integration-test binary (`tests/<name>.rs`) names its tests at the top
    /// level, and so does xtask. Every other filter is derived from `files`,
    /// because a hand-written substring over test *names* cannot be checked
    /// against a list of *paths*: one that selects a single test of a file,
    /// or none at all, reads exactly like one that selects them all.
    #[serde(default)]
    pub(crate) tests: Vec<String>,
    /// Test function names in `tests/scenarios/`.
    #[serde(default)]
    pub(crate) scenarios: Vec<String>,
    /// Features whose code runs when this feature is exercised; `impact`
    /// selects this feature when one of them changes.
    #[serde(default)]
    pub(crate) depends_on: Vec<String>,
    #[serde(default)]
    pub(crate) notes: Option<String>,
    /// How the feature is verified against the real thing
    /// (`cargo xtask verify-real`); every feature says.
    #[serde(default)]
    pub(crate) real: Option<verify_real::Real>,
}

pub(crate) type FeatureMap = BTreeMap<String, Feature>;

/// The feature map: one file per feature under `.agent/features/`, each
/// holding a single table named after the file. Adding a feature adds a file,
/// so two branches never append to the same one. Requiring the table to be
/// named after its file is what makes two files for one feature impossible,
/// and it is how a merge that kept both sides of a feature shows up: the file
/// then holds two tables and this refuses it.
pub(crate) fn load_features() -> Result<FeatureMap, String> {
    let directory = root().join(".agent/features");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map_err(|e| format!("{}: {e}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", directory.display()))?;
    entries.retain(|path| path.extension().is_some_and(|ext| ext == "toml"));
    entries.sort();
    let mut features = FeatureMap::new();
    for path in entries {
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .ok_or_else(|| format!("{}: no file name", path.display()))?;
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let parsed: FeatureMap =
            toml::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))?;
        let names: Vec<&String> = parsed.keys().collect();
        if names != [&stem] {
            return Err(format!(
                "{}: holds {:?}, not one table named {stem:?}",
                path.display(),
                names
            ));
        }
        features.extend(parsed);
    }
    if features.is_empty() {
        return Err(format!("{}: no feature files", directory.display()));
    }
    for (name, feature) in &mut features {
        for case in case_scenarios(name) {
            if !feature.scenarios.contains(&case) {
                feature.scenarios.push(case);
            }
        }
    }
    Ok(features)
}

/// The cases declared under `tests/cases/<feature>/`: one `.toml` file is one
/// scenario, named by the file and claimed by the directory, so the feature
/// map never lists them.
fn case_scenarios(feature: &str) -> Vec<String> {
    let mut cases: Vec<String> = cases::case_files(&root())
        .unwrap_or_default()
        .iter()
        .filter_map(|file| cases::case_path(file))
        .filter(|(owner, _)| *owner == feature)
        .map(|(_, id)| id.to_string())
        .collect();
    cases.sort();
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases of a feature are the `.toml` files of its directory, named by
    /// the file, and a feature with no directory has none.
    #[test]
    fn a_feature_lists_the_cases_of_its_directory() {
        let cases = case_scenarios("api-client");
        assert!(
            cases.contains(&"api_token_kind_401_is_not_retried".to_string()),
            "{cases:?}"
        );
        assert!(cases.windows(2).all(|pair| pair[0] < pair[1]), "{cases:?}");
        assert_eq!(case_scenarios("no-such-feature"), Vec::<String>::new());
    }

    /// An agent cannot run `cargo xtask map <feature>` without a name to
    /// give it, and without one it greps the tree instead; AGENTS.md lists
    /// the names on the line that starts with `Features:`.
    #[test]
    fn agents_md_names_every_feature() {
        let agents = std::fs::read_to_string(root().join("AGENTS.md")).unwrap();
        let listed: Vec<String> = agents
            .lines()
            .find_map(|line| line.strip_prefix("Features: "))
            .expect("AGENTS.md has a `Features: ` line")
            .split(", ")
            .map(|name| name.trim_matches('`').to_string())
            .collect();
        let mapped: Vec<String> = load_features().unwrap().into_keys().collect();
        assert_eq!(listed, mapped);
    }
}
