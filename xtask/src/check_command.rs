//! `cargo xtask check`: the integration-side gate -- the static checks,
//! every test, the architecture registry and fixtures, the scenario coverage
//! and the matrix -- written to target/agent/verification-report.{json,md}.

use crate::features::load_features;
use crate::gate::Gate;
use crate::verify::{clear_scenario_reports, coverage_gate, mapped_scenarios, write_report};
use crate::{
    TEST_FEATURES, architecture_audit, branch_check, cargo, matrix, root, static_checks, sweep,
};

pub(crate) fn check() -> Result<(), String> {
    branch_check::refuse_a_foreign_target_dir()?;
    // One heavy gate at a time, across every worktree of this clone.
    let _queue = branch_check::wait_for_gate_lock()?;
    clear_scenario_reports()?;
    let mut gates = Vec::new();
    let mut failed = false;

    gates.extend(static_checks::results());
    let args = [
        &["test", "--locked", "--workspace"][..],
        &TEST_FEATURES,
        &["--no-fail-fast"],
    ]
    .concat();
    eprintln!("==> tests: cargo {}", args.join(" "));
    let output = cargo()
        .args(&args)
        .current_dir(root())
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    failed |= !output.status.success();
    gates.push(Gate {
        name: "tests".into(),
        ok: output.status.success(),
        detail: summarize_test_results(&stdout),
    });

    let registry = architecture_gate();
    failed |= !registry.ok;
    gates.push(registry);

    // The fixtures run once the tests built and passed: a detection rate taken
    // over a binary that does not build would count nothing as detected.
    if !failed {
        let fixtures = audit_gate(
            "architecture-fixtures",
            architecture_audit::gate_with_fixtures,
        );
        failed |= !fixtures.ok;
        gates.push(fixtures);
    }

    // The tests ran unfiltered, so every scenario the feature map claims should
    // have written a report. One that did not stopped verifying anything.
    if !failed {
        let coverage = coverage_gate(&mapped_scenarios(&load_features()?))?;
        failed |= !coverage.ok;
        gates.push(coverage);
    }

    // Every scenario wrote its report, so the matrix can say which enumerated
    // value has a case and which case failed.
    if !failed {
        eprintln!("==> matrix: cargo xtask verify-matrix");
        let matrix = matrix::gate();
        failed |= !matrix.ok;
        gates.push(matrix);
    }

    let sweep = sweep::after_check();
    gates.push(Gate {
        name: "sweep".into(),
        ok: true,
        detail: sweep,
    });

    let report = write_report("all", gates)?;
    println!("{report}");
    if failed {
        Err("check failed".into())
    } else {
        eprintln!("==> all checks passed");
        Ok(())
    }
}

/// `cargo xtask architecture-audit` as a gate: the registry has to agree with
/// the tests and the rule list before a run can say the rules hold.
pub(crate) fn architecture_gate() -> Gate {
    audit_gate("architecture-registry", architecture_audit::gate)
}

/// One run of the architecture audit as the gate `name`; the problems it
/// found are printed, and the report names the file that lists them.
fn audit_gate(name: &str, audit: fn() -> Result<String, String>) -> Gate {
    eprintln!("==> {name}: {}", architecture_audit::REGISTRY);
    let result = audit();
    if let Err(problems) = &result {
        eprintln!("{problems}");
    }
    Gate {
        name: name.into(),
        ok: result.is_ok(),
        detail: result.unwrap_or_else(|_| "see target/agent/architecture-report.md".into()),
    }
}

/// "N passed, M failed" from every `test result:` line in cargo test output.
fn summarize_test_results(output: &str) -> String {
    let mut passed = 0;
    let mut failed = 0;
    for line in output.lines().filter(|l| l.starts_with("test result:")) {
        for part in line.split(';') {
            let words: Vec<&str> = part.split_whitespace().collect();
            let number = words
                .iter()
                .find_map(|w| w.parse::<usize>().ok())
                .unwrap_or(0);
            if part.contains("passed") {
                passed += number;
            } else if part.contains("failed") {
                failed += number;
            }
        }
    }
    format!("{passed} passed, {failed} failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary_sums_every_target() {
        let output = "test result: ok. 3 passed; 0 failed; 1 ignored\n\
                      test result: FAILED. 2 passed; 1 failed; 0 ignored\n";
        assert_eq!(summarize_test_results(output), "5 passed, 1 failed");
    }
}
