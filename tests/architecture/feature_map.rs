//! `.agent/features/` against the tree: every path, scenario and file is
//! claimed, and scenarios that need a fake require it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::support::*;

/// A file named after something other than the feature it declares would be
/// invisible to anyone looking for that feature, and `load_features` in xtask
/// refuses it too.
#[test]
fn feature_files_declare_the_feature_they_are_named_after() {
    for (path, content) in feature_files() {
        let parsed: BTreeMap<String, Feature> =
            toml::from_str(&content).unwrap_or_else(|e| panic!("{path} parses: {e}"));
        let stem = path
            .strip_prefix(".agent/features/")
            .and_then(|name| name.strip_suffix(".toml"))
            .expect("the listing built this path");
        let names: Vec<&str> = parsed.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [stem],
            "{path} must hold exactly one table named {stem:?}"
        );
    }
}

/// The endpoint override is compiled out of installed binaries. Even a scenario
/// that expects validation to stop before S3 needs the gate: a regression in
/// validation must not turn a failing test into a request to a real bucket.
#[test]
fn scenarios_that_reach_s3_require_the_fake_endpoint() {
    let mut ungated = Vec::new();
    for (file, source) in scenario_sources() {
        let lines: Vec<_> = source.lines().collect();
        for (start, line) in lines.iter().enumerate() {
            if !line.starts_with("fn ") {
                continue;
            }
            let end = start
                + lines[start..]
                    .iter()
                    .position(|line| *line == "}")
                    .expect("scenario closes at column zero");
            let uses_s3 = lines[start..=end].iter().any(|line| {
                line.contains("s3://")
                    || line.contains(".s3.amazonaws.com")
                    || line.contains(".s3.")
                    || line.contains("support::data::workspace_config()")
            });
            let attributes: Vec<_> = lines[..start]
                .iter()
                .rev()
                .take_while(|line| !line.is_empty())
                .collect();
            if uses_s3
                && !attributes
                    .iter()
                    .any(|line| line.contains("cfg_attr(not(feature = \"test-fakes\"), ignore"))
            {
                ungated.push(location(Path::new(&file), start + 1, line));
            }
        }
    }
    assert!(
        ungated.is_empty(),
        "S3 scenarios need #[cfg_attr(not(feature = \"test-fakes\"), ignore)]:\n{}",
        ungated.join("\n")
    );
}

/// The isolated token store is compiled in behind `test-fakes`; without the
/// feature the binary reaches the real macOS keychain. A scenario that obtains
/// or stores a token therefore needs the gate, whatever else it asserts, or a
/// plain `cargo test` writes into the developer's own keychain.
#[test]
fn scenarios_that_store_a_token_require_the_fake_store() {
    let mut ungated = Vec::new();
    for (file, source) in scenario_sources() {
        let lines: Vec<_> = source.lines().collect();
        for (start, line) in lines.iter().enumerate() {
            if !line.starts_with("fn ") {
                continue;
            }
            let end = start
                + lines[start..]
                    .iter()
                    .position(|line| *line == "}")
                    .expect("scenario closes at column zero");
            let stores_a_token = lines[start..=end]
                .iter()
                .any(|line| line.contains(".with_token_store()"));
            let attributes: Vec<_> = lines[..start]
                .iter()
                .rev()
                .take_while(|line| !line.is_empty())
                .collect();
            if stores_a_token
                && !attributes
                    .iter()
                    .any(|line| line.contains("cfg_attr(not(feature = \"test-fakes\"), ignore"))
            {
                ungated.push(location(Path::new(&file), start + 1, line));
            }
        }
    }
    assert!(
        ungated.is_empty(),
        "a scenario that uses the token store needs \
         #[cfg_attr(not(feature = \"test-fakes\"), ignore)]:\n{}",
        ungated.join("\n")
    );
}

/// A case runs the binary against the fakes `test-fakes` compiles in: the
/// file-backed session cache and token store, the S3 endpoint override. A
/// case that needs none of them today can start needing one with an edit
/// nobody connects to the gate, so every generated case carries it and
/// `verify`, which always builds with the feature, runs them all. The count
/// holds the committed `tests/scenarios/cases_generated.rs` to the one level
/// of `tests/cases/<feature>/` everything else reads.
#[test]
fn every_generated_case_requires_test_fakes() {
    let generated = include_str!("../scenarios/cases_generated.rs");
    let lines: Vec<&str> = generated.lines().collect();
    let tests: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| **line == "#[test]")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(tests.len(), case_claims().len(), "one test per case file");
    let ungated: Vec<&str> = tests
        .iter()
        .filter(|index| {
            !lines[**index + 1].contains("cfg_attr(not(feature = \"test-fakes\"), ignore")
        })
        .map(|index| lines[index + 2])
        .collect();
    assert!(
        ungated.is_empty(),
        "cargo xtask generate-cases wrote a case without the test-fakes gate:\n{}",
        ungated.join("\n")
    );
}

#[test]
fn feature_map_paths_and_scenarios_exist() {
    let features = feature_map();
    let scenarios = scenario_names();
    let mut problems = Vec::new();
    for (name, feature) in &features {
        for path in feature
            .files
            .iter()
            .chain(&feature.docs)
            .chain(std::iter::once(&feature.entry))
        {
            if !root().join(path.trim_end_matches('/')).exists() {
                problems.push(format!("[{name}] path does not exist: {path}"));
            }
        }
        for scenario in &feature.scenarios {
            if !scenarios.contains(scenario) {
                problems.push(format!(
                    "[{name}] scenario not under tests/scenarios/: {scenario}"
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn feature_map_dependencies_name_other_features() {
    let features = feature_map();
    let mut problems = Vec::new();
    for (name, feature) in &features {
        for dependency in &feature.depends_on {
            if dependency == name || !features.contains_key(dependency) {
                problems.push(format!(
                    "[{name}] depends_on is not another feature: {dependency}"
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_feature_entry_is_claimed_by_its_files() {
    let features = feature_map();
    let problems: Vec<String> = features
        .iter()
        .filter(|(_, feature)| {
            !feature
                .files
                .iter()
                .any(|file| matches_entry(file, &feature.entry))
        })
        .map(|(name, feature)| {
            format!("[{name}] entry is not included in files: {}", feature.entry)
        })
        .collect();

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The files no feature's `files` entry claims.
fn orphans(features: &BTreeMap<String, Feature>, files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|file| {
            !features
                .values()
                .any(|f| f.files.iter().any(|entry| matches_entry(entry, file)))
        })
        .cloned()
        .collect()
}

/// The scenarios no feature lists.
fn unclaimed_scenarios(
    features: &BTreeMap<String, Feature>,
    names: BTreeSet<String>,
) -> Vec<String> {
    let claimed: BTreeSet<&String> = features.values().flat_map(|f| &f.scenarios).collect();
    names
        .into_iter()
        .filter(|name| !claimed.contains(name))
        .collect()
}

fn fixture_features() -> BTreeMap<String, Feature> {
    BTreeMap::from([(
        "a".to_string(),
        Feature {
            entry: "src/a/mod.rs".into(),
            docs: vec![],
            files: vec!["src/a/".into(), "tests/scenarios/a.rs".into()],
            scenarios: vec!["a_works".into()],
            depends_on: vec![],
        },
    )])
}

#[test]
fn a_file_or_a_scenario_no_feature_claims_is_found() {
    let files = ["src/a/x.rs", "src/ab.rs", "src/b/y.rs"].map(String::from);
    let found = orphans(&fixture_features(), &files);
    assert_detected(
        "ARCH-003",
        found == ["src/ab.rs", "src/b/y.rs"],
        &format!("{found:?}"),
    );
    let names = ["a_works", "b_works"].map(String::from).into();
    let unclaimed = unclaimed_scenarios(&fixture_features(), names);
    assert_detected(
        "ARCH-003",
        unclaimed == ["b_works"],
        &format!("{unclaimed:?}"),
    );
}

#[test]
fn a_file_under_a_claimed_directory_and_a_listed_scenario_are_allowed() {
    let files = ["src/a/x.rs", "src/a/deep/y.rs", "tests/scenarios/a.rs"].map(String::from);
    let found = orphans(&fixture_features(), &files);
    assert_allowed("ARCH-003", found.is_empty(), &format!("{found:?}"));
    let names = ["a_works".to_string()].into();
    let unclaimed = unclaimed_scenarios(&fixture_features(), names);
    assert_allowed("ARCH-003", unclaimed.is_empty(), &format!("{unclaimed:?}"));
}

#[test]
fn every_source_and_test_file_belongs_to_a_feature() {
    let features = feature_map();
    let files: Vec<String> = ["src", "tests"]
        .iter()
        .flat_map(|dir| rust_files(&root().join(dir)))
        .map(|path| {
            path.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let orphans = orphans(&features, &files);
    assert!(
        orphans.is_empty(),
        "add these files to a feature in .agent/features/:\n{}",
        orphans.join("\n")
    );
}

#[test]
fn every_scenario_is_claimed_by_a_feature() {
    let features = feature_map();
    let claimed_by_a_directory = case_claims()
        .into_iter()
        .filter(|(_, feature)| features.contains_key(feature))
        .map(|(name, _)| name)
        .collect::<BTreeSet<_>>();
    let names = scenario_names()
        .into_iter()
        .filter(|name| !claimed_by_a_directory.contains(name))
        .collect();
    let unclaimed = unclaimed_scenarios(&features, names);
    assert!(
        unclaimed.is_empty(),
        "add these scenarios to a feature in .agent/features/:\n{}",
        unclaimed.join("\n")
    );
}
