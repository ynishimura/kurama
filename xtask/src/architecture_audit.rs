//! `cargo xtask architecture-audit`: the architecture rules as a registry
//! (`tests/architecture/rules.toml`), held to the tests that detect them and
//! the list that publishes them, reported rule by rule.
//!
//! "41 tests passed" says the tree is clean today. It does not say which rule
//! a test holds, whether a rule has a test at all, or whether that test would
//! fail on a violation. The registry names, for every rule, its detectors,
//! its documents, its allowlists and its fixtures; this command resolves each
//! name against the source and refuses one that points nowhere, a test no
//! rule names, and a rule only one of the registry and the list in
//! `tests/architecture/main.rs` carries. The report counts registrations and
//! says what it does not measure, rather than turning a pass count into a
//! coverage figure.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::architecture_fixtures::{self, Expect, Fixture, Outcome};
use crate::{root, rust_files};

const USAGE: &str = "\
usage: cargo xtask architecture-audit [--run-fixtures]

Reads tests/architecture/rules.toml, resolves every check, fixture, exemption
and document it names, and writes target/agent/architecture-report.{json,md}.
Fails when a name resolves to nothing, when a #[test] under tests/architecture/
is named by no rule or by two, when an id repeats, or when the registry and the
list in tests/architecture/main.rs disagree about which rules exist.

--run-fixtures also runs every registered fixture and reports the violation
detection rate and the allowed-code pass rate; a fixture that missed, refused,
failed for another reason or did not run fails the audit.
";

pub(crate) const REGISTRY: &str = "tests/architecture/rules.toml";
const RULE_LIST: &str = "tests/architecture/main.rs";
const ARCHITECTURE_DIR: &str = "tests/architecture";

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    rule: Vec<Rule>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    summary: String,
    #[serde(default)]
    method: Method,
    limits: Option<String>,
    checks: Vec<String>,
    #[serde(default)]
    docs: Vec<String>,
    #[serde(default)]
    exemptions: Vec<String>,
    #[serde(default)]
    violating_fixtures: Vec<String>,
    #[serde(default)]
    passing_fixtures: Vec<String>,
}

/// How a rule's checks read the source.
#[derive(Debug, Default, Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Method {
    #[default]
    Text,
    Syntax,
}

/// The audit of one rule: what resolved, counted.
#[derive(Debug, serde::Serialize)]
struct RuleReport {
    id: String,
    summary: String,
    method: Method,
    limits: Option<String>,
    checks: usize,
    docs: usize,
    exemption_lists: usize,
    exemption_entries: usize,
    violating_fixtures: usize,
    passing_fixtures: usize,
}

/// A registered count out of a total, reported as both so a rate never
/// stands without what it was taken over.
#[derive(Debug, PartialEq, serde::Serialize)]
struct Share {
    registered: usize,
    of: usize,
}

/// A rate the fixtures measure: how many did what they expected, of how
/// many. Until they run it is written down as not measured, with why,
/// instead of as zero or as a pass.
#[derive(Debug, PartialEq, serde::Serialize)]
struct Rate {
    measured: bool,
    expected: usize,
    of: usize,
    reason: Option<&'static str>,
}

/// A registered fixture and, once the fixtures ran, what it did.
#[derive(Debug, serde::Serialize)]
struct FixtureResult {
    #[serde(flatten)]
    fixture: Fixture,
    outcome: Option<Outcome>,
}

#[derive(Debug, serde::Serialize)]
struct Audit {
    rules: Vec<RuleReport>,
    /// `#[test]` functions under `tests/architecture/`. A count of tests, not
    /// of anything they detect.
    architecture_tests: usize,
    detectors: Share,
    syntax_rules: Share,
    passing_fixtures: Share,
    violating_fixtures: Share,
    detection_rate: Rate,
    passing_rate: Rate,
    fixtures: Vec<FixtureResult>,
    exemption_lists: usize,
    exemption_entries: usize,
    not_measured: Vec<&'static str>,
    problems: Vec<String>,
}

const NOT_MEASURED: [&str; 4] = [
    "whether a detector fails on a violation: only a violating fixture that runs shows it",
    "whether a detector passes allowed code: only a passing fixture that runs shows it",
    "whether an exemption's reason is right: ARCH-040 requires one and a person judges it",
    "what a text rule's scan misses: it reads the shapes it was written for, and the syntax rules list what they cannot see",
];

pub fn architecture_audit(args: &[String]) -> Result<(), String> {
    let run_fixtures = match args {
        [] => false,
        [flag] if flag == "--run-fixtures" => true,
        [flag] if flag == "--help" || flag == "-h" => {
            print!("{USAGE}");
            return Ok(());
        }
        _ => return Err(USAGE.trim_end().to_string()),
    };
    let mut audit = run()?;
    if run_fixtures {
        let fixtures: Vec<Fixture> = audit.fixtures.iter().map(|f| f.fixture.clone()).collect();
        apply_outcomes(&mut audit, architecture_fixtures::run(&fixtures));
    }
    let markdown = write_reports(&audit)?;
    print!("{markdown}");
    if audit.problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the architecture registry disagrees with the tree in {} place(s); see target/agent/architecture-report.md",
            audit.problems.len()
        ))
    }
}

/// The audit of the checkout, for the gates: `Ok` with a one-line summary, or
/// `Err` with the problems, one per line.
pub(crate) fn gate() -> Result<String, String> {
    let audit = run()?;
    write_reports(&audit)?;
    gate_result(&audit)
}

/// The gate `check` runs after the tests: the audit with its fixtures run.
pub(crate) fn gate_with_fixtures() -> Result<String, String> {
    let mut audit = run()?;
    let fixtures: Vec<Fixture> = audit.fixtures.iter().map(|f| f.fixture.clone()).collect();
    apply_outcomes(&mut audit, architecture_fixtures::run(&fixtures));
    write_reports(&audit)?;
    gate_result(&audit).map(|summary| {
        format!(
            "{summary}; {} / {} violations detected, {} / {} allowed code passed",
            audit.detection_rate.expected,
            audit.detection_rate.of,
            audit.passing_rate.expected,
            audit.passing_rate.of
        )
    })
}

/// Writes what each fixture did into the audit: the two rates, and a problem
/// for every fixture that did not do what it expected.
fn apply_outcomes(audit: &mut Audit, outcomes: Vec<Outcome>) {
    for (result, outcome) in audit.fixtures.iter_mut().zip(outcomes) {
        if !outcome.is_expected() {
            audit.problems.push(format!(
                "{}: {} {}",
                result.fixture.rule,
                result.fixture.reference,
                match &outcome {
                    Outcome::Missed => "let a violation through".to_string(),
                    Outcome::Refused => "refused allowed code".to_string(),
                    Outcome::OtherFailure(why) => format!("failed for another reason: {why}"),
                    Outcome::NotRun(why) => format!("did not run: {why}"),
                    Outcome::Detected | Outcome::Allowed => unreachable!(),
                }
            ));
        }
        result.outcome = Some(outcome);
    }
    let rate = |expects: Expect| {
        let of_kind: Vec<&FixtureResult> = audit
            .fixtures
            .iter()
            .filter(|result| result.fixture.expects == expects)
            .collect();
        Rate {
            measured: true,
            expected: of_kind
                .iter()
                .filter(|result| result.outcome.as_ref().is_some_and(Outcome::is_expected))
                .count(),
            of: of_kind.len(),
            reason: None,
        }
    };
    audit.detection_rate = rate(Expect::Refused);
    audit.passing_rate = rate(Expect::Allowed);
    audit
        .not_measured
        .retain(|item| !item.contains("fixture that runs"));
}

fn gate_result(audit: &Audit) -> Result<String, String> {
    if audit.problems.is_empty() {
        Ok(format!(
            "{} rules, {} with a detector",
            audit.rules.len(),
            audit.detectors.registered
        ))
    } else {
        Err(audit.problems.join("\n"))
    }
}

fn run() -> Result<Audit, String> {
    let root = root();
    let registry =
        std::fs::read_to_string(root.join(REGISTRY)).map_err(|e| format!("{REGISTRY}: {e}"))?;
    let architecture_files: Vec<String> = rust_files(&root.join(ARCHITECTURE_DIR))
        .iter()
        .map(|path| relative(&root, path))
        .collect();
    let read = |path: &str| std::fs::read_to_string(root.join(path)).ok();
    audit(&registry, &architecture_files, &read)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The audit itself, over whatever `read` answers: the checkout for the
/// command, a map of strings for the tests.
fn audit(
    registry: &str,
    architecture_files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Result<Audit, String> {
    let registry: Registry = toml::from_str(registry).map_err(|e| format!("{REGISTRY}: {e}"))?;
    let mut problems = Vec::new();

    let mut seen = BTreeSet::new();
    for rule in &registry.rule {
        if !is_rule_id(&rule.id) {
            problems.push(format!("{}: an id is ARCH- and three digits", rule.id));
        }
        if !seen.insert(rule.id.as_str()) {
            problems.push(format!("{}: the id is used by two rules", rule.id));
        }
    }

    let listed = match read(RULE_LIST) {
        Some(text) => listed_ids(&text),
        None => {
            problems.push(format!("{RULE_LIST}: not readable"));
            BTreeSet::new()
        }
    };
    for id in &seen {
        if !listed.contains(*id) {
            problems.push(format!(
                "{id}: in the registry but not in the list in {RULE_LIST}"
            ));
        }
    }
    for id in &listed {
        if !seen.contains(id.as_str()) {
            problems.push(format!(
                "{id}: in the list in {RULE_LIST} but not in the registry"
            ));
        }
    }

    let mut tests_by_file: BTreeMap<String, Option<BTreeSet<String>>> = BTreeMap::new();
    let mut tests_of = |file: &str| -> Option<BTreeSet<String>> {
        tests_by_file
            .entry(file.to_string())
            .or_insert_with(|| read(file).map(|text| test_names(&text)))
            .clone()
    };

    let mut owner: BTreeMap<String, &str> = BTreeMap::new();
    let mut rules = Vec::new();
    let mut fixtures = Vec::new();
    for rule in &registry.rule {
        if rule.checks.is_empty() {
            problems.push(format!("{}: names no check", rule.id));
        }
        match (rule.method, &rule.limits) {
            (Method::Syntax, None) => {
                problems.push(format!("{}: a syntax rule states its limits", rule.id))
            }
            (Method::Text, Some(_)) => {
                problems.push(format!("{}: limits belong to a syntax rule", rule.id))
            }
            _ => {}
        }
        let mut resolve_tests = |field: &str, references: &[String], problems: &mut Vec<String>| {
            let mut resolved = 0;
            for reference in references {
                match reference.split_once("::") {
                    Some((file, name)) => match tests_of(file) {
                        Some(names) if names.contains(name) => resolved += 1,
                        Some(_) => problems.push(format!(
                            "{}: {field} names `{reference}`, but {file} has no #[test] fn {name}",
                            rule.id
                        )),
                        None => problems.push(format!(
                            "{}: {field} names `{reference}`, but {file} is not readable",
                            rule.id
                        )),
                    },
                    None => problems.push(format!(
                        "{}: {field} entry `{reference}` is not `<file>::<test>`",
                        rule.id
                    )),
                }
            }
            resolved
        };
        let checks = resolve_tests("checks", &rule.checks, &mut problems);
        for (references, expects) in [
            (&rule.violating_fixtures, Expect::Refused),
            (&rule.passing_fixtures, Expect::Allowed),
        ] {
            fixtures.extend(references.iter().map(|reference| FixtureResult {
                fixture: Fixture {
                    rule: rule.id.clone(),
                    reference: reference.clone(),
                    expects,
                },
                outcome: None,
            }));
        }
        let violating = resolve_tests(
            "violating_fixtures",
            &rule.violating_fixtures,
            &mut problems,
        );
        let passing = resolve_tests("passing_fixtures", &rule.passing_fixtures, &mut problems);

        let named = rule
            .checks
            .iter()
            .chain(&rule.violating_fixtures)
            .chain(&rule.passing_fixtures);
        for reference in named {
            if let Some(previous) = owner.insert(reference.clone(), &rule.id) {
                problems.push(format!(
                    "`{reference}` is named by both {previous} and {}",
                    rule.id
                ));
            }
        }

        let mut docs = 0;
        for doc in &rule.docs {
            if read(doc).is_some() {
                docs += 1;
            } else {
                problems.push(format!(
                    "{}: docs names {doc}, which is not readable",
                    rule.id
                ));
            }
        }

        let mut exemption_entries = 0;
        for reference in &rule.exemptions {
            let entries = reference
                .split_once("::")
                .and_then(|(file, name)| read(file).and_then(|text| const_entries(&text, name)));
            match entries {
                Some(count) => exemption_entries += count,
                None => problems.push(format!(
                    "{}: exemptions names `{reference}`, which is not a const list in that file",
                    rule.id
                )),
            }
        }

        rules.push(RuleReport {
            id: rule.id.clone(),
            summary: rule.summary.clone(),
            method: rule.method,
            limits: rule.limits.clone(),
            checks,
            docs,
            exemption_lists: rule.exemptions.len(),
            exemption_entries,
            violating_fixtures: violating,
            passing_fixtures: passing,
        });
    }

    let mut architecture_tests = 0;
    for file in architecture_files {
        let Some(names) = tests_of(file) else {
            problems.push(format!("{file}: not readable"));
            continue;
        };
        for name in names {
            architecture_tests += 1;
            let reference = format!("{file}::{name}");
            if !owner.contains_key(&reference) {
                problems.push(format!("`{reference}` is a #[test] that no rule names"));
            }
        }
    }

    let total = rules.len();
    let count = |has: fn(&RuleReport) -> bool| rules.iter().filter(|rule| has(rule)).count();
    let detectors = Share {
        registered: count(|rule| rule.checks > 0),
        of: total,
    };
    let syntax_rules = Share {
        registered: count(|rule| rule.method == Method::Syntax),
        of: total,
    };
    let passing_fixtures = Share {
        registered: count(|rule| rule.passing_fixtures > 0),
        of: total,
    };
    let violating_fixtures = Share {
        registered: count(|rule| rule.violating_fixtures > 0),
        of: total,
    };
    let violating_count = fixtures
        .iter()
        .filter(|result: &&FixtureResult| result.fixture.expects == Expect::Refused)
        .count();
    let passing_count = fixtures.len() - violating_count;
    let exemption_lists = rules.iter().map(|rule| rule.exemption_lists).sum();
    let exemption_entries = rules.iter().map(|rule| rule.exemption_entries).sum();
    Ok(Audit {
        rules,
        architecture_tests,
        detectors,
        syntax_rules,
        passing_fixtures,
        violating_fixtures,
        detection_rate: Rate {
            measured: false,
            expected: 0,
            of: violating_count,
            reason: Some("run with --run-fixtures to run the violating fixtures"),
        },
        passing_rate: Rate {
            measured: false,
            expected: 0,
            of: passing_count,
            reason: Some("run with --run-fixtures to run the passing fixtures"),
        },
        fixtures,
        exemption_lists,
        exemption_entries,
        not_measured: NOT_MEASURED.to_vec(),
        problems,
    })
}

fn is_rule_id(id: &str) -> bool {
    id.strip_prefix("ARCH-")
        .is_some_and(|digits| digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The ids the `//!` list of `tests/architecture/main.rs` names, one per line
/// that starts with one.
fn listed_ids(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("//!"))
        .filter_map(|line| line.trim_start().get(..8))
        .filter(|id| is_rule_id(id))
        .map(str::to_string)
        .collect()
}

/// The names of the `#[test]` and `#[tokio::test]` functions in a source file.
pub(crate) fn test_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut after_test_attribute = false;
    for line in text.lines().map(str::trim) {
        if line == "#[test]" || line.starts_with("#[tokio::test") {
            after_test_attribute = true;
        } else if after_test_attribute && !line.starts_with("#[") && !line.starts_with("//") {
            let signature = line.strip_prefix("async ").unwrap_or(line);
            if let Some(name) = signature
                .strip_prefix("fn ")
                .and_then(|rest| rest.split(['(', '<']).next())
            {
                names.insert(name.to_string());
            }
            after_test_attribute = false;
        }
    }
    names
}

/// How many entries the const list `name` holds: the top-level elements of
/// the first `[...]` after its `=`, strings skipped.
fn const_entries(text: &str, name: &str) -> Option<usize> {
    let declaration = text.find(&format!("const {name}:"))?;
    let rest = &text[declaration..];
    let initializer = &rest[rest.find('=')?..];
    let body = &initializer[initializer.find('[')? + 1..];
    let (mut depth, mut entries, mut pending) = (0usize, 0usize, false);
    let mut in_string = false;
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                pending = true;
            }
            '[' | '(' => {
                depth += 1;
                pending = true;
            }
            ']' | ')' if depth == 0 => return Some(entries + usize::from(pending)),
            ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                entries += usize::from(pending);
                pending = false;
            }
            c if !c.is_whitespace() => pending = true,
            _ => {}
        }
    }
    None
}

fn write_reports(audit: &Audit) -> Result<String, String> {
    let dir = crate::agent_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let json = serde_json::json!({
        "schema_version": 1,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "registry": REGISTRY,
        "passed": audit.problems.is_empty(),
        "audit": audit,
    });
    std::fs::write(
        dir.join("architecture-report.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let markdown = render_markdown(audit);
    std::fs::write(dir.join("architecture-report.md"), &markdown).map_err(|e| e.to_string())?;
    Ok(markdown)
}

fn share(share: &Share) -> String {
    let percent = (share.registered * 100).checked_div(share.of).unwrap_or(0);
    format!("{} / {} ({percent}%)", share.registered, share.of)
}

fn rate(rate: &Rate) -> String {
    if rate.measured {
        share(&Share {
            registered: rate.expected,
            of: rate.of,
        })
    } else {
        format!(
            "not measured ({} fixtures): {}",
            rate.of,
            rate.reason.unwrap_or_default()
        )
    }
}

fn render_markdown(audit: &Audit) -> String {
    let mut out = String::from("# Architecture audit\n\n");
    out.push_str(&format!(
        "Registry: `{REGISTRY}`. Status: {}.\n\n",
        if audit.problems.is_empty() {
            "PASS"
        } else {
            "FAIL"
        }
    ));
    out.push_str("| Measure | Value |\n| --- | --- |\n");
    out.push_str(&format!("| Rules | {} |\n", audit.rules.len()));
    out.push_str(&format!(
        "| Rules with a registered detector | {} |\n",
        share(&audit.detectors)
    ));
    out.push_str(&format!(
        "| Rules checked through the syntax | {} |\n",
        share(&audit.syntax_rules)
    ));
    out.push_str(&format!(
        "| Rules with a passing fixture | {} |\n",
        share(&audit.passing_fixtures)
    ));
    out.push_str(&format!(
        "| Rules with a violating fixture | {} |\n",
        share(&audit.violating_fixtures)
    ));
    out.push_str(&format!(
        "| Violation detection rate | {} |\n",
        rate(&audit.detection_rate)
    ));
    out.push_str(&format!(
        "| Allowed-code pass rate | {} |\n",
        rate(&audit.passing_rate)
    ));
    out.push_str(&format!(
        "| Exemption lists / entries | {} / {} |\n",
        audit.exemption_lists, audit.exemption_entries
    ));
    out.push_str(&format!(
        "\n{} `#[test]` functions live under `tests/architecture/`. That they pass says the \
         tree is clean today; it is not a coverage figure, and this report does not turn it into one.\n",
        audit.architecture_tests
    ));
    out.push_str("\n## Not measured\n\n");
    for item in &audit.not_measured {
        out.push_str(&format!("- {item}\n"));
    }
    let syntax: Vec<&RuleReport> = audit
        .rules
        .iter()
        .filter(|rule| rule.method == Method::Syntax)
        .collect();
    if !syntax.is_empty() {
        out.push_str("\n## What the syntax rules cannot see\n\n");
        for rule in syntax {
            out.push_str(&format!(
                "- {}: {}\n",
                rule.id,
                rule.limits.as_deref().unwrap_or_default()
            ));
        }
    }
    if !audit.problems.is_empty() {
        out.push_str("\n## Problems\n\n");
        for problem in &audit.problems {
            out.push_str(&format!("- {problem}\n"));
        }
    }
    out.push_str(
        "\n## Rules\n\n| Id | Method | Checks | Violating | Passing | Exemptions | Docs | Summary |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for rule in &audit.rules {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} ({}) | {} | {} |\n",
            rule.id,
            match rule.method {
                Method::Text => "text",
                Method::Syntax => "syntax",
            },
            rule.checks,
            rule.violating_fixtures,
            rule.passing_fixtures,
            rule.exemption_lists,
            rule.exemption_entries,
            rule.docs,
            rule.summary.replace('|', "\\|"),
        ));
    }
    out
}

#[cfg(test)]
#[path = "architecture_audit_tests.rs"]
mod tests;
