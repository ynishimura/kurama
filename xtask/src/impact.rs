//! `cargo xtask impact`: the features a change reaches through the imports
//! and `depends_on`, and the `cargo test` filters derived from the files a
//! feature claims.

use std::collections::{BTreeMap, BTreeSet};

use crate::features::{Feature, FeatureMap, load_features};
use crate::map::test_command;
use crate::{cases, deps, git, root};

#[derive(Debug, serde::Serialize)]
pub(crate) struct Impact {
    pub(crate) base: String,
    pub(crate) changed_files: Vec<String>,
    /// Features that own a changed file.
    pub(crate) features: Vec<String>,
    /// Features selected because they depend on a changed feature.
    pub(crate) dependent_features: Vec<String>,
    /// Changed files that affect behavior but no feature claims (unclaimed
    /// source or test files, Cargo manifests). Treated as "everything".
    pub(crate) unmapped: Vec<String>,
    pub(crate) all_features: bool,
    pub(crate) test_filters: Vec<String>,
    pub(crate) scenarios: Vec<String>,
    pub(crate) commands: Vec<String>,
}

pub(crate) fn impact_command(args: &[String]) -> Result<(), String> {
    let base = match args {
        [flag, value] if flag == "--base" => value.clone(),
        [] => default_base(),
        _ => return Err("usage: cargo xtask impact [--base REF]".into()),
    };
    let impact = compute_impact(&base)?;
    println!("{}", serde_json::to_string_pretty(&impact).unwrap());
    Ok(())
}

/// Feature work branches from `dev` and merges back there; `dev` merges to
/// `main`.
pub(crate) fn default_base() -> String {
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    for candidate in base_candidates(branch.trim()) {
        if git(&["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return candidate.to_string();
        }
    }
    "HEAD".to_string()
}

/// The refs to measure a branch against, best first.
///
/// An integration branch answers for everything it has not pushed yet, so it
/// measures against the remote. A feature branch answers for what *it* added
/// to the branch it grew from, so it measures against the local one: `dev` is
/// regularly ahead of `origin/dev`, and a worktree branched from it inherits
/// those commits. Measuring a feature branch against the remote makes it
/// verify `dev`'s backlog as its own work -- 50 minutes of mutants on 23 files
/// a branch never touched, and a surviving mutant in someone else's code
/// failing a branch that cannot fix it.
fn base_candidates(branch: &str) -> &'static [&'static str] {
    match branch {
        "main" => &["origin/main", "main"],
        "dev" => &["origin/dev", "dev", "origin/main", "main"],
        _ => &["dev", "origin/dev", "main", "origin/main"],
    }
}

pub(crate) fn compute_impact(base: &str) -> Result<Impact, String> {
    let features = load_features()?;
    let mut changed: BTreeSet<String> = BTreeSet::new();
    let merge_base = git(&["merge-base", base, "HEAD"]).unwrap_or_else(|_| base.to_string());
    for args in [
        vec!["diff", "--name-only", merge_base.trim(), "HEAD"],
        vec!["diff", "--name-only", "HEAD"],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        for line in git(&args)?.lines().filter(|l| !l.is_empty()) {
            changed.insert(line.to_string());
        }
    }

    let selection = select_features(&features, &changed, &deps::importers(), |file| {
        root().join(file).exists()
    });
    Ok(impact_of(base, &features, changed, selection))
}

/// What `selection` of the `changed` files runs: every feature when a file
/// no feature claims changed, else the owners and their dependents, with
/// the test command only when there are filters to give it.
fn impact_of(
    base: &str,
    features: &FeatureMap,
    changed: BTreeSet<String>,
    selection: Selection,
) -> Impact {
    let all_features = !selection.unmapped.is_empty();
    let (direct, dependents): (Vec<String>, Vec<String>) = if all_features {
        (features.keys().cloned().collect(), vec![])
    } else {
        (
            selection.changed.iter().cloned().collect(),
            selection.dependents.iter().cloned().collect(),
        )
    };
    let selected: Vec<&String> = direct.iter().chain(&dependents).collect();
    let test_filters: Vec<String> = selected
        .iter()
        .flat_map(|name| test_filters(&features[*name]))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let scenarios: Vec<String> = selected
        .iter()
        .flat_map(|name| features[*name].scenarios.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut commands = Vec::new();
    if !test_filters.is_empty() {
        commands.push(test_command(&test_filters));
    }
    commands.push("cargo xtask verify affected".to_string());
    Impact {
        base: base.to_string(),
        changed_files: changed.into_iter().collect(),
        features: direct,
        dependent_features: dependents,
        unmapped: selection.unmapped,
        all_features,
        test_filters,
        scenarios,
        commands,
    }
}

/// The features whose files name every other feature's commands and errors
/// (`deps`): reaching one of them is not a reason to run everything.
use deps::REGISTRATION_HUBS;

/// Files every feature is built from: a change to one selects every feature.
const WHOLE_BUILD_FILES: [&str; 2] = ["Cargo.toml", "Cargo.lock"];

#[derive(Debug, Default)]
struct Selection {
    /// Features that own a changed file.
    changed: BTreeSet<String>,
    /// Features that depend, directly or through another feature, on a changed one.
    dependents: BTreeSet<String>,
    /// Changed files that affect behavior but no feature owns.
    unmapped: Vec<String>,
}

impl Selection {
    #[cfg(test)]
    fn features(&self) -> impl Iterator<Item = &String> {
        self.changed.iter().chain(&self.dependents)
    }
}

/// The features a change reaches. A changed `src/` file reaches the files
/// that import it, and those that import them, and so on: code can only run
/// what it imports, and a file-level walk does not drag in the whole of a
/// feature because one helper of it is shared (`oauth` uses one function of
/// `status`); the walk stops at a registration hub's file, which imports
/// every feature. The features whose `depends_on` names a reached one are
/// reached too, one step, because their scenarios run that code. Any other
/// changed file -- a scenario, a fake, a document -- reaches its owners and,
/// transitively, the features whose `depends_on` names them.
fn select_features(
    features: &FeatureMap,
    changed_files: &BTreeSet<String>,
    importers: &BTreeMap<String, BTreeSet<String>>,
    exists: impl Fn(&str) -> bool,
) -> Selection {
    let owners_of = |file: &str| -> Vec<String> {
        features
            .iter()
            .filter(|(_, feature)| feature_owns_file(feature, file))
            .map(|(name, _)| name.clone())
            .collect()
    };
    let mut selection = Selection::default();
    let mut sources: Vec<String> = Vec::new();
    let mut declared: BTreeSet<String> = BTreeSet::new();
    let mut sources_changed: BTreeSet<String> = BTreeSet::new();
    for file in changed_files {
        let owners = owners_of(file);
        if WHOLE_BUILD_FILES.contains(&file.as_str()) {
            selection.unmapped.push(file.clone());
        } else if owners.is_empty() {
            // A deleted file has no owner and no code; it cannot affect anything.
            if exists(file) && (file.starts_with("src/") || file.starts_with("tests/")) {
                selection.unmapped.push(file.clone());
            }
        } else {
            if file.starts_with("src/") {
                sources.push(file.clone());
                sources_changed.extend(owners.iter().cloned());
            } else {
                declared.extend(owners.iter().cloned());
            }
            selection.changed.extend(owners);
        }
    }

    // src/: every file that imports a changed one, transitively -- up to a
    // registration hub's file, which imports every feature: reaching the
    // dispatch table selects `cli-entry`, not everything the dispatch runs.
    let through_hub = |file: &str| {
        let owners = owners_of(file);
        !owners.is_empty()
            && owners
                .iter()
                .all(|name| REGISTRATION_HUBS.contains(&name.as_str()))
    };
    let mut reached: BTreeSet<String> = BTreeSet::new();
    while let Some(file) = sources.pop() {
        for importer in importers.get(&file).into_iter().flatten() {
            if reached.insert(importer.clone()) && !through_hub(importer) {
                sources.push(importer.clone());
            }
        }
    }
    let mut code_reached: BTreeSet<String> = sources_changed.clone();
    for file in &reached {
        code_reached.extend(owners_of(file));
    }
    selection.dependents.extend(code_reached.iter().cloned());
    // A scenario runs the whole binary, so a feature that declares it runs
    // code the walk reached is reached too. One step: `depends_on` has
    // cycles (`oauth` and `status` use each other's helpers), and following
    // it further selects every feature for a change to any file.
    for (name, feature) in features {
        let runs_reached_code = feature.depends_on.iter().any(|dependency| {
            code_reached.contains(dependency) && !REGISTRATION_HUBS.contains(&dependency.as_str())
        });
        if runs_reached_code && !REGISTRATION_HUBS.contains(&name.as_str()) {
            selection.dependents.insert(name.clone());
        }
    }

    // Any other file: the features that declare they run its owners' code,
    // transitively, as before there was an import graph to walk.
    loop {
        let next: Vec<String> = features
            .iter()
            .filter(|(name, feature)| {
                !declared.contains(*name)
                    && feature
                        .depends_on
                        .iter()
                        .any(|dependency| declared.contains(dependency))
            })
            .map(|(name, _)| name.clone())
            .collect();
        if next.is_empty() {
            break;
        }
        declared.extend(next.iter().cloned());
        selection.dependents.extend(next);
    }
    let changed = selection.changed.clone();
    selection.dependents.retain(|name| !changed.contains(name));
    selection
}

/// The module path `cargo test` knows a file's unit tests by, for the files
/// whose path says what it is: `src/a/b.rs` is `a::b::tests::` and
/// `tests/scenarios/x.rs` is `x::`. A file with no tests gets no filter,
/// because a filter that selects nothing makes `cargo test` exit 0 having run
/// nothing.
pub(crate) fn derived_filter(file: &str) -> Option<String> {
    let path = root().join(file);
    if !path.is_file() {
        return None;
    }
    let source = std::fs::read_to_string(&path).ok()?;
    if !source.contains("#[test]") && !source.contains("#[tokio::test]") {
        return None;
    }
    // A test file is `#[path]`-included as the `mod tests` of another module
    // (`<name>_tests.rs`, `tests.rs`), so its tests answer to that module's
    // name and never to its own file name.
    path_filter(&including_module(file).unwrap_or_else(|| file.to_string()))
}

/// The filter the tests of the module at `file` answer to, from its path.
fn path_filter(file: &str) -> Option<String> {
    if let Some(name) = file
        .strip_prefix("tests/scenarios/")
        .and_then(|f| f.strip_suffix(".rs"))
    {
        // `main.rs` and the generated cases it includes name their tests at
        // the top level.
        return (name != "main" && name != "cases_generated").then(|| format!("{name}::"));
    }
    if let Some(filter) = xtask_filter(file) {
        return Some(filter);
    }
    let stem = file.strip_prefix("src/")?.strip_suffix(".rs")?;
    if stem == "lib" || stem == "main" {
        return None;
    }
    let module = stem.strip_suffix("/mod").unwrap_or(stem).replace('/', "::");
    // The module path and not `<module>::tests::`: a file names its test
    // module whatever it likes (`tests`, `snapshots`, a `tests.rs` that is the
    // module itself), and a filter that guesses wrong selects nothing while
    // looking exactly like one that selects everything.
    Some(format!("{module}::"))
}

/// xtask is a binary: `xtask/src/<module>.rs` lists its tests as
/// `<module>::tests::...` and `main.rs` as `main_tests::...`. An integration
/// test binary lists bare function names, so every test of
/// `xtask/tests/<name>.rs` starts with `<name>_`
/// (`xtask_integration_tests_are_named_after_their_file`) and that prefix is
/// the filter.
fn xtask_filter(file: &str) -> Option<String> {
    if let Some(name) = file
        .strip_prefix("xtask/tests/")
        .and_then(|f| f.strip_suffix(".rs"))
    {
        // `support/` is a module the test files share, not a test binary.
        return (!name.contains('/')).then(|| format!("{name}_"));
    }
    let module = file.strip_prefix("xtask/src/")?.strip_suffix(".rs")?;
    Some(if module == "main" {
        "main_tests::".to_string()
    } else {
        format!("{module}::")
    })
}

/// The module that `#[path]`-includes this file, when one does. Sidecar test
/// files are the only thing the repository loads that way.
pub(crate) fn including_module(file: &str) -> Option<String> {
    let path = std::path::Path::new(file);
    let directory = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let needle = format!("#[path = \"{name}\"]");
    for sibling in std::fs::read_dir(root().join(directory)).ok()?.flatten() {
        let sibling = sibling.path();
        // The file itself never holds its own `#[path]`, so it needs no skip.
        if sibling.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        if std::fs::read_to_string(&sibling).is_ok_and(|source| source.contains(&needle)) {
            return sibling
                .strip_prefix(root())
                .ok()
                .map(|relative| relative.display().to_string());
        }
    }
    None
}

/// Every `cargo test` filter of a feature: derived from the files it claims,
/// plus the ones no path can imply, which the feature declares.
pub(crate) fn test_filters(feature: &Feature) -> Vec<String> {
    let mut filters: BTreeSet<String> = feature.tests.iter().cloned().collect();
    for entry in &feature.files {
        for file in claimed_files(entry) {
            if let Some(filter) = derived_filter(&file) {
                filters.insert(filter);
            }
        }
    }
    filters.into_iter().collect()
}

/// The files a `files` entry names: one path, or every `.rs` under a directory.
pub(crate) fn claimed_files(entry: &str) -> Vec<String> {
    if !entry.ends_with('/') {
        return vec![entry.to_string()];
    }
    let mut found = Vec::new();
    let mut stack = vec![root().join(entry)];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for item in entries.flatten() {
            let path = item.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && let Ok(relative) = path.strip_prefix(root())
            {
                found.push(relative.display().to_string());
            }
        }
    }
    found
}

pub(crate) fn matches_entry(entry: &str, file: &str) -> bool {
    if let Some(prefix) = entry.strip_suffix('/') {
        file.starts_with(prefix) && file[prefix.len()..].starts_with('/')
    } else {
        entry == file
    }
}

/// Every path a feature claims: its entry and its files. `impact` asks whether
/// a changed file is one of them, `conflicts` collects them per issue; one
/// definition, so the two cannot disagree about what a feature is made of.
pub(crate) fn feature_entries(feature: &Feature) -> impl Iterator<Item = &String> {
    std::iter::once(&feature.entry).chain(&feature.files)
}

/// The features that own `file`, and the tests they declare: the filters
/// their files imply plus their scenarios. `mutate` runs these for a mutant
/// of that file.
pub(crate) fn owners_and_tests(features: &FeatureMap, file: &str) -> (Vec<String>, Vec<String>) {
    // A case file is owned by the feature of its directory as well as by
    // whoever claims `tests/cases/`, so a change to it runs that feature.
    let owners: Vec<String> = features
        .iter()
        .filter(|(name, feature)| {
            feature_owns_file(feature, file)
                || cases::case_path(file).is_some_and(|(owner, _)| owner == name.as_str())
        })
        .map(|(name, _)| name.clone())
        .collect();
    let tests: BTreeSet<String> = owners
        .iter()
        .flat_map(|name| {
            let feature = &features[name];
            test_filters(feature)
                .into_iter()
                .chain(feature.scenarios.iter().cloned())
        })
        .collect();
    (owners, tests.into_iter().collect())
}

/// The tests a mutant of `file` runs: the unit tests and scenarios of the
/// features that own it, and the scenarios of the features that own a file it
/// imports. The code a file calls is often verified by that other feature's
/// scenarios -- `shell/cli/executor.rs` opens the console through the adapter
/// of `console-federation` -- and a selection of the owners alone lets such a
/// mutant survive for want of the scenario, not of a test. The unit tests of
/// an imported feature call that feature's code only, never the mutated file,
/// so they are left out. A file nobody owns selects nothing, so the caller
/// runs its fallback.
pub(crate) fn mutant_tests(
    features: &FeatureMap,
    file: &str,
    imports: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let (owners, owned_tests) = owners_and_tests(features, file);
    if owners.is_empty() {
        return (vec![], vec![]);
    }
    let imported: BTreeSet<String> = imports
        .iter()
        .flat_map(|target| owners_and_tests(features, target).0)
        .filter(|name| !owners.contains(name))
        .collect();
    let tests: BTreeSet<String> = owned_tests
        .into_iter()
        .chain(
            imported
                .iter()
                .flat_map(|name| features[name].scenarios.iter().cloned()),
        )
        .collect();
    let selected: BTreeSet<String> = owners.into_iter().chain(imported).collect();
    (selected.into_iter().collect(), tests.into_iter().collect())
}

pub(crate) fn feature_owns_file(feature: &Feature, file: &str) -> bool {
    feature_entries(feature).any(|entry| matches_entry(entry, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rust_files;

    /// A feature branch measures against the branch it grew from, not against
    /// the remote: `dev` is regularly ahead of `origin/dev`, and measuring
    /// there made a branch with no `src/` change run mutants on 23 files it
    /// never touched.
    #[test]
    fn a_feature_branch_measures_against_local_dev() {
        assert_eq!(
            super::base_candidates("feat/95-gate-base")[0],
            "dev",
            "a feature branch must not inherit dev's unpushed work"
        );
        // An integration branch answers for what it has not pushed yet.
        assert_eq!(super::base_candidates("dev")[0], "origin/dev");
        assert_eq!(super::base_candidates("main")[0], "origin/main");
        // Every list ends somewhere that exists in a fresh clone.
        for branch in ["feat/1", "dev", "main"] {
            assert!(super::base_candidates(branch).contains(&"main"), "{branch}");
        }
    }

    #[test]
    fn directory_entries_match_by_prefix_only() {
        assert!(matches_entry("src/shell/", "src/shell/cli/mod.rs"));
        assert!(!matches_entry("src/shell/", "src/shellfish.rs"));
        assert!(matches_entry("src/lib.rs", "src/lib.rs"));
    }

    fn feature(entry: &str, depends_on: &[&str]) -> Feature {
        Feature {
            summary: "test".into(),
            entry: entry.into(),
            files: vec![entry.into()],
            tests: vec![],
            scenarios: vec![],
            depends_on: depends_on.iter().map(|name| name.to_string()).collect(),
            notes: None,
            real: None,
        }
    }

    /// A mutant of a file runs the tests of every feature that owns it --
    /// the filters its files imply, the ones it declares and its scenarios --
    /// and none of a feature that does not.
    #[test]
    fn a_file_brings_the_tests_of_each_feature_that_owns_it() {
        let mut features = FeatureMap::new();
        let mut http = feature("src/domain/types/http.rs", &[]);
        http.scenarios = vec!["api_sends".into()];
        features.insert("api-client".into(), http);
        let mut shared = feature("src/domain/types/http.rs", &[]);
        shared.tests = vec!["declared::".into()];
        shared.scenarios = vec!["oauth_flows".into()];
        features.insert("oauth".into(), shared);
        let mut other = feature("src/console.rs", &[]);
        other.scenarios = vec!["console_only".into()];
        features.insert("console".into(), other);

        let (owners, tests) = owners_and_tests(&features, "src/domain/types/http.rs");

        assert_eq!(owners, ["api-client", "oauth"]);
        assert_eq!(
            tests,
            [
                "api_sends",
                "declared::",
                "domain::types::http::",
                "oauth_flows"
            ]
        );
        assert_eq!(
            owners_and_tests(&features, "src/unowned.rs"),
            (vec![], vec![])
        );
    }

    /// A case file belongs to the feature its directory names, and to no
    /// other, even when no feature claims `tests/cases/` itself.
    #[test]
    fn a_case_file_brings_the_tests_of_the_feature_it_sits_under() {
        let mut features = FeatureMap::new();
        let mut database = feature("src/adapters/database/mod.rs", &[]);
        database.scenarios = vec!["db_reads".into()];
        features.insert("database".into(), database);
        let mut api = feature("src/shell/api_runtime.rs", &[]);
        api.scenarios = vec!["api_calls".into()];
        features.insert("api-client".into(), api);

        let (owners, tests) = owners_and_tests(&features, "tests/cases/database/db_reads.toml");

        assert_eq!(owners, ["database"]);
        assert!(tests.contains(&"db_reads".to_string()), "{tests:?}");
        assert!(!tests.contains(&"api_calls".to_string()), "{tests:?}");
    }

    /// A mutant of a file also runs the scenarios of the features whose files
    /// it imports: the console code of `shell/cli/executor.rs` (owned by
    /// `exec`) is verified by the scenarios of `console-federation`, whose
    /// adapter it calls, and a selection of the owners alone let its mutants
    /// survive. The unit tests of an imported feature call that feature's own
    /// code, never the mutated file, so they are left out.
    #[test]
    fn a_file_also_brings_the_scenarios_of_the_features_it_imports() {
        let mut features = FeatureMap::new();
        let mut exec = feature("src/shell/cli/executor.rs", &[]);
        exec.scenarios = vec!["exec_runs".into()];
        exec.tests = vec!["exec_unit".into()];
        features.insert("exec".into(), exec);
        let mut console = feature("src/adapters/aws/federation.rs", &[]);
        console.scenarios = vec!["console_opens".into()];
        console.tests = vec!["federation_unit".into()];
        features.insert("console-federation".into(), console);
        let mut other = feature("src/other.rs", &[]);
        other.scenarios = vec!["other_only".into()];
        features.insert("other".into(), other);
        let imports = names(&["src/adapters/aws/federation.rs", "src/unowned.rs"]);

        let (owners, tests) = mutant_tests(&features, "src/shell/cli/executor.rs", &imports);

        assert_eq!(owners, ["console-federation", "exec"]);
        assert!(tests.contains(&"exec_runs".to_string()), "{tests:?}");
        assert!(tests.contains(&"console_opens".to_string()), "{tests:?}");
        assert!(tests.contains(&"exec_unit".to_string()), "{tests:?}");
        assert!(!tests.contains(&"federation_unit".to_string()), "{tests:?}");
        assert!(!tests.contains(&"other_only".to_string()), "{tests:?}");
        // A file nobody owns stays unowned, whatever it imports.
        assert_eq!(
            mutant_tests(&features, "src/unowned.rs", &imports),
            (vec![], vec![])
        );
    }

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn feature_entry_is_an_impact_owner() {
        assert!(feature_owns_file(
            &feature("src/entry.rs", &[]),
            "src/entry.rs"
        ));
    }

    /// A file that is not code reaches its owners and whoever declares that
    /// it runs their code, transitively.
    #[test]
    fn impact_selects_dependents_transitively() {
        let features: FeatureMap = [
            ("core".to_string(), feature("tests/fakes/core", &[])),
            ("command".to_string(), feature("src/command.rs", &["core"])),
            ("screen".to_string(), feature("src/screen.rs", &["command"])),
            ("other".to_string(), feature("src/other.rs", &[])),
        ]
        .into();

        let selection = select_features(
            &features,
            &names(&["tests/fakes/core"]),
            &BTreeMap::new(),
            |_| true,
        );

        assert_eq!(selection.changed, names(&["core"]));
        assert_eq!(selection.dependents, names(&["command", "screen"]));
        assert!(selection.unmapped.is_empty());
    }

    /// A file no feature claims selects every feature when it is code or a
    /// test that exists: nothing says what it affects. A deleted one, or one
    /// outside `src/` and `tests/`, selects nothing.
    #[test]
    fn impact_selects_everything_for_an_unclaimed_source_or_test_only() {
        let features: FeatureMap = [("core".to_string(), feature("src/core.rs", &[]))].into();

        let selection = select_features(
            &features,
            &names(&["docs/x.md", "src/gone.rs", "src/stray.rs", "tests/stray.rs"]),
            &BTreeMap::new(),
            |file| file != "src/gone.rs",
        );

        assert_eq!(selection.unmapped, ["src/stray.rs", "tests/stray.rs"]);
    }

    /// Read against this repository's own files: a sidecar test file answers
    /// to the module that includes it, a file whose only tests are async still
    /// has tests, and a file with none gets no filter.
    #[test]
    fn a_file_derives_the_filter_of_the_module_its_tests_run_in() {
        assert_eq!(
            including_module("src/domain/functions/openapi_tests.rs").as_deref(),
            Some("src/domain/functions/openapi.rs")
        );
        assert_eq!(including_module("src/domain/functions/openapi.rs"), None);
        assert_eq!(
            derived_filter("src/domain/functions/openapi_tests.rs").as_deref(),
            Some("domain::functions::openapi::")
        );
        assert_eq!(
            derived_filter("src/shell/oauth_executor.rs").as_deref(),
            Some("shell::oauth_executor::")
        );
        assert_eq!(derived_filter("src/domain/constants.rs"), None);
        assert_eq!(
            derived_filter("tests/scenarios/api_client.rs").as_deref(),
            Some("api_client::")
        );
        assert_eq!(derived_filter("tests/scenarios/cases_generated.rs"), None);
    }

    /// The path half of the filter, for the shapes the repository does not
    /// hold a tested file of.
    #[test]
    fn a_path_names_the_module_its_tests_answer_to() {
        assert_eq!(path_filter("tests/scenarios/main.rs"), None);
        assert_eq!(
            path_filter("tests/scenarios/db.rs").as_deref(),
            Some("db::")
        );
        assert_eq!(path_filter("src/lib.rs"), None);
        assert_eq!(path_filter("src/main.rs"), None);
        assert_eq!(path_filter("src/a/mod.rs").as_deref(), Some("a::"));
        assert_eq!(path_filter("src/a/b.rs").as_deref(), Some("a::b::"));
        assert_eq!(path_filter("docs/a.rs"), None);
    }

    /// An unclaimed change runs every feature and nothing narrower; a claimed
    /// one runs its owners, and a selection with no test filter asks for
    /// no `cargo test`.
    #[test]
    fn impact_runs_everything_only_for_an_unclaimed_change() {
        let mut core = feature("src/core.rs", &[]);
        core.scenarios = vec!["core_runs".into()];
        let features: FeatureMap = [
            ("core".to_string(), core),
            ("other".to_string(), feature("docs/other.md", &[])),
        ]
        .into();
        let select =
            |files: &[&str]| select_features(&features, &names(files), &BTreeMap::new(), |_| true);

        let everything = impact_of(
            "dev",
            &features,
            names(&["src/stray.rs"]),
            select(&["src/stray.rs"]),
        );
        assert!(everything.all_features);
        assert_eq!(everything.features, ["core", "other"]);

        let owned = impact_of(
            "dev",
            &features,
            names(&["src/core.rs"]),
            select(&["src/core.rs"]),
        );
        assert!(!owned.all_features);
        assert_eq!(owned.features, ["core"]);
        assert_eq!(owned.scenarios, ["core_runs"]);

        let untested = impact_of(
            "dev",
            &features,
            names(&["docs/other.md"]),
            select(&["docs/other.md"]),
        );
        assert_eq!(untested.test_filters, Vec::<String>::new());
        assert_eq!(untested.commands, ["cargo xtask verify affected"]);
    }

    #[test]
    fn a_directory_claims_the_rust_files_under_it() {
        let files = claimed_files("src/adapters/database/");
        assert!(
            files.contains(&"src/adapters/database/server.rs".to_string()),
            "{files:?}"
        );
        assert!(files.iter().all(|file| file.ends_with(".rs")), "{files:?}");
    }

    #[test]
    fn impact_selects_every_feature_for_cargo_manifests() {
        let features: FeatureMap = [("core".to_string(), feature("src/core.rs", &[]))].into();

        let selection = select_features(
            &features,
            &names(&["Cargo.lock", "README.md"]),
            &BTreeMap::new(),
            |_| true,
        );

        assert_eq!(selection.unmapped, ["Cargo.lock"]);
    }

    /// 09e5bbf changed `src/shell/executor.rs`, and `impact` selected only 9 of
    /// 22 scenarios: every scenario below runs AssumeRole through that file.
    #[test]
    fn impact_of_the_executor_reaches_every_assume_role_scenario() {
        let features = load_features().unwrap();

        let selection = select_features(
            &features,
            &names(&["src/shell/executor.rs"]),
            &deps::importers(),
            |_| true,
        );

        let scenarios: BTreeSet<&String> = selection
            .features()
            .flat_map(|name| &features[name].scenarios)
            .collect();
        for scenario in [
            "uc07_console_fetches_signin_token_and_opens_login_url",
            "uc11_second_run_reuses_the_cached_mfa_session",
            "session_cache_logout_all_requires_fresh_session",
            "login_caches_the_mfa_session_once_and_env_reuses_it",
            "status_json_reports_the_cached_session_and_the_active_profile",
            "mfa_via_onepassword_sends_the_totp_once",
        ] {
            assert!(
                scenarios.contains(&scenario.to_string()),
                "{scenario} is not selected"
            );
        }
    }

    /// `agent.rs` parses the configuration samples of `README.md` through
    /// `include_str!`: a change to the README alone runs that test.
    #[test]
    fn impact_of_the_readme_runs_the_test_that_parses_its_samples() {
        let features = load_features().unwrap();
        let changed = names(&["README.md"]);
        let selection = select_features(&features, &changed, &deps::importers(), |_| true);

        let impact = impact_of("dev", &features, changed, selection);

        assert!(
            impact
                .test_filters
                .contains(&"shell::cli::commands::agent::".to_string()),
            "{:?}",
            impact.test_filters
        );
    }

    #[test]
    fn an_xtask_file_derives_the_filter_its_tests_are_listed_under() {
        assert_eq!(
            xtask_filter("xtask/src/ready.rs").as_deref(),
            Some("ready::")
        );
        assert_eq!(
            xtask_filter("xtask/src/main.rs").as_deref(),
            Some("main_tests::")
        );
        assert_eq!(
            xtask_filter("xtask/tests/github.rs").as_deref(),
            Some("github_")
        );
        assert_eq!(xtask_filter("src/console.rs"), None);
        assert_eq!(xtask_filter("xtask/tests/support/mod.rs"), None);
    }

    /// The filter of an integration test file is its name, so a test named
    /// otherwise would never be selected by `impact`.
    #[test]
    fn xtask_integration_tests_are_named_after_their_file() {
        let mut stray = Vec::new();
        for path in rust_files(&root().join("xtask/tests"))
            .into_iter()
            .filter(|path| path.parent() == Some(&root().join("xtask/tests")))
        {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let source = std::fs::read_to_string(&path).unwrap();
            let mut lines = source.lines();
            while let Some(line) = lines.next() {
                if line.trim() != "#[test]" {
                    continue;
                }
                let Some(test) = lines
                    .next()
                    .and_then(|next| next.trim().strip_prefix("fn "))
                    .and_then(|rest| rest.split('(').next())
                else {
                    continue;
                };
                if !test.starts_with(&format!("{name}_")) {
                    stray.push(format!("xtask/tests/{name}.rs: {test}"));
                }
            }
        }
        assert!(
            stray.is_empty(),
            "start each test with its file name:\n{}",
            stray.join("\n")
        );
    }

    /// A changed source file reaches the files that import it, through any
    /// number of them; a feature that declares it runs reached code is
    /// reached one step further, and no further.
    #[test]
    fn a_source_change_reaches_its_importers_and_one_declared_step() {
        let features: FeatureMap = [
            ("core".to_string(), feature("src/core.rs", &[])),
            ("command".to_string(), feature("src/command.rs", &[])),
            ("screen".to_string(), feature("src/screen.rs", &[])),
            // Imports nothing of core, but its scenarios run it.
            ("console".to_string(), feature("src/console.rs", &["core"])),
            // Runs console's code: two steps from the change.
            ("far".to_string(), feature("src/far.rs", &["console"])),
            ("cli-entry".to_string(), feature("src/dispatch.rs", &[])),
            // Imported by the dispatch table only.
            ("beyond".to_string(), feature("src/main.rs", &[])),
        ]
        .into();
        let importers: BTreeMap<String, BTreeSet<String>> = [
            ("src/core.rs".to_string(), names(&["src/command.rs"])),
            (
                "src/command.rs".to_string(),
                names(&["src/screen.rs", "src/dispatch.rs"]),
            ),
            ("src/dispatch.rs".to_string(), names(&["src/main.rs"])),
        ]
        .into();

        let selection = select_features(&features, &names(&["src/core.rs"]), &importers, |_| true);

        assert_eq!(selection.changed, names(&["core"]));
        assert_eq!(
            selection.dependents,
            names(&["cli-entry", "command", "console", "screen"]),
            "the walk stops at the hub, and `far` is two declared steps away"
        );
    }

    /// The case that made the file walk necessary: a shared helper of one
    /// feature does not select everything that feature's neighbours reach.
    #[test]
    fn impact_of_a_database_renderer_stays_near_the_database() {
        let features = load_features().unwrap();

        let selection = select_features(
            &features,
            &names(&["src/shell/cli/commands/db_render.rs"]),
            &deps::importers(),
            |_| true,
        );

        let selected: BTreeSet<&String> = selection.features().collect();
        assert!(selected.contains(&"database".to_string()), "{selected:?}");
        assert!(!selected.contains(&"oauth".to_string()), "{selected:?}");
        assert!(selected.len() < features.len() / 2, "{selected:?}");
    }
}
