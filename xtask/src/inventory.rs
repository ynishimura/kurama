//! `cargo xtask inventory`: what the binary has, read from `kurama inventory`, written to target/agent/inventory.{json,md} with the registered scenarios no report was written for.
//!
//! The binary is the source: it walks its clap definition and probes its
//! config types, so a command, option, key or enum value added to the code
//! is in the document without anyone listing it. xtask runs the built binary
//! and does not depend on the kurama crate (that would compile DuckDB into
//! every `cargo xtask`). `--from FILE` reads a document written earlier, for
//! the tests and for a report over another build.
//!
//! The drift it shows is the scenarios a feature registers that no report
//! under target/agent/scenarios/ was written for. Whether each enumerated
//! value has a case or a declaration is `cargo xtask verify-matrix`'s
//! question; `coordinates` counts the values it classifies, by its rules.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::matrix::{array, items};
use crate::{agent_dir, cargo, load_features, root, scenario_report_dir};

const USAGE: &str = "usage: cargo xtask inventory [--json | --md] [--from FILE]";

/// What `inventory` or `verify-matrix` was asked for: both print the report
/// they write, and both read the document from the binary or `--from`.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) json_to_stdout: bool,
    pub(crate) from: Option<PathBuf>,
}

pub(crate) fn parse_request(args: &[String], usage: &str) -> Result<Request, String> {
    let mut request = Request {
        json_to_stdout: false,
        from: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => request.json_to_stdout = true,
            "--md" => request.json_to_stdout = false,
            "--from" => {
                let file = args
                    .next()
                    .ok_or_else(|| format!("--from needs FILE\n{usage}"))?;
                request.from = Some(PathBuf::from(file));
            }
            other => return Err(format!("unknown argument `{other}`\n{usage}")),
        }
    }
    Ok(request)
}

/// The inventory document `request` names: the file `--from` gives, or what
/// the built binary prints.
pub(crate) fn document(request: &Request) -> Result<Value, String> {
    match &request.from {
        Some(file) => read_document(file),
        None => run_binary(),
    }
}

pub(crate) fn inventory(args: &[String]) -> Result<(), String> {
    let request = parse_request(args, USAGE)?;
    let document = document(&request)?;
    let read_from = match &request.from {
        Some(file) => format!("{} (--from)", file.display()),
        None => "kurama inventory (cargo run)".to_owned(),
    };
    let report = report(&document, &read_from)?;
    let markdown = markdown(&report);

    let dir = agent_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let json_path = dir.join("inventory.json");
    let md_path = dir.join("inventory.md");
    let json_text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(&json_path, &json_text).map_err(|e| format!("{}: {e}", json_path.display()))?;
    std::fs::write(&md_path, &markdown).map_err(|e| format!("{}: {e}", md_path.display()))?;

    if request.json_to_stdout {
        println!("{json_text}");
    } else {
        print!("{markdown}");
    }
    Ok(())
}

pub(crate) fn read_document(file: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))
}

/// The document the built binary prints; built with the test features so
/// the artifacts are the ones `verify` already has.
pub(crate) fn run_binary() -> Result<Value, String> {
    let output = cargo()
        .args(["run", "--quiet"])
        .args(crate::TEST_FEATURES)
        .args(["--bin", "kurama", "--", "inventory"])
        .current_dir(root())
        .output()
        .map_err(|e| format!("cargo run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "kurama inventory failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("kurama inventory printed: {e}"))
}

/// The report: the document, what it counts to, and the drift.
fn report(document: &Value, read_from: &str) -> Result<Value, String> {
    let commands = array(document, &["commands"]);
    let keys = array(document, &["config", "keys"]);
    let schemes = array(document, &["secret_schemes", "values"]);
    let kinds = array(document, &["client_kinds", "values"]);
    let hidden = commands.iter().filter(|c| c["hidden"] == true).count();
    let enum_keys = keys.iter().filter(|k| k["kind"] == "enum").count();
    let secret_keys = keys.iter().filter(|k| k["kind"] == "secret").count();
    let coordinates = items(document).len();

    let features = load_features()?;
    let reports = scenario_report_dir();
    let mut without_report = Vec::new();
    for (feature, definition) in &features {
        for scenario in &definition.scenarios {
            if !reports.join(format!("{scenario}.json")).is_file() {
                without_report.push(json!({ "feature": feature, "scenario": scenario }));
            }
        }
    }

    Ok(json!({
        "schema_version": 1,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "read_from": read_from,
        "counts": {
            "commands": commands.len(),
            "hidden_commands": hidden,
            "config_keys": keys.len(),
            "enum_keys": enum_keys,
            "secret_keys": secret_keys,
            "secret_schemes": schemes.len(),
            "client_kinds": kinds.len(),
            "coordinates": coordinates,
        },
        "drift": {
            "scenarios_without_report": without_report,
        },
        "inventory": document,
    }))
}

fn markdown(report: &Value) -> String {
    let counts = &report["counts"];
    let inventory = &report["inventory"];
    let mut out = String::new();
    out.push_str("# Inventory\n\n");
    out.push_str(&format!(
        "Generated: {}\nRead from: {}\nBinary: {} {}\n\n",
        report["generated_at"].as_str().unwrap_or_default(),
        report["read_from"].as_str().unwrap_or_default(),
        inventory["binary"]["name"].as_str().unwrap_or_default(),
        inventory["binary"]["version"].as_str().unwrap_or_default(),
    ));

    out.push_str(&format!(
        "## Commands ({}, {} hidden)\n\nSource: the clap definition, per row.\n\n\
         | command | hidden | arguments | source |\n| --- | --- | --- | --- |\n",
        counts["commands"], counts["hidden_commands"]
    ));
    for command in array(inventory, &["commands"]) {
        let arguments: Vec<String> = array(command, &["arguments"])
            .iter()
            .map(argument_cell)
            .collect();
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            command["path"].as_str().unwrap_or_default(),
            yes_no(command["hidden"] == true),
            arguments.join(", "),
            command["source"].as_str().unwrap_or_default(),
        ));
    }

    out.push_str(&format!(
        "\n## Config keys ({}; {} enum, {} secret)\n\nSource: {}\n\n\
         | section | key | kind | values |\n| --- | --- | --- | --- |\n",
        counts["config_keys"],
        counts["enum_keys"],
        counts["secret_keys"],
        inventory["config"]["source"].as_str().unwrap_or_default(),
    ));
    for key in array(inventory, &["config", "keys"]) {
        let values: Vec<String> = array(key, &["values"])
            .iter()
            .map(|v| format!("`{}`", v.as_str().unwrap_or_default()))
            .collect();
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} |\n",
            key["section"].as_str().unwrap_or_default(),
            key["key"].as_str().unwrap_or_default(),
            key["kind"].as_str().unwrap_or_default(),
            values.join(", "),
        ));
    }

    for (title, path) in [
        ("Secret reference schemes", "secret_schemes"),
        ("JSON client kinds", "client_kinds"),
    ] {
        let values: Vec<String> = array(inventory, &[path, "values"])
            .iter()
            .map(|v| format!("`{}`", v.as_str().unwrap_or_default()))
            .collect();
        out.push_str(&format!(
            "\n## {title}\n\n{} (source: {})\n",
            values.join(", "),
            inventory[path]["source"].as_str().unwrap_or_default(),
        ));
    }

    out.push_str("\n## Drift\n\n");
    let without = array(report, &["drift", "scenarios_without_report"]);
    if without.is_empty() {
        out.push_str("- Every registered scenario has a report under target/agent/scenarios/.\n");
    } else {
        out.push_str(&format!(
            "- Registered scenarios with no report under target/agent/scenarios/ ({}): \
             `cargo xtask verify all` writes one per scenario it ran\n",
            without.len()
        ));
        for entry in without {
            out.push_str(&format!(
                "  - {}: `{}`\n",
                entry["feature"].as_str().unwrap_or_default(),
                entry["scenario"].as_str().unwrap_or_default(),
            ));
        }
    }
    out.push_str(&format!(
        "- {} coordinates to classify: `cargo xtask verify-matrix` says which a case, a declaration or nothing covers.\n",
        counts["coordinates"],
    ));
    out
}

fn argument_cell(argument: &Value) -> String {
    let id = argument["id"].as_str().unwrap_or_default();
    let mut name = match argument["long"].as_str() {
        Some(long) => format!("`--{long}`"),
        None => format!("`{id}`"),
    };
    if argument["positional"] == true {
        name.push_str(" (positional)");
    }
    let values = array(argument, &["values"]);
    if !values.is_empty() {
        let values: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
        name.push_str(&format!(" [{}]", values.join("|")));
    } else if argument["completer"] == true {
        name.push_str(" (completer)");
    } else if argument["takes_value"] == true {
        name.push_str(&format!(
            " <{}>",
            argument["value_name"].as_str().unwrap_or("VALUE")
        ));
    }
    name
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counts_tell_hidden_commands_and_each_kind_of_key_apart() {
        let document = json!({
            "commands": [{"hidden": true}, {"hidden": false}, {}],
            "config": {"keys": [
                {"kind": "enum"}, {"kind": "enum"}, {"kind": "secret"}, {"kind": "string"}
            ]},
            "secret_schemes": {"values": ["op", "aws-ssm"]},
            "client_kinds": {"values": ["data"]},
        });

        let counts = &report(&document, "file").unwrap()["counts"];

        assert_eq!(counts["commands"], 3);
        assert_eq!(counts["hidden_commands"], 1);
        assert_eq!(counts["config_keys"], 4);
        assert_eq!(counts["enum_keys"], 2);
        assert_eq!(counts["secret_keys"], 1);
        assert_eq!(counts["secret_schemes"], 2);
        assert_eq!(counts["client_kinds"], 1);
    }

    #[test]
    fn arguments_read_as_a_person_types_them() {
        assert_eq!(
            argument_cell(
                &json!({"id": "profile", "positional": true, "required": true, "completer": true, "takes_value": true, "values": []})
            ),
            "`profile` (positional) (completer)"
        );
        assert_eq!(
            argument_cell(
                &json!({"id": "kind", "long": "kind", "positional": false, "takes_value": true, "values": ["data", "db"]})
            ),
            "`--kind` [data|db]"
        );
        assert_eq!(
            argument_cell(
                &json!({"id": "json", "long": "json", "positional": false, "takes_value": false, "values": []})
            ),
            "`--json`"
        );
        assert_eq!(
            argument_cell(
                &json!({"id": "timeout", "long": "timeout", "positional": false, "takes_value": true, "value_name": "SECONDS", "values": []})
            ),
            "`--timeout` <SECONDS>"
        );
    }

    #[test]
    fn the_flags_are_json_md_and_from() {
        let parse = |args: &[String]| parse_request(args, USAGE);
        let request = parse(&["--json".into(), "--from".into(), "x.json".into()]).unwrap();
        assert!(request.json_to_stdout);
        assert_eq!(request.from, Some(PathBuf::from("x.json")));
        assert!(!parse(&[]).unwrap().json_to_stdout);
        assert!(parse(&["--from".into()]).unwrap_err().contains("FILE"));
        assert!(parse(&["--nope".into()]).unwrap_err().contains("--nope"));
    }
}
