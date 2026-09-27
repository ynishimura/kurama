//! A port-forwarding tunnel to a database through an SSM bastion.
//!
//! The websocket and multiplexing protocol of Session Manager is not
//! reimplemented: AWS ships `session-manager-plugin` for it, and this file
//! runs exactly one of them. What it owns is the part that goes wrong on its
//! own -- finding the bastion, keeping the session token out of the command
//! line, learning the local port the plugin chose, and making sure no plugin
//! survives whichever way the call ends.
use crate::adapters::config::InstanceRef;
use crate::adapters::utils::error_chain::causes;
use crate::domain::types::Credentials;
use crate::domain::types::database::{DbError, DbTunnelInfo, InvalidDb};
use aws_sdk_ssm::Client;
use aws_sdk_ssm::error::ProvideErrorMetadata;
use aws_sdk_ssm::types::InstanceInformation;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// The document that forwards a local port to a host the bastion can reach.
const PORT_FORWARD_DOCUMENT: &str = "AWS-StartPortForwardingSessionToRemoteHost";
/// The plugin reads the session response from an environment variable whose
/// name starts with this, and unsets it immediately. Older plugins take the
/// response on the command line instead, where every process can read it.
const RESPONSE_VARIABLE: &str = "AWS_SSM_START_SESSION_RESPONSE";
/// The first plugin release that reads `RESPONSE_VARIABLE`.
const REQUIRED_PLUGIN: (u32, u32, u32, u32) = (1, 2, 536, 0);
const PLUGIN: &str = "session-manager-plugin";

/// What a tunnel needs to know before it exists.
pub struct TunnelRequest<'a> {
    pub credentials: &'a Credentials,
    pub region: String,
    pub instance: InstanceRef,
    /// The database, as the bastion reaches it.
    pub remote_host: String,
    pub remote_port: u16,
    /// How long the plugin has to open the port, once AWS has answered. Each
    /// SSM call before it is bounded on its own, by the client's own timeout.
    pub connect_timeout_secs: u64,
}

/// Proof that a plugin which keeps the session token out of the command line
/// is installed. `OpenTunnel::open` asks for it, so the check cannot be
/// skipped, and the caller runs it before it asks AWS for anything: a stale
/// plugin makes the whole call impossible, and an MFA prompt for a call that
/// cannot work is a prompt nobody should have answered.
pub struct PluginReady(());

/// An open tunnel. `close` ends it; dropping it kills the plugin but cannot
/// tell AWS, so every path calls `close`.
pub struct OpenTunnel {
    pub local_port: u16,
    pub instance_id: String,
    pub region: String,
    session_id: String,
    plugin: Child,
    client: Client,
    /// The plugin keeps talking for as long as it runs. Nothing wants to read
    /// it, but closing its stdout ends it with a broken pipe and leaving the
    /// pipe unread eventually blocks it, so one task reads and discards.
    output: tokio::task::JoinHandle<()>,
    /// The same for its stderr, which is where it says what went wrong. Both
    /// pipes belong to the tunnel, so both end when it does.
    errors: tokio::task::JoinHandle<()>,
    /// The plugin's `$TMPDIR`, removed with its socket after the plugin ends:
    /// declared after `plugin`, so a dropped tunnel kills first.
    plugin_tmp: tempfile::TempDir,
}

impl OpenTunnel {
    /// What the result envelope says about the tunnel. The local port is left
    /// out: it is gone when the call ends, so it names nothing afterwards.
    pub fn reported(&self) -> DbTunnelInfo {
        reported_of(&self.instance_id, &self.region)
    }

    pub async fn open(request: TunnelRequest<'_>, ready: PluginReady) -> Result<Self, DbError> {
        let PluginReady(()) = ready;
        let client = super::ssm_client(request.credentials, &request.region);
        let instance_id = find_bastion(&client, &request.instance).await?;
        let parameters = [
            ("host".to_owned(), vec![request.remote_host.clone()]),
            (
                "portNumber".to_owned(),
                vec![request.remote_port.to_string()],
            ),
        ];
        // The plugin makes its mux socket in `$TMPDIR` and removes it only when
        // it ends on its own, which a killed plugin does not: a directory of
        // the tunnel's own takes the socket with it. A short name, because a
        // Unix socket path has to fit in 104 bytes. Made before the session,
        // so failing here leaves nothing open in AWS.
        let plugin_tmp = tempfile::Builder::new()
            .prefix("k")
            .tempdir()
            .map_err(|source| DbError::Io {
                path: std::env::temp_dir(),
                source,
            })?;
        let started = client
            .start_session()
            .target(&instance_id)
            .document_name(PORT_FORWARD_DOCUMENT)
            .set_parameters(Some(parameters.iter().cloned().collect()))
            .send()
            .await
            .map_err(|error| aws_failure("StartSession", error))?;
        let session_id = started.session_id().unwrap_or_default().to_owned();
        // `localPortNumber` is left out on purpose: the plugin listens on a
        // free port and prints it. Choosing one here would race with whoever
        // takes it between the check and the listen.
        let response = serde_json::json!({
            "SessionId": started.session_id(),
            "TokenValue": started.token_value(),
            "StreamUrl": started.stream_url(),
        })
        .to_string();
        let request_json = serde_json::json!({
            "Target": instance_id,
            "DocumentName": PORT_FORWARD_DOCUMENT,
            "Parameters": {
                "host": [request.remote_host],
                "portNumber": [request.remote_port.to_string()],
            },
        })
        .to_string();
        let spawned = Command::new(PLUGIN)
            // The response carries the session token. Passing the name of an
            // environment variable keeps it out of every process listing.
            .arg(RESPONSE_VARIABLE)
            .arg(&request.region)
            .arg("StartSession")
            // The profile is only used to resume a dropped websocket, which a
            // one-statement call never waits for.
            .arg("")
            .arg(request_json)
            .arg(endpoint(&request.region))
            .env(RESPONSE_VARIABLE, response)
            .env("TMPDIR", plugin_tmp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut plugin = match spawned {
            Ok(plugin) => plugin,
            Err(_) => {
                // The session exists in AWS from `StartSession` on, and no
                // `OpenTunnel` was built to close it. The plugin was there at
                // `require_usable_plugin` and is not now: whatever took it,
                // the session is not left open for the idle timeout.
                terminate_session(&client, &session_id).await;
                return Err(InvalidDb::PluginUnavailable.into());
            }
        };
        let stdout = plugin.stdout.take().expect("stdout is a pipe");
        let stderr = plugin.stderr.take().expect("stderr is a pipe");
        // Its stderr is read alongside, into a bounded buffer: the plugin says
        // why it could not reach AWS there, and `/dev/null` threw that away.
        // A pipe nobody reads eventually blocks the child, so it is drained
        // whether or not the tunnel opens.
        let said = Arc::new(Mutex::new(LastLines::default()));
        let (drained, stderr_drained) = tokio::sync::watch::channel(false);
        let collecting = tokio::spawn({
            let said = Arc::clone(&said);
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    said.lock().expect("the buffer is not poisoned").push(line);
                }
                let _ = drained.send(true);
            }
        });
        let opened = tokio::time::timeout(
            Duration::from_secs(request.connect_timeout_secs),
            read_local_port(stdout, &mut plugin, &said, stderr_drained),
        )
        .await;
        let (opened, rest) = match opened {
            Ok(Ok((port, rest))) => (Ok(port), Some(rest)),
            Ok(Err(error)) => (Err(error), None),
            Err(elapsed) => (
                Err(DbError::tunnel_unavailable(format!(
                    "the tunnel to {instance_id} did not open within {}s ({elapsed})",
                    request.connect_timeout_secs
                ))),
                None,
            ),
        };
        let output = tokio::spawn(async move {
            let Some(mut rest) = rest else { return };
            while matches!(rest.next_line().await, Ok(Some(_))) {}
        });
        let tunnel = Self {
            local_port: 0,
            instance_id: instance_id.clone(),
            region: request.region.clone(),
            session_id,
            plugin,
            client,
            output,
            errors: collecting,
            plugin_tmp,
        };
        match opened {
            Ok(local_port) => Ok(Self {
                local_port,
                ..tunnel
            }),
            Err(error) => {
                tunnel.close().await;
                Err(error)
            }
        }
    }

    /// End the tunnel: the plugin first, because it is the process that would
    /// outlive this one, then the session, so AWS does not keep it open.
    pub async fn close(mut self) {
        self.output.abort();
        self.errors.abort();
        let _ = self.plugin.kill().await;
        let _ = self.plugin.wait().await;
        // The killed plugin left its socket; its directory goes with it.
        let _ = self.plugin_tmp.close();
        terminate_session(&self.client, &self.session_id).await;
    }
}

/// End the Session Manager session, wherever the call gave up. Nothing depends
/// on the answer: the session either ends here or at its own idle timeout.
async fn terminate_session(client: &Client, session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    let _ = client
        .terminate_session()
        .session_id(session_id)
        .send()
        .await;
}

/// The last few lines a child process wrote, for an error that has to say why
/// it ended. Bounded on both counts: a child can write for as long as it runs.
#[derive(Default)]
struct LastLines(std::collections::VecDeque<String>);

impl LastLines {
    /// Lines kept, and characters kept of each.
    const LINES: usize = 3;
    const CHARS: usize = 120;

    fn push(&mut self, line: String) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if self.0.len() == Self::LINES {
            self.0.pop_front();
        }
        self.0.push_back(line.chars().take(Self::CHARS).collect());
    }

    fn said(&self) -> String {
        self.0.iter().cloned().collect::<Vec<_>>().join("; ")
    }
}

/// What the envelope says about a tunnel. A free function so a test can call
/// the code instead of rebuilding the struct beside it: the test that used to
/// stand here built a `DbTunnelInfo` itself and asserted the values it had
/// just written, so swapping `instance_id` and `region` here left it green.
fn reported_of(instance_id: &str, region: &str) -> DbTunnelInfo {
    DbTunnelInfo {
        kind: "ssm".to_owned(),
        instance_id: instance_id.to_owned(),
        region: region.to_owned(),
    }
}

/// The port the plugin listened on, from the line it prints when it is ready.
type PluginOutput = tokio::io::Lines<BufReader<tokio::process::ChildStdout>>;

/// How long an ended plugin's stderr is waited for. The pipe closes when the
/// plugin exits, unless something it started still holds it, and that must
/// not keep the failure from being reported.
const STDERR_DRAIN: Duration = Duration::from_secs(1);

async fn read_local_port(
    stdout: tokio::process::ChildStdout,
    plugin: &mut Child,
    said: &Mutex<LastLines>,
    mut stderr_drained: tokio::sync::watch::Receiver<bool>,
) -> Result<(u16, PluginOutput), DbError> {
    let mut lines = BufReader::new(stdout).lines();
    let mut printed = LastLines::default();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(port) = parse_opened_port(&line) {
                    // The reader goes back to the caller: dropping it here
                    // would close the plugin's stdout and end the plugin.
                    return Ok((port, lines));
                }
                printed.push(line);
            }
            // The plugin ended without opening anything. What it said about
            // why is the only thing that tells an expired token from a stream
            // nobody could reach, and the hint for this failure points at the
            // bastion, which `find_bastion` has already found online.
            Ok(None) | Err(_) => {
                let status = plugin.wait().await.ok();
                // Exiting is not having been read: the plugin's last line can
                // still be in the pipe, and it is the one that says why.
                let _ =
                    tokio::time::timeout(STDERR_DRAIN, stderr_drained.wait_for(|drained| *drained))
                        .await;
                let status = status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "no exit status".into());
                let mut said = said.lock().expect("the buffer is not poisoned").said();
                if said.is_empty() {
                    said = printed.said();
                }
                return Err(DbError::tunnel_unavailable(if said.is_empty() {
                    format!("the session manager plugin ended before the tunnel opened ({status})")
                } else {
                    format!(
                        "the session manager plugin ended before the tunnel opened ({status}): {said}"
                    )
                }));
            }
        }
    }
}

/// `Port 54321 opened for sessionId ...`, which is how the plugin says which
/// port it took.
fn parse_opened_port(line: &str) -> Option<u16> {
    let rest = line.trim().strip_prefix("Port ")?;
    let (port, rest) = rest.split_once(' ')?;
    rest.starts_with("opened").then(|| port.parse().ok())?
}

/// The bastion, and only if Session Manager can reach it right now. A running
/// EC2 instance whose agent stopped answering accepts no session.
async fn find_bastion(client: &Client, instance: &InstanceRef) -> Result<String, DbError> {
    let (key, value) = match instance {
        // A tag filter cannot be combined with any other filter, so the online
        // check is made here instead of by the service.
        InstanceRef::Name(name) => ("tag:Name", name.clone()),
        InstanceRef::Id(id) => ("InstanceIds", id.clone()),
    };
    let described = client
        .describe_instance_information()
        .filters(
            aws_sdk_ssm::types::InstanceInformationStringFilter::builder()
                .key(key)
                .values(value)
                .build()
                .map_err(|_| DbError::tunnel_unavailable("invalid bastion filter"))?,
        )
        .send()
        .await
        .map_err(|error| aws_failure("DescribeInstanceInformation", error))?;
    let found = described.instance_information_list();
    let online: Vec<&InstanceInformation> = found
        .iter()
        .filter(|instance| {
            instance
                .ping_status()
                .is_some_and(|status| status.as_str() == "Online")
        })
        .collect();
    match (found.len(), online.as_slice()) {
        (0, _) => Err(DbError::tunnel_unavailable(format!(
            "no Session Manager node matches {}",
            instance.as_str()
        ))),
        (_, [one]) => Ok(one.instance_id().unwrap_or_default().to_owned()),
        (_, []) => {
            let state = found
                .iter()
                .map(|instance| {
                    format!(
                        "{} is {} (last ping {})",
                        instance.instance_id().unwrap_or("?"),
                        instance.ping_status().map_or("unknown", |s| s.as_str()),
                        instance
                            .last_ping_date_time()
                            .map(|time| time.to_string())
                            .unwrap_or_else(|| "never".into())
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            Err(DbError::tunnel_unavailable(format!(
                "the bastion is not online: {state}"
            )))
        }
        (_, many) => Err(DbError::tunnel_unavailable(format!(
            "{} Session Manager nodes match {}; name one by instance_id",
            many.len(),
            instance.as_str()
        ))),
    }
}

/// Refuse a plugin that would take the session token on the command line.
pub async fn require_usable_plugin() -> Result<PluginReady, DbError> {
    let output = Command::new(PLUGIN)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| DbError::from(InvalidDb::PluginUnavailable))?;
    let found = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if parse_version(&found).is_none_or(|version| version < REQUIRED_PLUGIN) {
        // The version comes out of a child process: whatever a wrapper on the
        // PATH prints, the error line stays one bounded line.
        return Err(InvalidDb::PluginTooOld {
            found: found.as_str().into(),
        }
        .into());
    }
    Ok(PluginReady(()))
}

fn parse_version(text: &str) -> Option<(u32, u32, u32, u32)> {
    let mut parts = text.trim().split('.').map(str::parse::<u32>);
    let mut next = || parts.next()?.ok();
    Some((next()?, next()?, next()?, next().unwrap_or(0)))
}

fn endpoint(region: &str) -> String {
    #[cfg(feature = "test-fakes")]
    if let Ok(endpoint) = std::env::var("KURAMA_TEST_SSM_ENDPOINT") {
        return endpoint;
    }
    format!("https://ssm.{region}.amazonaws.com")
}

/// AWS refused or could not answer. Either way no statement was sent.
///
/// A refusal names itself; anything else has to be read out of the error and
/// its causes, because "no answer" hides the one thing the reader needs.
fn aws_failure<E, R>(operation: &str, error: aws_sdk_ssm::error::SdkError<E, R>) -> DbError
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug,
{
    let named = match (error.code(), error.message()) {
        (Some(code), Some(message)) => Some(format!("{code}: {message}")),
        (Some(code), None) => Some(code.to_owned()),
        (None, Some(message)) => Some(message.to_owned()),
        (None, None) => None,
    };
    let detail = named.unwrap_or_else(|| causes(&error));
    // `Detail` is what makes it one bounded line.
    DbError::tunnel_unavailable(format!("SSM {operation}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_tunnel_reads_the_port_out_of_the_line_the_plugin_prints() {
        assert_eq!(
            parse_opened_port("Port 54321 opened for sessionId abc-123"),
            Some(54321)
        );
        assert_eq!(parse_opened_port("  Port 1 opened  "), Some(1));
        for other in [
            "Waiting for connections...",
            "Port opened",
            "Port abc opened for sessionId x",
            "Starting session with SessionId: abc",
            "",
        ] {
            assert_eq!(parse_opened_port(other), None, "{other}");
        }
    }

    #[test]
    fn db_tunnel_requires_the_plugin_that_takes_the_token_out_of_argv() {
        // 1.2.497.0 is the last release without the environment variable.
        assert!(parse_version("1.2.497.0").unwrap() < REQUIRED_PLUGIN);
        assert!(parse_version("1.2.536.0").unwrap() >= REQUIRED_PLUGIN);
        assert!(parse_version("1.2.835.0").unwrap() >= REQUIRED_PLUGIN);
        assert!(parse_version("1.3.0.0").unwrap() >= REQUIRED_PLUGIN);
        assert!(parse_version("1.2.536").unwrap() >= REQUIRED_PLUGIN);
        for unusable in ["", "unknown", "1", "1.x.3.4"] {
            assert_eq!(parse_version(unusable), None, "{unusable}");
        }
    }

    #[test]
    fn db_tunnel_reports_the_bastion_and_never_the_local_port() {
        // The two arguments are given different values on purpose: equal
        // fixtures let a mix-up pass.
        let reported = reported_of("i-0123456789abcdef0", "ap-northeast-1");
        assert_eq!(reported.instance_id, "i-0123456789abcdef0");
        assert_eq!(reported.region, "ap-northeast-1");
        assert_eq!(reported.kind, "ssm");
        let value = serde_json::to_value(&reported).unwrap();
        assert_eq!(value["instance_id"], "i-0123456789abcdef0");
        assert_eq!(
            value.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["kind", "instance_id", "region"],
            "a local port is temporary and names nothing after the call"
        );
    }
}
