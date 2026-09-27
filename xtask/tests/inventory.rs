//! `cargo xtask inventory` run as the binary over a scratch tree with a saved
//! document (`--from`): the two reports it writes, what it prints, and the
//! drift it names.

use std::path::{Path, PathBuf};
use std::process::Output;

mod support;

/// A document the way `kurama inventory` prints one, with one more config
/// key than any real build has, so the test can watch it arrive.
const DOCUMENT: &str = r#"{
  "schema_version": 1,
  "binary": { "name": "kurama", "version": "0.0.0-test" },
  "commands": [
    { "path": "api", "hidden": false, "about": "call", "source": "src/shell/cli/args.rs: build_command",
      "arguments": [
        { "id": "api", "positional": true, "long": null, "short": null, "value_name": "API", "required": true, "takes_value": true, "values": [], "value_hint": "Unknown", "completer": true },
        { "id": "json", "positional": false, "long": "json", "short": null, "value_name": null, "required": false, "takes_value": false, "values": [], "value_hint": "Unknown", "completer": false }
      ] },
    { "path": "inventory", "hidden": true, "about": "this", "source": "src/shell/cli/args.rs: build_command", "arguments": [] }
  ],
  "config": {
    "source": "the config types",
    "keys": [
      { "section": "[api.*]", "key": "base_url", "kind": "string", "values": [] },
      { "section": "[api.*]", "key": "freshly_added_key", "kind": "integer", "values": [] },
      { "section": "[auth.*]", "key": "kind", "kind": "enum", "values": ["oauth", "token", "freshly_added_kind"] }
    ]
  },
  "secret_schemes": { "source": "secret_ref.rs", "values": ["op", "aws-ssm"] },
  "client_kinds": { "source": "client.rs", "values": ["data"] }
}"#;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("xtask-inventory-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".agent/features")).unwrap();
    std::fs::create_dir_all(repo.join("target/agent/scenarios")).unwrap();
    std::fs::write(
        repo.join(".agent/features/probed.toml"),
        r#"[probed]
summary = "s"
entry = "src/probed.rs"
files = ["src/probed.rs"]
scenarios = ["probed_ran", "probed_never_ran"]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("target/agent/scenarios/probed_ran.json"),
        "{\"scenario\":\"probed_ran\"}\n",
    )
    .unwrap();
    std::fs::write(dir.join("document.json"), DOCUMENT).unwrap();
    Scratch(dir)
}

fn inventory(scratch: &Path, args: &[&str]) -> Output {
    support::xtask(scratch)
        .arg("inventory")
        .arg("--from")
        .arg(scratch.join("document.json"))
        .args(args)
        .env("KURAMA_XTASK_ROOT", scratch.join("repo"))
        .output()
        .unwrap()
}

fn report(scratch: &Path, name: &str) -> String {
    std::fs::read_to_string(scratch.join("repo/target/agent").join(name)).unwrap()
}

#[test]
fn inventory_shows_a_key_added_to_the_config_types() {
    let dir = scratch("added");
    let output = inventory(&dir.0, &[]);
    assert!(output.status.success(), "{output:?}");
    let markdown = report(&dir.0, "inventory.md");
    assert!(
        markdown.contains("| `[api.*]` | `freshly_added_key` | integer |  |"),
        "{markdown}"
    );
    assert!(
        markdown
            .contains("| `[auth.*]` | `kind` | enum | `oauth`, `token`, `freshly_added_kind` |"),
        "{markdown}"
    );
    assert!(
        markdown.contains("| `api` | no | `api` (positional) (completer), `--json` |"),
        "{markdown}"
    );
    assert!(markdown.contains("| `inventory` | yes |"), "{markdown}");
    let json: serde_json::Value = serde_json::from_str(&report(&dir.0, "inventory.json")).unwrap();
    assert_eq!(json["counts"]["config_keys"], 3);
    assert_eq!(json["counts"]["enum_keys"], 1);
    assert_eq!(json["counts"]["hidden_commands"], 1);
    assert_eq!(
        json["inventory"]["config"]["keys"][1]["key"],
        "freshly_added_key"
    );
    // The markdown is what stdout carries by default.
    assert_eq!(String::from_utf8_lossy(&output.stdout), markdown);
}

/// `coordinates` is what `verify-matrix` classifies, counted by its rules:
/// the non-hidden commands, the enum values, the schemes and the client
/// kinds -- not every key -- and the report sends the reader there.
#[test]
fn inventory_counts_the_coordinates_verify_matrix_classifies() {
    let dir = scratch("cases");
    let output = inventory(&dir.0, &["--json"]);
    assert!(output.status.success(), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // command=api + 3 `source=` values + 2 schemes + 1 client kind.
    assert_eq!(json["counts"]["coordinates"], 7);
    assert_eq!(json["drift"].get("cases"), None);
    let markdown = report(&dir.0, "inventory.md");
    assert!(
        markdown.contains("- 7 coordinates to classify: `cargo xtask verify-matrix`"),
        "{markdown}"
    );
    assert!(!markdown.contains("not read yet"), "{markdown}");
}

#[test]
fn inventory_reports_a_registered_scenario_without_a_report() {
    let dir = scratch("scenarios");
    let output = inventory(&dir.0, &[]);
    assert!(output.status.success(), "{output:?}");
    let markdown = report(&dir.0, "inventory.md");
    assert!(
        markdown.contains("  - probed: `probed_never_ran`\n"),
        "{markdown}"
    );
    assert!(!markdown.contains("`probed_ran`"), "{markdown}");
    let json: serde_json::Value = serde_json::from_str(&report(&dir.0, "inventory.json")).unwrap();
    assert_eq!(
        json["drift"]["scenarios_without_report"],
        serde_json::json!([{ "feature": "probed", "scenario": "probed_never_ran" }])
    );
}

#[test]
fn inventory_refuses_an_unknown_flag_and_a_missing_file() {
    let dir = scratch("usage");
    let output = inventory(&dir.0, &["--nope"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--nope"));
    let output = support::xtask(&dir.0)
        .args(["inventory", "--from"])
        .arg(dir.0.join("absent.json"))
        .env("KURAMA_XTASK_ROOT", dir.0.join("repo"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("absent.json"));
}
