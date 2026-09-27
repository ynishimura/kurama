//! Runtime verification scenarios of the `agent-guide` feature that compare
//! whole outputs: two runs of the catalog with each other, and the client
//! contracts with the bytes they printed before the catalog existed. The
//! rest are cases under `tests/cases/agent-guide/`.
//! `tests/scenarios/main.rs` holds the naming rule and what each scenario
//! writes.

use crate::support::{Scenario, run};

/// `agent --kind data --json` and `agent --kind db --json` as the binary
/// printed them before `agent --json` was added: the catalog takes the
/// flag without a kind, and must not change what a kind prints.
const DATA_CONTRACT: &str = include_str!("../fixtures/agent/data-contract.json");
const DB_CONTRACT: &str = include_str!("../fixtures/agent/db-contract.json");

#[test]
fn agent_json_is_byte_identical_across_runs() {
    let mut v = run(Scenario::new(
        "agent_json_is_byte_identical_across_runs",
        "agent-guide",
        &["agent", "--json"],
    )
    .then_run(&["agent", "--json"]));
    v.expect_exit_code(0)
        .expect_stderr_empty()
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    let runs = v.observed.runs.clone();
    v.check(
        "two runs print the same catalog, byte for byte",
        runs[0].exit_code == Some(0)
            && runs[0].stdout.starts_with("{\"schema_version\":")
            && runs[0].stdout == runs[1].stdout,
        format!(
            "run 0 {} bytes, run 1 {} bytes",
            runs[0].stdout.len(),
            runs[1].stdout.len()
        ),
    );
    v.finish();
}

/// An invalid `config.toml`: a command that read it would fail, so a pass
/// says the kind contracts read none.
const INVALID_CONFIG: &str = "[mfa.onepassword]\nenabled = false\n";

#[test]
fn agent_json_kind_data_and_db_keep_their_current_output() {
    let mut v = run(Scenario::new(
        "agent_json_kind_data_and_db_keep_their_current_output",
        "agent-guide",
        &["agent", "--kind", "data", "--json"],
    )
    .with_config(INVALID_CONFIG)
    .then_run(&["agent", "--kind", "db", "--json"]));
    v.expect_no_files_written();
    let runs = v.observed.runs.clone();
    for (index, (kind, expected)) in [("data", DATA_CONTRACT), ("db", DB_CONTRACT)]
        .into_iter()
        .enumerate()
    {
        v.keyed_run(index, &format!("--kind {kind}"), |v| {
            v.expect_exit_code(0)
                .expect_stderr_empty()
                .expect_sts_actions(&[])
                .expect_op_calls(0)
                .expect_api_calls(0, None)
                .expect_token_grants(&[])
        });
        let observed = &runs[index];
        v.check(
            &format!("run {index}: --kind {kind} --json prints the contract it printed before"),
            observed.exit_code == Some(0) && observed.stdout == expected,
            format!(
                "exit {:?}, {} bytes, expected {} bytes",
                observed.exit_code,
                observed.stdout.len(),
                expected.len()
            ),
        );
    }
    v.finish();
}
