//! `kurama mcp`: an MCP server on stdio, or over Streamable HTTP with `--listen`, whose tools run kurama's own JSON commands as an agent.
//!
//! stdout carries the protocol only, one JSON-RPC message per line; over
//! HTTP it carries nothing and stderr gets one line per request. Each tool
//! call is this binary run again with `KURAMA_AGENT=1`, so the call goes
//! through the command line's own path: the `[agent]` policy, the audit
//! entry, the JSON envelope and the error document. The child gets no
//! terminal, so a call that needs a person ends with the error document and
//! its `next_actions` instead of waiting.

use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::adapters::config::Config;
use crate::adapters::mcp_http::{Endpoint, RunTool, bind, serve};
use crate::adapters::own_command::{ended_late, run_own_command};
use crate::adapters::secret_resolver::ConfiguredSecrets;
use crate::domain::functions::mcp::{
    Incoming, ToolRun, read_message, result, tool_failure, tool_result,
};
use crate::domain::functions::mcp_http::McpToken;
use crate::domain::types::limits::MCP_HTTP;
use crate::ports::secret::SecretError;
use crate::ports::{AwsProfileCredentials, SecretResolver};
use crate::shell::agent_policy::AGENT_ENV;
use crate::shell::aws_profile_credentials::AssumedRoles;

/// The address `[mcp] listen` names could not be listened on.
#[derive(Debug, thiserror::Error)]
#[error("cannot listen on {address}")]
pub struct McpListenFailed {
    pub address: SocketAddr,
    #[source]
    pub source: std::io::Error,
}

pub fn command() -> Command {
    Command::new("mcp")
        .about(
            "Serve api, data and db as MCP tools on stdio or over HTTP, every call as an agent's",
        )
        .long_about(
            "Serve kurama as an MCP server on stdio (JSON-RPC, one message per line).\n\n\
             Tools: ready, list_apis, list_operations, describe_operation, call_api,\n\
             query_data and query_db (read only), and with [obsidian] obsidian_search,\n\
             obsidian_read and obsidian_files; [mcp] tools names a subset. Each call\n\
             runs kurama's own JSON command with KURAMA_AGENT=1, so the [agent] policy\n\
             refuses what it refuses on the command line, the audit log records the call,\n\
             and a failure is the same JSON error document. Nothing prompts: a call that\n\
             needs a person returns that document with its next_actions. stdout carries\n\
             the protocol only. The configuration is read at start: an invalid one stops\n\
             kurama mcp with CONFIG_INVALID.\n\n\
             With --listen, serve MCP Streamable HTTP (JSON responses, no session) on the\n\
             loopback address [mcp] listen names, for a client in the cloud reaching it\n\
             through Tailscale Funnel or another TLS front. Every request carries the\n\
             token [mcp] token refers to in Authorization, with or without `Bearer `.",
        )
        .arg(
            Arg::new("listen")
                .long("listen")
                .action(ArgAction::SetTrue)
                .help("Serve over HTTP on [mcp] listen instead of stdio, checking [mcp] token"),
        )
}

pub async fn run(listen: bool, config: &Config) -> Result<()> {
    let program = std::env::current_exe()?;
    let exposed = config.mcp.exposed_tools(config.obsidian.is_some());
    let deadline = Duration::from_secs(config.mcp.call_timeout);
    if listen {
        return run_http(config, program, exposed, deadline).await;
    }
    // One request at a time: the next line is read only once this one is
    // answered, so a blocking read holds up nothing.
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match read_message(&line, &exposed) {
            Incoming::Nothing => continue,
            Incoming::Answer(answer) => answer,
            Incoming::Call { id, run } => result(id, call_tool(&program, &run, deadline).await),
        };
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{answer}")?;
        stdout.flush()?;
    }
    Ok(())
}

async fn run_http(
    config: &Config,
    program: PathBuf,
    exposed: Vec<String>,
    deadline: Duration,
) -> Result<()> {
    let listen = config.mcp.listening()?;
    // Bound first: a port another process holds is not worth a 1Password
    // prompt.
    let (listener, address) = bind(listen.address)
        .await
        .map_err(|source| McpListenFailed {
            address: listen.address,
            source,
        })?;
    // Read once: a changed token takes a restart.
    let roles: Arc<dyn AwsProfileCredentials> =
        Arc::new(AssumedRoles::of_config(Arc::new(config.clone())));
    let token = ConfiguredSecrets::new(&config.onepassword, roles)
        .resolve(listen.token)
        .await?;
    // An empty token would admit an empty `Authorization`: refused here,
    // before anything listens for a client.
    let token = McpToken::new(token).ok_or_else(|| {
        SecretError::invalid(format!(
            "[mcp] token {:?} holds an empty value",
            listen.token
        ))
    })?;
    crate::console::write_line(&format!("listening on {address}"));
    let slots = Arc::new(Semaphore::new(config.mcp.max_concurrent_calls));
    let program = Arc::new(program);
    let run_tool: RunTool = Arc::new(move |run: ToolRun| {
        let (slots, program) = (Arc::clone(&slots), Arc::clone(&program));
        Box::pin(async move {
            // The wait for a slot counts toward the call's deadline: a
            // client gives up after its own, whatever the server is doing.
            let call = async {
                let _slot = slots
                    .acquire_owned()
                    .await
                    .expect("the slots are never closed");
                call_tool(&program, &run, deadline).await
            };
            tokio::time::timeout(deadline, call)
                .await
                .unwrap_or_else(|_| tool_failure(&ended_late(deadline)))
        })
    });
    serve(
        listener,
        Arc::new(Endpoint {
            token,
            exposed,
            run_tool,
            limits: MCP_HTTP,
        }),
    )
    .await;
    Ok(())
}

/// Run one tool call as kurama's own command, and answer its tool result.
async fn call_tool(program: &Path, run: &ToolRun, deadline: Duration) -> Value {
    match run_own_command(
        program,
        &run.args,
        &[(AGENT_ENV, "1")],
        run.stdin.as_deref(),
        deadline,
    )
    .await
    {
        Ok(output) => tool_result(output.success, &output.stdout, &output.stderr),
        Err(error) => tool_failure(&error.to_string()),
    }
}
