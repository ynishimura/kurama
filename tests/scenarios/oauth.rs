//! Runtime verification scenarios of the `oauth` feature that need code: a
//! browser played through a PTY, or parallel runs. The rest are cases under
//! `tests/cases/oauth/`. `tests/scenarios/main.rs` holds the naming rule and
//! what each scenario writes.

use crate::support::tui::{self};
use crate::support::{
    AUTH_CODE_CONFIG, FAKE_AUTH_CODE, FAKE_USER_CODE, GRANTED_ACCESS_TOKEN, OAuthFake,
    REFRESHED_ACCESS_TOKEN, Scenario, run, stored_token_json,
};

/// Endpoints from OpenID Connect discovery, device code grant.
const DEVICE_CODE_CONFIG: &str = "\
[auth.device]
kind = \"oauth\"
grant_type = \"device_code\"
issuer = \"{server}\"
client_id = \"device-client\"
";
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn oauth_login_authorization_code_opens_the_browser_and_stores_the_token() {
    let id = "oauth_login_authorization_code_opens_the_browser_and_stores_the_token";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .with_fake_browser()
            .with_token_store()
            .with_extra_config(AUTH_CODE_CONFIG),
        tui::STANDARD,
        &["login", "github"],
    );
    t.wait_for("# Logged in: token for 'github' is valid for");
    let mut v = t.exit_then_run(&[
        &["api", "github", "/user", "--jq", ".login"],
        &["login", "github"],
        &["env", "github"],
    ]);
    v.expect_run_sts_actions(0, &[])
        .expect_run_token_grants(0, &["authorization_code"])
        .expect_run_token_grants(1, &[])
        .expect_run_token_grants(2, &[])
        .expect_run_token_grants(3, &[])
        .expect_exit_code(0)
        .expect_stdout_is_export_script()
        .expect_stdout_contains("export GITHUB_TOKEN='")
        .expect_stdout_contains("export KURAMA_AUTH='github'")
        .expect_stdout_contains("unset KURAMA_AUTH");
    let runs = v.observed.runs.clone();
    let opened = runs[0].open_calls.clone();
    v.check(
        "run 0: the browser opens the authorization URL with PKCE and a state",
        runs[0].exit_code == Some(0)
            && opened.len() == 1
            && opened[0].contains("/oauth/authorize?")
            && opened[0].contains("response_type=code")
            && opened[0].contains("client_id=gh-client")
            && opened[0].contains("code_challenge_method=S256")
            && opened[0].contains("scope=repo")
            && opened[0].contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"),
        format!("exit {:?} open {opened:?}", runs[0].exit_code),
    );
    let exchange = runs[0].oauth_calls[0].clone();
    v.check(
        "run 0: the code from the redirect is exchanged with the PKCE verifier",
        exchange.params.get("code").map(String::as_str) == Some(FAKE_AUTH_CODE)
            && exchange
                .params
                .get("code_verifier")
                .is_some_and(|v| v.len() == 43)
            && exchange.params.get("redirect_uri").is_some_and(|uri| {
                uri.starts_with("http://127.0.0.1:") && uri.ends_with("/callback")
            }),
        format!("{exchange:?}"),
    );
    let authorization: std::collections::BTreeMap<String, String> = opened
        .first()
        .and_then(|url| url::Url::parse(url).ok())
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default();
    let expected_challenge = {
        use base64::Engine;
        use sha2::Digest;
        let verifier = exchange
            .params
            .get("code_verifier")
            .cloned()
            .unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()))
    };
    v.check(
        "run 0: the authorization URL carries a state and the S256 challenge of the exchanged verifier",
        authorization.get("state").is_some_and(|state| state.len() == 43)
            && authorization.get("code_challenge") == Some(&expected_challenge)
            && !exchange.params.contains_key("state"),
        format!("url params {authorization:?} exchange {exchange:?}"),
    );
    v.check(
        "run 1: api sends the stored token as a bearer and --jq prints the login",
        runs[1].exit_code == Some(0)
            && runs[1].stdout == "octocat\n"
            && runs[1].api_calls.len() == 1
            && runs[1].api_calls[0].authorization.as_deref()
                == Some(&format!("Bearer {GRANTED_ACCESS_TOKEN}")),
        format!("stdout {:?} api {:?}", runs[1].stdout, runs[1].api_calls),
    );
    v.check(
        "run 2: a second login is a no-op while the token is valid",
        runs[2].exit_code == Some(0)
            && runs[2].stderr.contains("is still valid")
            && runs[2].open_calls.is_empty(),
        format!("stderr {:?}", runs[2].stderr),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn oauth_login_device_code_polls_until_the_code_is_entered() {
    let id = "oauth_login_device_code_polls_until_the_code_is_entered";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .with_fake_browser()
            .with_token_store()
            .with_extra_config(DEVICE_CODE_CONFIG),
        tui::STANDARD,
        &["login", "device"],
    );
    t.wait_for("# Logged in: token for 'device'")
        .expect_text(&format!("enter the code {FAKE_USER_CODE}"));
    let mut v = t.exit_then_run(&[&["status", "device", "--json"]]);
    v.expect_run_token_grants(
        0,
        &[
            "urn:ietf:params:oauth:grant-type:device_code",
            "urn:ietf:params:oauth:grant-type:device_code",
        ],
    )
    .expect_exit_code(0);
    let runs = v.observed.runs.clone();
    let paths: Vec<&str> = runs[0]
        .oauth_calls
        .iter()
        .map(|call| call.path.as_str())
        .collect();
    v.check(
        "run 0: discovery, the device authorization request, a pending poll and the token",
        runs[0].exit_code == Some(0)
            && paths
                == [
                    "/.well-known/openid-configuration",
                    "/oauth/device",
                    "/oauth/token",
                    "/oauth/token",
                ]
            && runs[0]
                .open_calls
                .iter()
                .any(|url| url.ends_with("/device")),
        format!("paths {paths:?} open {:?}", runs[0].open_calls),
    );
    let calls = runs[0].oauth_calls.clone();
    v.check(
        "run 0: the device request names the client and every poll sends the device code",
        calls
            .get(1)
            .and_then(|call| call.params.get("client_id"))
            .map(String::as_str)
            == Some("device-client")
            && calls.len() > 2
            && calls[2..].iter().all(|call| {
                call.params.get("device_code").map(String::as_str) == Some("fake-device-code")
            }),
        format!("{calls:?}"),
    );
    let statuses: Vec<serde_json::Value> =
        serde_json::from_str(&runs[1].stdout).unwrap_or_default();
    v.check(
        "run 1: status reports the device source as logged in",
        statuses.len() == 1
            && statuses[0]["grant_type"] == "device_code"
            && statuses[0]["logged_in"] == true,
        format!("stdout {}", runs[1].stdout),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn oauth_device_polling_obeys_interval_and_expiry() {
    // The code lives 3 seconds and asks for polls 2 seconds apart: the poll
    // at 2 is pending, and the next one would be at 4, after the code expired.
    let id = "oauth_device_polling_obeys_interval_and_expiry";
    let started = std::time::Instant::now();
    let t = tui::launch_command(
        Scenario::tui(id)
            .with_fake_browser()
            .with_token_store()
            .with_extra_config(DEVICE_CODE_CONFIG)
            .oauth(OAuthFake::DevicePending {
                expires_in: 3,
                interval: 2,
            }),
        tui::STANDARD,
        &["login", "device"],
    );
    let mut v = t.exit();
    let elapsed = started.elapsed();
    v.expect_exit_code(4)
        .expect_run_token_grants(0, &["urn:ietf:params:oauth:grant-type:device_code"])
        .expect_error_line(
            "OAUTH_REJECTED",
            "Failed to get a token for 'device': the device code expired before it was entered",
        );
    v.check(
        "the one poll came after the 2-second interval",
        elapsed >= std::time::Duration::from_secs(2),
        format!("elapsed {elapsed:?}"),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn oauth_refresh_replaces_an_expired_token_once_across_parallel_calls() {
    let mut v = run(Scenario::new(
        "oauth_refresh_replaces_an_expired_token_once_across_parallel_calls",
        "oauth",
        &["status", "github", "--json"],
    )
    .with_extra_config(AUTH_CODE_CONFIG)
    .with_stored_token("github", &stored_token_json("stale-token", -60, true))
    .with_parallel_runs(10, &["api", "github", "/user", "--jq", ".login"]));
    v.expect_exit_code(0)
        .expect_token_grants(&["refresh_token"])
        .expect_api_calls(10, Some(&format!("Bearer {REFRESHED_ACCESS_TOKEN}")));
    let runs = v.observed.runs.clone();
    let status: Vec<serde_json::Value> = serde_json::from_str(&runs[0].stdout).unwrap_or_default();
    v.check(
        "run 0: status reports the stored token as expired but refreshable, needing nobody",
        status.first().is_some_and(|s| {
            s["token"] == "expired" && s["refreshable"] == true && s["needs_human"] == false
        }),
        format!("stdout {}", runs[0].stdout),
    );
    v.check(
        "run 1: ten parallel calls print the login ten times",
        runs[1].stdout == "octocat\n".repeat(10),
        format!("stdout {:?}", runs[1].stdout),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn oauth_login_no_browser_prints_the_url_and_opens_nothing() {
    use std::io::{Read, Write};
    let id = "oauth_login_no_browser_prints_the_url_and_opens_nothing";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .with_fake_browser()
            .with_token_store()
            .with_extra_config(AUTH_CODE_CONFIG),
        tui::WIDE,
        &["login", "github", "--no-browser"],
    );
    t.wait_for("# Waiting for the browser")
        .expect_text("# Open this URL in a browser to authorize kurama:");
    // The URL wraps on the terminal: join the rows, then read its query.
    let screen: String = t.screen_text().lines().collect();
    let query = screen
        .split_once("/oauth/authorize?")
        .and_then(|(_, rest)| rest.split_once("scope=repo"))
        .map(|(query, _)| format!("{query}scope=repo"))
        .unwrap_or_default();
    let params: std::collections::BTreeMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
    let redirect = params
        .get("redirect_uri")
        .and_then(|uri| url::Url::parse(uri).ok());
    let state = params.get("state").cloned().unwrap_or_default();
    t.check(
        "the printed URL names the loopback redirect, a state and the PKCE challenge",
        redirect
            .as_ref()
            .is_some_and(|url| url.host_str() == Some("127.0.0.1") && url.path() == "/callback")
            && state.len() == 43
            && params.get("code_challenge_method").map(String::as_str) == Some("S256"),
        format!("query {query:?}"),
    );
    // Play the browser: follow the redirect the authorization server sends.
    if let Some(port) = redirect.as_ref().and_then(url::Url::port) {
        let mut stream =
            std::net::TcpStream::connect(("127.0.0.1", port)).expect("the loopback listener");
        write!(
            stream,
            "GET /callback?code={FAKE_AUTH_CODE}&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
    }
    t.wait_for("# Logged in: token for 'github' is valid for");
    let mut v = t.exit();
    v.expect_run_token_grants(0, &["authorization_code"])
        .expect_exit_code(0);
    let runs = v.observed.runs.clone();
    v.check(
        "no browser is started and the code from the redirect is exchanged",
        runs[0].exit_code == Some(0)
            && runs[0].open_calls.is_empty()
            && runs[0]
                .oauth_calls
                .first()
                .and_then(|call| call.params.get("code"))
                .map(String::as_str)
                == Some(FAKE_AUTH_CODE),
        format!(
            "open {:?} oauth {:?}",
            runs[0].open_calls, runs[0].oauth_calls
        ),
    );
    v.finish();
}
