//! A program run for `kurama mcp` (kurama itself, one subcommand): stdin fed and closed or null, stdout and stderr captured, killed at a deadline.
//!
//! The child never shares this process's stdin: `kurama mcp` reads its
//! protocol there, and a child that read it would take the next request.

use std::io::{Error, ErrorKind, Result};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// What the child printed, and whether it exited 0.
pub struct OwnCommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `program args` with `envs` added to this environment, `stdin` as its
/// whole input, and kill it when it has not ended after `deadline`.
pub async fn run_own_command(
    program: &Path,
    args: &[String],
    envs: &[(&str, &str)],
    stdin: Option<&str>,
    deadline: Duration,
) -> Result<OwnCommandOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(envs.iter().copied())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let input = input.as_bytes().to_vec();
        // Written beside the wait: a child that answers before it has read
        // everything must not block on a full stdout pipe.
        tokio::spawn(async move {
            let _ = pipe.write_all(&input).await;
        });
    }
    let output = tokio::time::timeout(deadline, child.wait_with_output())
        .await
        .map_err(|_| {
            Error::new(
                ErrorKind::TimedOut,
                format!("the call did not end within {} seconds", deadline.as_secs()),
            )
        })??;
    Ok(OwnCommandOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sh(script: &str, stdin: Option<&str>, deadline: Duration) -> Result<OwnCommandOutput> {
        run_own_command(
            Path::new("/bin/sh"),
            &["-c".into(), script.into()],
            &[("OWN_COMMAND_TEST", "set")],
            stdin,
            deadline,
        )
        .await
    }

    #[tokio::test]
    async fn stdin_reaches_the_child_and_both_streams_come_back() {
        let output = sh(
            "cat; echo \"$OWN_COMMAND_TEST\" >&2; exit 3",
            Some("input"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(output.stdout, "input");
        assert_eq!(output.stderr, "set\n");
        assert!(!output.success);
    }

    #[tokio::test]
    async fn without_stdin_the_child_reads_nothing_rather_than_our_stdin() {
        let output = sh("cat; echo end", None, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(output.stdout, "end\n");
        assert!(output.success);
    }

    #[tokio::test]
    async fn an_answer_bigger_than_the_pipe_buffer_still_arrives() {
        let output = sh(
            "head -c 300000 /dev/zero | tr '\\0' x",
            Some(&"y".repeat(300_000)),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(output.stdout.len(), 300_000);
    }

    #[tokio::test]
    async fn a_call_that_does_not_end_is_killed_at_the_deadline() {
        let started = std::time::Instant::now();
        let error = sh("sleep 30", None, Duration::from_millis(300))
            .await
            .err()
            .expect("killed");
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited too long"
        );
    }
}
