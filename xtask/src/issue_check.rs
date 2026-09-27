//! `cargo xtask issue-check <FILE|-|ISSUE>`: the declarations of an issue
//! body, checked before the issue is posted.
//!
//! `conflicts` needs two posted issues and `scenarios-check` one, so a body
//! used to be checked only after it was on GitHub, and a note written on the
//! `Affects:` line was found after five issues had gone out with it. This
//! reads a file or stdin as well as a posted issue and asks the same two
//! questions with the same functions: does the `Affects:` line resolve, and
//! is every `## シナリオ` item shaped like a scenario name. Labels and
//! `blocked_by` live on GitHub, not in the body, and are not checked here.

use std::io::Read;

use crate::board::issue_body;
use crate::conflicts::declaration_from;
use crate::load_features;
use crate::scenarios_check::{SECTION, naming_problem, parse_section};

const USAGE: &str = "\
usage: cargo xtask issue-check <FILE|-|ISSUE>

  FILE    an issue body not posted yet; nothing is read from GitHub
  -       the same, from stdin
  ISSUE   a posted issue, read through gh

Checks the `Affects:` line against .agent/features/ and the shape of every
`## シナリオ` item. Labels and blocked_by are not in the body and are not checked.
";

#[derive(Debug, PartialEq)]
enum Source {
    File(String),
    Stdin,
    Issue(u64),
}

fn parse(args: &[String]) -> Result<Source, String> {
    match args {
        [arg] if arg == "--help" || arg == "-h" => Err(USAGE.to_string()),
        [arg] if arg == "-" => Ok(Source::Stdin),
        [arg] => Ok(match crate::board::issue_number(arg, USAGE) {
            Ok(number) => Source::Issue(number),
            Err(_) => Source::File(arg.clone()),
        }),
        _ => Err(format!("one body at a time\n\n{USAGE}")),
    }
}

pub fn issue_check(args: &[String]) -> Result<(), String> {
    let source = parse(args)?;
    let (label, number, body) = match &source {
        Source::File(path) => (
            path.clone(),
            0,
            std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?,
        ),
        Source::Stdin => {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .map_err(|error| format!("stdin: {error}"))?;
            ("stdin".to_string(), 0, body)
        }
        Source::Issue(number) => (format!("#{number}"), *number, issue_body(*number)?),
    };
    let findings = check(number, &body, &load_features()?);
    print!("{label}\n{}", render(&findings));
    if findings.failed() {
        Err(format!("{label}: the body does not declare what it has to"))
    } else {
        Ok(())
    }
}

#[derive(Debug)]
struct Findings {
    /// The features and file count the `Affects:` line resolves to, or why
    /// it does not.
    affects: Result<(Vec<String>, usize), String>,
    /// `None`: no `## シナリオ` section.
    scenarios: Option<Vec<String>>,
    ill_named: Vec<(String, &'static str)>,
}

impl Findings {
    fn failed(&self) -> bool {
        self.affects.is_err()
            || self.scenarios.as_ref().is_some_and(Vec::is_empty)
            || !self.ill_named.is_empty()
    }
}

fn check(number: u64, body: &str, features: &crate::FeatureMap) -> Findings {
    // A body not posted yet has no number, and the messages say `issue #N`.
    let affects = declaration_from(number, body, features)
        .map(|declaration| (declaration.features.clone(), declaration.files.len()))
        .map_err(|error| match number {
            0 => error.replacen("issue #0", "the body", 1),
            _ => error,
        });
    let scenarios = parse_section(body);
    let ill_named = scenarios
        .iter()
        .flatten()
        .filter_map(|name| naming_problem(name).map(|reason| (name.clone(), reason)))
        .collect();
    Findings {
        affects,
        scenarios,
        ill_named,
    }
}

fn render(findings: &Findings) -> String {
    let mut out = String::new();
    match &findings.affects {
        Ok((features, files)) => out.push_str(&format!(
            "  Affects:      ok  {} ({files} file(s))\n",
            features.join(", ")
        )),
        Err(error) => out.push_str(&format!("  Affects:      FAIL  {error}\n")),
    }
    match &findings.scenarios {
        None => out.push_str(&format!(
            "  ## {SECTION}:  no section, so no scenario is declared (right for an xtask \
             command or a document)\n"
        )),
        Some(names) if names.is_empty() => out.push_str(&format!(
            "  ## {SECTION}:  FAIL  the section lists no item; write one \
             `- [ ] <feature>_<behavior>` per scenario\n"
        )),
        Some(names) if findings.ill_named.is_empty() => out.push_str(&format!(
            "  ## {SECTION}:  ok  {} scenario(s), each <feature>_<behavior>\n",
            names.len()
        )),
        Some(names) => {
            out.push_str(&format!(
                "  ## {SECTION}:  FAIL  {} of {} item(s) are not scenario names\n",
                findings.ill_named.len(),
                names.len()
            ));
            for (name, reason) in &findings.ill_named {
                out.push_str(&format!("      {name}: {reason}\n"));
            }
        }
    }
    out.push_str("  labels, blocked_by: not checked (they are not in the body)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn a_number_is_an_issue_a_dash_is_stdin_and_anything_else_a_file() {
        assert_eq!(parse(&args(&["91"])).unwrap(), Source::Issue(91));
        assert_eq!(parse(&args(&["#91"])).unwrap(), Source::Issue(91));
        assert_eq!(parse(&args(&["-"])).unwrap(), Source::Stdin);
        assert_eq!(
            parse(&args(&["draft.md"])).unwrap(),
            Source::File("draft.md".into())
        );
        assert!(parse(&[]).is_err());
        assert!(parse(&args(&["a.md", "b.md"])).is_err());
    }

    #[test]
    fn a_body_that_declares_what_it_has_to_passes() {
        let features = load_features().unwrap();
        let body = "Affects: oauth, xtask/xtask/\n\n## シナリオ\n\n\
                    - [ ] oauth_login_stores_the_token\n";

        let findings = check(0, body, &features);

        assert!(!findings.failed(), "{findings:?}");
        assert_eq!(findings.affects.as_ref().unwrap().0, ["oauth", "xtask"]);
        assert!(render(&findings).contains("1 scenario(s)"));
    }

    /// #55: the note is reported as a line that cannot be read, not as an
    /// unknown feature.
    #[test]
    fn a_note_on_the_affects_line_fails_as_an_unreadable_line() {
        let features = load_features().unwrap();

        let findings = check(0, "Affects: oauth （メモ）\n", &features);

        assert!(findings.failed());
        let error = findings.affects.unwrap_err();
        assert!(error.contains("cannot be read"), "{error}");
        assert!(!error.contains("unknown feature"), "{error}");
        assert!(error.starts_with("the body:"), "{error}");
    }

    #[test]
    fn an_ill_named_scenario_is_named() {
        let features = load_features().unwrap();
        let body = "Affects: oauth\n\n## シナリオ\n\n- [ ] Login works\n- [ ] oauth_ok_name\n";

        let findings = check(0, body, &features);

        assert!(findings.failed());
        let text = render(&findings);
        assert!(
            text.contains("Login: not a lowercase snake_case name"),
            "{text}"
        );
        assert!(!text.contains("oauth_ok_name:"), "{text}");
    }

    #[test]
    fn no_section_is_a_declaration_of_nothing_and_an_empty_one_fails() {
        let features = load_features().unwrap();

        let none = check(0, "Affects: oauth\n", &features);
        assert!(!none.failed());
        assert!(render(&none).contains("no section"));

        let empty = check(0, "Affects: oauth\n\n## シナリオ\n\nあとで\n", &features);
        assert!(empty.failed());
        assert!(render(&empty).contains("lists no item"));
    }

    #[test]
    fn what_the_body_cannot_say_is_reported_as_not_checked() {
        let features = load_features().unwrap();

        let text = render(&check(0, "Affects: oauth\n", &features));

        assert!(text.contains("labels, blocked_by: not checked"), "{text}");
    }
}
