//! `cargo xtask architecture-audit` run as the binary over a scratch tree:
//! the exit status, the two reports it writes, and its usage.

use std::path::{Path, PathBuf};
use std::process::Output;

mod support;

const RULES: &str = r#"
[[rule]]
id = "ARCH-001"
summary = "one rule"
checks = ["tests/architecture/layers.rs::the_rule_holds"]
docs = ["AGENTS.md"]
"#;

const LIST: &str = "//! ARCH-001 one rule.\n";

const LAYERS: &str = "//! Layers.\n#[test]\nfn the_rule_holds() {}\n";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str, list: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("xtask-audit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let architecture = dir.join("repo/tests/architecture");
    std::fs::create_dir_all(&architecture).unwrap();
    std::fs::write(architecture.join("rules.toml"), RULES).unwrap();
    std::fs::write(architecture.join("main.rs"), list).unwrap();
    std::fs::write(architecture.join("layers.rs"), LAYERS).unwrap();
    std::fs::write(dir.join("repo/AGENTS.md"), "# Agents\n").unwrap();
    Scratch(dir)
}

fn audit(scratch: &Path, args: &[&str]) -> Output {
    support::xtask(scratch)
        .arg("architecture-audit")
        .args(args)
        .env("KURAMA_XTASK_ROOT", scratch.join("repo"))
        .output()
        .unwrap()
}

fn report(scratch: &Path, name: &str) -> String {
    std::fs::read_to_string(scratch.join("repo/target/agent").join(name)).unwrap()
}

#[test]
fn architecture_audit_passes_a_consistent_tree_and_writes_both_reports() {
    let dir = scratch("clean", LIST);
    let output = audit(&dir.0, &[]);
    assert!(output.status.success(), "{output:?}");
    let markdown = report(&dir.0, "architecture-report.md");
    assert!(markdown.contains("Status: PASS"), "{markdown}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), markdown);
    let json: serde_json::Value =
        serde_json::from_str(&report(&dir.0, "architecture-report.json")).unwrap();
    assert_eq!(json["passed"], true);
    assert_eq!(json["audit"]["detectors"]["registered"], 1);
    assert_eq!(json["audit"]["detection_rate"]["measured"], false);
}

#[test]
fn architecture_audit_fails_when_the_list_names_a_rule_the_registry_lacks() {
    let dir = scratch("broken", "//! ARCH-001 one.\n//! ARCH-002 two.\n");
    let output = audit(&dir.0, &[]);
    assert_eq!(output.status.code(), Some(1));
    let markdown = report(&dir.0, "architecture-report.md");
    assert!(
        markdown.contains(
            "- ARCH-002: in the list in tests/architecture/main.rs but not in the registry"
        ),
        "{markdown}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&report(&dir.0, "architecture-report.json")).unwrap();
    assert_eq!(json["passed"], false);
}

/// The scratch tree registers no fixture, so running them starts no cargo
/// and still turns both rates into measured ones.
#[test]
fn architecture_audit_with_run_fixtures_reports_measured_rates() {
    let dir = scratch("fixtures", LIST);
    let output = audit(&dir.0, &["--run-fixtures"]);
    assert!(output.status.success(), "{output:?}");
    let json: serde_json::Value =
        serde_json::from_str(&report(&dir.0, "architecture-report.json")).unwrap();
    assert_eq!(json["audit"]["detection_rate"]["measured"], true);
    assert_eq!(json["audit"]["passing_rate"]["measured"], true);
    assert!(
        report(&dir.0, "architecture-report.md")
            .contains("| Violation detection rate | 0 / 0 (0%) |")
    );
}

#[test]
fn architecture_audit_prints_its_usage_and_refuses_an_argument() {
    let dir = scratch("usage", LIST);
    let help = audit(&dir.0, &["--help"]);
    assert!(help.status.success());
    assert!(
        String::from_utf8_lossy(&help.stdout).starts_with("usage: cargo xtask architecture-audit")
    );
    let short = audit(&dir.0, &["-h"]);
    assert_eq!(short.stdout, help.stdout);
    let refused = audit(&dir.0, &["--json"]);
    assert!(!refused.status.success());
    assert!(
        !dir.0.join("repo/target").exists(),
        "a refused run wrote a report"
    );
}
