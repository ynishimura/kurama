//! Every section of config.toml read on its own, so one run lists a problem in each section instead of stopping at the first.
//!
//! `Config::parse` stops at the first key serde refuses. Here the document is
//! parsed once for its syntax, and then each top-level table -- and each
//! `[auth.*]`, `[api.*]`, `[data.*]`, `[db.*]` and `[s3.*]` entry -- is read
//! into [`Config`] alone, with the same types and so the same rules. The
//! sections that read are then validated together by [`Config::problems`].
//! A syntax error is reported alone: nothing after it can be read.

use std::collections::BTreeSet;
use std::ops::Range;

use serde::Deserialize;
use toml::Spanned;
use toml::de::{DeString, DeTable, DeValue};

use super::saved::NAMED;
use super::{Config, line_of, read_error};
use crate::adapters::error::CoreError;

/// One thing wrong with the file.
#[derive(Debug)]
pub struct Problem {
    /// `api.github`, `aws`; `None` for the file as a whole.
    pub section: Option<String>,
    pub line: Option<usize>,
    pub error: CoreError,
}

/// What reading every section on its own found.
#[derive(Debug)]
pub struct Checked {
    /// The sections that read, as one configuration; `None` after a syntax
    /// error.
    pub config: Option<Config>,
    pub problems: Vec<Problem>,
    /// The line each section starts on.
    starts: Vec<(String, Option<usize>)>,
}

impl Checked {
    /// The line `section` (`api.github`, `aws.session_cache`) starts on: its
    /// own table, else the top-level table it is in.
    pub fn line(&self, section: &str) -> Option<usize> {
        let start = |section: &str| {
            self.starts
                .iter()
                .find(|(start, _)| start == section)
                .and_then(|(_, line)| *line)
        };
        start(section).or_else(|| start(section.split('.').next().unwrap_or_default()))
    }
}

/// Read `content` section by section; see the module documentation.
pub fn check(content: &str) -> Checked {
    let root = match DeTable::parse(content) {
        Ok(root) => root,
        Err(error) => {
            return Checked {
                config: None,
                problems: vec![Problem {
                    section: None,
                    line: line_of(content, error.span()),
                    error: read_error(content, &error),
                }],
                starts: Vec::new(),
            };
        }
    };
    let span = root.span();
    let root = root.into_inner();
    let mut problems = Vec::new();
    // Where each section starts, for a problem found after reading.
    let mut starts = Vec::new();
    let mut unread = BTreeSet::new();
    for (key, value) in root.iter() {
        let named = NAMED.contains(&key.get_ref().as_ref());
        let entries: Vec<_> = match value.get_ref() {
            DeValue::Table(table) if named => table
                .iter()
                .map(|(name, entry)| {
                    let one = DeValue::Table(single(name.clone(), entry.clone()));
                    (
                        format!("{}.{}", key.get_ref(), name.get_ref()),
                        name.span(),
                        Spanned::new(value.span(), one),
                    )
                })
                .collect(),
            _ => vec![(key.get_ref().to_string(), key.span(), value.clone())],
        };
        for (section, start, value) in entries {
            starts.push((section.clone(), line_of(content, Some(start))));
            if let Err(error) = read_alone(span.clone(), key.clone(), value) {
                problems.push(Problem {
                    section: Some(section.clone()),
                    line: line_of(content, error.span()),
                    error: read_error(content, &error),
                });
                unread.insert(section);
            }
        }
    }
    let root: DeTable = root
        .into_iter()
        .filter(|(key, _)| !unread.contains(&**key.get_ref()))
        .map(|(key, mut value)| {
            if let DeValue::Table(table) = value.get_mut() {
                let kind = key.get_ref().to_string();
                *table = std::mem::take(table)
                    .into_iter()
                    .filter(|(name, _)| !unread.contains(&format!("{kind}.{}", name.get_ref())))
                    .collect();
            }
            (key, value)
        })
        .collect();
    let config = Config::deserialize(toml::de::Deserializer::from(Spanned::new(span, root)))
        .expect("every section left read on its own");
    let mut checked = Checked {
        config: None,
        problems,
        starts,
    };
    let found: Vec<Problem> = config
        .problems()
        .into_iter()
        .filter(|(section, _)| !follows_an_unread_section(&config, section, &unread))
        .map(|(section, error)| Problem {
            line: checked.line(&section),
            section: Some(section),
            error,
        })
        .collect();
    checked.problems.extend(found);
    checked
        .problems
        .sort_by_key(|problem| problem.line.unwrap_or(usize::MAX));
    checked.config = Some(config);
    checked
}

fn single<'i>(key: Spanned<DeString<'i>>, value: Spanned<DeValue<'i>>) -> DeTable<'i> {
    let mut table = DeTable::new();
    table.insert(key, value);
    table
}

/// `key = value` read into [`Config`] as if it were the whole document.
fn read_alone<'i>(
    span: Range<usize>,
    key: Spanned<DeString<'i>>,
    value: Spanned<DeValue<'i>>,
) -> Result<Config, toml::de::Error> {
    Config::deserialize(toml::de::Deserializer::from(Spanned::new(
        span,
        single(key, value),
    )))
}

/// A problem that is only there because a section it names did not read:
/// `[api.x] auth = "y"` when `[auth.y]` had an unknown key is one problem, not
/// two.
fn follows_an_unread_section(config: &Config, section: &str, unread: &BTreeSet<String>) -> bool {
    // The whole table (`auth = 1`) or the one entry did not read.
    let named = |kind: &str, name: Option<&String>| {
        name.is_some_and(|name| unread.contains(kind) || unread.contains(&format!("{kind}.{name}")))
    };
    match section.split_once('.') {
        Some(("api", name)) => config
            .api
            .get(name)
            .is_some_and(|api| named("auth", api.auth.as_ref())),
        Some(("data", name)) => config
            .data
            .get(name)
            .is_some_and(|workspace| named("s3", workspace.s3_source.as_ref())),
        Some(("mcp", "tools")) => unread.contains("obsidian"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(content: &str) -> Vec<(Option<String>, Option<usize>, String)> {
        check(content)
            .problems
            .into_iter()
            .map(|problem| (problem.section, problem.line, problem.error.to_string()))
            .collect()
    }

    /// `[mcp] tools` naming an obsidian tool is a problem of its own only
    /// when there is no `[obsidian]`, not when the section is there and did
    /// not read.
    #[test]
    fn an_obsidian_tool_is_not_reported_for_an_obsidian_section_that_did_not_read() {
        let mcp = "[mcp]\ntools = [\"obsidian_read\"]\n";
        let problems = found(&format!(
            "{mcp}\n[obsidian]\nvault = \"b\"\nallow_paths = []\n"
        ));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].0.as_deref(), Some("obsidian"));
        let problems = found(mcp);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].0.as_deref(), Some("mcp.tools"));
        assert_eq!(problems[0].1, Some(1));
    }

    #[test]
    fn a_valid_file_has_no_problem_and_reads_whole() {
        let checked = check(
            "[aws.session_cache]\nduration = 3600\n\n[auth.gh]\nkind = \"token\"\ntoken = \"op://Agent/x/y\"\n\n\
             [api.gh]\nbase_url = \"https://api.github.com\"\n",
        );
        assert!(checked.problems.is_empty(), "{:?}", checked.problems);
        let config = checked.config.unwrap();
        assert_eq!(config.aws.session_cache.duration, 3600);
        assert_eq!(
            config.api_profile("gh").cloned().unwrap().auth.as_deref(),
            Some("gh")
        );
    }

    /// Four sections, four problems, each with its section and line: an
    /// unknown key, a reference to nothing, an exclusive pair and a value out
    /// of range.
    #[test]
    fn every_independent_section_reports_its_own_problem() {
        let problems = found(
            "[aws.session_cache]\nduration = 1\n\n\
             [api.a]\nbase_url = \"https://a\"\nbase_urll = \"x\"\n\n\
             [api.b]\nbase_url = \"https://b\"\nauth = \"nope\"\n\n\
             [api.c]\nbase_url = \"https://c\"\nauth = \"x\"\naws_profile = \"dev\"\n",
        );
        let sections: Vec<_> = problems.iter().map(|p| p.0.as_deref()).collect();
        assert_eq!(
            sections,
            [
                Some("aws.session_cache"),
                Some("api.a"),
                Some("api.b"),
                Some("api.c")
            ],
            "{problems:?}"
        );
        assert_eq!(problems[0].1, Some(1));
        assert_eq!(problems[1].1, Some(6), "the line of the unknown key");
        assert!(
            problems[1].2.contains("unknown field `base_urll`"),
            "{problems:?}"
        );
        assert_eq!(problems[2].1, Some(8), "the line of the section");
        assert!(
            problems[2].2.contains("names no [auth.nope]"),
            "{problems:?}"
        );
        assert!(problems[3].2.contains("exclusive"), "{problems:?}");
    }

    /// A section that did not type is reported and is not among the typed
    /// values the rest of `config check` reads; its neighbours are.
    #[test]
    fn a_section_that_did_not_type_is_a_problem_and_not_a_value() {
        let mut checked = check(
            "[auth.site]\nkind = \"secrets\"\n\n\
             [api.site]\nbase_url = \"https://x\"\n\n\
             [api.ok]\nbase_url = \"https://ok\"\n\n\
             [db.bad]\nengine = \"sqlite\"\n",
        );
        let sections: Vec<_> = checked
            .problems
            .iter()
            .map(|p| p.section.as_deref())
            .collect();
        assert_eq!(
            sections,
            [Some("auth.site"), Some("api.site"), Some("db.bad")],
            "{:?}",
            checked.problems
        );
        assert!(
            checked.problems[1]
                .error
                .to_string()
                .contains("[auth.site] is kind = \"secrets\""),
            "an API naming a secrets source is its own problem while the source has another"
        );
        let config = checked.config.take().unwrap();
        assert!(config.auth_source("site").is_none());
        assert!(config.api_profile("site").is_none());
        assert!(config.db_connection("bad").is_none());
        let apis: Vec<_> = config.api_profiles().into_iter().map(|a| a.name).collect();
        assert_eq!(apis, ["ok"]);
    }

    #[test]
    fn a_syntax_error_is_reported_alone() {
        let checked = check(
            "[api.a]\nbase_url = \"https://a\"\nauth = \n[aws.session_cache]\nduration = 1\n",
        );
        assert!(checked.config.is_none());
        assert_eq!(checked.problems.len(), 1, "{:?}", checked.problems);
        assert_eq!(checked.problems[0].section, None);
        assert_eq!(checked.problems[0].line, Some(3));
    }

    /// `[api.x]` names `[auth.x]`, which did not read: one problem, in the
    /// section that has it.
    #[test]
    fn a_reference_to_a_section_that_did_not_read_is_not_a_second_problem() {
        let problems = found(
            "[auth.x]\nkind = \"token\"\ntoken = \"op://Agent/x/y\"\ntypo = 1\n\n\
             [api.x]\nbase_url = \"https://x\"\nauth = \"x\"\n\n\
             [data.w]\ns3_source = \"s\"\n[[data.w.sources]]\nname = \"o\"\npath = \"s3://b/o.parquet\"\n\n\
             [s3.s]\naws_profile = 1\n",
        );
        let sections: Vec<_> = problems.iter().map(|p| p.0.as_deref()).collect();
        assert_eq!(sections, [Some("auth.x"), Some("s3.s")], "{problems:?}");
    }

    #[test]
    fn a_reference_into_a_table_that_did_not_read_is_not_a_second_problem() {
        let problems = found(
            "auth = 1\n\n[api.x]\nbase_url = \"https://x\"\nauth = \"x\"\n\n\
             [api.y]\nbase_url = \"not a url\"\n",
        );
        let sections: Vec<_> = problems.iter().map(|p| p.0.as_deref()).collect();
        assert_eq!(sections, [Some("auth"), Some("api.y")], "{problems:?}");
    }

    #[test]
    fn an_unknown_top_level_table_is_its_own_section() {
        let problems = found("[ui]\ntheme = \"dark\"\n\n[onepassword]\ntimeout = 0\n");
        let sections: Vec<_> = problems.iter().map(|p| p.0.as_deref()).collect();
        assert_eq!(sections, [Some("ui"), Some("onepassword")], "{problems:?}");
        assert_eq!(problems[1].1, Some(4));
    }
}
