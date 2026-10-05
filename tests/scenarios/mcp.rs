//! Runtime verification scenarios of the `mcp` feature over HTTP
//! (`kurama mcp --listen`): the server is started, HTTP requests are written
//! to the port its `listening on` line names, and it is stopped with
//! SIGTERM. `tests/scenarios/main.rs` holds the naming rule and what each
//! scenario writes; the stdio scenarios are TOML cases under
//! `tests/cases/mcp/`.

use crate::support::mcp_http::{Body, HttpRequest, bearer, serve, statuses};
use crate::support::{FAKE_CLIENT_SECRET, JsonExpect, OnePassword, Sandbox, Scenario};
use serde_json::json;

/// The server a cloud client reaches: loopback, a token from 1Password (the
/// fake prints `FAKE_CLIENT_SECRET`), three tools.
const SERVER_CONFIG: &str = r#"
[mcp]
listen = "127.0.0.1:0"
token = "op://Agent/kurama-mcp-token/credential"
tools = ["list_operations", "describe_operation", "call_api"]

[api.pets]
base_url = "{server}/api"
"#;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"harness","version":"0"}}}"#;
const TOOLS_LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
const PING: &str = r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#;
const CALL_GET: &str = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"call_api","arguments":{"api":"pets","target":"/pet/findByStatus?status=available"}}}"#;
const CALL_POST: &str = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"call_api","arguments":{"api":"pets","target":"/pet","body":{"name":"x"}}}}"#;

#[test]
fn mcp_http_answers_tools_list_with_the_token() {
    let (mut v, responses) = serve(
        "mcp_http_answers_tools_list_with_the_token",
        SERVER_CONFIG,
        &[],
        vec![
            HttpRequest::rpc(&bearer(), INITIALIZE),
            HttpRequest::rpc(&bearer(), TOOLS_LIST).header("MCP-Protocol-Version", "2025-06-18"),
        ],
        false,
    );
    let names: Vec<String> = responses[1].json()["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    v.check(
        "both requests are answered 200 with JSON",
        statuses(&responses) == [200, 200]
            && responses
                .iter()
                .all(|response| response.header("content-type") == Some("application/json")),
        format!("{responses:?}"),
    )
    .check(
        "initialize answers the version asked for and kurama's name",
        responses[0].json()["result"]["protocolVersion"] == "2025-06-18"
            && responses[0].json()["result"]["serverInfo"]["name"] == "kurama",
        responses[0].body.clone(),
    )
    .check(
        "tools/list lists the three tools [mcp] tools names, in order",
        names == ["list_operations", "describe_operation", "call_api"],
        format!("{names:?}"),
    )
    .keyed_run(0, "server", |v| {
        v.expect_stderr_lines_containing("POST /mcp 200", 2)
            .expect_api_call_count(0)
    });
    v.finish();
}

#[test]
fn mcp_http_accepts_the_token_with_and_without_the_bearer_prefix() {
    let (mut v, responses) = serve(
        "mcp_http_accepts_the_token_with_and_without_the_bearer_prefix",
        SERVER_CONFIG,
        &[],
        vec![
            HttpRequest::rpc(&bearer(), PING),
            HttpRequest::rpc(FAKE_CLIENT_SECRET, PING),
        ],
        false,
    );
    v.check(
        "`Bearer <token>` and `<token>` both get the ping's empty result",
        statuses(&responses) == [200, 200]
            && responses
                .iter()
                .all(|response| response.json()["result"] == serde_json::json!({})),
        format!("{responses:?}"),
    );
    v.finish();
}

#[test]
fn mcp_http_runs_call_api_as_an_agent_and_records_the_audit_entry() {
    let (mut v, responses) = serve(
        "mcp_http_runs_call_api_as_an_agent_and_records_the_audit_entry",
        SERVER_CONFIG,
        &[],
        vec![
            HttpRequest::rpc(&bearer(), CALL_GET),
            HttpRequest::rpc(&bearer(), CALL_POST),
        ],
        true,
    );
    let (get, post) = (responses[0].json(), responses[1].json());
    v.check(
        "the GET is answered with the {status, headers, body} envelope",
        responses[0].status == 200
            && get["id"] == 4
            && get["result"]["isError"] == false
            && get["result"]["structuredContent"]["status"] == 200,
        responses[0].body.clone(),
    )
    .check(
        "the POST is the AGENT_POLICY_DENIED error document, as on the command line",
        responses[1].status == 200
            && post["result"]["isError"] == true
            && post["result"]["structuredContent"]["error"]["code"] == "AGENT_POLICY_DENIED",
        responses[1].body.clone(),
    )
    .keyed_run(0, "server", |v| v.expect_api_call_count(1));
    let paths: Vec<String> = v.observed.runs[0]
        .api_calls
        .iter()
        .map(|call| format!("{} {}", call.method, call.path))
        .collect();
    v.check(
        "only the GET reached the API",
        paths == ["GET /api/pet/findByStatus?status=available"],
        format!("{paths:?}"),
    )
    .keyed_run(1, "audit", |v| {
        v.expect_exit_code(0)
            .expect_json_at("stdout", "/entries", &JsonExpect::Length(2))
            .expect_json_at(
                "stdout",
                "/entries/0/agent",
                &JsonExpect::Equals(json!(true)),
            )
            .expect_json_at(
                "stdout",
                "/entries/0/status",
                &JsonExpect::Equals(json!(200)),
            )
            .expect_json_at(
                "stdout",
                "/entries/1/method",
                &JsonExpect::Equals(json!("POST")),
            )
            .expect_json_at(
                "stdout",
                "/entries/1/exit_code",
                &JsonExpect::Equals(json!(3)),
            )
    })
    .expect_files_written(&["home/.local/state/kurama/audit.jsonl"]);
    v.finish();
}

#[test]
fn mcp_http_refuses_a_missing_or_wrong_token_and_runs_nothing() {
    let without = HttpRequest {
        method: "POST",
        path: "/mcp",
        headers: vec![("Content-Type", "application/json".to_owned())],
        body: Body::Bytes(CALL_GET.as_bytes().to_vec()),
    };
    let (mut v, responses) = serve(
        "mcp_http_refuses_a_missing_or_wrong_token_and_runs_nothing",
        SERVER_CONFIG,
        &[],
        vec![
            without,
            HttpRequest::rpc("Bearer not-the-token", CALL_GET),
            // The same length, one character off.
            HttpRequest::rpc(
                &format!(
                    "Bearer {}T",
                    &FAKE_CLIENT_SECRET[..FAKE_CLIENT_SECRET.len() - 1]
                ),
                CALL_GET,
            ),
            HttpRequest::rpc(&format!("Bearer {FAKE_CLIENT_SECRET}x"), CALL_GET),
            HttpRequest::rpc("Basic ZmFrZS1jbGllbnQtc2VjcmV0", CALL_GET),
        ],
        true,
    );
    v.check(
        "each is 401 with `WWW-Authenticate: Bearer` and no body",
        responses.iter().all(|response| {
            response.status == 401
                && response.header("www-authenticate") == Some("Bearer")
                && response.body.is_empty()
        }),
        format!("{responses:?}"),
    )
    .keyed_run(0, "server", |v| {
        v.expect_api_call_count(0)
            .expect_stderr_lines_containing("POST /mcp 401", 5)
            .expect_stderr_excludes("not-the-token")
    })
    .keyed_run(1, "audit", |v| {
        v.expect_json_at("stdout", "/entries", &JsonExpect::Length(0))
    })
    .expect_no_files_written();
    v.finish();
}

#[test]
fn mcp_http_answers_401_before_revealing_the_path_or_method() {
    let bare = |method, path| HttpRequest {
        method,
        path,
        headers: vec![("Origin", "https://evil.example".to_owned())],
        body: Body::Bytes(Vec::new()),
    };
    let with_token = |method, path| HttpRequest {
        method,
        path,
        headers: vec![("Authorization", bearer())],
        body: Body::Bytes(Vec::new()),
    };
    let (mut v, responses) = serve(
        "mcp_http_answers_401_before_revealing_the_path_or_method",
        SERVER_CONFIG,
        &[],
        vec![
            bare("GET", "/"),
            bare("DELETE", "/mcp"),
            bare("POST", "/admin"),
            bare("GET", "/mcp"),
            with_token("GET", "/other"),
            with_token("GET", "/mcp"),
            with_token("DELETE", "/mcp"),
        ],
        false,
    );
    v.check(
        "without the token every path and method is 401; with it, 404 and 405 say what is wrong",
        statuses(&responses) == [401, 401, 401, 401, 404, 405, 405],
        format!("{:?}", statuses(&responses)),
    )
    .keyed_run(0, "server", |v| v.expect_api_call_count(0));
    v.finish();
}

#[test]
fn mcp_http_refuses_a_request_with_an_origin() {
    let (mut v, responses) = serve(
        "mcp_http_refuses_a_request_with_an_origin",
        SERVER_CONFIG,
        &[],
        vec![
            HttpRequest::rpc(&bearer(), CALL_GET).header("Origin", "http://127.0.0.1:8807"),
            HttpRequest::rpc(&bearer(), PING).header("Origin", "null"),
        ],
        false,
    );
    v.check(
        "a request a browser sent is 403 with nothing run, whatever the origin",
        statuses(&responses) == [403, 403],
        format!("{responses:?}"),
    )
    .keyed_run(0, "server", |v| v.expect_api_call_count(0));
    v.finish();
}

#[test]
fn mcp_http_answers_a_notification_with_202() {
    let (mut v, responses) = serve(
        "mcp_http_answers_a_notification_with_202",
        SERVER_CONFIG,
        &[],
        vec![
            HttpRequest::rpc(
                &bearer(),
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            ),
            HttpRequest::rpc(&bearer(), r#"{"jsonrpc":"2.0","id":9,"result":{}}"#),
        ],
        false,
    );
    v.check(
        "a notification and a response are 202 with no body",
        statuses(&responses) == [202, 202]
            && responses.iter().all(|response| response.body.is_empty()),
        format!("{responses:?}"),
    );
    v.finish();
}

#[test]
fn mcp_http_refuses_a_body_that_is_not_one_json_rpc_message() {
    let (mut v, responses) = serve(
        "mcp_http_refuses_a_body_that_is_not_one_json_rpc_message",
        SERVER_CONFIG,
        &[],
        vec![
            HttpRequest::rpc(&bearer(), "{not json"),
            HttpRequest::rpc(&bearer(), &format!("[{PING}]")),
            HttpRequest::rpc(&bearer(), "{}"),
            HttpRequest {
                method: "POST",
                path: "/mcp",
                headers: vec![
                    ("Authorization", bearer()),
                    ("Content-Type", "text/plain".to_owned()),
                ],
                body: Body::Bytes(PING.as_bytes().to_vec()),
            },
        ],
        false,
    );
    let codes: Vec<serde_json::Value> = responses
        .iter()
        .map(|response| response.json()["error"]["code"].clone())
        .collect();
    v.check(
        "not JSON, a batch and an object with neither method nor id are 400; text/plain is 415",
        statuses(&responses) == [400, 400, 400, 415],
        format!("{responses:?}"),
    )
    .check(
        "each 400 says why as a JSON-RPC error",
        codes[..3] == [json!(-32700), json!(-32600), json!(-32600)],
        format!("{codes:?}"),
    );
    v.finish();
}

#[test]
fn mcp_http_refuses_an_unknown_protocol_version() {
    let (mut v, responses) = serve(
        "mcp_http_refuses_an_unknown_protocol_version",
        SERVER_CONFIG,
        &[],
        vec![
            HttpRequest::rpc(&bearer(), PING).header("MCP-Protocol-Version", "1999-01-01"),
            HttpRequest::rpc(&bearer(), PING).header("MCP-Protocol-Version", "2025-03-26"),
        ],
        false,
    );
    v.check(
        "a version this server does not speak is 400 naming it; one it speaks is served",
        statuses(&responses) == [400, 200]
            && responses[0].json()["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("1999-01-01")),
        format!("{responses:?}"),
    );
    v.finish();
}

#[test]
fn mcp_http_refuses_a_body_over_the_limit() {
    const LIMIT: usize = 1024 * 1024;
    let request = |body| HttpRequest {
        method: "POST",
        path: "/mcp",
        headers: vec![
            ("Authorization", bearer()),
            ("Content-Type", "application/json".to_owned()),
        ],
        body,
    };
    let (mut v, responses) = serve(
        "mcp_http_refuses_a_body_over_the_limit",
        SERVER_CONFIG,
        &[],
        vec![
            request(Body::DeclaredOnly(LIMIT + 1)),
            request(Body::Chunked(vec![b' '; LIMIT + 1])),
            HttpRequest::rpc(&bearer(), PING),
        ],
        false,
    );
    v.check(
        "a declared length past 1 MiB and a chunked body that grows past it are 413; the server goes on",
        statuses(&responses) == [413, 413, 200],
        format!("{:?}", statuses(&responses)),
    );
    v.finish();
}

#[test]
fn mcp_http_reports_a_port_in_use_as_listen_failed() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let scenario = Scenario::new(
        "mcp_http_reports_a_port_in_use_as_listen_failed",
        "mcp",
        &[],
    )
    .onepassword(OnePassword::Enabled)
    .with_extra_config(&SERVER_CONFIG.replace("127.0.0.1:0", &format!("127.0.0.1:{port}")));
    let mut sandbox = Sandbox::create(&scenario);
    let run = sandbox.run_cli(&["mcp", "--listen"]);
    drop(held);
    let mut v = sandbox.finish(scenario.id, scenario.feature, vec![run], vec![]);
    v.expect_error("MCP_LISTEN_FAILED", 1)
        .expect_stderr_contains(&format!("cannot listen on 127.0.0.1:{port}"))
        .expect_stderr_contains("hint: choose another [mcp] listen port")
        .expect_stdout_empty()
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}

#[test]
fn mcp_http_token_never_appears_in_output_or_logs() {
    let (mut v, responses) = serve(
        "mcp_http_token_never_appears_in_output_or_logs",
        SERVER_CONFIG,
        &[("RUST_LOG", "kurama=trace")],
        vec![
            HttpRequest::rpc(&bearer(), CALL_GET),
            HttpRequest::rpc("Bearer guessed-token-value", PING),
        ],
        false,
    );
    v.check(
        "the right token is served and the wrong one refused",
        statuses(&responses) == [200, 401],
        format!("{:?}", statuses(&responses)),
    )
    .check(
        "no response carries the token",
        !responses
            .iter()
            .any(|response| response.body.contains(FAKE_CLIENT_SECRET)),
        format!("{responses:?}"),
    )
    .keyed_run(0, "server", |v| {
        v.expect_stderr_excludes(FAKE_CLIENT_SECRET)
            .expect_stderr_excludes("guessed-token-value")
            .expect_stderr_excludes("authorization")
            .expect_stderr_excludes("Authorization")
    });
    v.finish();
}
