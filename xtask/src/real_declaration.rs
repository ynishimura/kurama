//! How a feature says it is verified against the real thing: the `real`
//! table of `.agent/features/<feature>.toml`, and what is wrong with one.
//! `verify_real.rs` runs what this declares.

use std::collections::BTreeSet;

use crate::{Feature, root};

/// The environments a probe may write to. A probe with `access = "throwaway"`
/// names one of these; anything else it could reach is read-only for it.
const THROWAWAY_ENVIRONMENTS: [&str; 1] = ["local-docker"];

/// How a feature is verified against the real thing.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum Real {
    Probe(Probe),
    /// The `[real]` cases under `tests/cases/<feature>/` are the probe:
    /// `cargo xtask verify-real <feature>` runs `verify --layer real
    /// <feature>` and writes the evidence from their reports. The text says
    /// which services they reach.
    Cases {
        cases: String,
    },
    Pending {
        pending: String,
    },
    NotApplicable {
        not_applicable: String,
    },
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Probe {
    /// The command, run from the repository root; `cargo` is xtask's cargo.
    pub(crate) command: Vec<String>,
    /// The file whose tests the probe runs; each of them is a contract.
    pub(crate) source: String,
    /// Which real environment answers, as a stable identifier.
    pub(crate) environment: String,
    pub(crate) access: Access,
    /// Variables the probe needs; without them it does not run.
    pub(crate) requires: Vec<String>,
    /// Fixture files the probe or its mock side read; the evidence records
    /// their hashes, so changing one makes the evidence stale.
    #[serde(default)]
    pub(crate) fixtures: Vec<String>,
    pub(crate) contracts: Vec<Contract>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Access {
    ReadOnly,
    Throwaway,
}

/// One test of the probe and the mock scenarios that pin what it observes.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Contract {
    pub(crate) real: String,
    pub(crate) mock: Vec<String>,
    /// Why no mock scenario can pin it, when `mock` is empty.
    #[serde(default)]
    pub(crate) note: Option<String>,
    /// Variables this test needs beyond the probe's own.
    #[serde(default)]
    pub(crate) requires: Vec<String>,
}

/// What is wrong with how a feature declares its real verification.
pub(crate) fn declaration_problems(
    name: &str,
    feature: &Feature,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let probe = match &feature.real {
        None => {
            return vec![format!(
                "{name}: declare `real` in .agent/features/{name}.toml: a probe, `pending` or `not_applicable`"
            )];
        }
        Some(Real::Pending { pending: why })
        | Some(Real::NotApplicable {
            not_applicable: why,
        }) => {
            return if why.trim().is_empty() {
                vec![format!("{name}: say why under `real`")]
            } else {
                vec![]
            };
        }
        Some(Real::Cases { cases }) => {
            let mut problems = Vec::new();
            if cases.trim().is_empty() {
                problems.push(format!(
                    "{name}: say which services the `[real]` cases reach under `real.cases`"
                ));
            }
            if !exists(&format!("tests/cases/{name}")) {
                problems.push(format!(
                    "{name}: `real.cases` names tests/cases/{name}/, which does not exist"
                ));
            }
            return problems;
        }
        Some(Real::Probe(probe)) => probe,
    };
    let mut problems = Vec::new();
    if probe.command.is_empty() {
        problems.push(format!("{name}: the probe names no command"));
    }
    if probe.requires.is_empty() {
        problems.push(format!(
            "{name}: the probe requires no variable, so nothing keeps it from running unasked"
        ));
    }
    if probe.access == Access::Throwaway
        && !THROWAWAY_ENVIRONMENTS.contains(&probe.environment.as_str())
    {
        problems.push(format!(
            "{name}: `{}` is not a throwaway environment; a probe may write only to {THROWAWAY_ENVIRONMENTS:?}",
            probe.environment
        ));
    }
    for path in std::iter::once(&probe.source).chain(&probe.fixtures) {
        if !exists(path) {
            problems.push(format!("{name}: {path} does not exist"));
        }
    }
    let mut seen = BTreeSet::new();
    for contract in &probe.contracts {
        if !seen.insert(contract.real.as_str()) {
            problems.push(format!("{name}: {} is a contract twice", contract.real));
        }
        if contract.mock.is_empty() && contract.note.as_deref().is_none_or(|n| n.trim().is_empty())
        {
            problems.push(format!(
                "{name}: {} names no mock scenario and no note saying why",
                contract.real
            ));
        }
        for mock in &contract.mock {
            if !feature.scenarios.contains(mock) {
                problems.push(format!(
                    "{name}: {} pairs with {mock}, which is not a scenario of {name}",
                    contract.real
                ));
            }
        }
    }
    problems
}

/// Every problem with a feature's declaration, read against the checkout:
/// the declaration itself, and a probe's contracts against its source.
pub(crate) fn all_problems(name: &str, feature: &Feature) -> Vec<String> {
    let mut problems = declaration_problems(name, feature, &|path| root().join(path).exists());
    if let Some(real @ Real::Probe(probe)) = &feature.real {
        let source = std::fs::read_to_string(root().join(&probe.source)).unwrap_or_default();
        let tests = crate::architecture_audit::test_names(&source);
        problems.extend(contract_coverage(name, &tests, real));
    }
    problems
}

/// The tests of the probe's source that no contract names, and contracts no
/// test holds.
pub(crate) fn contract_coverage(
    name: &str,
    probe_tests: &BTreeSet<String>,
    real: &Real,
) -> Vec<String> {
    let Real::Probe(probe) = real else {
        return vec![];
    };
    let named: BTreeSet<String> = probe.contracts.iter().map(|c| c.real.clone()).collect();
    let mut problems: Vec<String> = probe_tests
        .difference(&named)
        .map(|test| format!("{name}: {} test {test} is no contract", probe.source))
        .collect();
    problems.extend(
        named
            .difference(probe_tests)
            .map(|test| format!("{name}: contract {test} is not a test of {}", probe.source)),
    );
    problems
}
