//! `cargo xtask verify-matrix`: the inventory's enumerable values against the declared cases, written to target/agent/verification-matrix.{json,md}, and the gate that fails an unclassified value or a failed case.
//!
//! The matrix runs nothing. `verify` runs the scenarios -- Rust and TOML
//! alike -- and each writes one report under `target/agent/scenarios/`
//! (another layer writes its reports next to it, under
//! `target/agent/scenarios-<evidence>/`); this reads those reports, the case
//! files under `tests/cases/`, the inventory the binary prints, and the two
//! declaration files, and puts every row in exactly one status. A layer's
//! reports count only when its `verification-report-<layer>.json` was
//! written on a clean tree whose commit differs from the working tree in
//! nothing but `.agent/real/` (the evidence `verify-real` writes after it):
//! an older run is set aside as stale, so a real run from commits ago does
//! not verify today's code.
//!
//! What has to be classified is not a cross product. It is every value the
//! inventory enumerates, each as one coordinate `dimension=value`
//! ([`items`] holds the rules): a command, an enum config value, a value a
//! CLI option enumerates, a secret scheme, a client kind. A coordinate is
//! classified when a case's `combination` names it, or when
//! `tests/cases/not-applicable.toml` or `tests/cases/needs-human.toml`
//! declares it; a declaration names exactly one coordinate. The rows are the
//! cases and scenarios (PASS / FAIL / UNEXECUTED / UNVERIFIED_REAL), the
//! declarations (NOT_APPLICABLE / NEEDS_HUMAN) and the coordinates nothing
//! covers (UNCLASSIFIED). The gate fails on FAIL, on an UNCLASSIFIED
//! coordinate, and on a declaration that names other than one coordinate or
//! a value the inventory does not enumerate on a dimension it does. A case
//! may name such a value (a rejected one, or a refinement such as
//! `source=aws-profile`): those are listed, not gated.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::matrix_inputs::{
    CaseFile, Commit, Declaration, NeedsHumanFile, NotApplicableFile, Report, not_run_reason,
    read_cases, read_layer_commits, read_not_run, read_reports, read_toml,
};
use crate::matrix_report::write;
use crate::{Gate, inventory, root};

const USAGE: &str = "usage: cargo xtask verify-matrix [--json | --md] [--from FILE]";

pub(crate) const NOT_APPLICABLE_FILE: &str = "tests/cases/not-applicable.toml";
pub(crate) const NEEDS_HUMAN_FILE: &str = "tests/cases/needs-human.toml";

/// The dimension an enum config key is read as. A key not listed here is
/// its section and key joined (`data.sources.format`), which classifies just
/// as well; the names here are the ones cases already use.
const CONFIG_DIMENSIONS: [((&str, &str), &str); 6] = [
    (("[auth.*]", "kind"), "source"),
    (("[auth.*]", "grant_type"), "grant"),
    (("[db.*]", "engine"), "engine"),
    (("[db.*]", "tls"), "tls"),
    (("[db.*.tunnel]", "kind"), "tunnel"),
    (("[[data.*.sources]]", "format"), "format"),
];

/// The rules, as the report prints them.
pub(super) const RULES: [(&str, &str); 5] = [
    (
        "command",
        "every command that is not hidden: `command=<path>`, a subcommand's words joined by `-` (`command=config-check`)",
    ),
    (
        "<config enum>",
        "every value of an enum config key: `source=`, `grant=`, `engine=`, `tls=`, `tunnel=`, `format=`, or `<section>.<key>=` for a key without a name here",
    ),
    (
        "<command>.<option>",
        "every value a CLI option enumerates (`value_parser([..])`): `status.kind=data`",
    ),
    ("secret", "every secret reference scheme: `secret=op`"),
    ("client", "every JSON client kind: `client=data`"),
];

pub(crate) fn verify_matrix(args: &[String]) -> Result<(), String> {
    let request = inventory::parse_request(args, USAGE)?;
    let document = inventory::document(&request)?;
    let matrix = build_from_tree(&document)?;
    let (json_text, markdown) = write(&matrix)?;
    if request.json_to_stdout {
        println!("{json_text}");
    } else {
        print!("{markdown}");
    }
    if matrix.gate_problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "verify-matrix: {}\n{}",
            gate_summary(&matrix),
            matrix.gate_problems.join("\n")
        ))
    }
}

/// The matrix as a gate for `check` and `branch-check`: the inventory is
/// read from the built binary, the report is written either way.
pub(crate) fn gate() -> Gate {
    let result = inventory::run_binary()
        .and_then(|document| build_from_tree(&document))
        .and_then(|matrix| {
            write(&matrix)?;
            Ok(matrix)
        });
    match result {
        Ok(matrix) if matrix.gate_problems.is_empty() => Gate {
            name: "matrix".into(),
            ok: true,
            detail: gate_summary(&matrix),
        },
        Ok(matrix) => {
            eprintln!("{}", matrix.gate_problems.join("\n"));
            Gate {
                name: "matrix".into(),
                ok: false,
                detail: format!(
                    "{}; see target/agent/verification-matrix.md",
                    gate_summary(&matrix)
                ),
            }
        }
        Err(problem) => {
            eprintln!("{problem}");
            Gate {
                name: "matrix".into(),
                ok: false,
                detail: problem.lines().next().unwrap_or_default().to_string(),
            }
        }
    }
}

fn gate_summary(matrix: &Matrix) -> String {
    let count = |status: Status| matrix.counts.get(status.name()).copied().unwrap_or(0);
    format!(
        "PASS {}, FAIL {}, UNCLASSIFIED {}, UNEXECUTED {}, UNVERIFIED_REAL {}, NEEDS_HUMAN {}, NOT_APPLICABLE {}",
        count(Status::Pass),
        count(Status::Fail),
        count(Status::Unclassified),
        count(Status::Unexecuted),
        count(Status::UnverifiedReal),
        count(Status::NeedsHuman),
        count(Status::NotApplicable),
    )
}

// ---------------------------------------------------------------------------
// The inventory's coordinates

/// One value the inventory enumerates, as the coordinate that classifies it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct Item {
    dimension: String,
    value: String,
    /// Where it comes from, as a person reads it: `[auth.*] kind`.
    pub(super) subject: String,
}

impl Item {
    fn coordinate(&self) -> String {
        coordinate(&self.dimension, &self.value)
    }
}

pub(crate) fn array<'a>(value: &'a Value, path: &[&str]) -> &'a [Value] {
    let mut node = value;
    for key in path {
        node = &node[*key];
    }
    node.as_array().map(Vec::as_slice).unwrap_or_default()
}

pub(super) fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// The dimension an enum config key classifies under.
fn config_dimension(section: &str, key: &str) -> String {
    if let Some((_, dimension)) = CONFIG_DIMENSIONS
        .iter()
        .find(|((s, k), _)| *s == section && *k == key)
    {
        return (*dimension).to_string();
    }
    let section: String = section
        .trim_matches(|c| c == '[' || c == ']')
        .split('.')
        .filter(|part| *part != "*")
        .collect::<Vec<_>>()
        .join(".");
    format!("{section}.{key}")
}

/// Every coordinate the inventory enumerates, by the rules above.
pub(crate) fn items(document: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    for command in array(document, &["commands"]) {
        if command["hidden"] == true {
            continue;
        }
        let path = text(&command["path"]);
        items.push(Item {
            dimension: "command".into(),
            // A case names a value as one word, so `config check` is
            // `config-check`.
            value: path.replace(' ', "-"),
            subject: "command".into(),
        });
        for argument in array(command, &["arguments"]) {
            let values = array(argument, &["values"]);
            if values.is_empty() {
                continue;
            }
            let option = match argument["long"].as_str() {
                Some(long) => format!("--{long}"),
                None => text(&argument["id"]).to_string(),
            };
            let dimension = format!("{}.{}", path.replace(' ', "."), text(&argument["id"]));
            for value in values {
                items.push(Item {
                    dimension: dimension.clone(),
                    value: text(value).to_string(),
                    subject: format!("`{path} {option}`"),
                });
            }
        }
    }
    for key in array(document, &["config", "keys"]) {
        if key["kind"] != "enum" {
            continue;
        }
        let section = text(&key["section"]);
        let name = text(&key["key"]);
        let dimension = config_dimension(section, name);
        for value in array(key, &["values"]) {
            items.push(Item {
                dimension: dimension.clone(),
                value: text(value).to_string(),
                subject: format!("`{section} {name}`"),
            });
        }
    }
    for value in array(document, &["secret_schemes", "values"]) {
        items.push(Item {
            dimension: "secret".into(),
            value: text(value).to_string(),
            subject: "secret reference scheme".into(),
        });
    }
    for value in array(document, &["client_kinds", "values"]) {
        items.push(Item {
            dimension: "client".into(),
            value: text(value).to_string(),
            subject: "JSON client kind".into(),
        });
    }
    items.sort();
    items.dedup();
    items
}

// ---------------------------------------------------------------------------
// The matrix

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Status {
    Pass,
    Fail,
    Unclassified,
    Unexecuted,
    UnverifiedReal,
    NeedsHuman,
    NotApplicable,
}

impl Status {
    /// Every status, in the order rows are sorted and counted.
    const ALL: [Status; 7] = [
        Status::Pass,
        Status::Fail,
        Status::Unclassified,
        Status::Unexecuted,
        Status::UnverifiedReal,
        Status::NeedsHuman,
        Status::NotApplicable,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unclassified => "UNCLASSIFIED",
            Status::Unexecuted => "UNEXECUTED",
            Status::UnverifiedReal => "UNVERIFIED_REAL",
            Status::NeedsHuman => "NEEDS_HUMAN",
            Status::NotApplicable => "NOT_APPLICABLE",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct FailedCheck {
    pub(super) name: String,
    pub(super) detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Row {
    /// The case or scenario name; the coordinate for a declaration or an
    /// unclassified value.
    pub(super) id: String,
    pub(super) feature: String,
    /// The command, or the config key, the row is about.
    pub(super) subject: String,
    pub(super) combination: String,
    pub(super) expected: String,
    pub(super) observed: String,
    pub(super) evidence: String,
    pub(super) status: Status,
    pub(super) reproduce: String,
    pub(super) failed_checks: Vec<FailedCheck>,
    /// Scenarios that share a coordinate with a failed row.
    pub(super) related: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Matrix {
    pub(super) rows: Vec<Row>,
    pub(super) counts: BTreeMap<&'static str, usize>,
    pub(super) items: Vec<Item>,
    /// Dimensions cases declare that no inventory rule derives.
    pub(super) case_only_dimensions: Vec<String>,
    /// The case-only dimensions a single case names: listed, not gated.
    pub(super) possible_typos: Vec<String>,
    /// Values cases name on a dimension the inventory enumerates, that it
    /// does not enumerate: listed, not gated.
    pub(super) unenumerated_case_values: Vec<String>,
    pub(super) gate_problems: Vec<String>,
    pub(super) reports_read: usize,
    pub(super) cases_read: usize,
}

fn build_from_tree(document: &Value) -> Result<Matrix, String> {
    let root = root();
    let cases = read_cases(&root)?;
    let not_applicable: NotApplicableFile = read_toml(&root.join(NOT_APPLICABLE_FILE))?;
    let needs_human: NeedsHumanFile = read_toml(&root.join(NEEDS_HUMAN_FILE))?;
    let mut not_run = read_not_run();
    let reports = set_aside_stale_layers(
        read_reports()?,
        &mut not_run,
        &read_layer_commits()?,
        Commit::holds_the_current_code,
    );
    Ok(build(
        items(document),
        &cases,
        &not_applicable.not_applicable,
        &needs_human.needs_human,
        &reports,
        &not_run,
    ))
}

/// The reports of every layer that ran on the code the tree holds now
/// (`same_code`); a layer whose run was on other code, or that no
/// verification report dates, is dropped and its reason in `not_run` becomes
/// that it is stale. The fake reports are kept: `verify` rewrites them all on
/// every run.
fn set_aside_stale_layers(
    reports: Vec<Report>,
    not_run: &mut BTreeMap<String, String>,
    commits: &BTreeMap<String, Commit>,
    same_code: impl Fn(&Commit) -> bool,
) -> Vec<Report> {
    let mut kept = Vec::new();
    for report in reports {
        let layer = report.evidence.as_str();
        if layer == "fake" {
            kept.push(report);
            continue;
        }
        match commits.get(layer) {
            Some(commit) if same_code(commit) => kept.push(report),
            Some(commit) => {
                not_run.insert(
                    layer.to_string(),
                    format!("stale: its reports were written on {commit}, not on the code the tree holds now"),
                );
            }
            None => {
                not_run.insert(
                    layer.to_string(),
                    format!("stale: no verification-report-{layer}.json says which commit its reports were written on"),
                );
            }
        }
    }
    kept
}

/// A short reading of `[expect]`: the top-level keys with scalar values. A
/// value shaped like a credential (a bearer token the fake API was expected
/// to see) is masked, so the report scan reads the expectation as data and
/// not as a leak.
fn expected_cell(expect: &toml::Table) -> String {
    let mut parts = Vec::new();
    for (key, value) in expect {
        let rendered = match value {
            toml::Value::Table(table) => {
                let inner: Vec<String> = table
                    .iter()
                    .filter_map(|(k, v)| scalar(v).map(|v| format!("{k}={}", mask(&v))))
                    .collect();
                format!("{key}{{{}}}", inner.join(", "))
            }
            toml::Value::Array(values) => format!("{key}=[{}]", values.len()),
            other => format!("{key}={}", mask(&scalar(other).unwrap_or_default())),
        };
        parts.push(rendered);
    }
    if parts.is_empty() {
        "the common checks".to_string()
    } else {
        parts.join(", ")
    }
}

/// Every `Bearer <token>` in `value` replaced by `<bearer token>` (a token
/// ends at whitespace, a quote or a backslash, so one inside an echoed JSON
/// body is masked too); other text as it is. The scheme word goes with it:
/// the report scan reads `Bearer ` itself as a leak.
fn mask(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find("Bearer ") {
        let after = at + "Bearer ".len();
        out.push_str(&rest[..at]);
        out.push_str("<bearer token>");
        let token_end = rest[after..]
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\\')
            .map_or(rest.len(), |end| after + end);
        rest = &rest[token_end..];
    }
    out.push_str(rest);
    out
}

fn scalar(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn observed_cell(reports: &[&Report]) -> String {
    if reports.is_empty() {
        return "no report: not run".to_string();
    }
    let mut parts = Vec::new();
    for report in reports {
        let exits: Vec<String> = report
            .exit_codes
            .iter()
            .map(|code| code.map_or("none".to_string(), |c| c.to_string()))
            .collect();
        let mut part = format!("{}: exit {}", report.evidence, exits.join(","));
        if !report.failed_checks.is_empty() {
            let names: Vec<&str> = report
                .failed_checks
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            part.push_str(&format!("; {} failed: {}", names.len(), names.join(", ")));
        }
        parts.push(part);
    }
    parts.join("; ")
}

fn combination_cell(combination: &BTreeMap<String, String>) -> String {
    if combination.is_empty() {
        return "-".to_string();
    }
    combination
        .iter()
        .map(|(k, v)| coordinate(k, v))
        .collect::<Vec<_>>()
        .join(", ")
}

fn build(
    items: Vec<Item>,
    cases: &[CaseFile],
    not_applicable: &[Declaration],
    needs_human: &[Declaration],
    reports: &[Report],
    not_run: &BTreeMap<String, String>,
) -> Matrix {
    let enumerated = Enumerated::of(&items);
    let case_only = case_only_dimensions(cases, &enumerated);
    // A dimension one case alone names is more often a misspelling of one
    // the other cases share than a refinement only that case needs.
    let possible_typos = case_only
        .iter()
        .filter(|(_, uses)| **uses == 1)
        .map(|(dimension, _)| dimension.clone())
        .collect();
    let unenumerated_case_values: Vec<String> = cases
        .iter()
        .flat_map(|case| case.combination.iter().chain(&case.layer_combination))
        .filter_map(|(k, v)| enumerated.misses(k, v))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut problems = declaration_problems(&items, &enumerated, not_applicable, needs_human);
    let mut rows = case_rows(cases, reports, not_run, &mut problems);
    let cased: BTreeSet<String> = cases.iter().map(|case| case.id.clone()).collect();
    rows.extend(report_rows(reports, cased, &mut problems));
    rows.extend(declaration_rows(not_applicable, needs_human));
    let covered = covered(cases, not_applicable, needs_human);
    rows.extend(unclassified_rows(&items, &covered, &mut problems));
    rows.sort_by(|a, b| (a.status, &a.feature, &a.id).cmp(&(b.status, &b.feature, &b.id)));
    let counts = counts(&rows);
    Matrix {
        rows,
        counts,
        items,
        case_only_dimensions: case_only.into_keys().collect(),
        possible_typos,
        unenumerated_case_values,
        gate_problems: problems,
        reports_read: reports.len(),
        cases_read: cases.len(),
    }
}

/// `k=v`: a coordinate, as cases, declarations and the inventory spell it.
fn coordinate(k: &str, v: &str) -> String {
    format!("{k}={v}")
}

/// What the inventory enumerates: its dimensions and the coordinates on them.
struct Enumerated {
    dimensions: BTreeSet<String>,
    coordinates: BTreeSet<String>,
}

impl Enumerated {
    fn of(items: &[Item]) -> Self {
        Enumerated {
            dimensions: items.iter().map(|item| item.dimension.clone()).collect(),
            coordinates: items.iter().map(Item::coordinate).collect(),
        }
    }

    /// A value on a dimension the inventory enumerates, that it does not
    /// enumerate. A declaration says why a value is not verified, so one of
    /// a value that does not exist is a mistake; a case may name one on
    /// purpose (the value it shows is refused, or a refinement such as
    /// `source=aws-profile`), so those are listed.
    fn misses(&self, k: &str, v: &str) -> Option<String> {
        let coordinate = coordinate(k, v);
        (self.dimensions.contains(k) && !self.coordinates.contains(&coordinate))
            .then_some(coordinate)
    }
}

/// Which coordinates a case or a declaration covers: a layer's own
/// coordinates count, because the layer is the case run again elsewhere.
fn covered(
    cases: &[CaseFile],
    not_applicable: &[Declaration],
    needs_human: &[Declaration],
) -> BTreeSet<String> {
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for case in cases {
        for (k, v) in case.combination.iter().chain(&case.layer_combination) {
            covered.insert(coordinate(k, v));
        }
    }
    for declaration in not_applicable.iter().chain(needs_human) {
        for (k, v) in &declaration.combination {
            covered.insert(coordinate(k, v));
        }
    }
    covered
}

/// The dimensions cases name that the inventory does not derive, with how
/// many cases name each.
fn case_only_dimensions(cases: &[CaseFile], enumerated: &Enumerated) -> BTreeMap<String, usize> {
    let mut case_only: BTreeMap<String, usize> = BTreeMap::new();
    for case in cases {
        let mut dimensions: BTreeSet<&String> = case.combination.keys().collect();
        dimensions.extend(case.layer_combination.keys());
        for dimension in dimensions {
            if !enumerated.dimensions.contains(dimension) {
                *case_only.entry(dimension.clone()).or_default() += 1;
            }
        }
    }
    case_only
}

/// A declaration that names other than one coordinate, or a value the
/// inventory does not enumerate.
fn declaration_problems(
    items: &[Item],
    enumerated: &Enumerated,
    not_applicable: &[Declaration],
    needs_human: &[Declaration],
) -> Vec<String> {
    let mut problems = Vec::new();
    for (file, declarations) in [
        (NOT_APPLICABLE_FILE, not_applicable),
        (NEEDS_HUMAN_FILE, needs_human),
    ] {
        for declaration in declarations {
            let coordinates = combination_cell(&declaration.combination);
            if declaration.combination.len() != 1 {
                problems.push(format!(
                    "{file}: `{coordinates}` declares {} coordinates: a declaration names exactly one",
                    declaration.combination.len()
                ));
            }
            for (k, v) in &declaration.combination {
                if enumerated.misses(k, v).is_some() {
                    let values: Vec<&str> = items
                        .iter()
                        .filter(|item| &item.dimension == k)
                        .map(|item| item.value.as_str())
                        .collect();
                    problems.push(format!(
                        "{file}: `{k}={v}` is a value the inventory does not enumerate for `{k}` ({})",
                        values.join(", ")
                    ));
                }
            }
        }
    }
    problems
}

/// One row per case, with the scenarios that share a coordinate named next
/// to a failure.
fn case_rows(
    cases: &[CaseFile],
    reports: &[Report],
    not_run: &BTreeMap<String, String>,
    problems: &mut Vec<String>,
) -> Vec<Row> {
    let scenario_coordinates: Vec<(String, BTreeSet<String>)> = cases
        .iter()
        .map(|case| {
            (
                case.id.clone(),
                case.combination
                    .iter()
                    .map(|(k, v)| coordinate(k, v))
                    .collect(),
            )
        })
        .collect();
    let related_to = |id: &str, combination: &BTreeMap<String, String>| -> Vec<String> {
        let mine: BTreeSet<String> = combination.iter().map(|(k, v)| coordinate(k, v)).collect();
        scenario_coordinates
            .iter()
            .filter(|(other, theirs)| other != id && !theirs.is_disjoint(&mine))
            .map(|(other, _)| other.clone())
            .collect()
    };

    let mut rows = Vec::new();
    for case in cases {
        let mine: Vec<&Report> = reports
            .iter()
            .filter(|report| report.scenario == case.id)
            .collect();
        let has = |evidence: &str| mine.iter().any(|report| report.evidence == evidence);
        // A layer nobody ran (`[local]`, `[throwaway]`) is a case not executed
        // where it says it runs; the reason `verify --layer` left behind is
        // shown next to it.
        let layers_not_run: Vec<&str> = case
            .layers
            .iter()
            .map(String::as_str)
            .filter(|layer| !has(layer))
            .collect();
        // A layer report is the case run again elsewhere: without the fake
        // one the case itself did not run.
        let status = if mine.iter().any(|report| !report.passed) {
            Status::Fail
        } else if !has("fake") || !layers_not_run.is_empty() {
            Status::Unexecuted
        } else if case.declares_real && !has("real") {
            Status::UnverifiedReal
        } else {
            Status::Pass
        };
        let failed_checks: Vec<FailedCheck> = mine
            .iter()
            .flat_map(|report| report.failed_checks.iter())
            .map(failed_check)
            .collect();
        let evidence = if mine.is_empty() {
            "-".to_string()
        } else {
            let mut layers: Vec<&str> =
                mine.iter().map(|report| report.evidence.as_str()).collect();
            layers.sort();
            layers.dedup();
            layers.join(", ")
        };
        if status == Status::Fail {
            problems.push(failure(&case.id, &failed_checks));
        }
        let mut observed = observed_cell(&mine);
        if !mine.is_empty() && !has("fake") {
            observed.push_str("; fake: not run");
        }
        let mut reproduce = format!("cargo xtask verify {}", case.id);
        for layer in &case.layers {
            if layers_not_run.contains(&layer.as_str()) {
                let reason = not_run_reason(not_run, layer, &case.id);
                observed.push_str(&format!("; {layer}: not run ({reason})"));
            }
            reproduce.push_str(&format!(
                "; cargo xtask verify --layer {layer} {}{}",
                case.feature,
                if layer == "throwaway" {
                    " --profile <profile> --yes"
                } else {
                    ""
                }
            ));
        }
        // A `[real]` nobody ran keeps the fake evidence and says so: the
        // requirement `verify --layer real` found missing is what a person
        // prepares to make the row PASS on real evidence too.
        if case.declares_real {
            if !has("real") {
                let reason = not_run_reason(not_run, "real", &case.id);
                observed.push_str(&format!("; real: not run ({reason})"));
            }
            reproduce.push_str(&format!(
                "; cargo xtask verify --layer real {}",
                case.feature
            ));
        }
        rows.push(Row {
            id: case.id.clone(),
            feature: case.feature.clone(),
            subject: command_subject(&case.combination),
            combination: combination_cell(&case.combination),
            expected: expected_cell(&case.expect),
            observed,
            evidence,
            status,
            reproduce,
            failed_checks,
            related: if status == Status::Fail {
                related_to(&case.id, &case.combination)
            } else {
                Vec::new()
            },
        });
    }
    rows
}

/// One row per scenario a report names that no case file declares (the
/// Rust scenarios): its status is what the run said. `reported` holds the
/// cases, which have their rows already.
fn report_rows(
    reports: &[Report],
    mut reported: BTreeSet<String>,
    problems: &mut Vec<String>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for report in reports {
        if !reported.insert(report.scenario.clone()) {
            continue;
        }
        let status = if report.passed {
            Status::Pass
        } else {
            Status::Fail
        };
        let failed_checks: Vec<FailedCheck> =
            report.failed_checks.iter().map(failed_check).collect();
        if status == Status::Fail {
            problems.push(failure(&report.scenario, &failed_checks));
        }
        let combination = report.combination.clone().unwrap_or_default();
        rows.push(Row {
            id: report.scenario.clone(),
            feature: report.feature.clone(),
            subject: "scenario".into(),
            combination: combination_cell(&combination),
            expected: format!("tests/scenarios/{}.rs", report.feature.replace('-', "_")),
            observed: observed_cell(&[report]),
            evidence: report.evidence.clone(),
            status,
            reproduce: format!("cargo xtask verify {}", report.scenario),
            failed_checks,
            related: Vec::new(),
        });
    }
    rows
}

/// One row per declaration.
fn declaration_rows(not_applicable: &[Declaration], needs_human: &[Declaration]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (declarations, status) in [
        (not_applicable, Status::NotApplicable),
        (needs_human, Status::NeedsHuman),
    ] {
        for declaration in declarations {
            let expected = match &declaration.prepare {
                Some(prepare) => format!("{}; prepare: {prepare}", declaration.reason),
                None => declaration.reason.clone(),
            };
            rows.push(Row {
                id: combination_cell(&declaration.combination),
                feature: "-".into(),
                subject: command_subject(&declaration.combination),
                combination: combination_cell(&declaration.combination),
                expected,
                observed: "-".into(),
                evidence: "-".into(),
                status,
                reproduce: "-".into(),
                failed_checks: Vec::new(),
                related: Vec::new(),
            });
        }
    }
    rows
}

/// One row per coordinate nothing covers, each a gate problem.
fn unclassified_rows(
    items: &[Item],
    covered: &BTreeSet<String>,
    problems: &mut Vec<String>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for item in items {
        let coordinate = item.coordinate();
        if covered.contains(&coordinate) {
            continue;
        }
        problems.push(format!(
            "UNCLASSIFIED: {coordinate} ({} = {}): no case names it in `combination`, and neither {NOT_APPLICABLE_FILE} nor {NEEDS_HUMAN_FILE} declares it",
            item.subject.trim_matches('`'),
            item.value
        ));
        rows.push(Row {
            id: coordinate.clone(),
            feature: "-".into(),
            subject: item.subject.clone(),
            combination: coordinate,
            expected: "a case, or a reason it does not apply".into(),
            observed: "-".into(),
            evidence: "-".into(),
            status: Status::Unclassified,
            reproduce: "-".into(),
            failed_checks: Vec::new(),
            related: Vec::new(),
        });
    }
    rows
}

/// The rows per status, every status counted even when none has it.
fn counts(rows: &[Row]) -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> = Status::ALL
        .iter()
        .map(|status| (status.name(), 0))
        .collect();
    for row in rows {
        *counts.entry(row.status.name()).or_default() += 1;
    }
    counts
}

/// A check a report says failed, as a row lists it.
fn failed_check((name, detail): &(String, String)) -> FailedCheck {
    FailedCheck {
        name: name.clone(),
        detail: detail.clone(),
    }
}

/// The gate problem of a failed scenario: its failed checks and how to run
/// it again.
fn failure(id: &str, failed_checks: &[FailedCheck]) -> String {
    format!(
        "FAIL {id}: {}; reproduce: cargo xtask verify {id}",
        failed_checks
            .iter()
            .map(|check| format!("{}: {}", check.name, check.detail))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

/// The command a combination names, as the subject cell shows it.
fn command_subject(combination: &BTreeMap<String, String>) -> String {
    combination
        .get("command")
        .map(|command| format!("`{command}`"))
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
#[path = "matrix_tests.rs"]
mod tests;
