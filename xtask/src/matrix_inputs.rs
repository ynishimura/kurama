//! What `verify-matrix` reads from the tree: the case files under tests/cases/, the two declaration files next to them, the scenario reports under target/agent/ (or `CARGO_TARGET_DIR`), the commit each layer ran on and why a layer did not run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::cases::case_files;
use crate::matrix::{array, text};
use crate::{agent_dir, json_files};

// ---------------------------------------------------------------------------
// What the tree declares

/// A case file, as much of it as the matrix reads.
#[derive(Debug, Clone)]
pub(crate) struct CaseFile {
    pub(crate) id: String,
    pub(crate) feature: String,
    pub(crate) combination: BTreeMap<String, String>,
    /// The coordinates `[local]`, `[throwaway]` and `[real]` add: covered by
    /// the case too.
    pub(crate) layer_combination: BTreeMap<String, String>,
    pub(crate) expect: toml::Table,
    /// The layers the case declares besides the fake one, in `LAYERS` order.
    pub(crate) layers: Vec<String>,
    /// The stacks its `[throwaway]` names.
    pub(crate) stacks: Vec<String>,
    pub(crate) declares_real: bool,
    /// What `[real]` needs from the person's environment (`kind:name`).
    pub(crate) real_requires: Vec<String>,
}

/// The layers `cargo xtask verify --layer` runs, in the order the matrix
/// names them; `real` is `verify-real`'s and is read separately.
pub(crate) const LAYERS: [&str; 2] = ["local", "throwaway"];

impl CaseFile {
    pub(crate) fn declares(&self, layer: &str) -> bool {
        self.layers.iter().any(|name| name == layer)
    }
}

pub(crate) fn read_cases(root: &Path) -> Result<Vec<CaseFile>, String> {
    let mut cases = Vec::new();
    for file in case_files(root)?.iter().map(|file| root.join(file)) {
        let content =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let table: toml::Table =
            toml::from_str(&content).map_err(|e| format!("{}: {e}", file.display()))?;
        let string = |key: &str| {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("{}: `{key}` is not a string", file.display()))
        };
        let coordinates = |table: &toml::Table| match table.get("combination") {
            Some(toml::Value::Table(values)) => values
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect(),
            _ => BTreeMap::new(),
        };
        let layer = |name: &str| table.get(name).and_then(toml::Value::as_table);
        let mut layer_combination = BTreeMap::new();
        for layer in LAYERS.into_iter().chain(["real"]).filter_map(layer) {
            layer_combination.extend(coordinates(layer));
        }
        let stacks = layer("throwaway")
            .and_then(|layer| layer.get("stacks"))
            .and_then(toml::Value::as_array)
            .map(|stacks| {
                stacks
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        cases.push(CaseFile {
            id: string("id")?,
            feature: string("feature")?,
            combination: coordinates(&table),
            layer_combination,
            expect: table
                .get("expect")
                .and_then(toml::Value::as_table)
                .cloned()
                .unwrap_or_default(),
            layers: LAYERS
                .into_iter()
                .filter(|name| layer(name).is_some())
                .map(str::to_string)
                .collect(),
            stacks,
            declares_real: layer("real").is_some(),
            real_requires: layer("real")
                .and_then(|real| real.get("requires"))
                .and_then(toml::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Ok(cases)
}

/// One entry of `not-applicable.toml` or `needs-human.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Declaration {
    pub(crate) reason: String,
    /// What a person has to prepare (needs-human only).
    #[serde(default)]
    pub(crate) prepare: Option<String>,
    pub(crate) combination: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NotApplicableFile {
    #[serde(default)]
    pub(crate) not_applicable: Vec<Declaration>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NeedsHumanFile {
    #[serde(default)]
    pub(crate) needs_human: Vec<Declaration>,
}

/// Reads a declaration file; one that does not exist declares nothing.
pub(crate) fn read_toml<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
) -> Result<T, String> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))
}

/// Why a layer wrote no reports: `target/agent/scenarios-<layer>/not-run.txt`,
/// which `verify --layer` writes instead of reports when it could not run
/// (Docker down, approval needed), by the layer's name. The first line is
/// the reason a layer stopped on; the real layer writes one line per case
/// it skipped (`<id>: ...`), and `not_run_reason` reads the right one back.
pub(crate) fn read_not_run() -> BTreeMap<String, String> {
    std::fs::read_dir(agent_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let layer = name.strip_prefix("scenarios-")?.to_string();
            let reason = std::fs::read_to_string(entry.path().join("not-run.txt")).ok()?;
            Some((layer, reason.trim().to_string()))
        })
        .collect()
}

/// Why `id` has no report on `layer`: its own line of the layer's
/// `not-run.txt` when there is one, else the whole file (a reason that
/// stopped the layer before any case), else that the layer was not run.
pub(crate) fn not_run_reason(not_run: &BTreeMap<String, String>, layer: &str, id: &str) -> String {
    match not_run.get(layer) {
        Some(text) => text
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{id}: ")))
            .or_else(|| text.lines().next())
            .unwrap_or_default()
            .to_string(),
        None => format!("no `verify --layer {layer}` run"),
    }
}

// ---------------------------------------------------------------------------
// What the runs observed

/// The commit a run's reports were written on: `git` of its verification
/// report.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Commit {
    pub(crate) head: String,
    pub(crate) dirty: bool,
}

/// What a real run leaves in the tree after its layer report: `verify-real`
/// writes (and a person commits) its evidence there, which changes no code.
const EVIDENCE_PATHSPEC: &str = ":(exclude).agent/real";

impl Commit {
    /// Whether the tree holds the code this run was on: no difference, tracked
    /// or untracked, between `head` and the working tree outside
    /// `.agent/real/`. A run on a dirty tree never does: what it held is not
    /// recoverable from `head`.
    pub(crate) fn holds_the_current_code(&self) -> bool {
        !self.dirty
            && crate::git(&["diff", "--quiet", &self.head, "--", ".", EVIDENCE_PATHSPEC]).is_ok()
            && crate::git(&[
                "ls-files",
                "--others",
                "--exclude-standard",
                "--",
                ".",
                EVIDENCE_PATHSPEC,
            ])
            .is_ok_and(|untracked| untracked.trim().is_empty())
    }
}

/// Whether the tree has changes outside `.agent/real/`: what a
/// verification report records as `dirty`.
pub(crate) fn tree_is_dirty() -> bool {
    !crate::git(&["status", "--porcelain", "--", ".", EVIDENCE_PATHSPEC])
        .unwrap_or_default()
        .trim()
        .is_empty()
}

impl std::fmt::Display for Commit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.head)?;
        if self.dirty {
            write!(f, " (dirty)")?;
        }
        Ok(())
    }
}

/// The commit of each layer's last run, by the layer's name:
/// `git` of `target/agent/verification-report-<layer>.json`.
pub(crate) fn read_layer_commits() -> Result<BTreeMap<String, Commit>, String> {
    #[derive(Deserialize)]
    struct Written {
        git: Commit,
    }
    let mut commits = BTreeMap::new();
    for entry in std::fs::read_dir(agent_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(layer) = name
            .strip_prefix("verification-report-")
            .and_then(|rest| rest.strip_suffix(".json"))
        else {
            continue;
        };
        let path = entry.path();
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let written: Written =
            serde_json::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))?;
        commits.insert(layer.to_string(), written.git);
    }
    Ok(commits)
}

/// One scenario report, as much of it as the matrix reads.
#[derive(Debug, Clone)]
pub(crate) struct Report {
    pub(crate) scenario: String,
    pub(crate) feature: String,
    pub(crate) passed: bool,
    pub(crate) evidence: String,
    pub(crate) combination: Option<BTreeMap<String, String>>,
    pub(crate) exit_codes: Vec<Option<i64>>,
    pub(crate) failed_checks: Vec<(String, String)>,
}

/// Every report under `target/agent/scenarios/` and `scenarios-<layer>/`.
pub(crate) fn read_reports() -> Result<Vec<Report>, String> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(agent_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path.file_name().is_some_and(|name| {
                    name == "scenarios" || name.to_string_lossy().starts_with("scenarios-")
                })
        })
        .collect();
    dirs.sort();
    let mut reports = Vec::new();
    for dir in dirs {
        let files = json_files(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for file in files {
            let content =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let value: Value =
                serde_json::from_str(&content).map_err(|e| format!("{}: {e}", file.display()))?;
            let combination = value["combination"].as_object().map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), text(v).to_string()))
                    .collect()
            });
            reports.push(Report {
                scenario: text(&value["scenario"]).to_string(),
                feature: text(&value["feature"]).to_string(),
                passed: value["passed"] == true,
                evidence: value["evidence"].as_str().unwrap_or("fake").to_string(),
                combination,
                exit_codes: array(&value, &["observed", "runs"])
                    .iter()
                    .map(|run| run["exit_code"].as_i64())
                    .collect(),
                failed_checks: array(&value, &["checks"])
                    .iter()
                    .filter(|check| check["ok"] != true)
                    .map(|check| {
                        (
                            text(&check["name"]).to_string(),
                            text(&check["detail"]).to_string(),
                        )
                    })
                    .collect(),
            });
        }
    }
    Ok(reports)
}
