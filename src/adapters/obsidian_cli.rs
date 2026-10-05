//! The official Obsidian CLI, run once per call with no stdin and a deadline; its stdout, or why it gave none.
//!
//! The CLI exits 0 whatever happened and reports its own failures on stdout
//! (`Error: ...`, `Vault not found.`), so the answer is read for one before
//! it is used. The argv comes from `domain::functions::obsidian`, the only
//! place that builds one.

use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

use crate::adapters::own_command::run_own_command;
use crate::domain::functions::obsidian::cli_failure;

#[derive(Debug, thiserror::Error)]
pub enum ObsidianCliError {
    /// The CLI could not be started: absent, or not executable.
    #[error("cannot run the Obsidian CLI {cli:?}")]
    NotRunnable {
        cli: String,
        #[source]
        source: std::io::Error,
    },
    /// Obsidian did not answer in time: not running, or the CLI not enabled.
    #[error("the Obsidian CLI did not answer within {seconds} seconds")]
    NoAnswer { seconds: u64 },
    /// The CLI answered with a failure of its own.
    #[error("the Obsidian CLI failed: {message}")]
    Failed { message: String },
}

/// Run `cli argv` and return what it printed.
pub async fn run_obsidian_cli(
    cli: &str,
    argv: &[String],
    timeout: Duration,
) -> Result<String, ObsidianCliError> {
    let output = run_own_command(Path::new(cli), argv, &[], None, timeout)
        .await
        .map_err(|source| match source.kind() {
            ErrorKind::TimedOut => ObsidianCliError::NoAnswer {
                seconds: timeout.as_secs(),
            },
            _ => ObsidianCliError::NotRunnable {
                cli: cli.to_owned(),
                source,
            },
        })?;
    if !output.success {
        let message = match output.stderr.trim() {
            "" => output.stdout.trim(),
            stderr => stderr,
        };
        return Err(ObsidianCliError::Failed {
            message: message.replace(['\r', '\n'], " "),
        });
    }
    if let Some(message) = cli_failure(&output.stdout) {
        return Err(ObsidianCliError::Failed {
            message: message.to_owned(),
        });
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sh(script: &str, timeout: Duration) -> Result<String, ObsidianCliError> {
        run_obsidian_cli("/bin/sh", &["-c".into(), script.into()], timeout).await
    }

    #[tokio::test]
    async fn the_answer_is_stdout_unless_it_is_the_clis_own_failure() {
        let ok = sh("printf '# note\\nbody\\n'", Duration::from_secs(5)).await;
        assert_eq!(ok.unwrap(), "# note\nbody\n");
        let failed = sh("echo 'Vault not found.'", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(
            failed.to_string(),
            "the Obsidian CLI failed: Vault not found."
        );
        let exited = sh("echo 'it broke' >&2; exit 2", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(exited.to_string(), "the Obsidian CLI failed: it broke");
    }

    #[tokio::test]
    async fn a_missing_cli_and_a_silent_obsidian_are_told_apart() {
        let missing = run_obsidian_cli("/nonexistent/obsidian", &[], Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(
            matches!(missing, ObsidianCliError::NotRunnable { .. }),
            "{missing:?}"
        );
        let silent = sh("sleep 30", Duration::from_secs(1)).await.unwrap_err();
        assert!(
            matches!(silent, ObsidianCliError::NoAnswer { seconds: 1 }),
            "{silent:?}"
        );
    }
}
