//! `cargo xtask doctor`: whether the toolchain, the feature map, its test
//! filters and scenario names, and the fakes are ready.

use std::process::Command;

use crate::features::{FeatureMap, load_features};
use crate::impact::{claimed_files, default_base, derived_filter, test_filters};
use crate::verify::{list_scenarios, mapped_scenarios, qualify_scenarios, test_names};
use crate::{TEST_FEATURES, cargo_has, cases, git, plugin_installed, root, run_in_root};

pub(crate) fn doctor() -> Result<(), String> {
    // Before anything compiles: a stale file fails the build with a missing
    // file, or leaves a case out of it silently.
    cases::check_generated()?;
    println!(
        "ok   generated cases: {} matches tests/cases/",
        cases::GENERATED
    );
    let mut failed = false;
    let mut report = |ok: bool, name: &str, detail: String| {
        println!("{} {name}: {detail}", if ok { "ok  " } else { "FAIL" });
        failed |= !ok;
    };

    let required = read_rust_version()?;
    let rustc = run_in_root("rustc", &["--version"]).unwrap_or_default();
    let installed = rustc.split_whitespace().nth(1).unwrap_or("").to_string();
    report(
        version_at_least(&installed, &required),
        "rust toolchain",
        format!("{installed} (requires {required})"),
    );
    for (tool, subcommand) in [("cargo-fmt", "fmt"), ("cargo-clippy", "clippy")] {
        let ok = cargo_has(subcommand);
        report(
            ok,
            tool,
            if ok {
                "installed".into()
            } else {
                "missing (rustup component add)".into()
            },
        );
    }
    let git_ok = git(&["rev-parse", "--is-inside-work-tree"]).is_ok();
    report(
        git_ok,
        "git",
        format!(
            "base for impact: {} (branch-check, mutate and scenarios-check use it too)",
            default_base()
        ),
    );

    feature_map_checks(&mut report);
    for fake in ["op", "open"] {
        let path = root().join("tests/fakes").join(fake);
        let ok = Command::new(&path)
            .arg("--version")
            .env("KURAMA_FAKE_OP_LOG", "")
            .env("KURAMA_FAKE_OPEN_LOG", "")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        report(
            ok,
            &format!("fake {fake}"),
            format!("{} executable", path.display()),
        );
    }
    let zsh = Command::new("zsh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    optional(
        "zsh",
        zsh,
        "available",
        "missing; shell integration test is skipped",
    );
    optional(
        "cargo-machete",
        plugin_installed("cargo-machete"),
        "installed; `cargo xtask check` runs it",
        "optional: cargo install --locked cargo-machete (unused dependency gate)",
    );
    optional(
        "cargo-sweep",
        plugin_installed("cargo-sweep"),
        "installed; `cargo xtask check` caps target/ at 12GB",
        "optional: mise run setup or cargo install --locked cargo-sweep",
    );
    optional(
        "cargo-llvm-cov",
        cargo_has("llvm-cov"),
        "installed",
        "optional, for coverage",
    );

    if failed {
        Err("doctor found problems".into())
    } else {
        Ok(())
    }
}

/// The `.agent/features/` checks: every path exists, every test filter
/// selects a test, no declared filter is one a path implies, and every
/// scenario name resolves to a test.
fn feature_map_checks(report: &mut impl FnMut(bool, &str, String)) {
    match load_features() {
        Ok(features) => {
            let missing: Vec<String> = features
                .values()
                .flat_map(|f| f.files.iter().chain(std::iter::once(&f.entry)))
                .filter(|p| !root().join(p.trim_end_matches('/')).exists())
                .cloned()
                .collect();
            report(
                missing.is_empty(),
                ".agent/features/ paths",
                if missing.is_empty() {
                    format!("{} features", features.len())
                } else {
                    format!("missing {missing:?}")
                },
            );
            match list_all_tests() {
                Ok(tests) => {
                    let empty: Vec<String> = features
                        .iter()
                        .flat_map(|(name, f)| {
                            test_filters(f).into_iter().map(move |t| (name.clone(), t))
                        })
                        .filter(|(_, filter)| !tests.iter().any(|t| t.contains(filter.as_str())))
                        .map(|(name, filter)| format!("[{name}] {filter}"))
                        .collect();
                    report(
                        empty.is_empty(),
                        ".agent/features/ test filters",
                        if empty.is_empty() {
                            format!("every filter matches at least one of {} tests", tests.len())
                        } else {
                            format!("match nothing: {empty:?}")
                        },
                    );
                    let redundant = redundant_filters(&features, &tests);
                    report(
                        redundant.is_empty(),
                        ".agent/features/ declared filters",
                        if redundant.is_empty() {
                            "every declared filter selects a test no claimed path implies"
                                .to_string()
                        } else {
                            format!("already derived from `files`, so delete them: {redundant:?}")
                        },
                    );
                }
                Err(e) => report(false, "test listing", e),
            }
            // `verify` passes these to `--exact`, which matches the name the
            // test binary lists. A name that resolves to nothing would make a
            // run select nothing and still exit 0.
            match list_scenarios() {
                Ok(listed) => {
                    let mapped = mapped_scenarios(&features);
                    let (ok, detail) = match qualify_scenarios(&mapped, &listed) {
                        Ok(_) => (
                            true,
                            format!("every one of {} names matches a test", mapped.len()),
                        ),
                        Err(e) => (false, e),
                    };
                    report(ok, ".agent/features/ scenarios", detail);
                }
                Err(e) => report(false, "scenario listing", e),
            }
        }
        Err(e) => report(false, ".agent/features/", e),
    }
}

/// The declared filters a claimed path already implies, as `[feature]
/// filter`. A filter a path already implies is one more thing that can drift:
/// `tests` is for the tests no path can imply.
fn redundant_filters(features: &FeatureMap, tests: &[String]) -> Vec<String> {
    let mut redundant: Vec<String> = Vec::new();
    for (name, feature) in features {
        let derived: Vec<String> = feature
            .files
            .iter()
            .flat_map(|entry| claimed_files(entry))
            .filter_map(|file| derived_filter(&file))
            .collect();
        for filter in &feature.tests {
            let mut selected = 0;
            let mut all_covered = true;
            for test in tests {
                if !test.contains(filter.as_str()) {
                    continue;
                }
                selected += 1;
                if !derived.iter().any(|d| test.contains(d.as_str())) {
                    all_covered = false;
                }
            }
            if selected > 0 && all_covered {
                redundant.push(format!("[{name}] {filter}"));
            }
        }
    }
    redundant
}

/// A tool a run can do without: `ok  ` when it is there, `warn` with what it
/// costs when it is not.
fn optional(name: &str, ok: bool, if_ok: &str, if_missing: &str) {
    println!(
        "{} {name}: {}",
        if ok { "ok  " } else { "warn" },
        if ok { if_ok } else { if_missing }
    );
}

fn read_rust_version() -> Result<String, String> {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).map_err(|e| e.to_string())?;
    manifest
        .lines()
        .find_map(|line| {
            line.strip_prefix("rust-version = \"")
                .and_then(|r| r.strip_suffix('"'))
        })
        .map(str::to_string)
        .ok_or_else(|| "rust-version missing from Cargo.toml".into())
}

fn version_at_least(installed: &str, required: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split(|c: char| !c.is_ascii_digit())
            .filter_map(|p| p.parse().ok())
            .take(3)
            .collect()
    };
    parse(installed) >= parse(required)
}

/// Every test name in the workspace (`cargo test --workspace -- --list`).
fn list_all_tests() -> Result<Vec<String>, String> {
    let output = run_in_root(
        "cargo",
        &[
            &["test", "--locked", "--quiet", "--workspace"][..],
            &TEST_FEATURES,
            &["--", "--list"],
        ]
        .concat(),
    )?;
    Ok(test_names(&output).into_iter().collect())
}
