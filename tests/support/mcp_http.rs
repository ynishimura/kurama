//! `kurama mcp --listen` driven over HTTP: start the server, wait for its `listening on` line, send raw HTTP/1.1 requests, stop it with SIGTERM.
//!
//! The requests are written byte for byte over a `TcpStream`, so a scenario
//! can send what no client library would: no `Content-Type`, a declared
//! length past the limit, a body that is not JSON.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::{FAKE_CLIENT_SECRET, OnePassword, Run, Sandbox, Scenario, Verification, drain};

/// How long the server may take to print `listening on`.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// What `{mcp_token}` in a header stands for on the real layer: the token
/// the `[mcp] token` reference of the layer's configuration holds.
pub const REAL_TOKEN_VARIABLE: &str = "KURAMA_REAL_MCP_TOKEN";

/// One request, as it is written on the wire. `{mcp_token}` in a header
/// value is the token the server accepts: the fake `op`'s secret, or on a
/// run against the real services the value of [`REAL_TOKEN_VARIABLE`].
#[derive(Clone)]
pub struct HttpRequest {
    pub method: &'static str,
    pub path: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Body,
}

/// What follows the headers.
#[derive(Clone)]
pub enum Body {
    /// `Content-Length` and these bytes.
    Bytes(Vec<u8>),
    /// A `Content-Length` header with this value and nothing after it: the
    /// server has to answer from the headers alone.
    DeclaredOnly(usize),
    /// `Transfer-Encoding: chunked` carrying these bytes in one chunk, so
    /// the server learns the length only by reading.
    Chunked(Vec<u8>),
}

impl HttpRequest {
    /// A JSON-RPC POST to `/mcp` with `authorization` and JSON content.
    pub fn rpc(authorization: &str, body: &str) -> Self {
        Self {
            method: "POST",
            path: "/mcp",
            headers: vec![
                ("Authorization", authorization.to_owned()),
                ("Content-Type", "application/json".to_owned()),
            ],
            body: Body::Bytes(body.as_bytes().to_vec()),
        }
    }

    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }
}

/// What the server answered.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HttpResponse {
    pub status: u16,
    /// Header names in lowercase.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

impl Sandbox {
    /// Start `args`, send `requests` one after another once it listens, then
    /// stop it with SIGTERM. A server that exits before it listens is the
    /// run, with no response. The run's stdout and stderr are the server's.
    pub fn run_cli_serving(
        &mut self,
        args: &[&str],
        requests: Vec<HttpRequest>,
    ) -> (Run, Vec<HttpResponse>) {
        let mut child = self
            .create_cli_command(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run kurama");
        let stdout = drain(child.stdout.take().unwrap());
        let (lines, received) = mpsc::channel();
        let stderr = child.stderr.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut all = String::new();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap_or_default();
                all.push_str(&line);
                all.push('\n');
                let _ = lines.send(line);
            }
            all
        });
        let started = Instant::now();
        let port = loop {
            match received.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    if let Some(address) = line.strip_prefix("listening on ") {
                        break address
                            .rsplit(':')
                            .next()
                            .and_then(|port| port.parse().ok());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break None,
                Err(mpsc::RecvTimeoutError::Timeout) if started.elapsed() > START_TIMEOUT => {
                    break None;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        };
        let token = if self.real_aws {
            std::env::var(REAL_TOKEN_VARIABLE).unwrap_or_else(|_| {
                panic!("{REAL_TOKEN_VARIABLE} holds the token the real layer sends")
            })
        } else {
            FAKE_CLIENT_SECRET.to_owned()
        };
        let responses: Vec<HttpResponse> = match port {
            Some(port) => requests
                .into_iter()
                .map(|request| send(port, &request, &token))
                .collect(),
            None => Vec::new(),
        };
        let timed_out = port.is_none() && child.try_wait().unwrap().is_none();
        if child.try_wait().unwrap().is_none() {
            let _ = Command::new("/bin/kill")
                .args(["-TERM", &child.id().to_string()])
                .status();
        }
        let status = child.wait().expect("wait for kurama");
        let command = std::iter::once("kurama")
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect();
        let mut run = self.record_run(
            command,
            status.code(),
            timed_out,
            stdout.join().unwrap(),
            reader.join().unwrap(),
        );
        run.http_responses = responses.clone();
        (run, responses)
    }
}

/// `Authorization` with the token the fake `op` prints.
pub fn bearer() -> String {
    format!("Bearer {FAKE_CLIENT_SECRET}")
}

/// The status of each response, in order.
pub fn statuses(responses: &[HttpResponse]) -> Vec<u16> {
    responses.iter().map(|response| response.status).collect()
}

/// `kurama mcp --listen` with `config` and `env`, sent `requests`, then
/// `audit --json` as run 1 when `audit` says so. Run 0 is checked for what
/// every server run keeps: an answer to each request, nothing on stdout, one
/// token read, and stderr holding only the listening line and request lines
/// unless `env` raised the log level.
pub fn serve(
    id: &'static str,
    config: &str,
    env: &[(&'static str, &'static str)],
    requests: Vec<HttpRequest>,
    audit: bool,
) -> (Verification, Vec<HttpResponse>) {
    let mut scenario = Scenario::new(id, "mcp", &[])
        .onepassword(OnePassword::Enabled)
        .with_extra_config(config);
    for (name, value) in env {
        scenario = scenario.with_env(name, value);
    }
    let mut sandbox = Sandbox::create(&scenario);
    let (run, responses) = sandbox.run_cli_serving(&["mcp", "--listen"], requests);
    let mut runs: Vec<Run> = vec![run];
    if audit {
        runs.push(sandbox.run_cli(&["audit", "--json"]));
    }
    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "server", |v| {
        v.check(
            "the server answered every request",
            !responses.iter().any(|response| response.status == 0),
            format!("{responses:?}"),
        )
        .expect_stdout_empty()
        .expect_op_calls(1);
        if env.is_empty() {
            v.expect_stderr_lines_start_with(&[
                "listening on 127.0.0.1:",
                "POST ",
                "GET ",
                "DELETE ",
            ]);
        }
        v
    });
    (v, responses)
}

/// Write one request on its own connection and read the whole answer.
fn send(port: u16, request: &HttpRequest, token: &str) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to kurama");
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n",
        request.method, request.path
    );
    for (name, value) in &request.headers {
        let value = value.replace("{mcp_token}", token);
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let body = match &request.body {
        Body::Bytes(bytes) => {
            head.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
            bytes.clone()
        }
        Body::DeclaredOnly(length) => {
            head.push_str(&format!("Content-Length: {length}\r\n"));
            Vec::new()
        }
        Body::Chunked(bytes) => {
            head.push_str("Transfer-Encoding: chunked\r\n");
            let mut chunked = format!("{:x}\r\n", bytes.len()).into_bytes();
            chunked.extend_from_slice(bytes);
            chunked.extend_from_slice(b"\r\n0\r\n\r\n");
            chunked
        }
    };
    head.push_str("\r\n");
    // The body is written beside the read: a server that answers before it
    // has read everything must not leave this side blocked on a full buffer.
    let mut writer = stream.try_clone().unwrap();
    let written = std::thread::spawn(move || {
        let _ = writer.write_all(head.as_bytes());
        let _ = writer.write_all(&body);
    });
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    // A server that closes with unread bytes resets the connection; what it
    // sent before that is the answer.
    while let Ok(read) = stream.read(&mut buffer) {
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let _ = written.join();
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> HttpResponse {
    let text = String::from_utf8_lossy(bytes);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    HttpResponse {
        status,
        headers,
        body: body.to_owned(),
    }
}

/// What one response has to be.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HttpResponseExpect {
    pub status: u16,
    /// Texts the body contains.
    pub body_contains: Vec<String>,
    /// The body is empty.
    pub body_empty: bool,
    /// Headers, by lowercase name, and their exact values.
    pub headers: std::collections::BTreeMap<String, String>,
}

impl Verification {
    /// The focused run, a server, answered exactly `expected.len()`
    /// requests, each as `expected` says in order.
    pub fn expect_http_responses(&mut self, expected: &[HttpResponseExpect]) -> &mut Self {
        let observed = self.observed.runs[self.focused_index()]
            .http_responses
            .clone();
        self.check(
            &format!("the server answered {} request(s)", expected.len()),
            observed.len() == expected.len(),
            format!("observed {:?}", statuses(&observed)),
        );
        for (index, (expect, response)) in expected.iter().zip(&observed).enumerate() {
            let ok = response.status == expect.status
                && expect
                    .body_contains
                    .iter()
                    .all(|text| response.body.contains(text))
                && (!expect.body_empty || response.body.is_empty())
                && expect
                    .headers
                    .iter()
                    .all(|(name, value)| response.header(name) == Some(value.as_str()));
            self.check(
                &format!("response {index} is {expect:?}"),
                ok,
                format!("observed {response:?}"),
            );
        }
        self
    }
}
