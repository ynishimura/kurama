//! `kurama preset add <ID> [--as NAME] [--auth-as NAME] [--set k=v]... [--dry-run] [--json]`: one preset's sections appended to config.toml through the config writer, after the same plan and check `preset show` runs.
//!
//! What is saved is exactly what `preset show` prints for the same
//! arguments: `plan_against_file` and `check_plan` decide it, and
//! `ConfigFile::save` writes it after the file's own bytes. A compatible
//! `[auth.*]` the file has is reused and not written. `--dry-run` prints that
//! TOML on stdout and writes nothing. The notices, the steps left after the
//! append (from the append itself on a dry run) and the warnings go to
//! stderr; `--json` prints the save result `config add` prints, with those
//! steps as `next_steps`. No secret is resolved and nothing is contacted.

use anyhow::Result;
use serde::Serialize;

use super::config_add::SaveReport;
use super::preset::{check_plan, plan_against_file, print_reuse_notice, print_steps};
use crate::adapters::config::edit::Change;
use crate::console::progress;
use crate::domain::functions::preset_render::{AuthAction, PresetRequest};

/// `config add`'s save result, with the steps left after the append.
#[derive(Serialize)]
struct PresetAddReport {
    #[serde(flatten)]
    save: SaveReport,
    next_steps: Vec<String>,
}

pub async fn run(id: &str, request: &PresetRequest, dry_run: bool, json: bool) -> Result<()> {
    let (file, plan) = plan_against_file(id, request)?;
    let (appended, validated) = check_plan(&file, &plan).await?;
    let fragment = plan.fragment.as_ref().expect("check_plan read it");
    let warnings = validated.warnings.clone();
    if !dry_run {
        file.save(validated)?;
    }
    let path = file.path().display().to_string();
    // A dry run has not appended yet: its steps start with the append.
    let steps = &plan.setup[plan.after_append - usize::from(dry_run)..];
    if json {
        let reused = (plan.auth_action == AuthAction::Reuse)
            .then(|| Change::of(format!("auth.{}", plan.auth), "reuse"));
        let added = appended
            .units
            .iter()
            .map(|section| Change::of(section.clone(), "add"));
        let report = PresetAddReport {
            save: SaveReport {
                path,
                changed: true,
                applied: !dry_run,
                changes: reused.into_iter().chain(added).collect(),
                warnings,
            },
            next_steps: steps.to_vec(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if dry_run {
        print!("{fragment}");
    }
    print_reuse_notice(&plan);
    let sections: Vec<String> = appended
        .units
        .iter()
        .map(|unit| format!("[{unit}]"))
        .collect();
    if dry_run {
        progress!(
            "# dry run: {} would be added to {path}; nothing was written",
            sections.join(", ")
        );
    } else {
        progress!("# added {} to {path}", sections.join(", "));
    }
    progress!("# Next steps for {}", plan.id);
    print_steps(steps);
    for warning in &warnings {
        progress!("# warning: {warning}");
    }
    Ok(())
}
