//! The two files `verify-matrix` writes, target/agent/verification-matrix.{json,md}, from a built `Matrix`; refused when either would carry a credential.

use serde_json::json;

use crate::matrix::{Matrix, RULES, Row, Status};
use crate::verify_real::credential_marks;

// ---------------------------------------------------------------------------
// The report

/// Writes both reports, refusing when either would carry a credential.
pub(crate) fn write(matrix: &Matrix) -> Result<(String, String), String> {
    let json_value = json!({
        "schema_version": 1,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "counts": matrix.counts,
        "gate": { "ok": matrix.gate_problems.is_empty(), "problems": matrix.gate_problems },
        "dimensions": {
            "rules": RULES.iter().map(|(dimension, rule)| json!({ "dimension": dimension, "rule": rule })).collect::<Vec<_>>(),
            "case_only": matrix.case_only_dimensions,
            "possible_typos": matrix.possible_typos,
            "unenumerated_case_values": matrix.unenumerated_case_values,
        },
        "reports_read": matrix.reports_read,
        "cases_read": matrix.cases_read,
        "items": matrix.items,
        "rows": matrix.rows,
    });
    let json_text = serde_json::to_string_pretty(&json_value).map_err(|e| e.to_string())?;
    let markdown = markdown(matrix);
    for (name, text) in [
        ("verification-matrix.json", &json_text),
        ("verification-matrix.md", &markdown),
    ] {
        let marks = credential_marks(text);
        if !marks.is_empty() {
            return Err(format!(
                "{name} would carry {}: a scenario report put a credential in a check detail; nothing was written",
                marks.join(", ")
            ));
        }
    }
    let dir = crate::agent_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, text) in [
        ("verification-matrix.json", &json_text),
        ("verification-matrix.md", &markdown),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok((json_text, markdown))
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

pub(crate) fn markdown(matrix: &Matrix) -> String {
    let mut out = String::new();
    out.push_str("# Verification matrix\n\n");
    out.push_str(&format!(
        "Generated: {}\nCases read: {} (tests/cases/); reports read: {} (target/agent/scenarios/ and scenarios-<evidence>/)\n\n",
        chrono::Utc::now().to_rfc3339(),
        matrix.cases_read,
        matrix.reports_read,
    ));

    out.push_str("## Summary\n\n| status | rows |\n| --- | --- |\n");
    for (status, count) in &matrix.counts {
        out.push_str(&format!("| {status} | {count} |\n"));
    }
    out.push_str(&format!(
        "\nGate: **{}**. {}\n",
        if matrix.gate_problems.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        if matrix.gate_problems.is_empty() {
            "No failed case, and every enumerated value is classified.".to_string()
        } else {
            format!("{} problem(s):", matrix.gate_problems.len())
        }
    ));
    for problem in &matrix.gate_problems {
        out.push_str(&format!("\n- {problem}"));
    }
    out.push('\n');

    out.push_str("\n## Dimensions\n\nWhat has to be classified, derived from `kurama inventory`:\n\n| dimension | rule |\n| --- | --- |\n");
    for (dimension, rule) in RULES {
        out.push_str(&format!("| `{dimension}` | {rule} |\n"));
    }
    out.push_str(&format!(
        "\n{} coordinates. Dimensions cases declare that no rule derives (they refine a case, and are not gated): {}\n",
        matrix.items.len(),
        if matrix.case_only_dimensions.is_empty() {
            "none".to_string()
        } else {
            matrix
                .case_only_dimensions
                .iter()
                .map(|d| format!("`{d}`"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    if !matrix.possible_typos.is_empty() {
        out.push_str(&format!(
            "Named by one case only, so possibly a misspelling of a dimension the other cases share: {}\n",
            matrix
                .possible_typos
                .iter()
                .map(|d| format!("`{d}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if !matrix.unenumerated_case_values.is_empty() {
        out.push_str(&format!(
            "Values cases name that the inventory does not enumerate on that dimension (a rejected value or a refinement; not gated): {}\n",
            matrix
                .unenumerated_case_values
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    out.push_str("\n## Rows\n\n| feature | command / config | combination | expected | observed | evidence | status | reproduce |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for row in &matrix.rows {
        let status = row.status.name();
        let reproduce = if row.reproduce == "-" {
            "-".to_string()
        } else {
            format!("`{}`", row.reproduce)
        };
        let combination = if row.subject == "scenario"
            || row.status == Status::Unclassified
            || row.reproduce == "-"
        {
            row.combination.clone()
        } else {
            format!("{} ({})", row.id, row.combination)
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            cell(&row.feature),
            cell(&row.subject),
            cell(&combination),
            cell(&row.expected),
            cell(&row.observed),
            cell(&row.evidence),
            status,
            reproduce,
        ));
    }

    let failures: Vec<&Row> = matrix
        .rows
        .iter()
        .filter(|row| row.status == Status::Fail)
        .collect();
    out.push_str("\n## Failures\n\n");
    if failures.is_empty() {
        out.push_str("None.\n");
    }
    for row in failures {
        out.push_str(&format!(
            "### {}\n\nReproduce: `{}`\n\n",
            row.id, row.reproduce
        ));
        for check in &row.failed_checks {
            out.push_str(&format!("- {}: {}\n", check.name, check.detail));
        }
        if !row.related.is_empty() {
            out.push_str(&format!(
                "\nRelated (share a coordinate): {}\n",
                row.related
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push('\n');
    }

    out
}
