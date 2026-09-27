//! Running the 1Password CLI for a caller who may be a program: the service
//! account token from the keychain, and a deadline so an unanswered biometric
//! prompt cannot block a headless run forever. Every `op` invocation goes
//! through here, so none of them can skip the token or the deadline. The
//! deadline bounds `op` itself: a descendant that inherited its pipes keeps
//! them open, and reading them out is what the call waits on.

use std::collections::HashMap;
use std::io::{Error, ErrorKind, IsTerminal, Read, Result};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing::warn;
use zeroize::Zeroizing;

use crate::adapters::config::OnePasswordConfig;
use crate::adapters::keychain;

const SERVICE_ACCOUNT_TOKEN: &str = "OP_SERVICE_ACCOUNT_TOKEN";
const POLL: Duration = Duration::from_millis(25);

/// The keychain is read once per process, not once per `OpCli`. One command
/// builds several of these -- the MFA session, the AWS credentials provider,
/// `op read` for a client secret -- so an entry read per instance warned
/// twice in a single `kurama api`, and on a terminal it would ask a person to
/// approve the same entry as many times. Keyed by the service name, the only
/// thing the lookup varies by within a process.
static TOKENS: OnceLock<Mutex<HashMap<String, Option<Zeroizing<String>>>>> = OnceLock::new();

pub struct OpCli {
    pub(crate) cli_path: String,
    /// `None` when a person is at the terminal and may take as long as they
    /// need to answer the biometric prompt.
    deadline: Option<Duration>,
    service_account_keychain: Option<String>,
}

impl OpCli {
    pub fn from_config(config: &OnePasswordConfig) -> Self {
        Self {
            cli_path: config.cli_path.clone(),
            deadline: (!std::io::stdin().is_terminal())
                .then(|| Duration::from_secs(config.timeout)),
            service_account_keychain: config.service_account_keychain.clone(),
        }
    }

    pub fn is_available(&self) -> bool {
        self.run(&["--version"]).is_ok()
    }

    pub fn run(&self, args: &[&str]) -> Result<Output> {
        let token = self.service_account_token();
        self.run_with_token(args, token.as_ref().map(|token| token.as_str()))
    }

    /// The environment wins: a caller that exported a token means it, and the
    /// child inherits it. Otherwise the keychain answers once per process.
    fn service_account_token(&self) -> Option<Zeroizing<String>> {
        // An empty export is what a failed `security ... -w` leaves behind,
        // so it must not shadow the keychain entry the user configured.
        if std::env::var_os(SERVICE_ACCOUNT_TOKEN).is_some_and(|value| !value.is_empty()) {
            return None;
        }
        let service = self.service_account_keychain.as_ref()?;
        let mut tokens = TOKENS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .expect("nothing panics holding the token cache");
        if let Some(token) = tokens.get(service) {
            return token.clone();
        }
        // Read under the lock: two threads reaching an entry that asks a
        // person would otherwise put up two dialogs for one answer.
        let token = read_service_account_token(service);
        tokens.insert(service.clone(), token.clone());
        token
    }

    fn run_with_token(&self, args: &[&str], token: Option<&str>) -> Result<Output> {
        let mut command = Command::new(&self.cli_path);
        command.args(args);
        if let Some(token) = token {
            command.env(SERVICE_ACCOUNT_TOKEN, token);
        }
        // `op` must never read this process's terminal: headless it would block,
        // and under the TUI it would steal the keystrokes.
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let Some(deadline) = self.deadline else {
            return command.output();
        };
        let mut child = command.spawn()?;
        // Drain both pipes while polling: a child that fills one never exits.
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let stdout = std::thread::spawn(move || read_all(&mut stdout));
        let stderr = std::thread::spawn(move || read_all(&mut stderr));
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err(Error::new(ErrorKind::TimedOut, timed_out(args, deadline)));
            }
            std::thread::sleep(POLL);
        };
        Ok(Output {
            status,
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}

fn read_all(pipe: &mut impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer);
    buffer
}

/// The entry named by `[onepassword] service_account_keychain`, for `$USER`. A
/// keychain entry kurama may not read is the common first run (macOS grants
/// access per build signature), so say so instead of running `op` as if no
/// token had been configured.
fn read_service_account_token(service: &str) -> Option<Zeroizing<String>> {
    let Ok(user) = std::env::var("USER") else {
        warn!(
            keychain_service = %service,
            "no 1Password service account token: USER is unset, so the keychain \
             entry has no account to look up"
        );
        return None;
    };
    match keychain::load_secret(service, &user) {
        Ok(Some(token)) => Some(token),
        Ok(None) => {
            warn!(
                keychain_service = %service,
                account = %user,
                "no 1Password service account token: there is no such keychain entry"
            );
            None
        }
        #[cfg(target_os = "macos")]
        Err(keychain::KeychainError::Denied(denied)) => {
            keychain::log_denied(
                &format!("1Password service account token ({service}, account {user})"),
                denied,
            );
            None
        }
        Err(error) => {
            warn!(
                keychain_service = %service,
                account = %user,
                ?error,
                "no 1Password service account token: the keychain entry could not be read"
            );
            None
        }
    }
}

fn timed_out(args: &[&str], deadline: Duration) -> String {
    format!(
        "op {} gave up after {}s: nobody is at the terminal to approve it. \
         Export {SERVICE_ACCOUNT_TOKEN}, or name the keychain entry holding it \
         in [onepassword] service_account_keychain",
        args.join(" "),
        deadline.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;

    fn cli(deadline: Option<Duration>) -> OpCli {
        OpCli {
            cli_path: "/bin/sh".into(),
            deadline,
            service_account_keychain: None,
        }
    }

    /// One `kurama api` builds an `OpCli` for the MFA session and another for
    /// the credentials provider. Reading the entry per instance is what warned
    /// twice about the same keychain, and on a terminal would ask a person to
    /// approve it twice.
    #[cfg(feature = "test-fakes")]
    #[test]
    #[serial_test::serial]
    fn the_service_account_token_is_read_once_for_the_whole_process() {
        let directory = tempfile::tempdir().unwrap();
        // Unique, because the cache outlives the test that filled it.
        let service = format!("kurama-test-once-{}", std::process::id());
        let user = "kurama-test-user";
        std::fs::create_dir_all(directory.path().join(&service)).unwrap();
        let entry = directory.path().join(&service).join(user);
        std::fs::write(&entry, "ops_from_the_keychain").unwrap();

        let previous = std::env::var_os("USER");
        test_env::set("USER", user);
        test_env::set("KURAMA_TEST_KEYCHAIN_SECRET_DIR", directory.path());
        test_env::remove(SERVICE_ACCOUNT_TOKEN);

        let component = || OpCli {
            cli_path: "/bin/sh".into(),
            deadline: None,
            service_account_keychain: Some(service.clone()),
        };
        assert_eq!(
            component()
                .service_account_token()
                .as_ref()
                .map(|token| token.as_str()),
            Some("ops_from_the_keychain")
        );
        // The entry is gone, so a second read would come back empty; the
        // token still does not, because nobody reads it twice.
        std::fs::remove_file(&entry).unwrap();
        assert_eq!(
            component()
                .service_account_token()
                .as_ref()
                .map(|token| token.as_str()),
            Some("ops_from_the_keychain")
        );

        test_env::remove("KURAMA_TEST_KEYCHAIN_SECRET_DIR");
        test_env::set_or_remove("USER", previous);
    }

    #[test]
    fn a_read_nobody_answers_is_killed_at_the_deadline() {
        let started = Instant::now();
        let error = cli(Some(Duration::from_millis(200)))
            .run_with_token(&["-c", "exec sleep 30"], None)
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited too long"
        );
        let message = error.to_string();
        assert!(message.contains("gave up after 0s"), "{message}");
        assert!(message.contains(SERVICE_ACCOUNT_TOKEN), "{message}");
    }

    #[test]
    fn the_token_reaches_the_cli_as_its_environment_variable() {
        let output = cli(Some(Duration::from_secs(5)))
            .run_with_token(
                &["-c", "printf %s \"$OP_SERVICE_ACCOUNT_TOKEN\""],
                Some("ops_x"),
            )
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "ops_x");
    }

    // Reads the inherited environment, so it cannot run beside the tests
    // that set OP_SERVICE_ACCOUNT_TOKEN.
    #[test]
    #[serial_test::serial]
    fn without_a_token_the_cli_sees_none() {
        let output = cli(Some(Duration::from_secs(5)))
            .run_with_token(
                &["-c", "printf %s \"${OP_SERVICE_ACCOUNT_TOKEN:-unset}\""],
                None,
            )
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "unset");
    }

    /// The failure message of `op` is what the caller reports, so it has to
    /// come back in `stderr` rather than land on this process's own.
    #[test]
    fn stderr_is_captured_with_and_without_a_deadline() {
        for deadline in [Some(Duration::from_secs(5)), None] {
            let output = cli(deadline)
                .run_with_token(&["-c", "echo boom >&2; echo out; exit 1"], None)
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&output.stderr), "boom\n");
            assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
            assert_eq!(output.status.code(), Some(1));
        }
    }

    /// A reply larger than the pipe buffer must not look like a timeout.
    #[test]
    fn an_answer_bigger_than_the_pipe_buffer_still_arrives() {
        let output = cli(Some(Duration::from_secs(10)))
            .run_with_token(&["-c", "yes x | head -c 200000"], None)
            .unwrap();
        assert_eq!(output.stdout.len(), 200_000);
    }

    /// `op` inherits no terminal: a read of stdin ends at once.
    #[test]
    fn the_cli_gets_no_stdin() {
        let output = cli(Some(Duration::from_secs(5)))
            .run_with_token(&["-c", "cat; printf eof"], None)
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "eof");
    }

    #[test]
    #[serial_test::serial]
    fn an_exported_token_is_left_for_the_child_to_inherit() {
        let mut cli = cli(None);
        cli.service_account_keychain = Some("kurama-never-read".into());
        test_env::set(SERVICE_ACCOUNT_TOKEN, "ops_from_the_environment");
        let token = cli.service_account_token();
        test_env::remove(SERVICE_ACCOUNT_TOKEN);
        assert!(token.is_none(), "the keychain must not be consulted");
    }

    #[test]
    #[serial_test::serial]
    fn without_a_keychain_name_there_is_no_token_to_pass() {
        test_env::remove(SERVICE_ACCOUNT_TOKEN);
        assert!(cli(None).service_account_token().is_none());
    }
}
