//! Runtime verification scenarios of the `api-client` feature.
//! `tests/scenarios/main.rs` holds the naming rule and what each
//! scenario writes.

use crate::support::tui::{self};
use crate::support::{ApiFake, AwsSecrets, OnePassword, PUBLIC_API_CONFIG, Sandbox, Scenario, run};

#[test]
fn api_terminal_lays_out_the_json_body() {
    let id = "api_terminal_lays_out_the_json_body";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-client")
            .api(ApiFake::CompactArray)
            .with_extra_config(PUBLIC_API_CONFIG),
        tui::STANDARD,
        &["api", "public", "/user"],
    );
    t.wait_for("\"login\": \"octocat\"");
    let screen = t.screen_text();
    let laid_out = "[\n  {\n    \"login\": \"octocat\",\n    \
                    \"path\": \"/api/user\",\n    \"amount\": 10.00\n  }\n]";
    let ok = screen.contains(laid_out);
    t.check(
        "the terminal shows the body laid out, down to the number the server sent",
        ok,
        if ok { String::new() } else { screen },
    )
    .expect_plain_output()
    .snapshot("body");
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_api_calls(1, None)
        .expect_token_grants(&[])
        .expect_no_files_written();
    v.finish();
}
#[test]
fn api_terminal_json_envelope_stays_one_line() {
    let id = "api_terminal_json_envelope_stays_one_line";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-client")
            .api(ApiFake::CompactArray)
            .with_extra_config(PUBLIC_API_CONFIG),
        tui::STANDARD,
        &["api", "public", "/user", "--json"],
    );
    t.wait_for("\"status\":200");
    // The envelope wraps on the screen; join the rows to read the document.
    let joined: String = t.screen_text().lines().collect();
    // The envelope carries the parsed body, so 10.00 arrives as 10.0: the
    // bytes are only preserved on the body path.
    let ok = joined.contains("{\"status\":200,\"headers\":{")
        && !joined.contains("\"status\": 200")
        && joined.contains("\"amount\":10.0}");
    t.check(
        "--json is the compact envelope on a terminal too, with the body reserialized",
        ok,
        if ok { String::new() } else { joined },
    )
    .snapshot("envelope");
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_api_calls(1, None)
        .expect_no_files_written();
    v.finish();
}

/// The plan's effects are a prediction of what the run does, so they are
/// held to what the run did: for a bearer source that runs a grant with a
/// 1Password client secret, a `kind = "token"` source read from Parameter
/// Store through an MFA profile, and a SigV4 API, each dry run is followed
/// by the real run, every kind of external call the real run made has to be
/// an effect of the plan, and the dry run itself made none of them. The
/// SigV4 pair runs once more with `--output`: the file the run wrote is the
/// plan's `file_write`, and a plan without `--output` names none.
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn api_dry_run_json_names_every_call_the_run_makes() {
    let mut v = run(Scenario::new(
        "api_dry_run_json_names_every_call_the_run_makes",
        "api-client",
        &["api", "svc", "/items", "--dry-run", "--json"],
    )
    .then_run(&["api", "svc", "/items"])
    .then_run(&["api", "ssm", "/voices", "--dry-run", "--json"])
    .then_run(&["api", "ssm", "/voices"])
    .then_run(&["api", "apigw", "/items", "--dry-run", "--json"])
    .then_run(&["api", "apigw", "/items"])
    // HEAD: the fake API would echo the signature into a body, and the
    // file would then hold it.
    .then_run(&[
        "api",
        "apigw",
        "HEAD /items",
        "--dry-run",
        "--json",
        "-o",
        "{home}/items.json",
    ])
    .then_run(&["api", "apigw", "HEAD /items", "-o", "{home}/items.json"])
    .onepassword(OnePassword::Enabled)
    .with_session_cache()
    .aws_secrets(AwsSecrets::FromId)
    .with_extra_config(
        r#"
[auth.svc]
kind = "oauth"
grant_type = "client_credentials"
token_url = "{server}/oauth/token"
client_id = "svc-client"
client_secret = "op://Agent/Service/client_secret"

[api.svc]
base_url = "{server}/api"

[auth.ssm]
kind = "token"
token = "aws-ssm://ops-mfa/kurama/api-key"
header = "X-Api-Key"
format = "{token}"

[api.ssm]
base_url = "{server}/api"

[api.apigw]
base_url = "{server}/api"
aws_profile = "ops-mfa"
service = "execute-api"
region = "ap-northeast-1"
"#,
    ));
    v.expect_exit_code(0);
    let runs = v.observed.runs.clone();
    for pair in runs.chunks(2) {
        let (dry, real) = (&pair[0], &pair[1]);
        let made = |run: &crate::support::Run| {
            [
                ("sts", !run.sts_calls.is_empty()),
                ("1password", !run.op_calls.is_empty()),
                ("authorization_server", !run.oauth_calls.is_empty()),
                ("aws_secret_store", !run.secret_calls.is_empty()),
                ("browser", !run.open_calls.is_empty()),
                ("api_request", !run.api_calls.is_empty()),
            ]
            .into_iter()
            .filter_map(|(effect, made)| made.then_some(effect))
            .collect::<Vec<_>>()
        };
        let plan: serde_json::Value = serde_json::from_str(&dry.stdout).unwrap_or_default();
        let planned: Vec<&str> = plan["effects"]
            .as_array()
            .map(|effects| {
                effects
                    .iter()
                    .filter(|effect| effect["performed"] == false)
                    .filter_map(|effect| effect["effect"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        let wrote = real.command.iter().any(|arg| arg.ends_with("/items.json"))
            && v.observed
                .files_written
                .iter()
                .any(|file| file == "home/items.json");
        v.check(
            &format!(
                "{:?}: the plan names file_write exactly when the run wrote a file",
                real.command
            ),
            planned.contains(&"file_write") == wrote,
            format!("wrote {wrote}; the plan names {planned:?}"),
        );
        let real_calls = made(real);
        let missing: Vec<&&str> = real_calls
            .iter()
            .filter(|effect| !planned.contains(effect))
            .collect();
        v.check(
            &format!(
                "{:?}: every call kind the run made is an effect of the plan",
                real.command
            ),
            real.exit_code == Some(0) && real_calls.len() > 1 && missing.is_empty(),
            format!("the run made {real_calls:?}; the plan names {planned:?}; missing {missing:?}"),
        );
        v.check(
            &format!("{:?}: the dry run made no call", dry.command),
            dry.exit_code == Some(0) && made(dry).is_empty(),
            format!("the dry run made {:?}", made(dry)),
        );
    }
    v.finish();
}

/// `--output PATH` saves the body as the server sent it: bytes that are not
/// UTF-8, a JSON body whose duplicate keys, number notation and blank lines
/// a parse would lose, and an empty body. An existing file is replaced
/// whole, nothing is left beside it, and stdout and stderr stay empty.
#[test]
fn api_output_writes_the_body_bytes_to_the_path() {
    const BINARY: &[u8] = b"\x00\xff\xfe\x89PNG\r\n\x1a\n\x00tail";
    const JSON: &[u8] = b"{\"a\":1,\"a\":2,\"n\":10.00,\"big\":123456789012345678901234567890}\n\n";
    let scenario = Scenario::new(
        "api_output_writes_the_body_bytes_to_the_path",
        "api-client",
        &[],
    )
    .api(ApiFake::Echo)
    .with_extra_config(PUBLIC_API_CONFIG)
    .with_home_bytes("in/body.bin", BINARY)
    .with_home_bytes("in/body.json", JSON)
    .with_home_file(
        "out/replaced.json",
        "the previous content, longer than the new one\n",
    );
    let mut sandbox = Sandbox::create(&scenario);
    let saves: [(&str, &str, &[u8]); 3] = [
        ("-d@{home}/in/body.bin", "{home}/out/body.bin", BINARY),
        ("-d@{home}/in/body.json", "{home}/out/replaced.json", JSON),
        ("-d", "{home}/out/empty", b""),
    ];
    let mut runs = Vec::new();
    let mut saved = Vec::new();
    for (data, output, _) in saves {
        let mut args = vec!["api", "public", "/echo", data];
        if data == "-d" {
            args.push("");
        }
        args.extend(["--output", output]);
        runs.push(sandbox.run_cli(&args));
        let path = output.replace("{home}", sandbox.home.to_str().unwrap());
        saved.push(std::fs::read(path).ok());
    }
    let mut left: Vec<String> = std::fs::read_dir(sandbox.home.join("out"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    for (index, (_, output, expected)) in saves.iter().enumerate() {
        v.keyed_run(index, "save", |v| {
            v.expect_exit_code(0)
                .expect_stdout_empty()
                .expect_stderr_empty()
                .expect_api_call_count(1)
        });
        let observed = &saved[index];
        v.check(
            &format!(
                "{output} holds the {} bytes of the body, byte for byte",
                expected.len()
            ),
            observed.as_deref() == Some(*expected),
            format!("observed {observed:?}"),
        );
    }
    v.check(
        "the output directory holds the three files and no temporary file",
        left == ["body.bin", "empty", "replaced.json"],
        format!("observed {left:?}"),
    )
    .expect_files_written(&[
        "home/out/body.bin",
        "home/out/empty",
        "home/out/replaced.json",
    ]);
    v.finish();
}

/// A directory the body cannot be written into (mode 500) is
/// API_OUTPUT_FAILED after the request and stays empty; a read-only file at
/// the path is refused before any request and keeps its bytes and mode.
#[test]
fn api_output_refuses_a_read_only_directory_or_file_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let scenario = Scenario::new(
        "api_output_refuses_a_read_only_directory_or_file_untouched",
        "api-client",
        &[],
    )
    .with_extra_config(PUBLIC_API_CONFIG)
    .with_home_file("locked/.keep", "")
    .with_home_file("kept.json", "{\"kept\":true}\n");
    let mut sandbox = Sandbox::create(&scenario);
    let mode = |mode| std::fs::Permissions::from_mode(mode);
    let locked = sandbox.home.join("locked");
    let kept = sandbox.home.join("kept.json");
    std::fs::remove_file(locked.join(".keep")).unwrap();
    std::fs::set_permissions(&locked, mode(0o500)).unwrap();
    std::fs::set_permissions(&kept, mode(0o444)).unwrap();
    let runs = vec![
        sandbox.run_cli(&["api", "public", "/user", "-o", "{home}/locked/body.json"]),
        sandbox.run_cli(&["api", "public", "/user", "-o", "{home}/kept.json"]),
    ];
    let locked_left = std::fs::read_dir(&locked).unwrap().count();
    let kept_after = std::fs::read(&kept).unwrap();
    let kept_mode = std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777;
    std::fs::set_permissions(&locked, mode(0o700)).unwrap();
    std::fs::set_permissions(&kept, mode(0o644)).unwrap();
    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "directory", |v| {
        v.expect_error("API_OUTPUT_FAILED", 1)
            .expect_stdout_empty()
            .expect_stderr_contains("/locked/body.json: permission denied\n")
            .expect_stderr_excludes("locked/.tmp")
            .expect_api_call_count(1)
    });
    v.keyed_run(1, "read-only file", |v| {
        v.expect_error("API_OUTPUT_FAILED", 1)
            .expect_stdout_empty()
            .expect_stderr_contains("/kept.json: permission denied\n")
            .expect_api_call_count(0)
    });
    v.check(
        "the directory stays empty",
        locked_left == 0,
        format!("observed {locked_left} entries"),
    )
    .check(
        "the read-only file keeps its bytes and mode 444",
        kept_after == b"{\"kept\":true}\n" && kept_mode == 0o444,
        format!(
            "observed {:?} with mode {kept_mode:o}",
            String::from_utf8_lossy(&kept_after)
        ),
    );
    v.finish();
}
#[test]
fn api_non_interactive_on_a_pty_prints_the_same_bytes_as_a_pipe() {
    // `api_terminal_lays_out_the_json_body` is the same call for a person.
    let id = "api_non_interactive_on_a_pty_prints_the_same_bytes_as_a_pipe";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-client")
            .api(ApiFake::CompactArray)
            .with_extra_config(PUBLIC_API_CONFIG)
            .with_env("KURAMA_AGENT", "1"),
        tui::STANDARD,
        &["api", "public", "/user"],
    );
    t.wait_for("10.00}]");
    // The line discipline puts `\r` before every `\n`; nothing else on the
    // way is the terminal's.
    let terminal = String::from_utf8_lossy(&t.raw_output()).replace("\r\n", "\n");
    let mut v = t.exit_then_run(&[&["api", "public", "/user"]]);
    let piped = v.observed.runs[1].stdout.clone();
    v.check(
        "the pseudo terminal received the bytes the pipe did, which are the server's",
        terminal == piped
            && piped == "[{\"login\":\"octocat\",\"path\":\"/api/user\",\"amount\":10.00}]",
        format!("terminal {terminal:?}\npipe     {piped:?}"),
    );
    v.expect_exit_code(0)
        .expect_api_calls(1, None)
        .expect_token_grants(&[]);
    v.finish();
}
