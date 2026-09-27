//! `kurama mcp`: an MCP server on stdio whose tools run kurama's own JSON commands as an agent, one request at a time.
//!
//! stdout carries the protocol only, one JSON-RPC message per line. Each
//! tool call is this binary run again with `KURAMA_AGENT=1`, so the call
//! goes through the command line's own path: the `[agent]` policy, the
//! audit entry, the JSON envelope and the error document. The child gets no
//! terminal, so a call that needs a person ends with the error document and
//! its `next_actions` instead of waiting.

use std::io::{BufRead, Write};
use std::time::Duration;

use anyhow::Result;
use clap::Command;

use crate::adapters::own_command::run_own_command;
use crate::domain::functions::mcp::{Incoming, read_message, result, tool_failure, tool_result};
use crate::shell::agent_policy::AGENT_ENV;

/// How long one tool call may run; each command has its own, shorter bounds.
const CALL_DEADLINE: Duration = Duration::from_secs(600);

pub fn command() -> Command {
    Command::new("mcp")
        .about("Serve api, data and db as MCP tools on stdio, every call as an agent's")
        .long_about(
            "Serve kurama as an MCP server on stdio (JSON-RPC, one message per line).\n\n\
             Tools: ready, list_apis, list_operations, describe_operation, call_api,\n\
             query_data and query_db (read only). Each call runs kurama's own JSON command\n\
             with KURAMA_AGENT=1, so the [agent] policy refuses what it refuses on the\n\
             command line, the audit log records the call, and a failure is the same JSON\n\
             error document. Nothing prompts: a call that needs a person returns that\n\
             document with its next_actions. stdout carries the protocol only.",
        )
}

pub async fn run() -> Result<()> {
    let program = std::env::current_exe()?;
    // One request at a time: the next line is read only once this one is
    // answered, so a blocking read holds up nothing.
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match read_message(&line) {
            Incoming::Nothing => continue,
            Incoming::Answer(answer) => answer,
            Incoming::Call { id, run } => {
                let outcome = run_own_command(
                    &program,
                    &run.args,
                    &[(AGENT_ENV, "1")],
                    run.stdin.as_deref(),
                    CALL_DEADLINE,
                )
                .await;
                result(
                    id,
                    match outcome {
                        Ok(output) => tool_result(output.success, &output.stdout, &output.stderr),
                        Err(error) => tool_failure(&error.to_string()),
                    },
                )
            }
        };
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{answer}")?;
        stdout.flush()?;
    }
    Ok(())
}
