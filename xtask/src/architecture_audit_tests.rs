//! Tests of `architecture_audit.rs`: the audit over maps of strings, and the
//! checked-in registry against the tree.

use super::*;

const LIST: &str = "//! Rules.\n//!\n//! ARCH-001 one rule.\n//! ARCH-002 another.\n";

const LAYERS: &str = "\
//! Layers.
const ALLOWED: [&str; 2] = [\"a\", \"b, c\"];

#[test]
fn first_rule_holds() {}

/// Doc.
#[test]
#[ignore]
fn second_rule_holds() {}
";

const REGISTRY_TEXT: &str = r#"
[[rule]]
id = "ARCH-001"
summary = "one"
checks = ["tests/architecture/layers.rs::first_rule_holds"]
docs = ["AGENTS.md"]
exemptions = ["tests/architecture/layers.rs::ALLOWED"]

[[rule]]
id = "ARCH-002"
summary = "two"
checks = ["tests/architecture/layers.rs::second_rule_holds"]
"#;

fn run_audit(registry: &str, files: &[(&str, &str)]) -> Audit {
    let files: BTreeMap<String, String> = files
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect();
    let architecture: Vec<String> = files
        .keys()
        .filter(|path| path.starts_with("tests/architecture/") && path.ends_with(".rs"))
        .filter(|path| path.as_str() != RULE_LIST)
        .cloned()
        .collect();
    audit(registry, &architecture, &|path| files.get(path).cloned()).unwrap()
}

fn tree<'a>(list: &'a str, layers: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        (RULE_LIST, list),
        ("tests/architecture/layers.rs", layers),
        ("AGENTS.md", "# Agents"),
    ]
}

#[test]
fn a_consistent_registry_reports_no_problem_and_counts_what_resolved() {
    let audit = run_audit(REGISTRY_TEXT, &tree(LIST, LAYERS));
    assert_eq!(audit.problems, Vec::<String>::new());
    assert_eq!(audit.architecture_tests, 2);
    assert_eq!(
        audit.detectors,
        Share {
            registered: 2,
            of: 2
        }
    );
    assert_eq!(
        audit.violating_fixtures,
        Share {
            registered: 0,
            of: 2
        }
    );
    assert_eq!(
        audit.passing_fixtures,
        Share {
            registered: 0,
            of: 2
        }
    );
    assert_eq!((audit.exemption_lists, audit.exemption_entries), (1, 2));
    assert!(!audit.detection_rate.measured);
}

#[test]
fn a_test_no_rule_names_is_reported() {
    let layers = format!("{LAYERS}\n#[test]\nfn a_third_test() {{}}\n");
    let audit = run_audit(REGISTRY_TEXT, &tree(LIST, &layers));
    assert_eq!(
        audit.problems,
        ["`tests/architecture/layers.rs::a_third_test` is a #[test] that no rule names"]
    );
}

#[test]
fn a_check_naming_a_missing_test_is_reported() {
    let layers = LAYERS.replace("fn second_rule_holds", "fn renamed");
    let audit = run_audit(REGISTRY_TEXT, &tree(LIST, &layers));
    assert!(
        audit.problems.iter().any(|p| p.starts_with(
            "ARCH-002: checks names `tests/architecture/layers.rs::second_rule_holds`"
        )),
        "{:?}",
        audit.problems
    );
    assert_eq!(
        audit.detectors,
        Share {
            registered: 1,
            of: 2
        }
    );
}

#[test]
fn a_function_without_a_test_attribute_is_not_a_check() {
    let layers = LAYERS.replace("#[test]\nfn first_rule_holds", "fn first_rule_holds");
    let audit = run_audit(REGISTRY_TEXT, &tree(LIST, &layers));
    assert!(
        audit
            .problems
            .iter()
            .any(|p| p.starts_with("ARCH-001: checks names")),
        "{:?}",
        audit.problems
    );
}

#[test]
fn a_rule_only_the_list_names_is_reported() {
    let list = format!("{LIST}//! ARCH-003 only here.\n");
    let audit = run_audit(REGISTRY_TEXT, &tree(&list, LAYERS));
    assert_eq!(
        audit.problems,
        ["ARCH-003: in the list in tests/architecture/main.rs but not in the registry"]
    );
}

#[test]
fn a_rule_the_list_does_not_name_is_reported() {
    let list = LIST.replace("//! ARCH-002 another.\n", "");
    let audit = run_audit(REGISTRY_TEXT, &tree(&list, LAYERS));
    assert_eq!(
        audit.problems,
        ["ARCH-002: in the registry but not in the list in tests/architecture/main.rs"]
    );
}

#[test]
fn a_duplicate_id_is_reported() {
    let registry = REGISTRY_TEXT.replace("\"ARCH-002\"", "\"ARCH-001\"");
    let audit = run_audit(&registry, &tree(LIST, LAYERS));
    assert!(
        audit
            .problems
            .contains(&"ARCH-001: the id is used by two rules".to_string()),
        "{:?}",
        audit.problems
    );
}

#[test]
fn a_test_two_rules_name_is_reported() {
    let registry = REGISTRY_TEXT.replace("::second_rule_holds", "::first_rule_holds");
    let audit = run_audit(&registry, &tree(LIST, LAYERS));
    assert!(
        audit.problems.contains(
            &"`tests/architecture/layers.rs::first_rule_holds` is named by both ARCH-001 and ARCH-002"
                .to_string()
        ),
        "{:?}",
        audit.problems
    );
}

#[test]
fn a_missing_document_or_exemption_is_reported() {
    let registry = REGISTRY_TEXT
        .replace("AGENTS.md", "GONE.md")
        .replace("::ALLOWED", "::NOT_THERE");
    let audit = run_audit(&registry, &tree(LIST, LAYERS));
    assert_eq!(
        audit.problems,
        [
            "ARCH-001: docs names GONE.md, which is not readable",
            "ARCH-001: exemptions names `tests/architecture/layers.rs::NOT_THERE`, which is not a const list in that file",
        ]
    );
}

#[test]
fn a_malformed_id_is_reported() {
    let registry = REGISTRY_TEXT.replace("\"ARCH-002\"", "\"ARCH-2\"");
    let list = LIST.replace("ARCH-002", "ARCH-2  ");
    let audit = run_audit(&registry, &tree(&list, LAYERS));
    assert!(
        audit
            .problems
            .contains(&"ARCH-2: an id is ARCH- and three digits".to_string()),
        "{:?}",
        audit.problems
    );
}

#[test]
fn const_entries_counts_top_level_elements_of_every_list_shape() {
    let text = "\
const A: [&str; 2] = [\"x\", \"y\"];
const B: &[&str] = &[
\"one\",
\"two, still two\",
\"three\",
];
const C: [(&str, [&str; 2]); 2] = [(\"k\", [\"a\", \"b\"]), (\"l\", [\"c\", \"d\"])];
const D: [&str; 0] = [];
";
    assert_eq!(const_entries(text, "A"), Some(2));
    assert_eq!(const_entries(text, "B"), Some(3));
    assert_eq!(const_entries(text, "C"), Some(2));
    assert_eq!(const_entries(text, "D"), Some(0));
    assert_eq!(const_entries(text, "E"), None);
}

#[test]
fn a_syntax_rule_states_its_limits_and_a_text_rule_has_none() {
    let registry = REGISTRY_TEXT
        .replace(
            "summary = \"one\"",
            "summary = \"one\"\nmethod = \"syntax\"\nlimits = \"names\"",
        )
        .replace(
            "summary = \"two\"",
            "summary = \"two\"\nmethod = \"syntax\"",
        );
    let audit = run_audit(&registry, &tree(LIST, LAYERS));
    assert_eq!(
        audit.problems,
        ["ARCH-002: a syntax rule states its limits"]
    );
    assert_eq!(
        audit.syntax_rules,
        Share {
            registered: 2,
            of: 2
        }
    );
    let markdown = render_markdown(&audit);
    assert!(markdown.contains("| Rules checked through the syntax | 2 / 2 (100%) |"));
    assert!(markdown.contains("- ARCH-001: names\n"), "{markdown}");

    let registry = REGISTRY_TEXT.replace("summary = \"two\"", "summary = \"two\"\nlimits = \"x\"");
    let audit = run_audit(&registry, &tree(LIST, LAYERS));
    assert_eq!(audit.problems, ["ARCH-002: limits belong to a syntax rule"]);
    assert_eq!(
        audit.syntax_rules,
        Share {
            registered: 0,
            of: 2
        }
    );
}

#[test]
fn const_entries_skips_escaped_quotes_and_counts_bare_values() {
    let text = "const Q: [&str; 2] = [\"a \\\" , b\", \"c\"];\nconst N: [u8; 3] = [1, 2, 3];\n";
    assert_eq!(const_entries(text, "Q"), Some(2));
    assert_eq!(const_entries(text, "N"), Some(3));
}

#[test]
fn a_registered_fixture_counts_for_its_rule_only() {
    let registry = REGISTRY_TEXT.replace(
        "exemptions = [\"tests/architecture/layers.rs::ALLOWED\"]",
        "exemptions = [\"tests/architecture/layers.rs::ALLOWED\"]\n\
         violating_fixtures = [\"tests/architecture/fixtures.rs::refused\"]\n\
         passing_fixtures = [\"tests/architecture/fixtures.rs::allowed\", \"tests/architecture/fixtures.rs::allowed_too\"]",
    );
    let fixtures =
        "#[test]\nfn refused() {}\n#[test]\nfn allowed() {}\n#[test]\nfn allowed_too() {}\n";
    let files: BTreeMap<String, String> = tree(LIST, LAYERS)
        .into_iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .chain([(
            "tests/architecture/fixtures.rs".to_string(),
            fixtures.to_string(),
        )])
        .collect();
    // Both files are scanned: a fixture test is named by its rule, so it
    // is not a test that no rule names.
    let architecture = vec![
        "tests/architecture/fixtures.rs".to_string(),
        "tests/architecture/layers.rs".to_string(),
    ];
    let audit = audit(&registry, &architecture, &|path| files.get(path).cloned()).unwrap();
    assert_eq!(audit.problems, Vec::<String>::new());
    assert_eq!(
        audit.violating_fixtures,
        Share {
            registered: 1,
            of: 2
        }
    );
    assert_eq!(
        audit.passing_fixtures,
        Share {
            registered: 1,
            of: 2
        }
    );
    assert_eq!(
        (
            audit.rules[0].violating_fixtures,
            audit.rules[0].passing_fixtures
        ),
        (1, 2)
    );
    assert_eq!((audit.rules[0].docs, audit.rules[1].docs), (1, 0));
}

#[test]
fn the_gate_passes_a_clean_audit_and_lists_every_problem_otherwise() {
    let clean = run_audit(REGISTRY_TEXT, &tree(LIST, LAYERS));
    assert_eq!(gate_result(&clean), Ok("2 rules, 2 with a detector".into()));
    let list = format!("{LIST}//! ARCH-003 x.\n//! ARCH-004 y.\n");
    let broken = run_audit(REGISTRY_TEXT, &tree(&list, LAYERS));
    assert_eq!(gate_result(&broken), Err(broken.problems.join("\n")));
    assert_eq!(broken.problems.len(), 2);
    let markdown = render_markdown(&broken);
    assert!(
        markdown.contains("## Problems\n\n- ARCH-003:"),
        "{markdown}"
    );
    assert!(!render_markdown(&clean).contains("## Problems"));
}

#[test]
fn test_names_reads_sync_and_async_tests_only() {
    let text = "\
#[test]
fn plain() {}
#[tokio::test]
async fn awaited() {}
fn helper() {}
#[test]
// a comment
fn after_a_comment() {}
";
    assert_eq!(
        test_names(text),
        ["after_a_comment", "awaited", "plain"]
            .map(String::from)
            .into()
    );
}

#[test]
fn the_report_does_not_call_a_pass_count_coverage() {
    let audit = run_audit(REGISTRY_TEXT, &tree(LIST, LAYERS));
    let markdown = render_markdown(&audit);
    assert!(markdown.contains("| Violation detection rate | not measured (0 fixtures):"));
    assert!(markdown.contains("| Rules with a registered detector | 2 / 2 (100%) |"));
    assert!(markdown.contains("it is not a coverage figure"));
    assert!(!markdown.contains("tests passed"));
}

/// The checkout's own registry holds: every rule resolves, every
/// architecture test belongs to one rule, and the list agrees.
/// The gate is the audit of the checkout, summarized: it has to say what
/// `gate_result` says about the same audit.
#[test]
fn the_gate_reports_the_audit_of_the_checkout() {
    let summary = gate().unwrap();
    assert_eq!(Ok(summary.clone()), gate_result(&run().unwrap()));
    assert!(summary.ends_with("with a detector"), "{summary}");
}

#[test]
fn the_checked_in_registry_matches_the_tree() {
    let audit = run().unwrap();
    assert_eq!(audit.problems, Vec::<String>::new());
    assert!(audit.rules.len() >= 23, "{} rules", audit.rules.len());
}

#[test]
fn run_fixtures_set_both_rates_and_every_unexpected_outcome_is_a_problem() {
    let registry = REGISTRY_TEXT.replace(
        "exemptions = [\"tests/architecture/layers.rs::ALLOWED\"]",
        "exemptions = [\"tests/architecture/layers.rs::ALLOWED\"]\n\
         violating_fixtures = [\"tests/architecture/layers.rs::first_rule_holds\", \"tests/architecture/layers.rs::second_rule_holds\"]",
    )
    .replace(
        "checks = [\"tests/architecture/layers.rs::second_rule_holds\"]",
        "checks = [\"tests/architecture/layers.rs::third\"]\npassing_fixtures = [\"tests/architecture/layers.rs::fourth\"]",
    )
    .replace(
        "checks = [\"tests/architecture/layers.rs::first_rule_holds\"]",
        "checks = [\"tests/architecture/layers.rs::fifth\"]",
    );
    let layers = format!(
        "{LAYERS}#[test]\nfn third() {{}}\n#[test]\nfn fourth() {{}}\n#[test]\nfn fifth() {{}}\n"
    );
    let mut audit = run_audit(&registry, &tree(LIST, &layers));
    assert_eq!(audit.problems, Vec::<String>::new());
    assert!(!audit.detection_rate.measured);
    assert_eq!((audit.detection_rate.of, audit.passing_rate.of), (2, 1));
    assert!(
        render_markdown(&audit).contains("| Violation detection rate | not measured (2 fixtures):")
    );

    apply_outcomes(
        &mut audit,
        vec![
            Outcome::Detected,
            Outcome::OtherFailure("does not parse".into()),
            Outcome::Allowed,
        ],
    );
    assert_eq!(
        audit.detection_rate,
        Rate {
            measured: true,
            expected: 1,
            of: 2,
            reason: None
        }
    );
    assert_eq!(
        audit.passing_rate,
        Rate {
            measured: true,
            expected: 1,
            of: 1,
            reason: None
        }
    );
    assert_eq!(
        audit.problems,
        [
            "ARCH-001: tests/architecture/layers.rs::second_rule_holds failed for another reason: does not parse"
        ]
    );
    let markdown = render_markdown(&audit);
    assert!(
        markdown.contains("| Violation detection rate | 1 / 2 (50%) |"),
        "{markdown}"
    );
    assert!(markdown.contains("| Allowed-code pass rate | 1 / 1 (100%) |"));
    assert!(!markdown.contains("only a violating fixture that runs shows it"));
}

#[test]
fn every_way_a_fixture_can_go_wrong_is_named_in_its_problem() {
    let registry = REGISTRY_TEXT.replace(
        "exemptions = [\"tests/architecture/layers.rs::ALLOWED\"]",
        "exemptions = [\"tests/architecture/layers.rs::ALLOWED\"]\n\
         violating_fixtures = [\"tests/architecture/f.rs::a\", \"tests/architecture/f.rs::b\"]\n\
         passing_fixtures = [\"tests/architecture/f.rs::c\"]",
    );
    let files: BTreeMap<String, String> = tree(LIST, LAYERS)
        .into_iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .chain([(
            "tests/architecture/f.rs".to_string(),
            "#[test]\nfn a() {}\n#[test]\nfn b() {}\n#[test]\nfn c() {}\n".to_string(),
        )])
        .collect();
    let mut audit = audit(
        &registry,
        &["tests/architecture/layers.rs".to_string()],
        &|path| files.get(path).cloned(),
    )
    .unwrap();
    apply_outcomes(
        &mut audit,
        vec![
            Outcome::Missed,
            Outcome::NotRun("timed out after 1800s".into()),
            Outcome::Refused,
        ],
    );
    assert_eq!(
        audit.problems,
        [
            "ARCH-001: tests/architecture/f.rs::a let a violation through",
            "ARCH-001: tests/architecture/f.rs::b did not run: timed out after 1800s",
            "ARCH-001: tests/architecture/f.rs::c refused allowed code",
        ]
    );
    assert_eq!(
        (audit.detection_rate.expected, audit.passing_rate.expected),
        (0, 0)
    );
}
