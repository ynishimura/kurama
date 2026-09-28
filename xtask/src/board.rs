//! The GitHub Project that says which issue someone is already working on.
//!
//! Several agents share one account in this repository, so an assignee cannot
//! say who took an issue: assignees are a list, and two agents adding
//! themselves both succeed. The Project's `Status` field is a single value, so
//! `worktree add` can set it to `In progress` and `ready` can skip what is
//! already there. It is a declaration and not a lock -- two agents writing the
//! same value cannot be told apart -- and what it buys instead is a board a
//! person can read.

use std::process::{Command, Output};

/// The project this repository's issues live in. Every open issue is an item
/// of it, and new issues are added automatically; an issue that is missing is
/// reported rather than treated as unclaimed.
pub(crate) const PROJECT: u64 = 3;

pub(crate) const BACKLOG: &str = "Backlog";
pub(crate) const READY: &str = "Ready";
pub(crate) const IN_PROGRESS: &str = "In progress";
pub(crate) const IN_REVIEW: &str = "In review";
pub(crate) const DONE: &str = "Done";

/// A status that says someone is on it already.
const TAKEN: [&str; 3] = [IN_PROGRESS, IN_REVIEW, DONE];

/// A status that claims the issue can be started.
const STARTABLE: [&str; 2] = [READY, IN_PROGRESS];

/// A parent issue: the work is in its children, so an agent never implements
/// it directly. An issue with GitHub sub-issues is one without the label; the
/// label marks a parent whose children are not written yet.
pub(crate) const TRACKER: &str = "tracker";

/// Only a person can close it: real hardware, real credentials, a legal or
/// billing decision, a repository setting.
pub(crate) const NEEDS_HUMAN: &str = "needs-human";

/// What GitHub says about one open issue.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct IssueState {
    pub(crate) number: u64,
    pub(crate) title: String,
    /// The issue body, read here so that resolving `Affects:` for every
    /// candidate costs no request of its own.
    pub(crate) body: String,
    pub(crate) labels: Vec<String>,
    /// How many sub-issues GitHub lists under it, open or closed.
    pub(crate) sub_issues: u64,
    pub(crate) blockers: Vec<Blocker>,
    /// `None` when the issue is not an item of the project.
    pub(crate) status: Option<String>,
    /// When the status last changed, which is the only trace a crashed agent
    /// leaves behind.
    pub(crate) status_updated: Option<String>,
    /// The ids a status change needs, read with everything else so a write
    /// costs one request. `None` when the issue is not an item of the project.
    #[serde(skip)]
    pub(crate) item: Option<Item>,
}

/// Where one issue sits on the board: the item to edit, and the Status field
/// with the ids of its options.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Item {
    pub(crate) project_id: String,
    pub(crate) item_id: String,
    pub(crate) field_id: String,
    /// `(id, name)` of every Status option.
    pub(crate) options: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct Blocker {
    pub(crate) number: u64,
    pub(crate) open: bool,
}

/// Why an issue is, or is not, something an agent may pick up.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "standing", rename_all = "kebab-case")]
pub(crate) enum Standing {
    /// Nothing on GitHub's side stops an agent from starting it.
    Ready,
    Tracker,
    NeedsHuman,
    Blocked {
        by: Vec<u64>,
    },
    Taken {
        status: String,
    },
    /// Not an item of the project. "Not started" and "not tracked" must not
    /// read the same, so this is reported rather than counted as unclaimed.
    OffBoard,
    /// The board says it can be started, the dependencies say it cannot.
    Disagrees {
        status: String,
        by: Vec<u64>,
    },
}

impl Standing {
    /// Whether this standing is something to act on rather than to work on.
    /// `ready` reports these instead of silently dropping them.
    pub(crate) fn is_problem(&self) -> bool {
        matches!(self, Standing::OffBoard | Standing::Disagrees { .. })
    }
}

/// What stands between an agent and this issue. The order is the order of the
/// checks: a disagreement between the board and the dependencies outranks
/// everything, because whichever of the two is wrong has to be fixed before
/// either can be trusted.
pub(crate) fn standing(issue: &IssueState) -> Standing {
    let open: Vec<u64> = issue
        .blockers
        .iter()
        .filter(|blocker| blocker.open)
        .map(|blocker| blocker.number)
        .collect();
    let Some(status) = issue.status.as_deref() else {
        return Standing::OffBoard;
    };
    if !open.is_empty() && STARTABLE.contains(&status) {
        return Standing::Disagrees {
            status: status.to_string(),
            by: open,
        };
    }
    if issue.sub_issues > 0 || issue.labels.iter().any(|label| label == TRACKER) {
        return Standing::Tracker;
    }
    if issue.labels.iter().any(|label| label == NEEDS_HUMAN) {
        return Standing::NeedsHuman;
    }
    if TAKEN.contains(&status) {
        return Standing::Taken {
            status: status.to_string(),
        };
    }
    if !open.is_empty() {
        return Standing::Blocked { by: open };
    }
    Standing::Ready
}

// ---------------------------------------------------------------------------
// reading
// ---------------------------------------------------------------------------

/// Every open issue with its labels, its blockers and its status, one
/// request per page of `PAGE` issues. Asking REST for the dependencies of each issue in turn costs a
/// round trip per issue, which is minutes for a command meant to be run before
/// every pick.
pub(crate) fn open_issues() -> Result<Vec<IssueState>, String> {
    let repository = name_with_owner()?;
    let (owner, name) = repository
        .split_once('/')
        .ok_or_else(|| format!("`{repository}` is not <owner>/<name>"))?;
    let mut issues: Vec<IssueState> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut args = vec![
            "api".to_string(),
            "graphql".into(),
            "-f".into(),
            format!("owner={owner}"),
            "-f".into(),
            format!("name={name}"),
            "-F".into(),
            format!("first={PAGE}"),
            "-f".into(),
            format!("query={QUERY}"),
        ];
        if let Some(cursor) = &after {
            args.extend(["-f".into(), format!("after={cursor}")]);
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let value: serde_json::Value = serde_json::from_str(&gh(&args)?)
            .map_err(|error| format!("gh api graphql: {error}"))?;
        issues.extend(parse_issues(&value, PROJECT));
        let page = &value["data"]["repository"]["issues"]["pageInfo"];
        match page["endCursor"].as_str() {
            Some(cursor) if page["hasNextPage"].as_bool() == Some(true) => {
                after = Some(cursor.to_string());
            }
            _ => break,
        }
    }
    if issues.is_empty() {
        return Err("no open issue was read from GitHub".into());
    }
    if issues.iter().all(|issue| issue.status.is_none()) {
        return Err(format!(
            "no open issue is an item of project {PROJECT}; \
             check the project number in xtask/src/board.rs"
        ));
    }
    Ok(issues)
}

/// How many open issues one request reads: GitHub's largest page. A board
/// that fits one page is one request.
const PAGE: u64 = 100;

const QUERY: &str = "\
query($owner: String!, $name: String!, $first: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    issues(first: $first, after: $after, states: OPEN) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number
        title
        body
        labels(first: 20) { nodes { name } }
        subIssuesSummary { total }
        blockedBy(first: 20) { nodes { number state } }
        projectItems(first: 10) {
          nodes {
            id
            project {
              id
              number
              field(name: \"Status\") {
                ... on ProjectV2SingleSelectField { id options { id name } }
              }
            }
            status: fieldValueByName(name: \"Status\") {
              ... on ProjectV2ItemFieldSingleSelectValue { name updatedAt }
            }
          }
        }
      }
    }
  }
}";

/// The GraphQL answer -> what the commands read. A field the query could not
/// resolve is absent rather than wrong: an issue with no item in `project`
/// keeps `status: None`, which `standing` reports as `OffBoard`.
fn parse_issues(value: &serde_json::Value, project: u64) -> Vec<IssueState> {
    let nodes = value["data"]["repository"]["issues"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    nodes
        .iter()
        .filter_map(|node| {
            let number = node["number"].as_u64()?;
            let item = node["projectItems"]["nodes"]
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item["project"]["number"].as_u64() == Some(project))
                })
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Some(IssueState {
                number,
                title: node["title"].as_str().unwrap_or_default().to_string(),
                body: node["body"].as_str().unwrap_or_default().to_string(),
                labels: names(&node["labels"]["nodes"]),
                sub_issues: node["subIssuesSummary"]["total"].as_u64().unwrap_or(0),
                blockers: node["blockedBy"]["nodes"]
                    .as_array()
                    .map(|nodes| {
                        nodes
                            .iter()
                            .filter_map(|blocker| {
                                Some(Blocker {
                                    number: blocker["number"].as_u64()?,
                                    open: blocker["state"].as_str() == Some("OPEN"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                status: item["status"]["name"].as_str().map(str::to_string),
                status_updated: item["status"]["updatedAt"].as_str().map(str::to_string),
                item: parse_item(&item),
            })
        })
        .collect()
}

fn names(nodes: &serde_json::Value) -> Vec<String> {
    nodes
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| node["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// writing
// ---------------------------------------------------------------------------

/// The ids of one project item, when the query resolved all of them.
fn parse_item(item: &serde_json::Value) -> Option<Item> {
    let field = &item["project"]["field"];
    Some(Item {
        project_id: item["project"]["id"].as_str()?.to_string(),
        item_id: item["id"].as_str()?.to_string(),
        field_id: field["id"].as_str()?.to_string(),
        options: field["options"]
            .as_array()?
            .iter()
            .filter_map(|option| {
                Some((
                    option["id"].as_str()?.to_string(),
                    option["name"].as_str()?.to_string(),
                ))
            })
            .collect(),
    })
}

/// Move one issue to `status`. The caller decides whether it may: this writes
/// what it is told, so that "who may take an issue" stays in one place. The
/// ids come from the `open_issues` the caller already read, so this is one
/// request.
pub(crate) fn set_status(issue: &IssueState, status: &str) -> Result<(), String> {
    set_statuses(&[(issue, status)])
        .pop()
        .map(|(_, result)| result)
        .unwrap_or_else(|| Err(format!("#{} was not written", issue.number)))
}

/// Move several issues in one request: one aliased mutation per issue. The
/// answer is per issue, because a request that fails for one item still
/// writes the others, and "all or nothing" is not what GitHub does.
pub(crate) fn set_statuses(changes: &[(&IssueState, &str)]) -> Vec<(u64, Result<(), String>)> {
    let mut results: Vec<(u64, Result<(), String>)> = Vec::new();
    let mut writes: Vec<(u64, String)> = Vec::new();
    for (issue, status) in changes {
        match mutation_field(issue, status) {
            Ok(field) => writes.push((issue.number, field)),
            Err(error) => results.push((issue.number, Err(error))),
        }
    }
    if !writes.is_empty() {
        let query = format!(
            "mutation {{\n{}\n}}",
            writes
                .iter()
                .map(|(_, field)| field.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let numbers: Vec<u64> = writes.iter().map(|(number, _)| *number).collect();
        match gh_answer(&["api", "graphql", "-f", &format!("query={query}")]) {
            Ok(answer) => results.extend(write_results(&answer, &numbers)),
            Err(error) => results.extend(numbers.iter().map(|n| (*n, Err(error.clone())))),
        }
    }
    results.sort_by_key(|(number, _)| *number);
    results
}

/// The alias a write of issue `number` answers under.
fn alias(number: u64) -> String {
    format!("i{number}")
}

/// One aliased `updateProjectV2ItemFieldValue`. The ids are GitHub node ids,
/// which hold no quote, so they are written into the query as they are.
fn mutation_field(issue: &IssueState, status: &str) -> Result<String, String> {
    let item = issue.item.as_ref().ok_or_else(|| {
        format!(
            "issue #{} is not an item of project {PROJECT}",
            issue.number
        )
    })?;
    let option = item
        .options
        .iter()
        .find(|(_, name)| name == status)
        .map(|(id, _)| id)
        .ok_or_else(|| {
            format!(
                "issue #{} has no `{status}` status in project {PROJECT}",
                issue.number
            )
        })?;
    Ok(format!(
        "  {}: updateProjectV2ItemFieldValue(input: {{ projectId: \"{}\", itemId: \"{}\", \
         fieldId: \"{}\", value: {{ singleSelectOptionId: \"{option}\" }} }}) {{ projectV2Item {{ id }} }}",
        alias(issue.number),
        item.project_id,
        item.item_id,
        item.field_id
    ))
}

/// Each write's outcome from the GraphQL answer: its alias holds the item
/// when it was written, and an error whose `path` starts with the alias says
/// why it was not.
fn write_results(answer: &serde_json::Value, numbers: &[u64]) -> Vec<(u64, Result<(), String>)> {
    numbers
        .iter()
        .map(|number| {
            let alias = alias(*number);
            if !answer["data"][&alias].is_null() {
                return (*number, Ok(()));
            }
            let reason = answer["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|error| error["path"][0].as_str() == Some(alias.as_str()))
                .and_then(|error| error["message"].as_str())
                .unwrap_or("GitHub returned no answer for it");
            (*number, Err(format!("#{number}: {reason}")))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// the command
// ---------------------------------------------------------------------------

const USAGE: &str = "\
usage: cargo xtask board set-status <STATUS> <ISSUE>...

  STATUS   Backlog | Ready | In progress | In review | Done
  ISSUE    open issues of the project; all are written in one request

It writes what it is told, for a stocktaking: it does not ask whether an issue
may be started. `worktree add` and `claim` are what an agent takes work with.
";

/// Every status the board offers, in its order.
const STATUSES: [&str; 5] = [BACKLOG, READY, IN_PROGRESS, IN_REVIEW, DONE];

/// `cargo xtask board set-status`: many issues to one status, in one request,
/// and one line per issue saying whether it was written.
pub fn board(args: &[String]) -> Result<(), String> {
    let (status, numbers) = parse(args)?;
    let issues = open_issues()?;
    let mut changes: Vec<(&IssueState, &str)> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for number in &numbers {
        match issues.iter().find(|issue| issue.number == *number) {
            Some(issue) => changes.push((issue, status)),
            None => failed.push(format!("#{number}: not an open issue")),
        }
    }
    let mut written = 0;
    for (number, result) in set_statuses(&changes) {
        match result {
            Ok(()) => {
                written += 1;
                println!("#{number}\t{status}");
            }
            Err(error) => failed.push(error),
        }
    }
    for failure in &failed {
        eprintln!("not written: {failure}");
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{written} of {} written; the rest are listed above",
            numbers.len()
        ))
    }
}

/// Issues as a person reads them: `#1, #2`.
pub(crate) fn numbers(issues: &[u64]) -> String {
    issues
        .iter()
        .map(|issue| format!("#{issue}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The open issue numbered `issue`.
pub(crate) fn find_open(issues: &[IssueState], issue: u64) -> Result<&IssueState, String> {
    issues
        .iter()
        .find(|state| state.number == issue)
        .ok_or_else(|| format!("issue #{issue} is not open"))
}

/// An issue named on the command line, `42` or `#42`.
pub(crate) fn issue_number(arg: &str, usage: &str) -> Result<u64, String> {
    arg.strip_prefix('#')
        .unwrap_or(arg)
        .parse()
        .map_err(|_| format!("`{arg}` is not an issue number\n\n{usage}"))
}

fn parse(args: &[String]) -> Result<(&'static str, Vec<u64>), String> {
    let [command, status, issues @ ..] = args else {
        return Err(USAGE.to_string());
    };
    if command != "set-status" || issues.is_empty() {
        return Err(USAGE.to_string());
    }
    let status = STATUSES
        .iter()
        .find(|known| known.eq_ignore_ascii_case(status))
        .ok_or_else(|| format!("`{status}` is not a status of the board\n\n{USAGE}"))?;
    let numbers = issues
        .iter()
        .map(|issue| issue_number(issue, USAGE))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((status, numbers))
}

// ---------------------------------------------------------------------------
// gh
// ---------------------------------------------------------------------------

/// The body of an issue, read through `gh`. `conflicts`, `scenarios-check`
/// and `issue-check` read the same bodies, and one reader is what keeps them
/// failing alike when `gh` is missing or the issue cannot be read.
pub(crate) fn issue_body(issue: u64) -> Result<String, String> {
    let output = gh_output(&["issue", "view", &issue.to_string(), "--json", "body"])?;
    if !output.status.success() {
        return Err(format!(
            "gh issue view {issue} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let view: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("gh issue view {issue}: {error}"))?;
    Ok(view["body"].as_str().unwrap_or_default().to_string())
}

fn name_with_owner() -> Result<String, String> {
    let out = gh(&[
        "repo",
        "view",
        "--json",
        "nameWithOwner",
        "-q",
        ".nameWithOwner",
    ])?;
    Ok(out.trim().to_string())
}

/// `gh`, with the two failures that are worth telling apart: the CLI is not
/// there, and the token cannot see Projects. `gh auth login` does not ask for
/// the `project` scope, and `repo` does not cover Projects v2, so the second
/// is what a first run hits.
fn gh(args: &[&str]) -> Result<String, String> {
    let output = gh_output(args)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Err(scope_hint(&stderr).unwrap_or(format!("gh {} failed: {}", args[0], stderr.trim())))
}

/// The JSON `gh api graphql` answered, also when it exits non-zero: a request
/// in which one aliased mutation failed still carries the others' results.
fn gh_answer(args: &[&str]) -> Result<serde_json::Value, String> {
    let output = gh_output(args)?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    match serde_json::from_slice::<serde_json::Value>(&output.stdout) {
        Ok(answer) if answer.get("data").is_some() => Ok(answer),
        _ => {
            Err(scope_hint(&stderr).unwrap_or(format!("gh api graphql failed: {}", stderr.trim())))
        }
    }
}

/// What `gh` printed and how it exited; a `gh` that cannot start is the one
/// failure every caller words alike.
fn gh_output(args: &[&str]) -> Result<Output, String> {
    Command::new("gh")
        .args(args)
        .current_dir(crate::root())
        .output()
        .map_err(|error| {
            format!("gh is not available ({error}); install the GitHub CLI and run `gh auth login`")
        })
}

/// A missing Projects scope reads like an empty board unless it is named.
fn scope_hint(stderr: &str) -> Option<String> {
    stderr.contains("read:project").then(|| {
        format!(
            "the gh token cannot read Projects: {}\n\
             run `gh auth refresh -h github.com -s project` (`repo` does not cover Projects v2)",
            stderr.trim()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(status: Option<&str>) -> IssueState {
        IssueState {
            number: 84,
            title: "a title".into(),
            body: "Affects: verification-harness".into(),
            labels: vec!["enhancement".into()],
            sub_issues: 0,
            blockers: vec![],
            status: status.map(str::to_string),
            status_updated: None,
            item: None,
        }
    }

    #[test]
    fn an_open_unblocked_backlog_issue_is_ready() {
        assert_eq!(standing(&issue(Some(BACKLOG))), Standing::Ready);
        assert_eq!(standing(&issue(Some(READY))), Standing::Ready);
    }

    #[test]
    fn a_status_that_says_someone_is_on_it_is_taken() {
        for status in TAKEN {
            assert_eq!(
                standing(&issue(Some(status))),
                Standing::Taken {
                    status: status.to_string()
                },
                "{status}"
            );
        }
    }

    /// "Not started" and "not tracked" must not read the same: an issue the
    /// board never saw is reported, not offered.
    #[test]
    fn an_issue_that_is_not_an_item_is_off_board() {
        assert_eq!(standing(&issue(None)), Standing::OffBoard);
        assert!(standing(&issue(None)).is_problem());
    }

    #[test]
    fn labels_say_what_an_agent_must_not_pick_up() {
        let mut tracker = issue(Some(BACKLOG));
        tracker.labels.push(TRACKER.into());
        assert_eq!(standing(&tracker), Standing::Tracker);

        let mut human = issue(Some(READY));
        human.labels.push(NEEDS_HUMAN.into());
        assert_eq!(standing(&human), Standing::NeedsHuman);

        // GitHub's own sub-issues make a parent a tracker without the label.
        let mut parent = issue(Some(BACKLOG));
        parent.sub_issues = 2;
        assert_eq!(standing(&parent), Standing::Tracker);

        // Neither is a problem to fix; both are simply not agent work.
        assert!(!standing(&tracker).is_problem());
        assert!(!standing(&human).is_problem());
    }

    #[test]
    fn a_closed_blocker_does_not_block() {
        let mut state = issue(Some(BACKLOG));
        state.blockers = vec![
            Blocker {
                number: 75,
                open: false,
            },
            Blocker {
                number: 74,
                open: false,
            },
        ];
        assert_eq!(standing(&state), Standing::Ready);
    }

    #[test]
    fn an_open_blocker_blocks_and_names_itself() {
        let mut state = issue(Some(BACKLOG));
        state.blockers = vec![
            Blocker {
                number: 75,
                open: true,
            },
            Blocker {
                number: 74,
                open: false,
            },
        ];
        assert_eq!(standing(&state), Standing::Blocked { by: vec![75] });
    }

    /// Whichever of the two is wrong has to be fixed before either can be
    /// trusted, so this outranks the label and the status.
    #[test]
    fn a_startable_status_with_an_open_blocker_is_a_disagreement() {
        for status in STARTABLE {
            let mut state = issue(Some(status));
            state.labels.push(TRACKER.into());
            state.blockers = vec![Blocker {
                number: 75,
                open: true,
            }];
            assert_eq!(
                standing(&state),
                Standing::Disagrees {
                    status: status.to_string(),
                    by: vec![75]
                },
                "{status}"
            );
            assert!(standing(&state).is_problem());
        }
        // Backlog and an open blocker agree with each other.
        let mut agreeing = issue(Some(BACKLOG));
        agreeing.blockers = vec![Blocker {
            number: 75,
            open: true,
        }];
        assert_eq!(standing(&agreeing), Standing::Blocked { by: vec![75] });
    }

    #[test]
    fn the_graphql_answer_becomes_issue_states() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"data":{"repository":{"issues":{"nodes":[
              {"number":84,"title":"claim","body":"Affects: verification-harness","labels":{"nodes":[{"name":"enhancement"}]},
               "blockedBy":{"nodes":[]},
               "projectItems":{"nodes":[
                 {"project":{"number":9},"status":{"name":"Done","updatedAt":"2026-01-01T00:00:00Z"}},
                 {"project":{"number":3},"status":{"name":"In progress","updatedAt":"2026-09-22T15:13:23Z"}}]}},
              {"number":76,"title":"add","labels":{"nodes":[{"name":"enhancement"},{"name":"tracker"}]},
               "subIssuesSummary":{"total":3},
               "blockedBy":{"nodes":[{"number":75,"state":"OPEN"},{"number":74,"state":"CLOSED"}]},
               "projectItems":{"nodes":[]}}
            ]}}}}"#,
        )
        .unwrap();

        let issues = parse_issues(&value, 3);

        assert_eq!(
            issues,
            [
                IssueState {
                    number: 84,
                    title: "claim".into(),
                    body: "Affects: verification-harness".into(),
                    labels: vec!["enhancement".into()],
                    sub_issues: 0,
                    blockers: vec![],
                    status: Some(IN_PROGRESS.into()),
                    status_updated: Some("2026-09-22T15:13:23Z".into()),
                    // The query answered no ids, so there is nothing to write with.
                    item: None,
                },
                IssueState {
                    number: 76,
                    title: "add".into(),
                    body: String::new(),
                    labels: vec!["enhancement".into(), "tracker".into()],
                    sub_issues: 3,
                    blockers: vec![
                        Blocker {
                            number: 75,
                            open: true
                        },
                        Blocker {
                            number: 74,
                            open: false
                        }
                    ],
                    // An item of no project at all, not of another one.
                    status: None,
                    status_updated: None,
                    item: None,
                },
            ]
        );
    }

    #[test]
    fn the_item_of_this_project_carries_the_ids_the_mutation_needs() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"data":{"repository":{"issues":{"nodes":[
              {"number":84,"title":"t","body":"","labels":{"nodes":[]},"blockedBy":{"nodes":[]},
               "projectItems":{"nodes":[
                 {"id":"OTHER","project":{"id":"P9","number":9,
                  "field":{"id":"F9","options":[{"id":"o9","name":"Ready"}]}},"status":null},
                 {"id":"ITEM","project":{"id":"P3","number":3,
                  "field":{"id":"F3","options":[
                    {"id":"backlog","name":"Backlog"},{"id":"progress","name":"In progress"}]}},
                  "status":{"name":"Backlog","updatedAt":"2026-09-22T15:13:23Z"}}]}}
            ]}}}}"#,
        )
        .unwrap();

        let issues = parse_issues(&value, 3);
        let item = issues[0].item.as_ref().expect("the item of project 3");

        assert_eq!(item.project_id, "P3");
        assert_eq!(item.item_id, "ITEM");
        assert_eq!(item.field_id, "F3");
        let field = mutation_field(&issues[0], IN_PROGRESS).unwrap();
        assert!(
            field.contains("i84: updateProjectV2ItemFieldValue"),
            "{field}"
        );
        assert!(field.contains("itemId: \"ITEM\""), "{field}");
        assert!(
            field.contains("singleSelectOptionId: \"progress\""),
            "{field}"
        );
        // A status the field does not offer is not guessed at.
        assert!(mutation_field(&issues[0], "Shipped").is_err());
    }

    /// Writing several items in one request must not hide which were
    /// written: GitHub applies the ones it can and names the others.
    #[test]
    fn a_partial_failure_names_the_items_that_were_not_written() {
        let answer: serde_json::Value = serde_json::from_str(
            r#"{"data":{"i84":{"projectV2Item":{"id":"A"}},"i85":null,"i86":{"projectV2Item":{"id":"C"}}},
                "errors":[{"path":["i85"],"message":"Could not resolve to a node"}]}"#,
        )
        .unwrap();

        let results = write_results(&answer, &[84, 85, 86, 87]);

        assert_eq!(results[0], (84, Ok(())));
        assert_eq!(
            results[1],
            (85, Err("#85: Could not resolve to a node".to_string()))
        );
        assert_eq!(results[2], (86, Ok(())));
        assert_eq!(
            results[3],
            (87, Err("#87: GitHub returned no answer for it".to_string()))
        );
    }

    /// An issue the board never saw is refused before any request.
    #[test]
    fn an_issue_with_no_item_is_not_sent() {
        let error = mutation_field(&issue(Some(BACKLOG)), IN_PROGRESS).unwrap_err();

        assert!(error.contains("not an item of project"), "{error}");
    }

    /// A token without the Projects scope answers like an empty board, so the
    /// scope is named rather than left to look like "nothing to do".
    #[test]
    fn a_missing_projects_scope_is_named() {
        let hint = scope_hint("error: Your token has not been granted the required scopes to execute this query. The 'number' field requires one of the following scopes: ['read:project']").expect("a hint");
        assert!(
            hint.contains("gh auth refresh -h github.com -s project"),
            "{hint}"
        );
        assert!(scope_hint("could not resolve to an Issue").is_none());
    }

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn set_status_takes_a_status_of_the_board_and_issue_numbers() {
        assert_eq!(
            parse(&words(&["set-status", "in progress", "84", "#85"])).unwrap(),
            (IN_PROGRESS, vec![84, 85])
        );
        assert!(parse(&words(&["set-status", "Shipped", "84"])).is_err());
        assert!(parse(&words(&["set-status", "Ready"])).is_err());
        assert!(parse(&words(&["set-status", "Ready", "HEAD"])).is_err());
        assert!(parse(&words(&["move", "Ready", "84"])).is_err());
    }
}
