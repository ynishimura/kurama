//! `kurama mcp`, pure: the JSON-RPC messages it answers, the tools it lists, the kurama command line each tool call runs, and the result that call becomes.
//!
//! A tool call is never a request built here: it is one of kurama's own
//! JSON commands (`api --json`, `data --request - --json`, ...), so the
//! `[agent]` policy, the audit log and the error document are the ones the
//! command line has. Every value an agent passes goes after `--` or glued to
//! its option with `=`, so a value that starts with `-` can never become an
//! option such as `--confirm`.

use serde_json::{Map, Value, json};

/// The protocol versions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The kurama command line one tool call runs, and what it reads on stdin.
#[derive(Debug, PartialEq)]
pub struct ToolRun {
    pub args: Vec<String>,
    pub stdin: Option<String>,
}

/// What one incoming line asks for.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// A notification, or a response: nothing is answered.
    Nothing,
    /// The answer is known without running anything.
    Answer(Value),
    /// Run a tool, then answer `id` with its result.
    Call { id: Value, run: ToolRun },
}

/// Every tool this server can offer, in the order `tools/list` lists them;
/// `[mcp] tools` names a subset.
pub const TOOL_NAMES: [&str; 7] = [
    "ready",
    "list_apis",
    "list_operations",
    "describe_operation",
    "call_api",
    "query_data",
    "query_db",
];

/// Read one line of the stdio transport. `exposed` names the tools offered.
pub fn read_message(line: &str, exposed: &[String]) -> Incoming {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Incoming::Answer(error(Value::Null, -32700, "the line is not JSON"));
    };
    read_value(&message, exposed)
}

/// Read one JSON-RPC message, whichever transport carried it.
pub fn read_value(message: &Value, exposed: &[String]) -> Incoming {
    let method = message.get("method").and_then(Value::as_str);
    let (Some(id), Some(method)) = (message.get("id").cloned(), method) else {
        return match method {
            Some(_) => Incoming::Nothing,
            None if message.get("id").is_some() => Incoming::Nothing,
            None => Incoming::Answer(error(Value::Null, -32600, "not a JSON-RPC request")),
        };
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => Incoming::Answer(result(id, initialize(&params))),
        "ping" => Incoming::Answer(result(id, json!({}))),
        "tools/list" => Incoming::Answer(result(id, json!({ "tools": tools(exposed) }))),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let offered = exposed.iter().any(|tool| tool == name);
            match tool_run(name, &arguments).and_then(|run| {
                offered
                    .then_some(run)
                    .ok_or_else(|| format!("no tool {name}"))
            }) {
                Ok(run) => Incoming::Call { id, run },
                Err(message) => Incoming::Answer(result(id, refused(&message))),
            }
        }
        _ => Incoming::Answer(error(id, -32601, &format!("no method {method}"))),
    }
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS
        .into_iter()
        .find(|version| Some(*version) == asked)
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "kurama", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Every call runs as an agent's (KURAMA_AGENT): the [agent] policy \
                         applies and the audit log records it. A failure is kurama's JSON \
                         error document; when a person has to act, its next_actions say what \
                         they run.",
    })
}

pub fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool call refused before anything ran.
fn refused(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// The result of a tool whose command ran: its stdout when it succeeded,
/// its JSON error document (the last stderr line that is one) when it did not.
pub fn tool_result(success: bool, stdout: &str, stderr: &str) -> Value {
    let text = if success {
        stdout.trim().to_owned()
    } else {
        stderr
            .lines()
            .rev()
            .find(|line| json_object(line).is_some())
            .unwrap_or(stderr.trim())
            .to_owned()
    };
    let mut content = Map::new();
    content.insert("content".into(), json!([{ "type": "text", "text": text }]));
    if let Some(document) = json_object(&text) {
        content.insert("structuredContent".into(), document);
    }
    content.insert("isError".into(), Value::Bool(!success));
    Value::Object(content)
}

/// A command that could not be run or did not end in time.
pub fn tool_failure(message: &str) -> Value {
    refused(message)
}

fn json_object(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text.trim())
        .ok()
        .filter(Value::is_object)
}

/// The tools `tools/list` answers with: those `exposed` names.
pub fn tools(exposed: &[String]) -> Value {
    let Value::Array(all) = every_tool() else {
        unreachable!("the tools are an array")
    };
    all.into_iter()
        .filter(|tool| exposed.iter().any(|name| tool["name"] == name.as_str()))
        .collect()
}

fn every_tool() -> Value {
    let name = |what: &str| json!({ "type": "string", "description": what });
    let object = |properties: Value, required: &[&str]| json!({ "type": "object", "properties": properties, "required": required });
    json!([
        {
            "name": "ready",
            "description": "Whether each AWS profile, [auth.*], [api.*] and [db.*] can be used \
                            right now, and what a person has to run when not (kurama agent ready --json).",
            "inputSchema": object(json!({}), &[]),
        },
        {
            "name": "list_apis",
            "description": "The configured [api.*] profiles: name, base_url, description, \
                            credential state (kurama status --only api --json).",
            "inputSchema": object(json!({}), &[]),
        },
        {
            "name": "list_operations",
            "description": "The operations of an API's OpenAPI description, those matching \
                            query when given (kurama api API --ops --json).",
            "inputSchema": object(json!({
                "api": name("[api.*] profile"),
                "query": name("words the operationId, path or summary contains"),
            }), &["api"]),
        },
        {
            "name": "describe_operation",
            "description": "The machine-readable contract of one operation: parameters, body \
                            schema, limitations (kurama api API --schema OPERATION).",
            "inputSchema": object(json!({
                "api": name("[api.*] profile"),
                "operation": name("operationId, or METHOD /path/{param}"),
            }), &["api", "operation"]),
        },
        {
            "name": "call_api",
            "description": "Send one request to an [api.*] profile with its credential and \
                            answer the {status, headers, body} envelope (kurama api --json). \
                            The [agent] policy decides which methods and paths are sent.",
            "inputSchema": object(json!({
                "api": name("[api.*] profile"),
                "target": name("operationId, /path, `METHOD /path`, or a URL on the API's origin"),
                "method": name("HTTP method; GET by default, POST with a body"),
                "params": {
                    "type": "object",
                    "description": "operation parameters, name to value (-P name=value)",
                    "additionalProperties": { "type": ["string", "number", "boolean"] },
                },
                "body": { "description": "request body: a JSON value, or a string sent as it is" },
                "shape": { "type": "boolean", "description": "answer the type of each value instead of the value" },
                "sample": { "type": "integer", "minimum": 0, "description": "cut every array of the body to its first N elements" },
            }), &["api", "target"]),
        },
        {
            "name": "query_data",
            "description": "Read local or S3 data with DuckDB: a strict request as in \
                            kurama agent --kind data --json (kurama data --request - --json). \
                            Read only: export is refused.",
            "inputSchema": object(json!({
                "workspace": name("[data.*] workspace; absent for args.from"),
                "request": { "type": "object", "description": "{\"operation\": ..., \"args\": {...}}" },
            }), &["request"]),
        },
        {
            "name": "query_db",
            "description": "Read a database: a strict request as in kurama agent --kind db --json \
                            (kurama db DATABASE --request - --json). Read only: execute is refused.",
            "inputSchema": object(json!({
                "database": name("[db.*] section or SQLite file"),
                "request": { "type": "object", "description": "{\"operation\": ..., \"args\": {...}}" },
            }), &["database", "request"]),
        },
    ])
}

/// The kurama command line of a tool call, or why it is refused.
pub fn tool_run(name: &str, arguments: &Value) -> Result<ToolRun, String> {
    let text = |key: &str| -> Result<String, String> {
        match arguments.get(key) {
            Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
            _ => Err(format!("{name}: `{key}` is a required string")),
        }
    };
    let optional = |key: &str| arguments.get(key).and_then(Value::as_str);
    let run = |args: Vec<String>| ToolRun { args, stdin: None };
    let owned = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    Ok(match name {
        "ready" => run(owned(&["agent", "ready", "--json"])),
        "list_apis" => run(owned(&["status", "--only", "api", "--json"])),
        "list_operations" => {
            let ops = match optional("query") {
                Some(query) => format!("--ops={query}"),
                None => "--ops".into(),
            };
            run(vec![
                "api".into(),
                ops,
                "--json".into(),
                "--".into(),
                text("api")?,
            ])
        }
        "describe_operation" => run(vec![
            "api".into(),
            format!("--schema={}", text("operation")?),
            "--".into(),
            text("api")?,
        ]),
        "call_api" => call_api(arguments, text("api")?, text("target")?)?,
        "query_data" => {
            let request = read_request(name, arguments, "export", |request| {
                request
                    .pointer("/args/export")
                    .is_some_and(|v| !v.is_null())
            })?;
            let mut args = owned(&["data", "--request", "-", "--json"]);
            if let Some(workspace) = optional("workspace") {
                args.extend(["--".into(), workspace.to_owned()]);
            }
            ToolRun {
                args,
                stdin: Some(request),
            }
        }
        "query_db" => {
            let database = text("database")?;
            let request = read_request(name, arguments, "execute", |request| {
                request.get("operation").and_then(Value::as_str) == Some("execute")
            })?;
            let mut args = owned(&["db", "--request", "-", "--json", "--"]);
            args.push(database);
            ToolRun {
                args,
                stdin: Some(request),
            }
        }
        _ => return Err(format!("no tool {name}")),
    })
}

fn call_api(arguments: &Value, api: String, target: String) -> Result<ToolRun, String> {
    let mut args = vec!["api".to_owned(), "--json".to_owned()];
    if let Some(method) = arguments.get("method").and_then(Value::as_str) {
        args.push(format!("--method={method}"));
    }
    if let Some(params) = arguments.get("params").and_then(Value::as_object) {
        for (key, value) in params {
            let value = match value {
                Value::String(value) => value.clone(),
                other => other.to_string(),
            };
            args.push(format!("--param={key}={value}"));
        }
    }
    if arguments.get("shape").and_then(Value::as_bool) == Some(true) {
        args.push("--shape".into());
    }
    if let Some(sample) = arguments.get("sample").and_then(Value::as_u64) {
        args.push(format!("--sample={sample}"));
    }
    let stdin = match arguments.get("body") {
        None | Some(Value::Null) => None,
        Some(Value::String(body)) => Some(body.clone()),
        Some(body) => Some(body.to_string()),
    };
    if stdin.is_some() {
        args.push("--data=@-".into());
    }
    args.extend(["--".into(), api, target]);
    Ok(ToolRun { args, stdin })
}

/// The `request` argument as the JSON a `--request -` reads, unless it
/// asks for the write a read-only tool refuses.
fn read_request(
    name: &str,
    arguments: &Value,
    write: &str,
    writes: impl Fn(&Value) -> bool,
) -> Result<String, String> {
    let request = arguments
        .get("request")
        .filter(|request| request.is_object())
        .ok_or_else(|| format!("{name}: `request` is a required object"))?;
    if writes(request) {
        return Err(format!(
            "{name} is read only: {write} is not offered over MCP"
        ));
    }
    Ok(request.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every() -> Vec<String> {
        TOOL_NAMES.map(str::to_owned).to_vec()
    }

    fn args(run: &ToolRun) -> Vec<&str> {
        run.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn a_notification_is_not_answered_and_a_bad_line_is_a_parse_error() {
        assert_eq!(
            read_message(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &every()
            ),
            Incoming::Nothing
        );
        let Incoming::Answer(answer) = read_message("{not json", &every()) else {
            panic!("a parse error is answered");
        };
        assert_eq!(answer["error"]["code"], -32700);
        assert_eq!(answer["id"], Value::Null);
        let Incoming::Answer(answer) =
            read_message(r#"{"jsonrpc":"2.0","id":7,"method":"x"}"#, &every())
        else {
            panic!("an unknown method is answered");
        };
        assert_eq!(answer["error"]["code"], -32601);
        assert_eq!(answer["id"], 7);
    }

    #[test]
    fn initialize_answers_the_version_asked_for_when_it_is_spoken() {
        let answer = |asked: &str| {
            let line = format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{asked}"}}}}"#
            );
            match read_message(&line, &every()) {
                Incoming::Answer(answer) => answer["result"]["protocolVersion"].clone(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(answer("2024-11-05"), "2024-11-05");
        assert_eq!(answer("1999-01-01"), PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn every_listed_tool_has_a_command_line() {
        let tools = tools(&every());
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOL_NAMES);
        let arguments = json!({
            "api": "a", "operation": "o", "target": "/t", "database": "d",
            "request": {"operation": "query", "args": {"sql": "SELECT 1"}}
        });
        for name in names {
            assert!(tool_run(name, &arguments).is_ok(), "{name}");
        }
        assert_eq!(tool_run("exec", &arguments), Err("no tool exec".into()));
    }

    #[test]
    fn a_message_reads_the_same_whichever_transport_carried_it() {
        let ping = json!({"jsonrpc": "2.0", "id": "a", "method": "ping"});
        assert_eq!(
            read_value(&ping, &every()),
            read_message(&ping.to_string(), &every())
        );
        assert_eq!(
            read_value(&json!({"jsonrpc": "2.0", "id": 1, "result": {}}), &every()),
            Incoming::Nothing
        );
    }

    #[test]
    fn a_tool_left_out_is_neither_listed_nor_run() {
        let exposed = ["call_api".to_owned(), "list_operations".to_owned()];
        let listed = tools(&exposed);
        let names: Vec<&str> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["list_operations", "call_api"]);
        let line = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ready","arguments":{}}}"#;
        let Incoming::Answer(answer) = read_message(line, &exposed) else {
            panic!("refused before running");
        };
        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(answer["result"]["content"][0]["text"], "no tool ready");
        let line = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"call_api","arguments":{"api":"a","target":"/t"}}}"#;
        assert!(matches!(
            read_message(line, &exposed),
            Incoming::Call { .. }
        ));
    }

    #[test]
    fn every_agent_value_stays_a_value_and_never_becomes_an_option() {
        let run = tool_run(
            "call_api",
            &json!({
                "api": "--confirm", "target": "-v", "method": "-k",
                "params": {"id": 42, "q": "--confirm"}, "sample": 2, "shape": true,
                "body": {"name": "x"}
            }),
        )
        .unwrap();
        assert_eq!(
            args(&run),
            [
                "api",
                "--json",
                "--method=-k",
                "--param=id=42",
                "--param=q=--confirm",
                "--shape",
                "--sample=2",
                "--data=@-",
                "--",
                "--confirm",
                "-v"
            ]
        );
        assert_eq!(run.stdin.as_deref(), Some(r#"{"name":"x"}"#));
        let run = tool_run(
            "describe_operation",
            &json!({"api": "-x", "operation": "-y"}),
        )
        .unwrap();
        assert_eq!(args(&run), ["api", "--schema=-y", "--", "-x"]);
        let run = tool_run("list_operations", &json!({"api": "p", "query": "-z"})).unwrap();
        assert_eq!(args(&run), ["api", "--ops=-z", "--json", "--", "p"]);
        let run = tool_run(
            "query_db",
            &json!({"database": "-d", "request": {"operation": "tables"}}),
        )
        .unwrap();
        assert_eq!(args(&run), ["db", "--request", "-", "--json", "--", "-d"]);
        assert_eq!(run.stdin.as_deref(), Some(r#"{"operation":"tables"}"#));
    }

    #[test]
    fn a_string_body_is_sent_as_it_is_and_no_body_reads_no_stdin() {
        let run = tool_run(
            "call_api",
            &json!({"api": "a", "target": "/t", "body": "plain text"}),
        )
        .unwrap();
        assert_eq!(run.stdin.as_deref(), Some("plain text"));
        let run = tool_run("call_api", &json!({"api": "a", "target": "/t"})).unwrap();
        assert_eq!(args(&run), ["api", "--json", "--", "a", "/t"]);
        assert_eq!(run.stdin, None);
    }

    #[test]
    fn the_read_tools_refuse_a_write_and_a_missing_argument() {
        let db = tool_run(
            "query_db",
            &json!({"database": "d", "request": {"operation": "execute", "args": {}}}),
        );
        assert_eq!(
            db,
            Err("query_db is read only: execute is not offered over MCP".into())
        );
        let data = tool_run(
            "query_data",
            &json!({"request": {"operation": "query", "args": {"sql": "SELECT 1", "export": "x.csv"}}}),
        );
        assert_eq!(
            data,
            Err("query_data is read only: export is not offered over MCP".into())
        );
        let data = tool_run(
            "query_data",
            &json!({"workspace": "lake", "request": {"operation": "tables"}}),
        )
        .unwrap();
        assert_eq!(
            args(&data),
            ["data", "--request", "-", "--json", "--", "lake"]
        );
        assert_eq!(
            tool_run("call_api", &json!({"api": "a"})),
            Err("call_api: `target` is a required string".into())
        );
        assert_eq!(
            tool_run("query_db", &json!({"database": "d", "request": "SELECT 1"})),
            Err("query_db: `request` is a required object".into())
        );
    }

    #[test]
    fn a_refused_call_is_answered_as_a_tool_error_not_a_protocol_error() {
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"query_db","arguments":{}}}"#;
        let Incoming::Answer(answer) = read_message(line, &every()) else {
            panic!("refused before running");
        };
        assert_eq!(answer["id"], 3);
        assert_eq!(answer["result"]["isError"], true);
    }

    #[test]
    fn a_failure_answers_the_error_document_and_a_success_its_stdout() {
        let document = r#"{"schema_version":1,"error":{"code":"OAUTH_LOGIN_REQUIRED"}}"#;
        let failed = tool_result(false, "", &format!("# a line before\n{document}\n"));
        assert_eq!(failed["isError"], true);
        assert_eq!(
            failed["structuredContent"]["error"]["code"],
            "OAUTH_LOGIN_REQUIRED"
        );
        assert_eq!(failed["content"][0]["text"], document);
        let plain = tool_result(false, "", "error[X]: plain\n");
        assert_eq!(plain["content"][0]["text"], "error[X]: plain");
        assert!(plain.get("structuredContent").is_none());
        let ok = tool_result(true, "{\"status\":200}\n", "");
        assert_eq!(ok["isError"], false);
        assert_eq!(ok["structuredContent"]["status"], 200);
        let list = tool_result(true, "[1]\n", "");
        assert_eq!(list["content"][0]["text"], "[1]");
        assert!(list.get("structuredContent").is_none());
        let timed_out = tool_failure("the call did not end within 600 seconds");
        assert_eq!(timed_out["isError"], true);
        assert_eq!(
            timed_out["content"][0]["text"],
            "the call did not end within 600 seconds"
        );
    }
}
