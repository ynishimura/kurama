//! `cargo xtask ready`: the issues an agent may start right now.
//!
//! Four things have to agree before an issue is work an agent can pick up:
//! GitHub says it is open and unblocked, the board says nobody is on it, the
//! labels say it is not a tracker and not a job for a person, and
//! `conflicts` says it would not edit the files a running branch is editing.
//! This asks all four in one place, and reports -- rather than hides -- the
//! issues it could not judge, so that "nothing to do" and "nothing could be
//! read" never look the same.
//!
//! It also groups the offers one worktree can take together: `ready` issues
//! that declare the same single feature and are serial with each other, which
//! would otherwise pay for a build and a gate one issue at a time
//! (`worktree add <ISSUE>...`).

use std::collections::{BTreeMap, BTreeSet};

use crate::board::{self, IssueState, Standing, numbers};
use crate::conflicts::{self, Declaration, Verdict};
use crate::{FeatureMap, load_features};

const USAGE: &str = "\
usage: cargo xtask ready [--all] [--json] [--agents N]

  --all         also list the trackers and the issues a person has to close,
                which is what the board's Ready column shows
  --json        the same answer as one document
  --agents N    also pick N offers no two of which are serial, to start N
                agents at once; fewer when fewer exist, and says why

Batches are offers that declare one and the same feature and are serial with
each other: one worktree takes them together (`worktree add <ISSUE>...`).
";

/// An `In progress` that has not moved for this long is either a very long
/// session or an agent that died. A person decides which; this only says
/// where to look.
const STALE_HOURS: i64 = 24;

#[derive(Debug, PartialEq)]
struct Request {
    all: bool,
    json: bool,
    agents: Option<usize>,
}

/// An issue an agent may take.
#[derive(Debug, serde::Serialize)]
struct Offer {
    number: u64,
    title: String,
    /// Why it is offered: `ready`, or the label that keeps it out of the
    /// agent queue when `--all` widens the list.
    standing: &'static str,
    /// Other offered issues this one cannot run beside. Starting several
    /// agents at once means picking issues that do not appear in each
    /// other's list.
    serial_with: Vec<u64>,
}

#[derive(Debug, serde::Serialize)]
struct Running {
    number: u64,
    title: String,
    status: String,
    since: Option<String>,
    /// Nothing has moved for `STALE_HOURS`; the worktree may be abandoned.
    stale: bool,
}

impl Running {
    fn of(issue: &IssueState, status: &str, now: chrono::DateTime<chrono::Utc>) -> Self {
        Running {
            number: issue.number,
            title: issue.title.clone(),
            status: status.to_string(),
            since: issue.status_updated.clone(),
            stale: is_stale(issue.status_updated.as_deref(), now),
        }
    }
}

/// An issue that would be ready if a running branch were not in its way.
#[derive(Debug, serde::Serialize)]
struct Held {
    number: u64,
    title: String,
    behind: Vec<u64>,
    /// Running issues whose declaration could not be read, so whether this
    /// one may run beside them is unknown. Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unjudged: Vec<u64>,
}

/// An issue that waits for other issues, with where each of them stands, so
/// the one to start first is visible even when it is outside the tracker.
#[derive(Debug, PartialEq, serde::Serialize)]
struct Waiting {
    number: u64,
    title: String,
    after: Vec<BlockerStanding>,
}

#[derive(Debug, PartialEq, serde::Serialize)]
struct BlockerStanding {
    number: u64,
    /// `ready`, a board status, `blocked`, `tracker`, `needs-human`, or
    /// `open` for an open issue this read did not include.
    standing: String,
}

/// An issue this command could not judge. Reported, never dropped.
#[derive(Debug, serde::Serialize)]
struct Problem {
    number: u64,
    problem: String,
}

pub fn ready(args: &[String]) -> Result<(), String> {
    let request = parse(args)?;
    let issues = board::open_issues()?;
    let features = load_features()?;
    let mut answer = decide(&issues, &features, request.all);
    answer.together = request
        .agents
        .map(|asked| pick_together(&answer.ready, asked));
    if request.json {
        println!("{}", serde_json::to_string_pretty(&answer).unwrap());
    } else {
        print!("{}", render(&answer));
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Request, String> {
    let mut request = Request {
        all: false,
        json: false,
        agents: None,
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--all" => request.all = true,
            "--json" => request.json = true,
            "--agents" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("--agents needs a number\n\n{USAGE}"))?;
                request.agents = Some(
                    value
                        .parse()
                        .map_err(|_| format!("`{value}` is not a number of agents\n\n{USAGE}"))?,
                );
            }
            "--help" | "-h" => return Err(USAGE.to_string()),
            other => return Err(format!("`{other}` is not an option\n\n{USAGE}")),
        }
    }
    Ok(request)
}

#[derive(Debug, serde::Serialize)]
struct Answer {
    project: u64,
    ready: Vec<Offer>,
    running: Vec<Running>,
    held: Vec<Held>,
    /// Issues waiting for open ones, with where those stand.
    blocked: Vec<Waiting>,
    problems: Vec<Problem>,
    /// Serial offers closed over one feature, for one worktree each.
    batches: Vec<Batch>,
    /// What `--agents N` picked; absent without the flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    together: Option<Together>,
}

/// Offers one worktree takes together.
#[derive(Debug, PartialEq, serde::Serialize)]
struct Batch {
    feature: String,
    issues: Vec<u64>,
}

/// Whether `worktree add` takes this offer.
fn startable(offer: &Offer) -> bool {
    offer.standing == "ready"
}

/// Offers that can run at the same time, for `--agents N`.
#[derive(Debug, PartialEq, serde::Serialize)]
struct Together {
    asked: usize,
    picked: Vec<u64>,
    /// Offers left out, each with the picked ones it is serial with. When
    /// fewer than `asked` were picked, this is why no more could be.
    left_out: Vec<LeftOut>,
}

#[derive(Debug, PartialEq, serde::Serialize)]
struct LeftOut {
    number: u64,
    serial_with: Vec<u64>,
}

/// Up to `asked` offers, no two of them serial, taken in issue-number order:
/// the same board gives the same pick, so two people reading it start the
/// same set. Greedy rather than a maximum independent set, which the few
/// dozen offers do not need. The pairs come from `serial_with`, which is
/// `conflicts::verdict`: the rule lives in one place. Only a `ready` offer is
/// picked: `worktree add` refuses the trackers and the jobs for a person that
/// `--all` lists.
fn pick_together(ready: &[Offer], asked: usize) -> Together {
    let mut offers: Vec<&Offer> = ready.iter().filter(|offer| startable(offer)).collect();
    offers.sort_by_key(|offer| offer.number);
    let mut picked: Vec<u64> = Vec::new();
    let mut left_out: Vec<LeftOut> = Vec::new();
    for offer in offers {
        let serial_with: Vec<u64> = offer
            .serial_with
            .iter()
            .filter(|other| picked.contains(other))
            .copied()
            .collect();
        if picked.len() < asked && serial_with.is_empty() {
            picked.push(offer.number);
        } else if !serial_with.is_empty() {
            left_out.push(LeftOut {
                number: offer.number,
                serial_with,
            });
        }
    }
    Together {
        asked,
        picked,
        left_out,
    }
}

/// The whole judgement, with the clock and GitHub already behind it.
fn decide(issues: &[IssueState], features: &FeatureMap, all: bool) -> Answer {
    let now = chrono::Utc::now();
    let mut problems: Vec<Problem> = Vec::new();
    let mut running: Vec<Running> = Vec::new();
    let mut candidates: Vec<(&IssueState, &'static str)> = Vec::new();
    let mut blocked: Vec<Waiting> = Vec::new();

    for issue in issues {
        match board::standing(issue) {
            Standing::Ready => candidates.push((issue, "ready")),
            Standing::Tracker if all => candidates.push((issue, board::TRACKER)),
            Standing::NeedsHuman if all => candidates.push((issue, board::NEEDS_HUMAN)),
            Standing::Blocked { ref by } => blocked.push(Waiting {
                number: issue.number,
                title: issue.title.clone(),
                after: by
                    .iter()
                    .map(|blocker| BlockerStanding {
                        number: *blocker,
                        standing: standing_of(*blocker, issues),
                    })
                    .collect(),
            }),
            Standing::Tracker | Standing::NeedsHuman => {}
            Standing::Taken { ref status } => {
                if status == board::IN_PROGRESS {
                    running.push(Running::of(issue, status, now));
                }
            }
            // A branch someone is running whatever the blockers say: it holds
            // its feature back, and the disagreement is still reported.
            ref disagreement @ Standing::Disagrees { ref status, .. }
                if status == board::IN_PROGRESS =>
            {
                running.push(Running::of(issue, status, now));
                problems.push(Problem {
                    number: issue.number,
                    problem: describe(disagreement),
                });
            }
            other if other.is_problem() => problems.push(Problem {
                number: issue.number,
                problem: describe(&other),
            }),
            _ => {}
        }
    }

    // Declarations come from bodies already read, so judging costs no request.
    let mut declarations: BTreeMap<u64, Declaration> = BTreeMap::new();
    for issue in issues {
        let wanted = candidates
            .iter()
            .any(|(state, _)| state.number == issue.number)
            || running.iter().any(|entry| entry.number == issue.number);
        if !wanted {
            continue;
        }
        match conflicts::declaration_from(issue.number, &issue.body, features) {
            Ok(declaration) => {
                declarations.insert(issue.number, declaration);
            }
            Err(problem) => problems.push(Problem {
                number: issue.number,
                problem,
            }),
        }
    }

    let claimed = conflicts::claimed_entries(features);
    let running_numbers: Vec<u64> = running.iter().map(|entry| entry.number).collect();
    // A running branch whose declaration could not be read may be editing
    // anything, so nothing is said to run beside it.
    let unjudged: Vec<u64> = running_numbers
        .iter()
        .filter(|number| !declarations.contains_key(number))
        .copied()
        .collect();
    let mut ready: Vec<Offer> = Vec::new();
    let mut held: Vec<Held> = Vec::new();
    for (issue, standing) in &candidates {
        let Some(declaration) = declarations.get(&issue.number) else {
            continue; // already reported as a problem
        };
        let behind = serial_against(declaration, &running_numbers, &declarations, &claimed);
        if behind.is_empty() && unjudged.is_empty() {
            ready.push(Offer {
                number: issue.number,
                title: issue.title.clone(),
                standing,
                serial_with: vec![],
            });
        } else {
            held.push(Held {
                number: issue.number,
                title: issue.title.clone(),
                behind,
                unjudged: unjudged.clone(),
            });
        }
    }

    // Which offers cannot run beside each other, so that starting N agents is
    // a matter of reading the list rather than of judging it again.
    let offered: Vec<u64> = ready.iter().map(|offer| offer.number).collect();
    for offer in &mut ready {
        let Some(declaration) = declarations.get(&offer.number) else {
            continue;
        };
        offer.serial_with = serial_against(declaration, &offered, &declarations, &claimed);
    }

    problems.sort_by_key(|problem| problem.number);
    let batches = batches(&ready, &declarations);
    Answer {
        project: board::PROJECT,
        ready,
        running,
        held,
        blocked,
        problems,
        batches,
        together: None,
    }
}

/// Startable offers that declare exactly one feature, grouped by it; an
/// offer joins its group only when it is serial with another member, so a
/// batch is work that would otherwise wait in line for the same files.
fn batches(ready: &[Offer], declarations: &BTreeMap<u64, Declaration>) -> Vec<Batch> {
    let mut by_feature: BTreeMap<&str, Vec<&Offer>> = BTreeMap::new();
    for offer in ready.iter().filter(|offer| startable(offer)) {
        if let Some([feature]) = declarations
            .get(&offer.number)
            .map(|declaration| declaration.features.as_slice())
        {
            by_feature.entry(feature).or_default().push(offer);
        }
    }
    by_feature
        .into_iter()
        .filter_map(|(feature, offers)| {
            let members: Vec<u64> = offers.iter().map(|offer| offer.number).collect();
            let mut issues: Vec<u64> = offers
                .iter()
                .filter(|offer| offer.serial_with.iter().any(|n| members.contains(n)))
                .map(|offer| offer.number)
                .collect();
            issues.sort();
            (!issues.is_empty()).then(|| Batch {
                feature: feature.to_string(),
                issues,
            })
        })
        .collect()
}

/// Which of `others` this issue cannot run beside, judged by the code
/// `cargo xtask conflicts` prints. Hub files are append-only, which is
/// parallel, so only a shared feature file holds an issue back.
fn serial_against(
    declaration: &Declaration,
    others: &[u64],
    declarations: &BTreeMap<u64, Declaration>,
    claimed: &BTreeSet<String>,
) -> Vec<u64> {
    others
        .iter()
        .filter(|other| **other != declaration.issue)
        .filter(|other| {
            declarations.get(other).is_some_and(|against| {
                conflicts::verdict(declaration, against, claimed).0 == Verdict::Serial
            })
        })
        .copied()
        .collect()
}

/// Where an open blocker stands, in one word or a board status.
fn standing_of(number: u64, issues: &[IssueState]) -> String {
    let Some(issue) = issues.iter().find(|issue| issue.number == number) else {
        return "open".to_string();
    };
    match board::standing(issue) {
        Standing::Ready => "ready".to_string(),
        Standing::Taken { status } | Standing::Disagrees { status, .. } => status,
        Standing::Blocked { .. } => "blocked".to_string(),
        Standing::Tracker => board::TRACKER.to_string(),
        Standing::NeedsHuman => board::NEEDS_HUMAN.to_string(),
        Standing::OffBoard => "off the board".to_string(),
    }
}

fn describe(standing: &Standing) -> String {
    match standing {
        Standing::OffBoard => format!(
            "not an item of project {}; the board cannot say whether anyone is on it",
            board::PROJECT
        ),
        Standing::Disagrees { status, by } => format!(
            "the board says `{status}` but {} is still open; move it back or close the blocker",
            numbers(by)
        ),
        other => format!("{other:?}"),
    }
}

fn is_stale(updated: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    updated
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .is_some_and(|stamp| (now - stamp.with_timezone(&chrono::Utc)).num_hours() >= STALE_HOURS)
}

/// How many characters of a title the table keeps.
const TITLE: usize = 56;

fn render(answer: &Answer) -> String {
    let mut out = String::new();
    out.push_str(&format!("ready ({})\n", answer.ready.len()));
    for offer in &answer.ready {
        let tail = if offer.serial_with.is_empty() {
            String::new()
        } else {
            format!("  (not beside {})", numbers(&offer.serial_with))
        };
        let mark = if offer.standing == "ready" {
            String::new()
        } else {
            format!("[{}] ", offer.standing)
        };
        out.push_str(&format!(
            "  #{:<4} {mark}{}{tail}\n",
            offer.number,
            clip(&offer.title)
        ));
    }
    if answer.ready.is_empty() {
        out.push_str("  nothing; every unblocked issue is taken or held\n");
    }

    if !answer.running.is_empty() {
        out.push_str(&format!("\nin progress ({})\n", answer.running.len()));
        for entry in &answer.running {
            let since = entry.since.as_deref().unwrap_or("?");
            let stale = if entry.stale { "  STALE" } else { "" };
            out.push_str(&format!(
                "  #{:<4} {}  since {since}{stale}\n",
                entry.number,
                clip(&entry.title)
            ));
        }
    }
    if answer.running.iter().any(|entry| entry.stale) {
        out.push_str(&format!(
            "  a status that has not moved for {STALE_HOURS}h may be an abandoned worktree:\n  \
             `cargo xtask worktree remove <ISSUE>` puts it back\n"
        ));
    }

    if !answer.held.is_empty() {
        out.push_str(&format!("\nheld ({})\n", answer.held.len()));
        for entry in &answer.held {
            let mut why: Vec<String> = Vec::new();
            if !entry.behind.is_empty() {
                why.push(format!("behind {}", numbers(&entry.behind)));
            }
            if !entry.unjudged.is_empty() {
                why.push(format!(
                    "not judged against {} (its declaration cannot be read)",
                    numbers(&entry.unjudged)
                ));
            }
            out.push_str(&format!(
                "  #{:<4} {}  {}\n",
                entry.number,
                clip(&entry.title),
                why.join("; ")
            ));
        }
    }

    if !answer.blocked.is_empty() {
        out.push_str(&format!("\nblocked ({})\n", answer.blocked.len()));
        for entry in &answer.blocked {
            let after: Vec<String> = entry
                .after
                .iter()
                .map(|blocker| format!("#{} ({})", blocker.number, blocker.standing))
                .collect();
            out.push_str(&format!(
                "  #{:<4} {}  after {}\n",
                entry.number,
                clip(&entry.title),
                after.join(", ")
            ));
        }
    }

    if !answer.problems.is_empty() {
        out.push_str(&format!("\nproblems ({})\n", answer.problems.len()));
        for problem in &answer.problems {
            out.push_str(&format!("  #{:<4} {}\n", problem.number, problem.problem));
        }
    }
    if !answer.batches.is_empty() {
        out.push_str(&format!("\nbatch ({})\n", answer.batches.len()));
        for batch in &answer.batches {
            let listed: Vec<String> = batch.issues.iter().map(u64::to_string).collect();
            out.push_str(&format!(
                "  {}  {}  cargo xtask worktree add {}\n",
                batch.feature,
                numbers(&batch.issues),
                listed.join(" ")
            ));
        }
    }
    if let Some(together) = &answer.together {
        let offered = answer.ready.iter().filter(|offer| startable(offer)).count();
        out.push_str(&render_together(together, offered));
    }
    out
}

fn render_together(together: &Together, offered: usize) -> String {
    let mut out = format!(
        "\nstart together ({} of {} asked)\n",
        together.picked.len(),
        together.asked
    );
    if together.picked.is_empty() {
        out.push_str("  none\n");
    } else {
        out.push_str(&format!("  {}\n", numbers(&together.picked)));
    }
    if together.picked.len() < together.asked {
        if offered == together.picked.len() {
            out.push_str(&format!("  only {offered} issue(s) are offered at all\n"));
        } else {
            out.push_str("  every other offer is serial with one already picked:\n");
            for entry in &together.left_out {
                out.push_str(&format!(
                    "    #{:<4} beside {}\n",
                    entry.number,
                    numbers(&entry.serial_with)
                ));
            }
        }
    }
    out
}

fn clip(title: &str) -> String {
    let mut out: String = title.chars().take(TITLE).collect();
    if out.chars().count() < title.chars().count() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn ready_takes_two_options_and_nothing_else() {
        assert_eq!(
            parse(&[]).unwrap(),
            Request {
                all: false,
                json: false,
                agents: None,
            }
        );
        assert_eq!(
            parse(&args(&["--all", "--json", "--agents", "3"])).unwrap(),
            Request {
                all: true,
                json: true,
                agents: Some(3),
            }
        );
        assert!(parse(&args(&["--agents"])).is_err());
        assert!(parse(&args(&["--agents", "many"])).is_err());
        assert!(parse(&args(&["--stale"])).is_err());
        assert!(parse(&args(&["84"])).is_err());
    }

    #[test]
    fn a_status_that_has_not_moved_for_a_day_is_stale() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-23T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(is_stale(Some("2026-09-22T11:00:00Z"), now));
        assert!(!is_stale(Some("2026-09-23T04:00:00Z"), now));
        // No timestamp is not a claim that the work is stale.
        assert!(!is_stale(None, now));
        assert!(!is_stale(Some("not a timestamp"), now));
    }

    #[test]
    fn a_title_longer_than_the_column_is_marked_where_it_was_cut() {
        assert_eq!(clip("short"), "short");
        let long = "あ".repeat(TITLE + 3);
        assert_eq!(clip(&long).chars().count(), TITLE + 1);
        assert!(clip(&long).ends_with('…'));
    }

    fn state(
        number: u64,
        affects: &str,
        status: Option<&str>,
        labels: &[&str],
        blockers: &[(u64, bool)],
    ) -> IssueState {
        IssueState {
            number,
            title: format!("issue {number}"),
            body: format!("Affects: {affects}\n\n## 背景\n"),
            labels: labels.iter().map(|label| label.to_string()).collect(),
            sub_issues: 0,
            blockers: blockers
                .iter()
                .map(|(number, open)| board::Blocker {
                    number: *number,
                    open: *open,
                })
                .collect(),
            status: status.map(str::to_string),
            status_updated: Some("2026-09-22T15:13:23Z".into()),
            item: None,
        }
    }

    /// The whole judgement in one case: what is offered, what is running,
    /// what a running branch holds back, and what could not be judged.
    #[test]
    fn ready_offers_what_nothing_stands_in_the_way_of() {
        let features = load_features().unwrap();
        let issues = vec![
            state(1, "oauth", Some(board::BACKLOG), &[], &[]),
            state(2, "tui", Some(board::READY), &[], &[]),
            state(3, "local-analytics", Some(board::IN_PROGRESS), &[], &[]),
            // `local-analytics` is what #3 is editing, so this waits.
            state(4, "local-analytics", Some(board::BACKLOG), &[], &[]),
            state(5, "config", Some(board::BACKLOG), &[board::TRACKER], &[]),
            state(
                6,
                "config",
                Some(board::BACKLOG),
                &[board::NEEDS_HUMAN],
                &[],
            ),
            state(7, "config", Some(board::BACKLOG), &[], &[(1, true)]),
            state(8, "config", None, &[], &[]),
        ];

        let answer = decide(&issues, &features, false);

        assert_eq!(
            answer.ready.iter().map(|o| o.number).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            answer.running.iter().map(|r| r.number).collect::<Vec<_>>(),
            [3]
        );
        assert_eq!(
            answer.held.iter().map(|h| h.number).collect::<Vec<_>>(),
            [4]
        );
        assert_eq!(answer.held[0].behind, [3]);
        // A blocked issue and a label are not problems; an issue the board
        // never saw is.
        assert_eq!(
            answer.problems.iter().map(|p| p.number).collect::<Vec<_>>(),
            [8]
        );
        assert!(
            answer.problems[0]
                .problem
                .contains("not an item of project")
        );
    }

    /// The wider list is the board's Ready column: it adds the two labels and
    /// nothing else.
    #[test]
    fn all_adds_the_trackers_and_the_work_for_a_person() {
        let features = load_features().unwrap();
        let issues = vec![
            state(5, "config", Some(board::BACKLOG), &[board::TRACKER], &[]),
            state(6, "oauth", Some(board::BACKLOG), &[board::NEEDS_HUMAN], &[]),
            state(7, "tui", Some(board::BACKLOG), &[], &[(1, true)]),
        ];

        assert!(decide(&issues, &features, false).ready.is_empty());

        let wide = decide(&issues, &features, true);

        assert_eq!(
            wide.ready
                .iter()
                .map(|o| (o.number, o.standing))
                .collect::<Vec<_>>(),
            [(5, board::TRACKER), (6, board::NEEDS_HUMAN)]
        );
    }

    /// Starting several agents at once means picking issues that do not
    /// appear in each other's list, so the list has to be there.
    #[test]
    fn offers_name_the_offers_they_cannot_run_beside() {
        let features = load_features().unwrap();
        let issues = vec![
            state(1, "local-analytics", Some(board::BACKLOG), &[], &[]),
            state(2, "local-analytics", Some(board::BACKLOG), &[], &[]),
            state(3, "oauth", Some(board::BACKLOG), &[], &[]),
        ];

        let answer = decide(&issues, &features, false);

        assert_eq!(answer.ready[0].serial_with, [2]);
        assert_eq!(answer.ready[1].serial_with, [1]);
        assert!(answer.ready[2].serial_with.is_empty());
    }

    /// An issue whose declaration cannot be read is reported. "Nothing to do"
    /// and "nothing could be read" must not look the same.
    #[test]
    fn an_issue_that_declares_nothing_is_a_problem_not_a_silent_drop() {
        let features = load_features().unwrap();
        let mut undeclared = state(1, "oauth", Some(board::BACKLOG), &[], &[]);
        undeclared.body = "## 背景\n\nno declaration here\n".into();
        let issues = vec![
            undeclared,
            state(2, "not-a-feature", Some(board::BACKLOG), &[], &[]),
        ];

        let answer = decide(&issues, &features, false);

        assert!(answer.ready.is_empty());
        assert_eq!(
            answer.problems.iter().map(|p| p.number).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(answer.problems[0].problem.contains("Affects:"));
        assert!(answer.problems[1].problem.contains("unknown feature"));
    }

    /// A running branch whose declaration cannot be read may be editing any
    /// feature, so no candidate is said to run beside it: each is held and
    /// names it as the issue it could not be judged against.
    #[test]
    fn a_running_issue_that_declares_nothing_holds_every_candidate() {
        let features = load_features().unwrap();
        let mut running = state(3, "local-analytics", Some(board::IN_PROGRESS), &[], &[]);
        running.body = "## 背景\n\nno declaration here\n".into();
        let issues = vec![
            state(1, "oauth", Some(board::BACKLOG), &[], &[]),
            running,
            state(4, "local-analytics", Some(board::BACKLOG), &[], &[]),
        ];

        let answer = decide(&issues, &features, false);

        assert!(
            answer.ready.is_empty(),
            "{:?}",
            answer.ready.iter().map(|o| o.number).collect::<Vec<_>>()
        );
        assert_eq!(
            answer
                .held
                .iter()
                .map(|h| (h.number, h.behind.clone(), h.unjudged.clone()))
                .collect::<Vec<_>>(),
            [(1, vec![], vec![3]), (4, vec![], vec![3])]
        );
        assert_eq!(
            answer.problems.iter().map(|p| p.number).collect::<Vec<_>>(),
            [3]
        );
        let text = render(&answer);
        assert!(
            text.contains("#4    issue 4  not judged against #3"),
            "{text}"
        );
    }

    /// `In progress` with an open blocker is a board to fix, and still a
    /// branch someone is running: it holds its feature back meanwhile.
    #[test]
    fn an_in_progress_issue_with_an_open_blocker_still_holds_its_feature() {
        let features = load_features().unwrap();
        let issues = vec![
            state(1, "oauth", Some(board::BACKLOG), &[], &[]),
            state(
                3,
                "local-analytics",
                Some(board::IN_PROGRESS),
                &[],
                &[(9, true)],
            ),
            state(4, "local-analytics", Some(board::BACKLOG), &[], &[]),
            // `Ready` with an open blocker is a board to fix, not a branch.
            state(5, "oauth", Some(board::READY), &[], &[(9, true)]),
        ];

        let answer = decide(&issues, &features, false);

        assert_eq!(
            answer.ready.iter().map(|o| o.number).collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(
            answer.running.iter().map(|r| r.number).collect::<Vec<_>>(),
            [3]
        );
        assert_eq!(
            answer
                .held
                .iter()
                .map(|h| (h.number, h.behind.clone()))
                .collect::<Vec<_>>(),
            [(4, vec![3])]
        );
        // Both disagreements are still reported.
        assert_eq!(
            answer.problems.iter().map(|p| p.number).collect::<Vec<_>>(),
            [3, 5]
        );
        // With every declaration read, the document is what it was.
        let held = serde_json::to_value(&answer.held[0]).unwrap();
        assert!(held.get("unjudged").is_none(), "{held}");
    }

    #[test]
    fn a_problem_names_what_to_do_about_it() {
        assert!(describe(&Standing::OffBoard).contains("not an item of project"));
        let disagreement = describe(&Standing::Disagrees {
            status: board::READY.into(),
            by: vec![75, 74],
        });
        assert!(disagreement.contains("#75, #74"), "{disagreement}");
        assert!(disagreement.contains("Ready"), "{disagreement}");
    }

    /// Small issues of one feature go to one worktree; an issue that also
    /// touches another feature, or a feature nobody else is on, stays alone.
    #[test]
    fn ready_batches_serial_offers_of_one_feature() {
        let features = load_features().unwrap();
        let issues = vec![
            state(1, "xtask", Some(board::BACKLOG), &[], &[]),
            state(2, "oauth", Some(board::BACKLOG), &[], &[]),
            state(3, "xtask", Some(board::BACKLOG), &[], &[]),
            state(4, "xtask, oauth", Some(board::BACKLOG), &[], &[]),
            state(5, "tui", Some(board::BACKLOG), &[], &[]),
            state(6, "xtask", Some(board::BACKLOG), &[board::TRACKER], &[]),
        ];

        let answer = decide(&issues, &features, true);

        assert_eq!(
            answer.batches,
            [Batch {
                feature: "xtask".into(),
                issues: vec![1, 3],
            }]
        );
        let text = render(&answer);
        assert!(
            text.contains("xtask  #1, #3  cargo xtask worktree add 1 3"),
            "{text}"
        );
    }

    fn offer(number: u64, serial_with: &[u64]) -> Offer {
        Offer {
            number,
            title: format!("issue {number}"),
            standing: "ready",
            serial_with: serial_with.to_vec(),
        }
    }

    /// 2026-09-23, the pick that had to be worked out by hand: #37 is serial
    /// with #63 and #65, #55 with #64 and #72, #84 with nothing.
    fn board() -> Vec<Offer> {
        vec![
            offer(63, &[37, 65]),
            offer(37, &[63, 65]),
            offer(84, &[]),
            offer(55, &[64, 72]),
            offer(64, &[55]),
            offer(65, &[37, 63]),
            offer(72, &[55]),
        ]
    }

    #[test]
    fn agents_picks_offers_no_two_of_which_are_serial() {
        let together = pick_together(&board(), 3);

        assert_eq!(together.picked, [37, 55, 84]);
        for a in &together.picked {
            let serial = &board()
                .into_iter()
                .find(|o| o.number == *a)
                .unwrap()
                .serial_with;
            assert!(
                together.picked.iter().all(|b| !serial.contains(b)),
                "#{a} is serial with a picked offer"
            );
        }
    }

    /// Two agents reading the same board must start the same set.
    #[test]
    fn the_same_board_gives_the_same_pick_whatever_order_it_came_in() {
        let mut reversed = board();
        reversed.reverse();

        assert_eq!(pick_together(&reversed, 3), pick_together(&board(), 3));
    }

    #[test]
    fn fewer_than_asked_says_which_offers_stood_in_the_way() {
        let together = pick_together(&board(), 5);

        assert_eq!(together.picked, [37, 55, 84]);
        assert_eq!(
            together.left_out,
            [
                LeftOut {
                    number: 63,
                    serial_with: vec![37]
                },
                LeftOut {
                    number: 64,
                    serial_with: vec![55]
                },
                LeftOut {
                    number: 65,
                    serial_with: vec![37]
                },
                LeftOut {
                    number: 72,
                    serial_with: vec![55]
                },
            ]
        );
        let text = render_together(&together, 7);
        assert!(text.contains("3 of 5 asked"), "{text}");
        assert!(text.contains("#63   beside #37"), "{text}");
    }

    /// `--agents` answers what to hand `worktree add`, which refuses a
    /// tracker and a job for a person: `--all` widens the list, not the pick.
    #[test]
    fn agents_picks_only_what_worktree_add_takes_with_or_without_all() {
        let features = load_features().unwrap();
        let issues = vec![
            state(5, "config", Some(board::BACKLOG), &[board::TRACKER], &[]),
            state(6, "tui", Some(board::BACKLOG), &[board::NEEDS_HUMAN], &[]),
            state(7, "oauth", Some(board::BACKLOG), &[], &[]),
            state(8, "local-analytics", Some(board::BACKLOG), &[], &[]),
        ];

        let narrow = pick_together(&decide(&issues, &features, false).ready, 3);
        let mut listed = decide(&issues, &features, true);
        let wide = pick_together(&listed.ready, 3);

        assert_eq!(narrow.picked, [7, 8]);
        assert_eq!(wide, narrow);
        // Why fewer than asked: two can be started, not the four listed.
        listed.together = Some(wide);
        let text = render(&listed);
        assert!(text.contains("only 2 issue(s) are offered"), "{text}");
    }

    #[test]
    fn zero_agents_or_no_offer_picks_nothing_and_does_not_fail() {
        let none = pick_together(&board(), 0);
        assert!(none.picked.is_empty());
        assert!(render_together(&none, 7).contains("0 of 0 asked"));

        let empty = pick_together(&[], 3);
        assert!(empty.picked.is_empty());
        assert!(render_together(&empty, 0).contains("only 0 issue(s) are offered"));
    }

    /// #86 waited for #64, which no tracker listed: the blocker and where it
    /// stands are what says to start #64 first.
    #[test]
    fn a_blocked_issue_names_its_blockers_and_where_they_stand() {
        let features = load_features().unwrap();
        let issues = vec![
            state(64, "xtask", Some(board::BACKLOG), &[], &[]),
            state(
                86,
                "xtask",
                Some(board::BACKLOG),
                &[],
                &[(64, true), (84, false)],
            ),
            state(
                87,
                "xtask",
                Some(board::BACKLOG),
                &[],
                &[(86, true), (12, true)],
            ),
            state(90, "oauth", Some(board::IN_PROGRESS), &[], &[]),
            state(91, "tui", Some(board::BACKLOG), &[], &[(90, true)]),
        ];

        let answer = decide(&issues, &features, false);

        assert_eq!(
            answer.blocked,
            [
                Waiting {
                    number: 86,
                    title: "issue 86".into(),
                    after: vec![BlockerStanding {
                        number: 64,
                        standing: "ready".into()
                    }],
                },
                Waiting {
                    number: 87,
                    title: "issue 87".into(),
                    after: vec![
                        BlockerStanding {
                            number: 86,
                            standing: "blocked".into()
                        },
                        BlockerStanding {
                            number: 12,
                            standing: "open".into()
                        },
                    ],
                },
                Waiting {
                    number: 91,
                    title: "issue 91".into(),
                    after: vec![BlockerStanding {
                        number: 90,
                        standing: board::IN_PROGRESS.into()
                    }],
                },
            ]
        );
        let text = render(&answer);
        assert!(text.contains("blocked (3)"), "{text}");
        assert!(text.contains("#86   issue 86  after #64 (ready)"), "{text}");
    }
}
