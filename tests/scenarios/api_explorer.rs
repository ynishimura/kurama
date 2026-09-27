//! Runtime verification scenarios of the `api-explorer` feature.
//! `tests/scenarios/main.rs` holds the naming rule and what each
//! scenario writes.

use crate::support::tui::{self, Key};
use crate::support::{
    ApiFake, COMPLETION_CONFIG, FAKE_EDITOR, GRANTED_ACCESS_TOKEN, REFRESHED_ACCESS_TOKEN,
    STORED_ACCESS_TOKEN, Scenario, SpecFake, completion_values, run, stored_token_json,
};

/// A public API described by a local file.
const PETSTORE_FILE_CONFIG: &str = "\
[api.pets]
base_url = \"{server}/api\"
openapi = \"{fixtures}/petstore.json\"
";
/// A public API whose description the fake server serves.
const PETSTORE_URL_CONFIG: &str = "\
[api.pets]
base_url = \"{server}/api\"
openapi = \"{server}/spec/petstore.json\"
";
/// The github source with the petstore description on its API.
const GITHUB_SPEC_CONFIG: &str = "\
[auth.github]
kind = \"oauth\"
grant_type = \"authorization_code\"
auth_url = \"{server}/oauth/authorize\"
token_url = \"{server}/oauth/token\"
client_id = \"gh-client\"

[api.github]
base_url = \"{server}/api\"
openapi = \"{fixtures}/petstore.json\"
";
#[test]
fn api_spec_expired_cache_revalidates_and_304_starts_the_next_interval() {
    let mut v = run(Scenario::new(
        "api_spec_expired_cache_revalidates_and_304_starts_the_next_interval",
        "api-explorer",
        &["api", "pets", "--ops"],
    )
    .with_extra_config(PETSTORE_URL_CONFIG)
    .with_seeded_spec_cache("/spec/petstore.json", "petstore.json", 7200)
    .then_run(&["api", "pets", "--ops"]));
    v.expect_exit_code(0)
        .expect_run_spec_calls(0, 1, true, 304)
        .expect_run_spec_calls(1, 0, false, 0)
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_op_calls(0);
    let runs = v.observed.runs.clone();
    v.check(
        "the validated and interval-cached copies list the same operations",
        runs[0].exit_code == Some(0)
            && runs[0].stdout == runs[1].stdout
            && runs[1].stdout.contains("pets/list"),
        runs[1].stdout.clone(),
    );
    let files = v.observed.files_written.clone();
    v.check(
        "304 refreshes metadata without rewriting the cached document",
        !files.iter().any(|path| path.ends_with(".body")),
        format!("{files:?}"),
    );
    v.finish();
}
#[test]
fn api_spec_future_timestamp_is_revalidated() {
    let mut v = run(Scenario::new(
        "api_spec_future_timestamp_is_revalidated",
        "api-explorer",
        &["api", "pets", "--ops"],
    )
    .with_extra_config(PETSTORE_URL_CONFIG)
    .with_seeded_spec_cache("/spec/petstore.json", "petstore.json", -86400)
    .then_run(&["api", "pets", "--ops"]));
    v.expect_exit_code(0)
        .expect_run_spec_calls(0, 1, true, 304)
        .expect_run_spec_calls(1, 0, false, 0)
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_op_calls(0);
    v.finish();
}
#[test]
fn api_spec_corrupt_metadata_is_fetched_again() {
    let mut v = run(Scenario::new(
        "api_spec_corrupt_metadata_is_fetched_again",
        "api-explorer",
        &["api", "pets", "--ops"],
    )
    .with_extra_config(PETSTORE_URL_CONFIG)
    .with_seeded_spec_cache("/spec/petstore.json", "petstore.json", 60)
    .with_corrupt_spec_metadata());
    v.expect_exit_code(0)
        .expect_run_spec_calls(0, 1, false, 200)
        .expect_stdout_contains("pets/list")
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_op_calls(0);
    v.finish();
}
#[test]
fn api_spec_a_recent_cached_copy_works_offline_without_a_warning() {
    let mut v = run(Scenario::new(
        "api_spec_a_recent_cached_copy_works_offline_without_a_warning",
        "api-explorer",
        &["api", "pets", "--ops", "-v"],
    )
    .with_extra_config(PETSTORE_URL_CONFIG)
    .spec(SpecFake::Down)
    .with_seeded_spec_cache("/spec/petstore.json", "petstore.json", 60));
    let fetched_at = v.seeded.spec_fetched_at.clone().unwrap();
    v.expect_exit_code(0)
        .expect_run_spec_calls(0, 0, false, 0)
        .expect_stdout_contains("pets/list")
        .expect_stderr_contains("revalidation skipped")
        .expect_stderr_contains(&fetched_at)
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    let stderr = v.observed.last().stderr.clone();
    v.check(
        "a recent cached copy is not reported as stale",
        !stderr.contains("# warning:"),
        stderr,
    );
    v.finish();
}
#[test]
fn api_spec_offline_uses_the_cached_copy_with_a_warning() {
    let mut v = run(Scenario::new(
        "api_spec_offline_uses_the_cached_copy_with_a_warning",
        "api-explorer",
        &["api", "pets", "--ops", "owner"],
    )
    .with_extra_config(PETSTORE_URL_CONFIG)
    .spec(SpecFake::Down)
    .with_seeded_spec_cache("/spec/petstore.json", "petstore.json", 24 * 60 * 60)
    .then_run(&["api", "pets", "--refresh-spec"]));
    v.expect_error("API_SPEC_UNAVAILABLE", 1)
        .expect_stderr_contains("HTTP 503: spec server down")
        .expect_run_spec_calls(0, 1, true, 503)
        .expect_run_spec_calls(1, 1, false, 503);
    let runs = v.observed.runs.clone();
    let fetched_at = v.seeded.spec_fetched_at.clone().unwrap();
    v.check(
        "with the server down the cached copy answers, with a warning that says so",
        runs[0].exit_code == Some(0)
            && runs[0].stdout.contains("owners/list-pets")
            && runs[0].stderr.contains(&format!(
                "# warning: using the cached API description of [api.pets] from {fetched_at} (HTTP 503: spec server down)"
            )),
        format!("stdout {:?} stderr {:?}", runs[0].stdout, runs[0].stderr),
    );
    v.finish();
}
#[test]
fn tui_explorer_lists_operations_search_and_detail() {
    let id = "tui_explorer_lists_operations_search_and_detail";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .expect_text("kurama api")
        .expect_text("pets  Petstore 1.2.0")
        .expect_row_with(&["METHOD", "ID"])
        .expect_row_with(&["▸", "GET", "pets/list"])
        .expect_row_with(&["POST", "pets/create"])
        .expect_row_with(&["DELETE", "DELETE /pets/{petId}"])
        .expect_text("┏ Operation ")
        .expect_text("Request   GET /pets")
        .expect_text("Params    limit (query, integer); status (query,")
        .expect_footer_contains("↑↓ move")
        .expect_footer_contains("Enter open")
        .snapshot("list")
        .key(Key::Down)
        .expect_row_with(&["▸", "POST", "pets/create"])
        .expect_text("Scopes    read:pets, write:pets")
        .expect_text("Body      application/json (required)")
        .expect_text("Docs      https://docs.example.com/pets#create")
        .snapshot("create-selected")
        .key(Key::Char('/'))
        .expect_footer_contains("Esc clear")
        .type_text("owner")
        .wait_for("Operations 1/5  /owner_")
        .expect_row_with(&["▸", "GET", "owners/list-pets"])
        .expect_no_text("pets/create")
        .snapshot("filtered")
        .type_text("zz")
        .wait_for("No operation matches \"ownerzz\"")
        .key(Key::Esc)
        .wait_for("Operations (5)")
        .key(Key::End)
        .expect_row_with(&["▸", "GET", "owners/list-pets"])
        .key(Key::Char('?'))
        .wait_for("┏ Help ")
        .expect_text("jq filter on the result")
        .snapshot("help")
        .key(Key::Esc)
        .wait_until("the help closes", |screen| !screen.contains("┏ Help "));
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_no_files_written();
    v.finish();
}
#[test]
fn tui_explorer_help_q_matches_its_quit_hint() {
    let id = "tui_explorer_help_q_matches_its_quit_hint";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Char('?'))
        .wait_for("┏ Help ")
        .expect_row_with(&["q or Ctrl-C", "quit"]);
    // The help says q quits, so q pressed over the help ends the explorer.
    t.key(Key::Char('q'));
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_api_calls(0, None)
        .expect_no_files_written();
    v.finish();
}
#[test]
fn tui_explorer_shows_the_response_shape() {
    let mut t = tui::launch_command(
        Scenario::tui("tui_explorer_shows_the_response_shape")
            .for_feature("api-explorer")
            .with_extra_config(&PETSTORE_FILE_CONFIG.replace("petstore.json", "responses.json")),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (2)")
        .expect_text("Response  200 application/json")
        .expect_text("\"name\": \"string\"")
        .expect_text("\"parent\": {}")
        .snapshot("response-shape");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_no_files_written();
    v.finish();
}
#[test]
fn tui_explorer_no_color_preserves_selection_and_form_emphasis() {
    let mut t = tui::launch_command(
        Scenario::tui("tui_explorer_no_color_preserves_selection_and_form_emphasis")
            .for_feature("api-explorer")
            .with_env("NO_COLOR", "1")
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .expect_default_colors()
        .expect_bold_text("▸")
        .snapshot("list")
        .key(Key::Enter)
        .expect_default_colors()
        .expect_footer_contains("Esc back")
        .snapshot("form")
        .key(Key::Esc)
        .resize(tui::SMALL)
        .expect_default_colors()
        .expect_bold_text("▸")
        .snapshot("small")
        .resize(tui::WIDE)
        .expect_default_colors()
        .expect_bold_text("▸")
        .snapshot("wide");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
#[test]
fn tui_explorer_dumb_terminal_exits_2_without_control_sequences() {
    let mut t = tui::launch_command(
        Scenario::tui("tui_explorer_dumb_terminal_exits_2_without_control_sequences")
            .for_feature("api-explorer")
            .with_env("TERM", "dumb")
            .with_env("RUST_LOG", "kurama=debug")
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::WIDE,
        &["api", "pets"],
    );
    t.wait_for("error[API_TARGET_REQUIRED]")
        .expect_text("DEBUG Starting kurama")
        .expect_text("hint: pass a TARGET")
        .expect_plain_output()
        .snapshot("unsupported");
    let mut v = t.exit();
    v.expect_error("API_TARGET_REQUIRED", 2)
        .expect_api_calls(0, None)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_form_sends_the_request_and_filters_the_result_with_jq() {
    let id = "tui_explorer_form_sends_the_request_and_filters_the_result_with_jq";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_fake_tools()
            .with_extra_config(GITHUB_SPEC_CONFIG)
            .with_stored_token(
                "github",
                &stored_token_json(STORED_ACCESS_TOKEN, 3600, false),
            ),
        tui::STANDARD,
        &["api", "github"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Down)
        .key(Key::Down)
        .expect_row_with(&["▸", "GET", "pets/get"])
        .key(Key::Enter)
        .wait_for("┏ pets/get  GET /pets/{petId} ")
        .expect_row_with(&["▸", "petId*", "path  string"])
        .expect_row_with(&["X-Trace", "header  string"])
        .expect_footer_contains("Tab next")
        .expect_footer_contains("Enter send")
        .snapshot("form")
        .key(Key::Enter)
        .wait_for("! operation pets/get needs -P petId=<value>")
        .snapshot("form-refused")
        .type_text("px1")
        .key(Key::Left)
        .key(Key::Left)
        .key(Key::Delete)
        .type_text(" ")
        .expect_row_with(&["petId*", "p ▏1"])
        .snapshot("form-middle-edit")
        .key(Key::Char('\t'))
        .type_text("t1")
        .expect_row_with(&["▸", "X-Trace", "t1▏"])
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .expect_text("┏ pets/get  HTTP 200 OK  ")
        .expect_text("\"path\": \"/api/pets/p%201\"")
        .expect_text("\"login\": \"octocat\"")
        .expect_footer_contains("j jq")
        .snapshot("result")
        .key(Key::Char('h'))
        .wait_for("content-type: application/json")
        .key(Key::Char('h'))
        .key(Key::Char('j'))
        .wait_for("┏ jq filter ")
        .type_text(".path")
        .expect_text("Filter  .path▏")
        .snapshot("jq-input")
        .key(Key::Enter)
        .wait_for("jq: .path")
        .wait_until("the filter's lines arrive", |screen| {
            !screen.contains("jq: running")
        })
        .expect_row_with(&["┃ /api/pets/p%201"])
        .expect_no_text("\"login\"")
        .snapshot("result-jq")
        .key(Key::Char('c'))
        .wait_for("command copied to the clipboard")
        .key(Key::Esc)
        .wait_for("┏ pets/get  GET /pets/{petId} ")
        .expect_row_with(&["petId*", "p 1"])
        .key(Key::Esc)
        .wait_for("Operations (5)")
        .key(Key::Char('q'));
    // The command the explorer copied, run by the CLI in the same sandbox.
    let mut v = t.exit_then_run(&[&[
        "api",
        "github",
        "pets/get",
        "-P",
        "petId=p 1",
        "-P",
        "X-Trace=t1",
        "--jq",
        ".path",
    ]]);
    v.expect_exit_code(0).expect_token_grants(&[]);
    let (tui, cli) = (v.observed.runs[0].clone(), v.observed.runs[1].clone());
    let bearer = format!("Bearer {STORED_ACCESS_TOKEN}");
    v.check(
        "c copied the re-runnable CLI command with the jq filter",
        tui.clipboard_calls
            == ["kurama api github pets/get -P 'petId=p 1' -P X-Trace=t1 --jq .path"],
        format!("{:?}", tui.clipboard_calls),
    );
    v.check(
        "the explorer and the CLI sent the same request: method, path, headers, body and the token",
        tui.api_calls.len() == 1
            && cli.api_calls.len() == 1
            && tui.api_calls[0].method == cli.api_calls[0].method
            && tui.api_calls[0].path == cli.api_calls[0].path
            && tui.api_calls[0].path == "/api/pets/p%201"
            && tui.api_calls[0].headers == cli.api_calls[0].headers
            && tui.api_calls[0].body == cli.api_calls[0].body
            && tui.api_calls[0].authorization == cli.api_calls[0].authorization
            && tui.api_calls[0].authorization.as_deref() == Some(bearer.as_str())
            && cli.stdout == "/api/pets/p%201\n",
        format!(
            "tui {:?} cli {:?} stdout {:?}",
            tui.api_calls, cli.api_calls, cli.stdout
        ),
    );
    v.finish();
}
#[test]
fn tui_explorer_edits_the_body_in_the_editor_and_copies_the_command() {
    let id = "tui_explorer_edits_the_body_in_the_editor_and_copies_the_command";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_fake_tools()
            .with_env("EDITOR", FAKE_EDITOR)
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Down)
        .key(Key::Enter)
        .wait_for("┏ pets/create  POST /pets ")
        .expect_row_with(&["▸", "body*", "application/json", "Ctrl-E edits"])
        .expect_text("\"name\": \"\"")
        .expect_no_text("octocat")
        .snapshot("form-body")
        .key(Key::Ctrl('e'))
        .wait_for("body updated")
        .expect_text("{\"name\":\"from-editor\",\"tags\":[\"edited\"]}")
        .snapshot("form-edited")
        .key(Key::Ctrl('y'))
        .wait_for("command copied to the clipboard")
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .expect_text("\"method\": \"POST\"")
        .snapshot("result");
    let mut v = t.quit();
    v.expect_exit_code(0);
    let run = v.observed.last().clone();
    v.check(
        "the edited body was posted with the operation's media type",
        run.api_calls.len() == 1
            && run.api_calls[0].method == "POST"
            && run.api_calls[0].path == "/api/pets"
            && run.api_calls[0].body == "{\"name\":\"from-editor\",\"tags\":[\"edited\"]}"
            && run.api_calls[0]
                .headers
                .get("content-type")
                .map(String::as_str)
                == Some("application/json"),
        format!("{:?}", run.api_calls),
    );
    v.check(
        "Ctrl-Y copied the command with the edited body",
        run.clipboard_calls
            == [
                "kurama api pets pets/create -d '{\"name\":\"from-editor\",\"tags\":[\"edited\"]}'",
            ],
        format!("{:?}", run.clipboard_calls),
    );
    v.check(
        "the editor was opened once, on the required-only skeleton",
        run.editor_calls == ["{   \"name\": \"\" }"],
        format!("{:?}", run.editor_calls),
    );
    v.check(
        "the editor's temporary file is gone",
        !v.observed
            .files_written
            .iter()
            .any(|file| file.contains("kurama-body-")),
        format!("{:?}", v.observed.files_written),
    );
    v.finish();
}
#[test]
fn tui_explorer_resize_keeps_the_selection_and_switches_panes() {
    let id = "tui_explorer_resize_keeps_the_selection_and_switches_panes";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Down)
        .expect_row_with(&["▸", "POST", "pets/create"])
        .expect_text("┏ Operation ")
        .snapshot("120x40")
        .resize(tui::SMALL)
        .wait_until("the detail pane disappears", |screen| {
            !screen.contains("┏ Operation ")
        })
        .expect_row_with(&["▸", "POST", "pets/create"])
        .expect_footer_contains("q quit")
        .snapshot("80x24")
        .resize(tui::WIDE)
        .wait_for("┏ Operation ")
        .expect_row_with(&["▸", "POST", "pets/create"])
        .expect_text("Docs      https://docs.example.com/pets#create")
        .snapshot("160x50")
        .key(Key::Enter)
        .wait_for("┏ pets/create  POST /pets ")
        .resize(tui::SMALL)
        .wait_for("┏ pets/create  POST /pets ")
        .expect_footer_contains("Enter send")
        .snapshot("form-80x24")
        .key(Key::Esc)
        .wait_for("Operations (5)");
    let mut v = t.quit();
    v.expect_exit_code(0).expect_api_calls(0, None);
    for step in ["120x40", "80x24", "160x50", "form-80x24"] {
        let text = tui::read_screen(&tui::artifact_dir(id).join(step));
        let size = step.trim_start_matches("form-");
        let (cols, rows) = size.split_once('x').unwrap();
        let rows: usize = rows.parse().unwrap();
        let cols: usize = cols.parse().unwrap();
        v.check(
            &format!("{step}: screen.txt has {rows} rows and every row fits the width"),
            text.lines().count() == rows
                && text
                    .lines()
                    .all(|line| unicode_width::UnicodeWidthStr::width(line) <= cols),
            format!("rows {}", text.lines().count()),
        );
    }
    v.finish();
}
#[test]
fn tui_explorer_shows_a_failed_description_as_a_modal() {
    let id = "tui_explorer_shows_a_failed_description_as_a_modal";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_extra_config(PETSTORE_URL_CONFIG)
            .spec(SpecFake::Down),
        tui::SMALL,
        &["api", "pets"],
    );
    t.wait_for("┏ Error ")
        .expect_text("API_SPEC_UNAVAILABLE: cannot load the API")
        .expect_text("503")
        .expect_text("hint: check the openapi URL under [api.pets]")
        .expect_footer_contains("Enter back")
        .snapshot("spec-error")
        .key(Key::Enter)
        .wait_until("the error closes", |screen| !screen.contains("┏ Error "))
        .expect_text("The API description could not be loaded; q quits.")
        .snapshot("no-description");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_run_spec_calls(0, 1, false, 503)
        .expect_api_calls(0, None);
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_shows_a_failed_call_as_a_modal() {
    let id = "tui_explorer_shows_a_failed_call_as_a_modal";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .api(ApiFake::Slow)
            .with_extra_config(GITHUB_SPEC_CONFIG)
            .with_stored_token(
                "github",
                &stored_token_json(STORED_ACCESS_TOKEN, 3600, false),
            ),
        tui::STANDARD,
        &["api", "github", "--timeout", "1"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Enter)
        .wait_for("┏ pets/list  GET /pets ")
        .key(Key::Enter)
        .wait_for("┏ Error ")
        .expect_footer_contains("Enter back")
        .snapshot("call-error");
    // Join wrapped text without treating the modal's borders as message content.
    let message = t
        .screen_text()
        .replace('┃', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    t.check(
        "the modal shows the full error with one timeout cause",
        message.contains("API_REQUEST_FAILED: request failed: request timed out after 1s")
            && message.matches("request timed out after 1s").count() == 1,
        message,
    )
    .key(Key::Enter)
    .wait_until("the error closes", |screen| !screen.contains("┏ Error "))
    .expect_text("┏ pets/list  GET /pets ")
    .key(Key::Esc)
    .wait_for("Operations (5)");
    let mut v = t.quit();
    v.expect_exit_code(0);
    let run = v.observed.last().clone();
    v.check(
        "the call was made once and the form is back after the modal",
        run.api_calls.len() == 1 && run.api_calls[0].path == "/api/pets",
        format!("{:?}", run.api_calls),
    );
    v.finish();
}
#[test]
fn tui_explorer_prints_the_description_warnings_after_exit() {
    let id = "tui_explorer_prints_the_description_warnings_after_exit";
    let mut t = tui::launch_command(
        Scenario::tui(id).for_feature("api-explorer").with_extra_config(
            "[api.legacy]\nbase_url = \"{server}/api\"\nopenapi = \"{fixtures}/legacy-swagger.json\"\n",
        ),
        tui::STANDARD,
        &["api", "legacy"],
    );
    t.wait_for("Operations (3)")
        .expect_text("2 warning(s) about the description are printed after exit")
        .snapshot("warnings-notice");
    let mut v = t.quit();
    v.expect_exit_code(0).expect_api_calls(0, None);
    let terminal = v.observed.last().stdout.clone();
    v.check(
        "the warnings reach the terminal once the explorer has exited",
        terminal.contains("# warning: upload: formData parameter 'file' is not supported")
            && terminal.contains(
                "# warning: $ref https://schemas.example.com/ref.json#/Ref is not resolved",
            ),
        format!("terminal after exit: {terminal:?}"),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_shows_a_rejected_call_with_its_status() {
    let id = "tui_explorer_shows_a_rejected_call_with_its_status";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .api(ApiFake::NotFound)
            .with_extra_config(GITHUB_SPEC_CONFIG)
            .with_stored_token(
                "github",
                &stored_token_json(STORED_ACCESS_TOKEN, 3600, false),
            ),
        tui::STANDARD,
        &["api", "github"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Enter)
        .wait_for("┏ pets/list  GET /pets ")
        .expect_row_with(&["▸", "limit", "query  integer"])
        .expect_row_with(&["status", "query  available|sold"])
        .key(Key::Enter)
        .wait_for("HTTP 404 Not Found")
        .expect_text("\"message\": \"Not Found\"")
        .snapshot("result-404");
    let mut v = t.quit();
    v.expect_exit_code(0);
    let run = v.observed.last().clone();
    v.check(
        "the rejected call was made once with the token and shown, not retried",
        run.api_calls.len() == 1 && run.api_calls[0].path == "/api/pets",
        format!("{:?}", run.api_calls),
    );
    v.finish();
}
#[test]
fn completion_jq_uses_response_schemas_without_io() {
    let config = COMPLETION_CONFIG.replace("petstore.json", "responses.json");
    let mut v = run(Scenario::completion(
        "completion_jq_uses_response_schemas_without_io",
        &["--", "kurama", "api", "pets", "pets/list", "--jq", "."],
        "5",
    )
    .with_config(&config)
    .spec(SpecFake::Down)
    .with_seeded_spec_cache("/spec/responses.json", "responses.json", 86400)
    .then_run(&["--", "kurama", "api", "cached", "pets/list", "--jq", "."])
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "pets/list",
        "--jq",
        ".content[].na",
    ])
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "pets/list",
        "--jq",
        ".content[] | select(.na",
    ])
    .then_run(&["--", "kurama", "api", "pets", "GET /pets", "--jq", ".page"]));
    let runs = v.observed.runs.clone();
    for (i, expected) in [
        "\t.content:array\n\t.page:integer",
        "\t.content:array\n\t.page:integer",
        "\t.content[].name:string",
        "\t.content[] | select(.name:string",
        "\t.page:integer",
    ]
    .iter()
    .enumerate()
    {
        v.check(
            &format!("run {i} completes the schema path and reports its type"),
            runs[i].stdout == *expected,
            runs[i].stdout.clone(),
        );
    }
    v.finish();
}
#[test]
fn completion_jq_matches_a_bare_path_target() {
    let config = COMPLETION_CONFIG.replace("petstore.json", "jq-completion.json");
    let mut v = run(Scenario::completion(
        "completion_jq_matches_a_bare_path_target",
        &["--", "kurama", "api", "pets", "/pets", "--jq", "."],
        "5",
    )
    .with_config(&config)
    .then_run(&["--", "kurama", "api", "pets", "/odd", "--jq", ".[\"a:"]));
    let runs = v.observed.runs.clone();
    let stdout = runs[0].stdout.clone();
    v.check(
        "a bare path without -X or -d uses the GET response shape only",
        stdout == "\t.content:array\n\t.page:integer",
        stdout,
    );
    v.check(
        "zsh protocol escaping keeps a colon inside a quoted jq key",
        runs[1].stdout == "\t.[\"a\\:b\"]:string",
        runs[1].stdout.clone(),
    );
    v.finish();
}
#[test]
fn completion_jq_without_a_target_is_silent() {
    let config = COMPLETION_CONFIG.replace("petstore.json", "jq-completion.json");
    let mut v = run(Scenario::completion(
        "completion_jq_without_a_target_is_silent",
        &["--", "kurama", "api", "pets", "--jq", "."],
        "4",
    )
    .with_config(&config));
    v.check(
        "jq completion does not invent a response shape before an operation target",
        v.observed.last().stdout.is_empty(),
        v.observed.last().stdout.clone(),
    );
    v.finish();
}
#[test]
fn completion_jq_matches_a_path_and_explicit_method_post() {
    let config = COMPLETION_CONFIG.replace("petstore.json", "jq-completion.json");
    let mut v = run(Scenario::completion(
        "completion_jq_matches_a_path_and_explicit_method_post",
        &[
            "--", "kurama", "api", "pets", "/pets", "-X", "POST", "--jq", ".",
        ],
        "7",
    )
    .with_config(&config)
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "GET /pets",
        "-X",
        "POST",
        "--jq",
        ".",
    ])
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "pets/list",
        "-X",
        "POST",
        "--jq",
        ".",
    ])
    .then_run(&[
        "--", "kurama", "api", "pets", "/pets", "-d", "{}", "--jq", ".",
    ])
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "pets/list",
        "-d",
        "{}",
        "--jq",
        ".",
    ]));
    let runs = v.observed.runs.clone();
    for (i, run) in runs.iter().enumerate() {
        let expected = if i == 4 {
            "\t.content:array\n\t.page:integer"
        } else {
            "\t.created:boolean"
        };
        v.check(
            &format!("run {i} uses the request's effective method"),
            run.stdout == expected,
            run.stdout.clone(),
        );
    }
    v.finish();
}
#[test]
fn completion_jq_uses_the_json_response_envelope() {
    let config = COMPLETION_CONFIG.replace("petstore.json", "responses.json");
    let mut v = run(Scenario::completion(
        "completion_jq_uses_the_json_response_envelope",
        &[
            "--",
            "kurama",
            "api",
            "pets",
            "pets/list",
            "--json",
            "--jq",
            ".",
        ],
        "6",
    )
    .with_config(&config)
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "pets/list",
        "--json",
        "--jq",
        ".body.content[].na",
    ]));
    let runs = v.observed.runs.clone();
    v.check(
        "--json starts at the envelope",
        runs[0].stdout == "\t.status:integer\n\t.headers:object\n\t.body:object",
        runs[0].stdout.clone(),
    );
    v.check(
        "the response schema is below body",
        runs[1].stdout == "\t.body.content[].name:string",
        runs[1].stdout.clone(),
    );
    v.finish();
}
#[test]
fn completion_jq_declines_missing_or_unknown_schemas() {
    let config = format!(
        "{}\n[api.no_spec]\nbase_url = \"{{server}}/api\"\n",
        COMPLETION_CONFIG.replace("petstore.json", "jq-completion.json")
    );
    let mut v = run(Scenario::completion(
        "completion_jq_declines_missing_or_unknown_schemas",
        &["--", "kurama", "api", "no_spec", "/pets", "--jq", "."],
        "5",
    )
    .with_config(&config)
    .then_run(&["--", "kurama", "api", "remote", "pets/list", "--jq", "."])
    .then_run(&["--", "kurama", "api", "pets", "missing", "--jq", "."])
    .then_run(&["--", "kurama", "api", "pets", "/unknown", "--jq", ""])
    .then_run(&["--", "kurama", "api", "pets", "/dictionary", "--jq", "."])
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "pets/list",
        "--jq",
        ".content | map({x: .name}) | .",
    ]));
    let runs = v.observed.runs.clone();
    for (i, run) in runs.iter().enumerate() {
        v.check(
            &format!("run {i} has no known response fields"),
            run.stdout.is_empty(),
            run.stdout.clone(),
        );
    }
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_jq_continues_paths_and_preserves_quoted_pipelines() {
    let id = "completion_zsh_jq_continues_paths_and_preserves_quoted_pipelines";
    let config = COMPLETION_CONFIG.replace("petstore.json", "jq-completion.json");
    let mut t = tui::launch_zsh(
        Scenario::tui(id)
            .for_feature("shell-integration")
            .with_config(&config),
        tui::STANDARD,
    );
    t.type_text("kurama api pets pets/list --jq .con")
        .key(Key::Tab)
        .expect_command_line("kurama api pets pets/list --jq .content")
        .snapshot("array-no-space");
    for quote in ['\'', '"'] {
        t.key(Key::Ctrl('u'))
            .type_text(&format!(
                "kurama api pets pets/list --jq {quote}.content[].na{quote}"
            ))
            .key(Key::Tab)
            .expect_command_line(&format!(
                "kurama api pets pets/list --jq {quote}.content[].name{quote}"
            ));
        t.key(Key::Ctrl('u'))
            .type_text(&format!(
                "kurama api pets pets/list --jq {quote}.content[] | select(.na{quote}"
            ))
            .key(Key::Tab)
            .expect_command_line(&format!(
                "kurama api pets pets/list --jq {quote}.content[] | select(.name{quote}"
            ));
    }
    t.snapshot("quoted-pipeline");
    t.key(Key::Ctrl('u'))
        .type_text("kurama api pets pets/list --jq '.content[0].own'")
        .key(Key::Tab)
        .expect_command_line("kurama api pets pets/list --jq '.content[0].owner'")
        .key(Key::Left)
        .type_text(".")
        .key(Key::Tab)
        .expect_command_line("kurama api pets pets/list --jq '.content[0].owner.active'")
        .snapshot("object-continuation");
    t.key(Key::Ctrl('u'))
        .type_text(r#"kurama api pets /odd --jq '.["a:"#)
        .key(Key::Tab)
        .expect_command_line(r#"kurama api pets /odd --jq '.["a:b"]"#);
    let mut v = t.exit_zsh();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_api_calls(0, None)
        .expect_run_spec_calls(0, 0, false, 0);
    let files = v.observed.files_written.clone();
    v.check(
        "completion leaves the harness setup unchanged",
        files.is_empty(),
        format!("{files:?}"),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_jq_keeps_an_unfinished_expression_open() {
    let id = "completion_zsh_jq_keeps_an_unfinished_expression_open";
    let config = COMPLETION_CONFIG.replace("petstore.json", "responses.json");
    let mut t = tui::launch_zsh(
        Scenario::tui(id)
            .for_feature("shell-integration")
            .with_config(&config),
        tui::STANDARD,
    );
    for quote in ['\'', '"'] {
        let prefix = format!("kurama api pets pets/list --jq {quote}.content[] | select(.name");
        t.key(Key::Ctrl('u'))
            .type_text(&format!(
                "kurama api pets pets/list --jq {quote}.content[] | select(.na"
            ))
            .key(Key::Tab)
            .expect_command_line(&prefix)
            .type_text(&format!(" != null){quote}"))
            .expect_command_line(&format!("{prefix} != null){quote}"));
    }
    t.snapshot("unfinished-expression");
    let mut v = t.exit_zsh();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_api_calls(0, None)
        .expect_run_spec_calls(0, 0, false, 0);
    let files = v.observed.files_written.clone();
    v.check(
        "completion leaves the harness setup unchanged",
        files.is_empty(),
        format!("{files:?}"),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_applying_jq_keeps_events_responsive() {
    let id = "tui_explorer_applying_jq_keeps_events_responsive";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_fake_tools()
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    // The preview stops at its first rows at once; applied, the filter
    // walks a trillion numbers for its last one.
    t.wait_for("Operations (5)")
        .key(Key::Enter)
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .key(Key::Char('j'))
        .wait_for("┏ jq filter ")
        .type_text("range(1e12) | select(. < 9 or . == 1e11)")
        .wait_for("preview (3 shown)")
        .key(Key::Enter)
        .wait_for("jq: running range(1e12)")
        .snapshot("jq-running")
        .key(Key::Char('h'))
        .wait_for("content-type: application/json")
        .wait_for("jq: the filter ran longer than 5 seconds and was stopped")
        .snapshot("jq-stopped")
        .key(Key::Char('q'));
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_api_calls(1, None)
        .expect_op_calls(0)
        .expect_sts_actions(&[]);
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_jq_completes_from_the_response_and_previews() {
    let id = "tui_explorer_jq_completes_from_the_response_and_previews";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_fake_tools()
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Enter)
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .key(Key::Char('j'))
        .wait_for("┏ jq filter ")
        .type_text(".")
        .key(Key::Tab)
        .wait_for("Complete (1/5)")
        .expect_row_with(&["login", "string", "octocat"])
        .snapshot("jq-completing")
        .key(Key::Esc)
        .key(Key::Ctrl('u'))
        .type_text(".log")
        .key(Key::Tab)
        .wait_for("Filter  .login▏")
        .wait_for("preview (1 shown)")
        .expect_text("octocat")
        .snapshot("jq-preview")
        .key(Key::Enter)
        .wait_for("jq: .login")
        .wait_until("the filter's lines arrive", |screen| {
            !screen.contains("jq: running")
        })
        .key(Key::Char('c'))
        .wait_for("command copied to the clipboard")
        .key(Key::Char('j'))
        .key(Key::F1)
        .wait_for("Examples (1/")
        .snapshot("jq-examples")
        .key(Key::Enter)
        .wait_for("Filter  keys▏")
        .wait_for("preview (1 shown)")
        .key(Key::Enter)
        .wait_for("jq: keys")
        .wait_until("the filter's lines arrive", |screen| {
            !screen.contains("jq: running")
        })
        .expect_row_with(&["authorization", "login", "path"])
        .snapshot("jq-applied")
        .key(Key::Char('c'))
        .wait_for("command copied to the clipboard")
        .key(Key::Char('q'));
    let mut v = t.exit_then_run(&[&["api", "pets", "pets/list", "--jq", "keys"]]);
    v.expect_exit_code(0)
        .expect_api_calls(1, None)
        .expect_token_grants(&[])
        .expect_op_calls(0)
        .expect_sts_actions(&[]);
    v.check(
        "completion and example filters are copied without changes",
        v.observed.runs[0].clipboard_calls
            == [
                "kurama api pets pets/list --jq .login",
                "kurama api pets pets/list --jq keys",
            ],
        format!("{:?}", v.observed.runs[0].clipboard_calls),
    );
    v.check(
        "the explorer makes exactly one unauthenticated request",
        v.observed.runs[0].api_calls.len() == 1
            && v.observed.runs[0].api_calls[0].authorization.is_none(),
        "",
    );
    let output: serde_json::Value = serde_json::from_str(&v.observed.runs[1].stdout).unwrap();
    v.check(
        "the copied example runs through the CLI with the same keys",
        output == serde_json::json!(["authorization", "body", "login", "method", "path"]),
        output.to_string(),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_jq_disables_live_preview_for_an_oversized_response() {
    let id = "tui_explorer_jq_disables_live_preview_for_an_oversized_response";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_fake_tools()
            .with_extra_config(PETSTORE_FILE_CONFIG)
            .api(ApiFake::Padding(1024 * 1024)),
        tui::SMALL,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Enter)
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .key(Key::Char('j'))
        .wait_for("Preview disabled: response exceeds 1 MiB.")
        .type_text(".login")
        .expect_text("Preview disabled: response exceeds 1 MiB.")
        .expect_no_text("preview (1 shown)")
        .snapshot("jq-preview-disabled")
        .key(Key::Enter)
        .wait_for("jq: .login")
        .wait_until("the filter's lines arrive", |screen| {
            !screen.contains("jq: running")
        })
        // The title carries the filter as soon as the key is handled; the
        // output arrives when jq has run through the megabyte behind it.
        .wait_until("the jq output replaces the body", |screen| {
            screen.contains("┃ octocat")
        })
        .expect_row_with(&["┃ octocat"])
        .key(Key::Char('q'));
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_api_calls(1, None)
        .expect_token_grants(&[])
        .expect_op_calls(0)
        .expect_sts_actions(&[]);
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_jq_preview_limits_outputs_before_evaluation() {
    let id = "tui_explorer_jq_preview_limits_outputs_before_evaluation";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_config(PETSTORE_FILE_CONFIG)
            .api(ApiFake::Ok),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Enter)
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .key(Key::Char('j'))
        .type_text(r#"range(0; 6) | if . < 4 then . else error("past preview limit") end"#)
        .wait_for("preview (3 shown)")
        .expect_row_with(&["┃  0"])
        .expect_row_with(&["┃  1"])
        .expect_row_with(&["┃  2"])
        .snapshot("bounded-preview")
        .key(Key::Enter)
        .wait_for("┃ jq: ")
        .expect_row_with(&["┃ jq: ", "past preview limit"])
        .snapshot("unbounded-apply")
        .key(Key::Char('q'));
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_api_calls(1, None)
        .expect_token_grants(&[])
        .expect_op_calls(0)
        .expect_sts_actions(&[]);
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_explorer_header_shows_the_token_time_left() {
    // A stored token with 10 minutes left: the header shows it once the
    // description is loaded, read from the store without a grant or a call.
    let mut t = tui::launch_command(
        Scenario::tui("tui_explorer_header_shows_the_token_time_left")
            .for_feature("api-explorer")
            .with_extra_config(GITHUB_SPEC_CONFIG)
            .with_stored_token(
                "github",
                &stored_token_json(STORED_ACCESS_TOKEN, 600, false),
            ),
        tui::STANDARD,
        &["api", "github"],
    );
    t.wait_for("Operations (5)")
        .wait_until("the header shows the token's time left", |screen| {
            let line = screen.lines().next().unwrap_or_default();
            line.contains("token 9m") || line.contains("token 10m")
        })
        .snapshot("token-time-left");
    let mut v = t.quit();
    v.expect_exit_code(0).expect_token_grants(&[]);
    let calls = v.observed.last().api_calls.len();
    v.check("no API call", calls == 0, format!("{calls} calls"));
    v.finish();
}

/// Where the explorer keeps the request history of `[api.pets]`.
const PETS_HISTORY: &str = ".local/state/kurama/history/pets.jsonl";

/// A request sent from the form is listed by `h`, saved under a name with
/// `s`, and the form it opens sends the same request again.
#[test]
fn tui_explorer_history_refills_the_form() {
    let id = "tui_explorer_history_refills_the_form";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_extra_config(PETSTORE_FILE_CONFIG),
        tui::STANDARD,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .expect_footer_contains("h history")
        .key(Key::Down)
        .key(Key::Down)
        .key(Key::Enter)
        .wait_for("┏ pets/get  GET /pets/{petId} ")
        .type_text("p 1")
        .key(Key::Tab)
        .type_text("t1")
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .key(Key::Esc)
        .wait_for("┏ pets/get  GET /pets/{petId} ")
        .key(Key::Esc)
        .wait_for("Operations (5)")
        .key(Key::Char('h'))
        .wait_for("┏ History ")
        .expect_row_with(&["▸ pets/get", "petId=p 1", "X-Trace=t1"])
        .snapshot("history")
        .key(Key::Char('s'))
        .type_text("one pet")
        .expect_text("Name  one pet▏")
        .key(Key::Enter)
        .wait_for("saved as one pet")
        .expect_row_with(&["▸ [one pet] pets/get", "petId=p 1"])
        .snapshot("favorite")
        .key(Key::Enter)
        .wait_for("┏ pets/get  GET /pets/{petId} ")
        .expect_row_with(&["petId*", "p 1"])
        .expect_row_with(&["X-Trace", "t1"])
        .snapshot("refilled")
        .key(Key::Enter)
        .wait_for("HTTP 200 OK");
    let lines = t.home_file(PETS_HISTORY);
    t.check(
        "the history holds the sent requests and the favorite, and no response",
        lines.lines().count() == 3
            && lines
                .lines()
                .filter(|line| line.contains("\"one pet\""))
                .count()
                == 1
            && !lines.contains("octocat")
            && !lines.contains("\"path\""),
        lines.clone(),
    );
    let mut v = t.quit();
    v.expect_exit_code(0);
    let calls = v.observed.last().api_calls.clone();
    v.check(
        "the form opened from the history sent the same request",
        calls.len() == 2
            && calls[0].method == calls[1].method
            && calls[0].path == "/api/pets/p%201"
            && calls[0].path == calls[1].path
            && calls[0].headers == calls[1].headers
            && calls[0].body == calls[1].body,
        format!("{calls:?}"),
    );
    v.check(
        "the history is the only file the explorer wrote",
        v.observed.files_written == [format!("home/{PETS_HISTORY}")],
        format!("{:?}", v.observed.files_written),
    );
    v.finish();
}

/// A credential typed into a header or a query parameter is sent, and never
/// written to the history; the other values are.
#[test]
fn tui_explorer_history_keeps_no_secret() {
    let id = "tui_explorer_history_keeps_no_secret";
    let mut t = tui::launch_command(
        Scenario::tui(id).for_feature("api-explorer").with_extra_config(
            "[api.keys]\nbase_url = \"{server}/api\"\nopenapi = \"{fixtures}/secret-params.json\"\n",
        ),
        tui::STANDARD,
        &["api", "keys"],
    );
    t.wait_for("Operations (1)")
        .key(Key::Enter)
        .wait_for("┏ keys/get  GET /keys/{keyId} ")
        .type_text("k1")
        .key(Key::Tab)
        .type_text(GRANTED_ACCESS_TOKEN)
        .key(Key::Tab)
        .type_text(REFRESHED_ACCESS_TOKEN)
        .key(Key::Tab)
        .type_text("find")
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .key(Key::Esc)
        .key(Key::Esc)
        .wait_for("Operations (1)")
        .key(Key::Char('h'))
        .wait_for("┏ History ")
        .expect_row_with(&["▸ keys/get", "keyId=k1", "q=find"])
        .expect_no_text("X-Api-Key=")
        .expect_no_text("access_token=")
        .snapshot("history");
    let history = t.home_file(".local/state/kurama/history/keys.jsonl");
    t.check(
        "the history line keeps the plain values and neither credential",
        history.contains("\"keyId\"")
            && history.contains("\"find\"")
            && !history.contains(GRANTED_ACCESS_TOKEN)
            && !history.contains(REFRESHED_ACCESS_TOKEN)
            && !history.contains("X-Api-Key")
            && !history.contains("access_token"),
        history.clone(),
    );
    let mut v = t.quit();
    v.expect_exit_code(0);
    let calls = v.observed.last().api_calls.clone();
    v.check(
        "the request itself carried both credentials",
        calls.len() == 1
            && calls[0].path.contains(REFRESHED_ACCESS_TOKEN)
            && calls[0].headers.get("x-api-key").map(String::as_str) == Some(GRANTED_ACCESS_TOKEN),
        format!("{calls:?}"),
    );
    v.finish();
}

/// `t` shows the response as a tree; the path `y` copies from a node is a
/// jq filter, and `j` applied on it prints that node's value.
#[test]
fn tui_explorer_tree_copies_the_path_of_a_node() {
    let id = "tui_explorer_tree_copies_the_path_of_a_node";
    let mut t = tui::launch_command(
        Scenario::tui(id)
            .for_feature("api-explorer")
            .with_fake_tools()
            .with_extra_config(PETSTORE_FILE_CONFIG)
            .api(ApiFake::CompactArray),
        tui::SMALL,
        &["api", "pets"],
    );
    t.wait_for("Operations (5)")
        .key(Key::Enter)
        .wait_for("┏ pets/list  GET /pets ")
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .expect_footer_contains("t tree")
        .key(Key::Char('t'))
        .wait_for("tree ━")
        .expect_row_with(&["▾ [1]"])
        .expect_row_with(&["▸ [0] {3}"])
        .key(Key::Down)
        .key(Key::Right)
        .expect_row_with(&["▾ [0] {3}"])
        .key(Key::Down)
        .expect_row_with(&["login: \"octocat\""])
        .expect_footer_contains("y copy path")
        .snapshot("tree")
        .key(Key::Char('y'))
        .wait_for(".[0].login copied to the clipboard")
        .key(Key::Char('j'))
        .wait_for("┏ jq filter ")
        .expect_text("Filter  .[0].login▏")
        .key(Key::Enter)
        .wait_until("the filter's lines arrive", |screen| {
            screen.contains("jq: .[0].login") && !screen.contains("jq: running")
        })
        .expect_row_with(&["┃ octocat"])
        .expect_no_text("amount")
        .snapshot("filtered");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_files_written(&[&format!("home/{PETS_HISTORY}")]);
    let run = v.observed.last().clone();
    v.check(
        "y copied the node's jq path and nothing else",
        run.clipboard_calls == [".[0].login"],
        format!("{:?}", run.clipboard_calls),
    );
    v.check(
        "the tree sent nothing: one request, the one the form sent",
        run.api_calls.len() == 1 && run.api_calls[0].path == "/api/pets",
        format!("{:?}", run.api_calls),
    );
    v.finish();
}

/// A GraphQL API whose schema was introspected once: completion reads the
/// cached result as it reads a cached description, so it offers the
/// operations while the endpoint is down and sends no introspection, reads
/// no credential and runs no grant; without a cached result it is silent.
#[test]
fn api_graphql_introspection_is_cached_and_completion_calls_nothing() {
    const GRAPHQL_CONFIG: &str = "\
[auth.linear]
kind = \"token\"
token = \"op://Agent/linear/credential\"
[api.linear]
base_url = \"{server}\"
graphql = \"/spec/graphql-linear.json\"
[api.uncached]
base_url = \"{server}\"
graphql = \"/spec/graphql-uncached.json\"
";
    let mut v = run(Scenario::completion(
        "api_graphql_introspection_is_cached_and_completion_calls_nothing",
        &["--", "kurama", "api", "linear", "query."],
        "3",
    )
    .with_config(GRAPHQL_CONFIG)
    .spec(SpecFake::Down)
    .with_seeded_spec_cache("/spec/graphql-linear.json", "graphql-linear.json", 86400)
    .then_run(&["--", "kurama", "api", "linear", "mut"])
    .then_run(&["--", "kurama", "api", "uncached", "query."]));
    let runs = v.observed.runs.clone();
    v.check(
        "the cached introspection result completes the query operations",
        completion_values(&runs[0].stdout)
            == [
                "query.attachmentIssue",
                "query.issue",
                "query.issues",
                "query.viewer",
            ],
        runs[0].stdout.clone(),
    );
    v.check(
        "the mutations complete too",
        completion_values(&runs[1].stdout) == ["mutation.issueCreate"],
        runs[1].stdout.clone(),
    );
    v.check(
        "an endpoint never introspected completes nothing",
        runs[2].stdout.is_empty(),
        runs[2].stdout.clone(),
    );
    v.expect_run_spec_calls(0, 0, false, 0)
        .expect_run_spec_calls(1, 0, false, 0)
        .expect_run_spec_calls(2, 0, false, 0)
        .expect_op_calls(0);
    v.finish();
}
