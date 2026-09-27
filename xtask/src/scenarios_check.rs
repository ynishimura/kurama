//! `cargo xtask scenarios-check <issue>`: whether the runtime scenarios an
//! issue declares exist. An issue body lists them as a checklist under
//! `## Scenarios` (or `## シナリオ`), one scenario name per item; this reads that section and
//! answers with what is still missing, what the branch adds without declaring
//! it, what no feature in `.agent/features/` claims, and which declarations
//! are not shaped like a scenario name. An issue with no such section is not
//! satisfied: it declared nothing, which is a different answer.

use std::collections::BTreeSet;

use crate::board::issue_body;
use crate::scenario_name;

const USAGE: &str = "usage: cargo xtask scenarios-check <issue> [--base REF] [--json]";

/// The heading an issue body puts its scenario checklist under.
pub(crate) const SECTION: &str = "Scenarios";

/// The same section under the heading the issues written before the English
/// templates carry.
const SECTION_JA: &str = "シナリオ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum Verdict {
    /// The issue names no scenario at all.
    NoDeclaration,
    /// Every declared scenario exists.
    Satisfied,
    /// A declared scenario has no test of that name.
    Incomplete,
}

/// A declaration that cannot be read as a scenario name.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct IllNamed {
    scenario: String,
    reason: &'static str,
}

#[derive(Debug, serde::Serialize)]
struct Report {
    issue: u64,
    verdict: Verdict,
    declared: Vec<String>,
    /// Declared, but the scenario binary lists no test of that name.
    missing: Vec<String>,
    /// Scenarios this branch adds that the issue does not declare. Not wrong
    /// on its own; the integrator is the one who decides.
    undeclared: Vec<String>,
    /// Scenarios that exist but no feature in `.agent/features/` claims.
    unregistered: Vec<String>,
    /// Declarations that are not shaped like `<feature>_<behavior>`.
    ill_named: Vec<IllNamed>,
}

pub fn scenarios_check(args: &[String]) -> Result<(), String> {
    let mut json = false;
    let mut issue: Option<u64> = None;
    let mut base: Option<String> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--json" {
            json = true;
            continue;
        }
        if arg == "--base" {
            base = Some(
                rest.next()
                    .ok_or_else(|| format!("--base needs a revision\n\n{USAGE}"))?
                    .clone(),
            );
            continue;
        }
        let number = crate::board::issue_number(arg, USAGE)?;
        if issue.replace(number).is_some() {
            return Err(format!("one issue at a time\n\n{USAGE}"));
        }
    }
    let issue = issue.ok_or_else(|| format!("an issue number is needed\n\n{USAGE}"))?;

    let declared = declared_scenarios(issue)?;
    let known: BTreeSet<String> = crate::list_scenarios()?
        .iter()
        .map(|listed| scenario_name(listed).to_string())
        .collect();
    let mapped: BTreeSet<String> = crate::load_features()?
        .values()
        .flat_map(|feature| feature.scenarios.clone())
        .collect();
    let added = added_scenarios(&known, base.unwrap_or_else(crate::default_base))?;
    let report = evaluate(issue, declared, &known, &added, &mapped);

    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        print!("{}", render(&report));
    }
    if report.missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "issue #{issue} declares {} scenario(s) that do not exist",
            report.missing.len()
        ))
    }
}

/// The names the issue declares, or `None` when it carries no `## Scenarios`
/// (or `## シナリオ`) section. A section that lists nothing is an error: an empty checklist and a
/// missing one are different mistakes.
fn declared_scenarios(issue: u64) -> Result<Option<Vec<String>>, String> {
    match parse_section(&issue_body(issue)?) {
        Some(names) if names.is_empty() => Err(format!(
            "issue #{issue}: the `## {SECTION}` section lists no scenario; write one \
             `- [ ] <feature>_<behavior>` item per scenario"
        )),
        other => Ok(other),
    }
}

/// The checklist under the `## Scenarios` (or `## シナリオ`) heading, up to
/// the next heading.
/// Prose in the section is ignored; only checklist items declare a scenario.
pub(crate) fn parse_section(body: &str) -> Option<Vec<String>> {
    let mut lines = outside_code_fences(body)
        .skip_while(|line| !matches!(heading(line), Some(SECTION | SECTION_JA)));
    lines.next()?;
    Some(
        lines
            .take_while(|line| heading(line).is_none())
            .filter_map(checklist_name)
            .collect(),
    )
}

/// The body without its fenced code blocks. An issue that shows what the
/// section looks like carries the example in a fence, and this command's own
/// issue is one: the example is documentation, not a declaration.
fn outside_code_fences(body: &str) -> impl Iterator<Item = &str> {
    let mut inside = false;
    body.lines().filter(move |line| {
        if line.trim_start().starts_with("```") {
            inside = !inside;
            return false;
        }
        !inside
    })
}

fn heading(line: &str) -> Option<&str> {
    let text = line.trim().trim_start_matches('#');
    (text.len() < line.trim().len()).then(|| text.trim())
}

/// `- [ ] name` (or `- [x] name`) -> `name`, backticks stripped. Anything the
/// item says after the name is a comment.
fn checklist_name(line: &str) -> Option<String> {
    let item = line
        .trim()
        .strip_prefix("- ")
        .or_else(|| line.trim().strip_prefix("* "))?
        .trim_start();
    let rest = item
        .strip_prefix("[ ]")
        .or_else(|| item.strip_prefix("[x]"))
        .or_else(|| item.strip_prefix("[X]"))?;
    let name = rest.split_whitespace().next()?.trim_matches('`');
    (!name.is_empty()).then(|| name.to_string())
}

/// A declaration that cannot be a scenario name. Which feature the prefix
/// names cannot be checked mechanically -- a scenario is claimed by every
/// feature it verifies, and `errors` claims scenarios named after the command
/// they are about -- so what is checked is the shape: a lowercase snake_case
/// name carrying a prefix and a behavior.
pub(crate) fn naming_problem(name: &str) -> Option<&'static str> {
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Some("not a lowercase snake_case name");
    }
    if name.starts_with('_') || name.ends_with('_') || name.contains("__") {
        return Some("empty name segment");
    }
    if !name.contains('_') {
        return Some("names no behavior: <feature>_<behavior>");
    }
    None
}

fn evaluate(
    issue: u64,
    declared: Option<Vec<String>>,
    known: &BTreeSet<String>,
    added: &BTreeSet<String>,
    mapped: &BTreeSet<String>,
) -> Report {
    let verdict = match &declared {
        None => Verdict::NoDeclaration,
        Some(names) if names.iter().all(|name| known.contains(name)) => Verdict::Satisfied,
        Some(_) => Verdict::Incomplete,
    };
    let declared = declared.unwrap_or_default();
    let present: BTreeSet<&String> = declared
        .iter()
        .filter(|name| known.contains(*name))
        .chain(added)
        .collect();
    Report {
        issue,
        verdict,
        missing: declared
            .iter()
            .filter(|name| !known.contains(*name))
            .cloned()
            .collect(),
        undeclared: added
            .iter()
            .filter(|name| !declared.contains(*name))
            .cloned()
            .collect(),
        unregistered: present
            .into_iter()
            .filter(|name| !mapped.contains(*name))
            .cloned()
            .collect(),
        ill_named: declared
            .iter()
            .filter_map(|name| {
                naming_problem(name).map(|reason| IllNamed {
                    scenario: name.clone(),
                    reason,
                })
            })
            .collect(),
        declared,
    }
}

/// Scenario functions this branch adds: `fn <name>(` lines that
/// `git diff <base>` shows as added under `tests/scenarios/`, plus the ones in
/// files nothing has committed yet, kept to the names the scenario binary
/// lists -- so a helper added next to a scenario is not counted as one. The
/// base is the one `impact` uses, and `--base` is how a caller names another:
/// a remote branch the local one is ahead of answers with its whole history.
fn added_scenarios(known: &BTreeSet<String>, base: String) -> Result<BTreeSet<String>, String> {
    let merge_base = crate::git(&["merge-base", &base, "HEAD"]).unwrap_or(base);
    let diff = crate::git(&[
        "diff",
        "--unified=0",
        merge_base.trim(),
        "--",
        "tests/scenarios/",
    ])?;
    let mut names = scenarios_in(added_lines(&diff), known);
    for file in crate::git(&[
        "ls-files",
        "--others",
        "--exclude-standard",
        "tests/scenarios/",
    ])?
    .lines()
    .filter(|line| !line.is_empty())
    {
        let content = std::fs::read_to_string(crate::root().join(file)).unwrap_or_default();
        names.extend(scenarios_in(content.lines(), known));
    }
    // A case file under tests/cases/ is one scenario, named by the file.
    let added_cases = crate::git(&[
        "diff",
        "--name-only",
        "--diff-filter=A",
        merge_base.trim(),
        "--",
        "tests/cases/",
    ])?;
    let untracked_cases =
        crate::git(&["ls-files", "--others", "--exclude-standard", "tests/cases/"])?;
    names.extend(
        added_cases
            .lines()
            .chain(untracked_cases.lines())
            .filter_map(|file| crate::cases::case_path(file).map(|(_, id)| id))
            .filter(|name| known.contains(*name))
            .map(str::to_string),
    );
    // A scenario the diff removes from Rust and adds back as a case (or
    // under another file) was moved, not added: it needs no declaration.
    for moved in scenarios_in(removed_lines(&diff), known) {
        names.remove(&moved);
    }
    Ok(names)
}

/// The content of the lines a unified diff removes.
fn removed_lines(diff: &str) -> impl Iterator<Item = &str> {
    diff.lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .map(|line| &line[1..])
}

/// The content of the lines a unified diff adds.
fn added_lines(diff: &str) -> impl Iterator<Item = &str> {
    diff.lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .map(|line| &line[1..])
}

fn scenarios_in<'a>(
    lines: impl Iterator<Item = &'a str>,
    known: &BTreeSet<String>,
) -> BTreeSet<String> {
    lines
        .filter_map(function_name)
        .filter(|name| known.contains(*name))
        .map(str::to_string)
        .collect()
}

fn function_name(line: &str) -> Option<&str> {
    let (name, _) = line.trim_start().strip_prefix("fn ")?.split_once('(')?;
    Some(name)
}

/// How many scenarios a list names before it counts the rest. A base the
/// branch is far behind makes "undeclared" the whole test suite, and a page of
/// names buries the two lines that matter; `--json` carries all of them.
const SHOWN: usize = 10;

fn render(report: &Report) -> String {
    let mut out = format!("#{}  {}\n", report.issue, report.verdict.label());
    if report.verdict == Verdict::NoDeclaration {
        out.push_str(&format!(
            "\nThe body carries no `## {SECTION}` section, so no scenario was declared and \
             nothing was\nchecked against it. That is right only for a change with no runtime \
             behavior of its\nown (an xtask command, a document); otherwise add the section and \
             list one\n`- [ ] <feature>_<behavior>` item per scenario.\n"
        ));
    } else {
        out.push_str(&format!(
            "\n{} declared, {} present, {} missing\n",
            report.declared.len(),
            report.declared.len() - report.missing.len(),
            report.missing.len(),
        ));
    }
    for (title, names) in [
        (
            "missing (declared, no test of that name exists)",
            &report.missing,
        ),
        (
            "undeclared (this branch adds them, the issue does not name them)",
            &report.undeclared,
        ),
        (
            "unregistered (no feature in .agent/features/ claims them)",
            &report.unregistered,
        ),
    ] {
        if !names.is_empty() {
            out.push_str(&format!("\n{title}:\n"));
            for name in names.iter().take(SHOWN) {
                out.push_str(&format!("  {name}\n"));
            }
            let rest = names.len().saturating_sub(SHOWN);
            if rest > 0 {
                out.push_str(&format!("  (+{rest} more; --json lists them all)\n"));
            }
        }
    }
    if !report.ill_named.is_empty() {
        out.push_str("\nill-named (a scenario is named <feature>_<behavior>):\n");
        for entry in &report.ill_named {
            out.push_str(&format!("  {}: {}\n", entry.scenario, entry.reason));
        }
    }
    out
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::NoDeclaration => "no declaration",
            Verdict::Satisfied => "satisfied",
            Verdict::Incomplete => "incomplete",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn declared(items: &[&str]) -> Option<Vec<String>> {
        Some(items.iter().map(|item| item.to_string()).collect())
    }

    #[test]
    fn a_scenario_section_lists_the_names_of_its_checklist() {
        let body = "## 受入条件\n\n- 散文の条件\n\n\
                    ## シナリオ\n\n\
                    runtime verification で示すもの:\n\
                    - [ ] secret_ref_aws_secrets_resolves_db_password\n\
                    - [x] `secret_ref_json_key_fetches_the_secret_once`\n\
                    - [ ] secret_ref_unknown_scheme_exits_2 -- 2 で落ちる\n\n\
                    ## スコープ外\n\n- [ ] out_of_scope_never_declared\n";

        assert_eq!(
            parse_section(body),
            Some(vec![
                "secret_ref_aws_secrets_resolves_db_password".to_string(),
                "secret_ref_json_key_fetches_the_secret_once".to_string(),
                "secret_ref_unknown_scheme_exits_2".to_string(),
            ])
        );
    }

    /// The English heading the issue templates use reads the same as the
    /// Japanese one the older issues carry.
    #[test]
    fn the_scenarios_heading_is_read_in_english_too() {
        let body = "## Acceptance\n\n- prose\n\n## Scenarios\n\n\
                    - [ ] api_token_query_puts_the_credential_in_its_parameter\n\n\
                    ## Out of scope\n\n- [ ] not_declared\n";

        assert_eq!(
            parse_section(body),
            declared(&["api_token_query_puts_the_credential_in_its_parameter"])
        );
    }

    #[test]
    fn a_body_without_a_scenario_section_declares_nothing() {
        assert_eq!(
            parse_section("## 目的\n\n- [ ] not_under_a_heading\n"),
            None
        );
    }

    /// #54, this command's own issue, shows what the section looks like inside
    /// a fenced block. Reading that example as a declaration made the command
    /// report three missing scenarios for an issue that declares none.
    #[test]
    fn an_example_section_in_a_code_block_is_not_a_declaration() {
        let body = "## 設計\n\n\
                    受入条件のうち runtime verification で示すものを、チェックリストにする。\n\n\
                    ```markdown\n\
                    ## シナリオ\n\n\
                    - [ ] secret_ref_aws_secrets_resolves_db_password\n\
                    - [ ] secret_ref_unknown_scheme_exits_2\n\
                    ```\n\n\
                    ## 受入条件\n\n- 散文の条件\n";

        assert_eq!(parse_section(body), None);
    }

    /// The section is what the issue declares; an empty one is a mistake the
    /// caller has to see, not a satisfied issue.
    #[test]
    fn a_scenario_section_without_an_item_is_empty_not_absent() {
        assert_eq!(
            parse_section("## シナリオ\n\nあとで書く\n"),
            Some(Vec::new())
        );
    }

    #[test]
    fn a_declared_scenario_with_no_test_of_that_name_is_missing() {
        let report = evaluate(
            48,
            declared(&["secret_ref_resolves", "secret_ref_unknown_scheme_exits_2"]),
            &names(&["secret_ref_resolves"]),
            &names(&[]),
            &names(&["secret_ref_resolves"]),
        );

        assert_eq!(report.verdict, Verdict::Incomplete);
        assert_eq!(report.missing, ["secret_ref_unknown_scheme_exits_2"]);
        assert!(report.undeclared.is_empty());
    }

    #[test]
    fn every_declared_scenario_present_is_satisfied() {
        let report = evaluate(
            48,
            declared(&["secret_ref_resolves"]),
            &names(&["secret_ref_resolves", "api_sends_the_bearer_token"]),
            &names(&[]),
            &names(&["secret_ref_resolves", "api_sends_the_bearer_token"]),
        );

        assert_eq!(report.verdict, Verdict::Satisfied);
        assert!(report.missing.is_empty());
        assert!(
            report.undeclared.is_empty(),
            "a scenario the branch did not add is not an undeclared addition: {:?}",
            report.undeclared
        );
    }

    /// A scenario the branch adds without the issue naming it is reported,
    /// and does not make the issue unsatisfied.
    #[test]
    fn a_scenario_the_branch_adds_without_declaring_it_is_reported_on_its_own() {
        let report = evaluate(
            48,
            declared(&["secret_ref_resolves"]),
            &names(&["secret_ref_resolves", "secret_ref_cache_is_reused"]),
            &names(&["secret_ref_resolves", "secret_ref_cache_is_reused"]),
            &names(&["secret_ref_resolves", "secret_ref_cache_is_reused"]),
        );

        assert_eq!(report.verdict, Verdict::Satisfied);
        assert_eq!(report.undeclared, ["secret_ref_cache_is_reused"]);
    }

    #[test]
    fn a_scenario_the_diff_removes_is_not_one_the_branch_adds() {
        let diff = "-fn api_moved() {\n+fn api_new() {\n";
        let known = names(&["api_moved", "api_new"]);
        let mut added = scenarios_in(added_lines(diff), &known);
        for moved in scenarios_in(removed_lines(diff), &known) {
            added.remove(&moved);
        }
        assert_eq!(added, names(&["api_new"]));
    }

    #[test]
    fn a_scenario_no_feature_claims_is_unregistered() {
        let report = evaluate(
            48,
            declared(&["secret_ref_resolves"]),
            &names(&["secret_ref_resolves", "secret_ref_cache_is_reused"]),
            &names(&["secret_ref_cache_is_reused"]),
            &names(&["secret_ref_resolves"]),
        );

        assert_eq!(report.unregistered, ["secret_ref_cache_is_reused"]);
        assert_eq!(report.verdict, Verdict::Satisfied);
    }

    #[test]
    fn a_declaration_that_is_not_a_scenario_name_says_so() {
        let report = evaluate(
            48,
            declared(&["Secret ref resolves", "secretref", "secret__ref"]),
            &names(&[]),
            &names(&[]),
            &names(&[]),
        );

        assert_eq!(
            report.ill_named,
            [
                IllNamed {
                    scenario: "Secret ref resolves".to_string(),
                    reason: "not a lowercase snake_case name",
                },
                IllNamed {
                    scenario: "secretref".to_string(),
                    reason: "names no behavior: <feature>_<behavior>",
                },
                IllNamed {
                    scenario: "secret__ref".to_string(),
                    reason: "empty name segment",
                },
            ]
        );
        assert_eq!(report.verdict, Verdict::Incomplete);
    }

    /// `cargo test -- --list` prints `<module>::<scenario>` since the
    /// scenarios were split into one file per feature; the declaration and the
    /// feature map both name the scenario itself.
    #[test]
    fn the_listed_name_is_qualified_by_the_module_of_its_feature() {
        assert_eq!(
            scenario_name("oauth::oauth_rejected_grant_exits_4"),
            "oauth_rejected_grant_exits_4"
        );
        assert_eq!(scenario_name("api_timeout_exits_1"), "api_timeout_exits_1");
    }

    #[test]
    fn a_scenario_name_of_the_repository_is_well_named() {
        assert_eq!(
            naming_problem("api_sends_the_bearer_token_and_prints_the_body"),
            None
        );
        assert_eq!(naming_problem("uc07_console_fetches_signin_token"), None);
    }

    /// #54 itself declares no scenario, because an xtask command adds no
    /// runtime behavior. The command has to say that, and must not call it
    /// satisfied.
    #[test]
    fn an_issue_that_declares_nothing_is_not_satisfied() {
        let report = evaluate(
            54,
            None,
            &names(&["api_timeout_exits_1"]),
            &names(&[]),
            &names(&[]),
        );

        assert_eq!(report.verdict, Verdict::NoDeclaration);
        let text = render(&report);
        assert!(text.contains("no declaration"), "{text}");
        assert!(text.contains(SECTION), "{text}");
        assert!(!text.contains("satisfied"), "{text}");
    }

    /// What the branch adds is read from the added lines of the diff, and kept
    /// to the names the scenario binary lists: a helper function added beside
    /// a scenario is not a scenario, and a scenario the diff removes is not an
    /// addition.
    #[test]
    fn added_scenarios_come_from_the_added_lines_of_the_diff() {
        let diff = "\
--- a/tests/scenarios/oauth.rs
+++ b/tests/scenarios/oauth.rs
@@ -10,0 +11,4 @@
+#[test]
+fn oauth_new_grant_is_refused() {
+fn config_with(server: &str) -> String {
@@ -30,0 +40 @@
-fn oauth_removed_scenario() {
";
        let known = names(&[
            "oauth_new_grant_is_refused",
            "oauth_removed_scenario",
            "oauth_untouched_scenario",
        ]);

        assert_eq!(
            scenarios_in(added_lines(diff), &known),
            names(&["oauth_new_grant_is_refused"])
        );
    }

    /// A base the branch is far behind answers "undeclared" with the whole
    /// test suite. The table counts the rest instead of printing a page of it.
    #[test]
    fn a_long_list_is_cut_and_the_rest_counted() {
        let scenarios: Vec<String> = (0..SHOWN + 3)
            .map(|index| format!("api_scenario_{index:02}"))
            .collect();
        let report = evaluate(
            48,
            declared(&[]),
            &scenarios.iter().cloned().collect(),
            &scenarios.iter().cloned().collect(),
            &scenarios.iter().cloned().collect(),
        );

        let text = render(&report);

        assert!(text.contains("api_scenario_09"), "{text}");
        assert!(!text.contains("api_scenario_10"), "{text}");
        assert!(text.contains("(+3 more; --json lists them all)"), "{text}");
    }

    #[test]
    fn the_report_names_every_finding() {
        let report = evaluate(
            48,
            declared(&["secret_ref_resolves", "Secret ref"]),
            &names(&["secret_ref_cache_is_reused"]),
            &names(&["secret_ref_cache_is_reused"]),
            &names(&[]),
        );

        let text = render(&report);

        assert!(text.contains("incomplete"), "{text}");
        assert!(text.contains("missing"), "{text}");
        assert!(text.contains("undeclared"), "{text}");
        assert!(text.contains("unregistered"), "{text}");
        assert!(text.contains("ill-named"), "{text}");
        assert!(
            text.contains("Secret ref: not a lowercase snake_case name"),
            "{text}"
        );
    }
}
