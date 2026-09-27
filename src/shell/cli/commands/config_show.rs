//! `kurama config path|list|show [--json]`: the file kurama uses, the sections and keys it holds, and their saved values with literal secrets redacted, read for syntax only.
//!
//! `path` does not open the file, so it answers for one that does not parse.
//! `list` and `show` read the TOML syntax and nothing more: a file that is
//! valid TOML but not a valid configuration is shown as it is saved, without
//! defaults or environment overrides. A literal secret (`client_secret`,
//! `token`, `password` holding the value itself) is shown as `<redacted>`;
//! nothing is resolved.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use clap_complete::engine::ArgValueCandidates;
use serde::Serialize;
use serde_json::json;

use super::config_check::ConfigFileState;
use crate::adapters::config::Config;
use crate::adapters::config::input::InputError;
use crate::adapters::config::saved::Saved;
use crate::adapters::error::CoreError;
use crate::shell::cli::completion;

fn json_arg() -> Arg {
    Arg::new("json")
        .long("json")
        .help("Print one JSON document")
        .action(ArgAction::SetTrue)
}

pub fn path_command() -> Command {
    Command::new("path")
        .about("Print the path of the config.toml kurama uses, without reading it")
        .arg(json_arg())
}

pub fn list_command() -> Command {
    Command::new("list")
        .about("List the sections of config.toml and the keys each holds, as saved")
        .arg(
            Arg::new("section")
                .value_name("SECTION")
                .help("Only this section or table: auth, auth.github, aws.session_cache")
                .add(ArgValueCandidates::new(completion::config_sections)),
        )
        .arg(json_arg())
}

pub fn show_command() -> Command {
    Command::new("show")
        .about("Print the saved values of config.toml, literal secrets redacted; no default or override is added")
        .arg(
            Arg::new("path")
                .value_name("PATH")
                .help("A dotted key: auth.github, aws.session_cache.duration; the whole file when absent")
                .add(ArgValueCandidates::new(completion::config_sections)),
        )
        .arg(json_arg())
}

pub fn run_path(json: bool) -> Result<()> {
    let (path, named) = Config::config_source()?;
    if json {
        let file = ConfigFileState::of(&path, named, path.exists());
        println!("{}", serde_json::to_string_pretty(&file)?);
    } else {
        println!("{}", path.display());
    }
    Ok(())
}

/// The saved file, parsed for its syntax, with its literal secrets redacted,
/// and the path to name in output.
async fn read_saved() -> Result<(PathBuf, Saved)> {
    let (path, named) = Config::config_source()?;
    let content = Config::read_source(&path, named).await?;
    let mut saved =
        Saved::parse(content.as_deref().unwrap_or_default()).map_err(CoreError::from)?;
    saved.redact();
    Ok((path, saved))
}

#[derive(Debug, Serialize)]
struct Section {
    name: String,
    keys: Vec<String>,
}

pub async fn run_list(filter: Option<&str>, json: bool) -> Result<()> {
    let (path, saved) = read_saved().await?;
    let sections: Vec<Section> = saved
        .units()
        .into_iter()
        .filter_map(|name| {
            let keys = selected(&name, saved.keys(&name), filter)?;
            Some(Section { name, keys })
        })
        .collect();
    if let Some(filter) = filter.filter(|_| sections.is_empty()) {
        return Err(nothing_at(&path, filter));
    }
    if json {
        let document = json!({ "path": path.display().to_string(), "sections": sections });
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        for section in sections {
            if section.keys.is_empty() {
                println!("{}", section.name);
            } else {
                println!("{}: {}", section.name, section.keys.join(", "));
            }
        }
    }
    Ok(())
}

/// The keys of `unit` that `filter` selects: all of them when it names the
/// unit or a table the unit is in, the ones under it when it names a table
/// inside the unit, `None` when it selects none.
fn selected(unit: &str, keys: Vec<String>, filter: Option<&str>) -> Option<Vec<String>> {
    let Some(filter) = filter else {
        return Some(keys);
    };
    if unit == filter || unit.starts_with(&format!("{filter}.")) {
        return Some(keys);
    }
    let inner = filter.strip_prefix(&format!("{unit}."))?;
    let keys: Vec<String> = keys
        .into_iter()
        .filter(|key| key == inner || key.starts_with(&format!("{inner}.")))
        .collect();
    (!keys.is_empty()).then_some(keys)
}

pub async fn run_show(key: Option<&str>, json: bool) -> Result<()> {
    let (path, saved) = read_saved().await?;
    if json {
        let value = match key {
            Some(key) => saved.json_of(key).ok_or_else(|| nothing_at(&path, key))?,
            None => saved.json(),
        };
        let document = json!({ "path": path.display().to_string(), "key": key, "value": value });
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        let text = match key {
            Some(key) => saved.text_of(key).ok_or_else(|| nothing_at(&path, key))?,
            None => saved.text(),
        };
        print!("{text}");
    }
    Ok(())
}

fn nothing_at(path: &Path, key: &str) -> anyhow::Error {
    InputError::NoSuchKey {
        path: path.to_owned(),
        key: key.to_owned(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_selects_a_table_a_unit_or_the_keys_inside_one() {
        let keys = || {
            vec![
                "session_cache.duration".to_owned(),
                "session_name.prefix".to_owned(),
            ]
        };
        assert_eq!(selected("aws", keys(), None), Some(keys()));
        assert_eq!(selected("aws", keys(), Some("aws")), Some(keys()));
        assert_eq!(
            selected("aws", keys(), Some("aws.session_cache")),
            Some(vec!["session_cache.duration".to_owned()])
        );
        assert_eq!(selected("aws", keys(), Some("aws.nothing")), None);
        assert_eq!(selected("auth.gh", vec![], Some("auth")), Some(vec![]));
        assert_eq!(selected("api.gh", vec![], Some("auth")), None);
        assert_eq!(selected("authz", vec![], Some("auth")), None);
    }
}
