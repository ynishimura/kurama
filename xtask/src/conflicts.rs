//! `cargo xtask conflicts <issue> <issue> ...`: whether issues can be
//! implemented at the same time. Each issue body declares one `Affects:` line
//! naming the features it touches; this resolves those names to the files the
//! features claim and reports, for every pair, whether the two branches would
//! edit the same file.

use std::collections::BTreeSet;

use crate::{FeatureMap, feature_entries, load_features, matches_entry};

const USAGE: &str = "usage: cargo xtask conflicts <issue> <issue> [<issue> ...] [--json]";

/// The line an issue body carries, e.g. `Affects: database, config`.
const AFFECTS: &str = "Affects:";

/// Files every feature's work funnels through and that cannot be split: the
/// clap definition, the dispatch table, the `ClientKind` table, the error
/// classification, and the xtask subcommand branch with its HELP. Two issues
/// that meet only here can still run in parallel, as long as both append at
/// the end (AGENTS.md, parallel work).
const HUB_FILES: [&str; 5] = [
    "src/shell/cli/args.rs",
    "src/shell/cli/client.rs",
    "src/shell/cli/dispatch.rs",
    "src/shell/cli/error_code.rs",
    "xtask/src/main.rs",
];

/// What one issue says it touches, and the files that follow from it.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Declaration {
    pub(crate) issue: u64,
    pub(crate) features: Vec<String>,
    pub(crate) files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Verdict {
    /// No file in common: the two can be implemented at the same time.
    Parallel,
    /// Only hub files in common: parallel as long as both append at the end.
    AppendOnly,
    /// A feature file in common: one branch at a time.
    Serial,
}

#[derive(Debug, serde::Serialize)]
struct Pair {
    a: u64,
    b: u64,
    verdict: Verdict,
    /// Features both issues declare: what a person acts on.
    shared_features: Vec<String>,
    /// Files both issues would edit. Two issues can share a file without
    /// sharing a feature, so this is what the verdict is made of.
    shared: Vec<String>,
}

pub fn conflicts(args: &[String]) -> Result<(), String> {
    let mut json = false;
    let mut issues: Vec<u64> = Vec::new();
    for arg in args {
        if arg == "--json" {
            json = true;
            continue;
        }
        let number = crate::board::issue_number(arg, USAGE)?;
        if !issues.contains(&number) {
            issues.push(number);
        }
    }
    if issues.len() < 2 {
        return Err(format!("two issues or more are needed\n\n{USAGE}"));
    }

    let features = load_features()?;
    let declarations: Vec<Declaration> = issues
        .into_iter()
        .map(|issue| resolve(issue, affects(issue)?, &features))
        .collect::<Result<_, String>>()?;
    let pairs = judge(&declarations, &claimed_entries(&features));

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "issues": declarations,
                "pairs": pairs,
            }))
            .unwrap()
        );
    } else {
        print_report(&declarations, &pairs);
    }
    Ok(())
}

/// How many shared files the table names before it counts the rest; `--json`
/// carries all of them.
const SHOWN_FILES: usize = 3;

fn print_report(declarations: &[Declaration], pairs: &[Pair]) {
    for declaration in declarations {
        println!(
            "#{}  {}  ({} file(s))",
            declaration.issue,
            declaration.features.join(", "),
            declaration.files.len()
        );
    }
    for pair in pairs {
        println!("\n#{} + #{}  {}", pair.a, pair.b, pair.verdict.label());
        if !pair.shared_features.is_empty() {
            println!("  features: {}", pair.shared_features.join(", "));
        }
        if !pair.shared.is_empty() {
            let shown: Vec<&str> = pair
                .shared
                .iter()
                .take(SHOWN_FILES)
                .map(String::as_str)
                .collect();
            let rest = pair.shared.len().saturating_sub(shown.len());
            println!(
                "  files:    {}{}",
                shown.join(", "),
                if rest > 0 {
                    format!(" (+{rest} more)")
                } else {
                    String::new()
                }
            );
        }
    }
    println!(
        "\nparallel: no file in common. \
         append-only: only files that cannot be split, so append at the end. \
         serial: one branch at a time."
    );
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Parallel => "parallel",
            Verdict::AppendOnly => "append-only",
            Verdict::Serial => "serial",
        }
    }
}

/// The `Affects:` line of a body, or `None` when it carries none.
fn parse_affects(body: &str) -> Option<Vec<String>> {
    let line = body
        .lines()
        .find_map(|line| line.trim().strip_prefix(AFFECTS))?;
    Some(
        line.split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// What issue `issue` declares, read through `gh`. An issue that declares
/// nothing is an error, not an empty file set: "nothing in common" and
/// "nothing was declared" must not read the same.
fn affects(issue: u64) -> Result<Vec<String>, String> {
    affects_in(&crate::board::issue_body(issue)?, issue)
}

fn affects_in(body: &str, issue: u64) -> Result<Vec<String>, String> {
    let names = parse_affects(body).ok_or_else(|| {
        format!(
            "issue #{issue} has no `{AFFECTS}` line; add one naming the features it touches, \
             e.g. `{AFFECTS} database, config` (names from .agent/features/)"
        )
    })?;
    if names.is_empty() {
        return Err(format!(
            "issue #{issue}: the `{AFFECTS}` line names no feature"
        ));
    }
    if let Some(unreadable) = names.iter().find(|name| !is_declaration(name)) {
        return Err(format!(
            "issue #{issue}: the `{AFFECTS}` line cannot be read at `{unreadable}`; \
             it holds only `<feature>` or `<feature>/<path>` names separated by commas, \
             so a note about them goes on the next line"
        ));
    }
    Ok(names)
}

/// Whether a comma-separated item of the `Affects:` line can be a feature name
/// or a `<feature>/<path>`. A note written on the line (a space, a full-width
/// parenthesis) is not an unknown feature but a line this cannot read, and
/// the two need different fixes.
fn is_declaration(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

/// Declarations -> the files they stand for. A name is a feature, and the
/// issue takes every file that feature claims; `<feature>/<path>` narrows it
/// to one path, which the feature has to claim -- a directory entry is enough,
/// so a file the issue is about to write is inside the feature before it
/// exists. An unknown feature and a path outside the feature are separate
/// errors: "no file in common" and "the declaration cannot be read" must not
/// look the same.
/// What an issue declares, from a body the caller already has. `ready` reads
/// every body in one request, so resolving a candidate costs nothing more.
pub(crate) fn declaration_from(
    issue: u64,
    body: &str,
    features: &FeatureMap,
) -> Result<Declaration, String> {
    resolve(issue, affects_in(body, issue)?, features)
}

fn resolve(issue: u64, names: Vec<String>, features: &FeatureMap) -> Result<Declaration, String> {
    let mut declared: Vec<String> = Vec::new();
    let mut files: BTreeSet<String> = BTreeSet::new();
    for name in &names {
        let (feature_name, path) = match name.split_once('/') {
            Some((feature_name, path)) => (feature_name, Some(path)),
            None => (name.as_str(), None),
        };
        let feature = features.get(feature_name).ok_or_else(|| {
            format!(
                "issue #{issue} names unknown feature `{feature_name}`; known: {:?}",
                features.keys().collect::<Vec<_>>()
            )
        })?;
        match path {
            None => files.extend(feature_entries(feature).cloned()),
            Some(path) => {
                let claimed = feature_entries(feature)
                    .any(|entry| entry == path || matches_entry(entry, path.trim_end_matches('/')));
                if !claimed {
                    return Err(format!(
                        "issue #{issue} declares `{name}`, but `{feature_name}` does not claim \
                         `{path}`; it claims {:?}",
                        feature_entries(feature).collect::<Vec<_>>()
                    ));
                }
                files.insert(path.to_string());
            }
        }
        if !declared.iter().any(|name| name == feature_name) {
            declared.push(feature_name.to_string());
        }
    }
    Ok(Declaration {
        issue,
        features: declared,
        files: files.into_iter().collect(),
    })
}

/// What two issues share, and what that makes of them. `ready` asks this of a
/// candidate and every running issue, so the two commands cannot drift.
pub(crate) fn verdict(
    a: &Declaration,
    b: &Declaration,
    claimed: &BTreeSet<String>,
) -> (Verdict, Vec<String>) {
    let shared = shared_entries(&a.files, &b.files, claimed);
    let verdict = if shared.is_empty() {
        Verdict::Parallel
    } else if shared
        .iter()
        .all(|entry| HUB_FILES.contains(&entry.as_str()))
    {
        Verdict::AppendOnly
    } else {
        Verdict::Serial
    };
    (verdict, shared)
}

fn judge(declarations: &[Declaration], claimed: &BTreeSet<String>) -> Vec<Pair> {
    let mut pairs = Vec::new();
    for (index, a) in declarations.iter().enumerate() {
        for b in &declarations[index + 1..] {
            let (verdict, shared) = verdict(a, b, claimed);
            pairs.push(Pair {
                a: a.issue,
                b: b.issue,
                verdict,
                shared_features: a
                    .features
                    .iter()
                    .filter(|name| b.features.contains(name))
                    .cloned()
                    .collect(),
                shared,
            });
        }
    }
    pairs
}

/// Entries of either side that cover an entry of the other, both spellings
/// kept: a directory entry and a file inside it are both worth printing.
fn shared_entries(a: &[String], b: &[String], claimed: &BTreeSet<String>) -> Vec<String> {
    let mut shared: BTreeSet<String> = BTreeSet::new();
    for left in a {
        for right in b {
            if entries_overlap(left, right, claimed) {
                shared.insert(left.clone());
                shared.insert(right.clone());
            }
        }
    }
    shared.into_iter().collect()
}

/// Two entries are the same work when they name the same path, or when one is
/// a directory holding the other and no feature claims the held one by name.
/// The more precise claim is the owner: `tests/fakes/session-manager-plugin`
/// belongs to `database`, not to the `tests/fakes/` of `verification-harness`.
/// `matches_entry` is what `impact` asks of a changed file, so a directory
/// entry means the same thing in both commands.
fn entries_overlap(a: &str, b: &str, claimed: &BTreeSet<String>) -> bool {
    if a == b {
        return true;
    }
    (matches_entry(a, b.trim_end_matches('/')) && !claimed.contains(b))
        || (matches_entry(b, a.trim_end_matches('/')) && !claimed.contains(a))
}

/// Every path the feature map claims, at the precision it claims it.
pub(crate) fn claimed_entries(features: &FeatureMap) -> BTreeSet<String> {
    features
        .values()
        .flat_map(feature_entries)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(issue: u64, files: &[&str]) -> Declaration {
        Declaration {
            issue,
            features: vec![],
            files: files.iter().map(|file| file.to_string()).collect(),
        }
    }

    /// The feature map's own claims, for the cases that do not depend on them.
    fn nothing_claimed() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn verdict(a: &[&str], b: &[&str]) -> Verdict {
        judge(&[declared(1, a), declared(2, b)], &nothing_claimed())[0].verdict
    }

    #[test]
    fn an_affects_line_names_the_features_an_issue_touches() {
        let body = "## 目的\n\nAffects: database, config\n\n親: #50\n";

        assert_eq!(
            parse_affects(body),
            Some(vec!["database".to_string(), "config".to_string()])
        );
    }

    /// 2026-09-23: a note written on the line was reported as an unknown
    /// feature named `verification-harness （LICENSE `.
    #[test]
    fn a_note_on_the_affects_line_is_an_unreadable_line_not_an_unknown_feature() {
        let body = "Affects: verification-harness （LICENSE / THIRD-PARTY-NOTICES は管理外）\n";

        let error = affects_in(body, 70).unwrap_err();

        assert!(error.contains("cannot be read"), "{error}");
        assert!(error.contains("next line"), "{error}");
        assert!(!error.contains("unknown feature"), "{error}");
        assert_eq!(
            affects_in("Affects: xtask/xtask/src/main.rs, tui\n", 70).unwrap(),
            ["xtask/xtask/src/main.rs", "tui"]
        );
    }

    #[test]
    fn a_body_without_an_affects_line_is_not_a_declaration() {
        assert_eq!(
            parse_affects("## 目的\n\nこの issue は何も宣言しない\n"),
            None
        );
    }

    #[test]
    fn issues_that_share_no_file_run_in_parallel() {
        let pair = verdict(&["src/shell/tui/"], &["src/workflows/oauth_token/"]);

        assert_eq!(pair, Verdict::Parallel);
    }

    #[test]
    fn issues_that_share_a_feature_file_are_serial() {
        let pair = verdict(
            &["src/shell/cli/commands/api.rs"],
            &["src/shell/cli/commands/api.rs", "src/shell/cli/client.rs"],
        );

        assert_eq!(pair, Verdict::Serial);
    }

    /// The hub files cannot be split, so meeting there is not a reason to
    /// serialize: it is a reason to append at the end.
    #[test]
    fn issues_that_share_only_a_hub_file_append_instead_of_serializing() {
        let pair = verdict(
            &["src/shell/cli/args.rs", "src/shell/tui/"],
            &["src/shell/cli/args.rs", "src/workflows/oauth_token/"],
        );

        assert_eq!(pair, Verdict::AppendOnly);
        assert_eq!(
            judge(
                &[
                    declared(1, &["src/shell/cli/args.rs", "src/shell/tui/"]),
                    declared(2, &["src/shell/cli/args.rs"]),
                ],
                &nothing_claimed()
            )[0]
            .shared,
            ["src/shell/cli/args.rs"]
        );
    }

    /// A directory entry owns the files under it, so one feature's
    /// `src/adapters/database/` and another's file inside it are the same work.
    #[test]
    fn a_directory_entry_and_a_file_inside_it_are_shared() {
        let claimed = nothing_claimed();

        assert!(entries_overlap(
            "src/adapters/database/",
            "src/adapters/database/postgres.rs",
            &claimed
        ));
        assert!(entries_overlap(
            "src/adapters/",
            "src/adapters/database/",
            &claimed
        ));
        assert!(!entries_overlap(
            "src/adapters/aws/",
            "src/adapters/auth/",
            &claimed
        ));
    }

    /// The more precise claim is the owner. `tests/fakes/session-manager-plugin`
    /// is `database`'s, so an issue that only claims the `tests/fakes/` of
    /// `verification-harness` does not touch it.
    #[test]
    fn a_directory_does_not_own_a_file_another_feature_claims_by_name() {
        let claimed: BTreeSet<String> = ["tests/fakes/session-manager-plugin".to_string()].into();

        assert!(!entries_overlap(
            "tests/fakes/",
            "tests/fakes/session-manager-plugin",
            &claimed
        ));
        assert!(
            entries_overlap("tests/fakes/", "tests/fakes/op", &claimed),
            "a file no feature claims by name stays with the directory"
        );
    }

    /// #36 (tui, database) writes new TUI files and #47 (verification-harness)
    /// prepares the repository for release. They met only because
    /// `tests/fakes/` holds `tests/fakes/session-manager-plugin`.
    #[test]
    fn a_fake_of_one_feature_does_not_serialize_the_harness() {
        let features = load_features().unwrap();
        let a = resolve(
            36,
            vec!["tui".to_string(), "database".to_string()],
            &features,
        )
        .unwrap();
        let b = resolve(47, vec!["verification-harness".to_string()], &features).unwrap();

        let pairs = judge(&[a, b], &claimed_entries(&features));

        assert_eq!(pairs[0].verdict, Verdict::Parallel, "{:?}", pairs[0].shared);
    }

    /// Two features claiming the same file at the same precision is a real
    /// ambiguity, and stays serial.
    #[test]
    fn a_file_two_features_claim_alike_stays_shared() {
        let features = load_features().unwrap();
        let a = resolve(40, vec!["api-client".to_string()], &features).unwrap();
        let b = resolve(49, vec!["oauth".to_string()], &features).unwrap();

        let pairs = judge(&[a, b], &claimed_entries(&features));

        assert_eq!(pairs[0].verdict, Verdict::Serial);
        assert_eq!(pairs[0].shared, ["src/shell/api_runtime_tests.rs"]);
    }

    #[test]
    fn a_declaration_may_name_one_path_of_a_feature() {
        let features = load_features().unwrap();

        let declaration = resolve(
            53,
            vec!["verification-harness/lefthook.yml".to_string()],
            &features,
        )
        .unwrap();

        assert_eq!(declaration.features, ["verification-harness"]);
        assert_eq!(declaration.files, ["lefthook.yml"]);
    }

    /// A directory the feature claims is a declaration of its own: an issue
    /// that edits the whole of `.agent/features/` says so without listing
    /// every file in it.
    #[test]
    fn a_declaration_may_name_a_directory_entry_of_a_feature() {
        let features = load_features().unwrap();

        let declaration = resolve(
            34,
            vec!["verification-harness/.agent/features/".to_string()],
            &features,
        )
        .unwrap();

        assert_eq!(declaration.files, [".agent/features/"]);
    }

    /// `xtask/` is a directory entry, so a file the issue is about to add is
    /// inside the feature even though nothing has written it yet.
    #[test]
    fn a_path_under_a_directory_entry_need_not_exist_yet() {
        let features = load_features().unwrap();

        let declaration = resolve(
            53,
            vec!["xtask/xtask/src/worktree.rs".to_string()],
            &features,
        )
        .unwrap();

        assert_eq!(declaration.files, ["xtask/src/worktree.rs"]);
    }

    #[test]
    fn a_path_the_feature_does_not_claim_is_its_own_error() {
        let features = load_features().unwrap();

        let error = resolve(
            53,
            vec!["verification-harness/src/main.rs".to_string()],
            &features,
        )
        .unwrap_err();

        assert!(error.contains("src/main.rs"), "{error}");
        assert!(error.contains("verification-harness"), "{error}");
        assert!(
            !error.contains("unknown feature"),
            "a path is not an unknown feature: {error}"
        );
    }

    /// #53 adds `branch-check` and `worktree`, #54 adds `scenarios-check`.
    /// Both append a line to the xtask subcommand branch and its HELP, which
    /// #50 names as a hub: that is not a reason to serialize.
    #[test]
    fn two_xtask_commands_meet_only_in_the_dispatch_and_append() {
        let features = load_features().unwrap();
        let a = resolve(
            53,
            vec![
                "xtask/xtask/src/main.rs".to_string(),
                "xtask/xtask/src/worktree.rs".to_string(),
                "verification-harness/lefthook.yml".to_string(),
            ],
            &features,
        )
        .unwrap();
        let b = resolve(
            54,
            vec![
                "xtask/xtask/src/main.rs".to_string(),
                "xtask/xtask/src/scenarios_check.rs".to_string(),
            ],
            &features,
        )
        .unwrap();

        let pairs = judge(&[a, b], &claimed_entries(&features));

        assert_eq!(pairs[0].verdict, Verdict::AppendOnly);
        assert_eq!(pairs[0].shared, ["xtask/src/main.rs"]);
    }

    /// A renamed hub file would turn "append at the end" into "serial" without
    /// anything saying so.
    #[test]
    fn every_hub_file_exists() {
        for file in HUB_FILES {
            assert!(
                crate::root().join(file).exists(),
                "{file} is in the hub allowlist but not in the repository"
            );
        }
    }

    #[test]
    fn an_unknown_feature_name_is_not_an_empty_file_set() {
        let features = load_features().unwrap();

        let error = resolve(7, vec!["databse".to_string()], &features).unwrap_err();

        assert!(error.contains("databse"), "{error}");
        assert!(error.contains("#7"), "{error}");
    }

    /// #41 extends `kurama api` and #43 changes the same command's error
    /// contract: both claim `api-client`, so they cannot run at the same time.
    #[test]
    fn two_issues_on_the_same_command_are_serial() {
        let features = load_features().unwrap();
        let a = resolve(41, vec!["api-client".to_string()], &features).unwrap();
        let b = resolve(
            43,
            vec!["api-client".to_string(), "errors".to_string()],
            &features,
        )
        .unwrap();

        let pairs = judge(&[a, b], &claimed_entries(&features));

        assert_eq!(pairs[0].verdict, Verdict::Serial);
        assert_eq!(pairs[0].shared_features, ["api-client"]);
        assert!(
            pairs[0]
                .shared
                .contains(&"src/shell/cli/commands/api.rs".to_string()),
            "{:?}",
            pairs[0].shared
        );
    }

    #[test]
    fn features_that_claim_nothing_in_common_are_parallel() {
        let features = load_features().unwrap();
        let a = resolve(36, vec!["tui".to_string()], &features).unwrap();
        let b = resolve(48, vec!["oauth".to_string()], &features).unwrap();

        assert_eq!(
            judge(&[a, b], &claimed_entries(&features))[0].verdict,
            Verdict::Parallel
        );
    }
}
