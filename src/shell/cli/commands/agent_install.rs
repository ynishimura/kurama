//! `kurama agent install [--dir DIR] [--offline] [--dry-run] [--json]`:
//! kurama's Agent Skill and one per `[api.*]` with a description, written
//! as `<DIR>/<skill>/SKILL.md` (`~/.claude/skills` by default).
//!
//! A file that already holds the same bytes is not touched, one that holds
//! other bytes is replaced, and nothing else under DIR is written or
//! removed. An API without a description, or whose description cannot be
//! read, is skipped with the reason. A description is loaded as `api
//! --skill` loads it (the cached copy, fetched when there is none or it is
//! due); `--offline` and `--dry-run` read the cache only, so a dry run
//! fetches nothing and writes nothing. The report goes to stdout.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use super::agent::AGENT_SKILL;
use super::api_spec::render_api_skill;
use crate::adapters::config::Config;
use crate::adapters::utils::path::get_home_dir;
use crate::domain::functions::skill_install::{
    KURAMA_SKILL, SKILL_FILE, SkillRow, SkillStatus, api_skill_directory,
};
use crate::shell::api_runtime::{ApiRuntime, ApiRuntimeOptions};
use crate::shell::cli::client::{json_line, tab_separated};
use crate::shell::spec_loader::SpecAccess;

/// How long a description fetch may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// What `agent install` was told on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInstall {
    /// `--dir`: where the Skill directories go; `~/.claude/skills` without.
    pub dir: Option<PathBuf>,
    pub offline: bool,
    pub dry_run: bool,
    pub json: bool,
}

/// A Skill that could not be written; what was written before it stays.
#[derive(Debug, thiserror::Error)]
#[error("cannot write the Agent Skill {path}")]
pub struct SkillWriteFailed {
    pub path: String,
    #[source]
    pub source: std::io::Error,
}

#[derive(Serialize)]
struct InstallOutput {
    schema_version: u8,
    kind: &'static str,
    dir: String,
    dry_run: bool,
    skills: Vec<SkillRow>,
}

pub async fn run(options: &AgentInstall, config: Config) -> Result<()> {
    let dir = match &options.dir {
        Some(dir) => dir.clone(),
        None => get_home_dir()?.join(".claude").join("skills"),
    };
    let apis = config.api_profiles()?;
    let runtime = ApiRuntime::from_config(
        Arc::new(config),
        ApiRuntimeOptions {
            timeout: FETCH_TIMEOUT,
            accept_invalid_certs: false,
            open_browser: false,
            report_secret_reads: false,
        },
    )?;
    let access = if options.offline || options.dry_run {
        SpecAccess::CacheOnly
    } else {
        SpecAccess::WithCredential
    };
    let mut skills = vec![place(
        &dir,
        KURAMA_SKILL,
        None,
        AGENT_SKILL,
        options.dry_run,
    )?];
    for api in &apis {
        let skipped = |skill: String, reason: String| SkillRow {
            skill,
            api: Some(api.name.clone()),
            path: None,
            status: SkillStatus::Skipped,
            reason: Some(reason),
        };
        let Some(directory) = api_skill_directory(&api.name) else {
            skills.push(skipped(
                api.name.clone(),
                "the API's name cannot be one directory name".into(),
            ));
            continue;
        };
        if api.spec.is_none() {
            skills.push(skipped(
                directory,
                "no openapi, discovery or graphql description".into(),
            ));
            continue;
        }
        match render_api_skill(&runtime, api, false, access, false).await {
            Ok(skill) => skills.push(place(
                &dir,
                &directory,
                Some(&api.name),
                &skill,
                options.dry_run,
            )?),
            Err(error) => skills.push(skipped(directory, format!("{error:#}"))),
        }
    }
    let output = InstallOutput {
        schema_version: 1,
        kind: "agent_install",
        dir: dir.display().to_string(),
        dry_run: options.dry_run,
        skills,
    };
    if options.json {
        println!("{}", json_line(&output));
    } else {
        print!("{}", render_table(&output.skills));
    }
    Ok(())
}

/// `<dir>/<directory>/SKILL.md` holding `content`: written when it holds
/// anything else, unless this is a dry run.
fn place(
    dir: &Path,
    directory: &str,
    api: Option<&str>,
    content: &str,
    dry_run: bool,
) -> Result<SkillRow, SkillWriteFailed> {
    let path = dir.join(directory).join(SKILL_FILE);
    let status = SkillStatus::of(std::fs::read(&path).ok().as_deref(), content);
    if status == SkillStatus::Written && !dry_run {
        std::fs::create_dir_all(dir.join(directory))
            .and_then(|()| std::fs::write(&path, content))
            .map_err(|source| SkillWriteFailed {
                path: path.display().to_string(),
                source,
            })?;
    }
    Ok(SkillRow {
        skill: directory.to_string(),
        api: api.map(str::to_string),
        path: Some(path.display().to_string()),
        status,
        reason: None,
    })
}

fn render_table(rows: &[SkillRow]) -> String {
    let cells: Vec<Vec<Value>> = rows
        .iter()
        .map(|row| {
            vec![
                row.status.as_str().into(),
                row.skill.clone().into(),
                row.path.clone().unwrap_or_default().into(),
                row.reason.clone().unwrap_or_default().into(),
            ]
        })
        .collect();
    tab_separated(["status", "skill", "path", "reason"].into_iter(), &cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_is_written_once_and_left_alone_while_it_says_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let first = place(dir.path(), "kurama", None, "one", false).unwrap();
        assert_eq!(first.status, SkillStatus::Written);
        let path = dir.path().join("kurama/SKILL.md");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        assert_eq!(
            place(dir.path(), "kurama", None, "one", false)
                .unwrap()
                .status,
            SkillStatus::Unchanged
        );
        let dry = place(dir.path(), "kurama", None, "two", true).unwrap();
        assert_eq!(dry.status, SkillStatus::Written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        assert_eq!(
            place(dir.path(), "kurama", None, "two", false)
                .unwrap()
                .status,
            SkillStatus::Written
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    }

    #[test]
    fn the_table_names_the_status_the_skill_and_the_reason() {
        let rows = [SkillRow {
            skill: "kurama-api-plain".into(),
            api: Some("plain".into()),
            path: None,
            status: SkillStatus::Skipped,
            reason: Some("no openapi, discovery or graphql description".into()),
        }];
        assert_eq!(
            render_table(&rows),
            "status\tskill\tpath\treason\nskipped\tkurama-api-plain\t\tno openapi, discovery or graphql description\n"
        );
    }
}
