//! Runtime verification scenarios of the `tui` feature.
//! `tests/scenarios/main.rs` holds the naming rule and what each
//! scenario writes.

use crate::support::tui::{self, Key};
use crate::support::{
    OnePassword, RESULT_ACCESS_KEY, SIGNIN_TOKEN, SOURCE_ACCESS_KEY, STORED_ACCESS_TOKEN, Scenario,
    StsFake, stored_token_json,
};

/// Full-width names and a name wider than the table, for width handling.
const UNICODE_AWS_CONFIG: &str = "\
[default]
region = us-east-1

[profile 本番環境-管理者]
role_arn = arn:aws:iam::123456789012:role/Admin
source_profile = default
mfa_serial = arn:aws:iam::123456789012:mfa/agent

[profile 開発環境]
role_arn = arn:aws:iam::123456789012:role/Dev
source_profile = default

[profile platform-engineering-production-administrator-eu-central-1-very-long-profile-name]
role_arn = arn:aws:iam::123456789012:role/organization-wide-platform-engineering-administrator
source_profile = default
region = eu-central-1
";
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_home_lists_profiles_with_session_state_and_quits() {
    let id = "tui_home_lists_profiles_with_session_state_and_quits";
    // `login` caches an MFA session first, so the SESSION column has a value,
    // and KURAMA_AWS marks `dev` as the profile this shell holds.
    let mut t = tui::launch(
        Scenario::tui(id)
            .onepassword(OnePassword::Enabled)
            .with_session_cache()
            .with_env("KURAMA_AWS", "dev")
            .then_run(&["login", "ops-mfa"]),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .snapshot("home")
        .expect_row_with(&["KIND", "PROFILE", "SESSION"])
        .expect_row_with(&["▸", "aws", "default"])
        .expect_row_with(&["*", "aws", "dev"])
        .expect_row_with(&["ops-mfa", "valid ("])
        .expect_text("┏ Details ")
        .expect_text("Shell   not active")
        .expect_footer_contains("↑↓ move")
        .expect_footer_contains("q quit");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_run_sts_actions(0, &["GetSessionToken"])
        .expect_run_sts_actions(1, &[])
        .expect_op_calls(0);
    let dir = tui::artifact_dir(id).join("home");
    let text = tui::read_screen(&dir);
    let ansi = std::fs::read(dir.join("screen.ansi")).unwrap_or_default();
    let png = std::fs::read(dir.join("screen.png")).unwrap_or_default();
    v.check(
        "screen.txt, screen.ansi and screen.png were written for the home step",
        text.lines().count() == 40
            && ansi.windows(2).any(|w| w == b"\x1b[")
            && png.starts_with(b"\x89PNG"),
        format!(
            "txt lines {}, ansi bytes {}, png bytes {}",
            text.lines().count(),
            ansi.len(),
            png.len()
        ),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_header_shows_the_session_time_left() {
    // `login` caches an MFA session for ops-mfa: selecting it puts the
    // session's time left in the header; a profile without an MFA device
    // shows none. Reading the cache is all it does: no STS call.
    let mut t = tui::launch(
        Scenario::tui("tui_header_shows_the_session_time_left")
            .onepassword(OnePassword::Enabled)
            .with_session_cache()
            .then_run(&["login", "ops-mfa"]),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)");
    let header = |screen: &str| screen.lines().next().unwrap_or_default().to_string();
    let first = header(&t.screen_text());
    t.check(
        "no time in the header for default",
        !first.contains("MFA"),
        first,
    )
    .key(Key::End)
    .expect_row_with(&["▸", "aws", "ops-mfa"])
    .wait_until("the header shows the MFA session's time left", |screen| {
        let line = screen.lines().next().unwrap_or_default();
        line.split_once("MFA ").is_some_and(|(_, left)| {
            left.starts_with(|c: char| c.is_ascii_digit()) && left.contains("m   readonly")
        })
    })
    .snapshot("ops-mfa");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_run_sts_actions(0, &["GetSessionToken"])
        .expect_run_sts_actions(1, &[])
        .expect_op_calls(0);
    v.finish();
}
/// One source of every tab: an OAuth source with its API, the SQLite
/// fixture and a local data workspace.
const SOURCE_TABS_CONFIG: &str = concat!(
    "[auth.github]\nkind = \"oauth\"\ngrant_type = \"authorization_code\"\n",
    "auth_url = \"{server}/oauth/authorize\"\ntoken_url = \"{server}/oauth/token\"\n",
    "client_id = \"gh-client\"\n\n",
    "[api.github]\nbase_url = \"{server}/api\"\nauth = \"github\"\n",
    "openapi = \"{fixtures}/petstore.json\"\n\n",
    "[db.app]\nengine = \"sqlite\"\npath = \"",
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/db/app.sqlite3\"\n\n",
    "[data.events]\nroot_dir = \"",
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/data\"\n[[data.events.sources]]\nname = \"events\"\npath = \"events.jsonl\"\n",
);
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_home_switches_between_the_source_tabs() {
    // Every tab shows its rows with the facts `kurama status` reports, and
    // switching between them calls nothing: no STS, no 1Password, no token
    // endpoint, no API.
    let mut t = tui::launch(
        Scenario::tui("tui_home_switches_between_the_source_tabs")
            .with_extra_config(SOURCE_TABS_CONFIG)
            .with_stored_token(
                "github",
                &stored_token_json(STORED_ACCESS_TOKEN, 3600, false),
            ),
        tui::SMALL,
    );
    t.wait_for("Profiles (3)")
        .expect_text("1 AWS   2 Auth   3 API   4 DB   5 Data   6 S3")
        .key(Key::Char('2'))
        .wait_for("Auth sources (1)")
        .expect_row_with(&["▸", "auth", "github", "valid ("])
        .expect_footer_contains("Enter login")
        .snapshot("auth")
        .key(Key::Char('3'))
        .wait_for("APIs (1)")
        .expect_row_with(&["▸", "api", "github", "valid ("])
        .expect_footer_contains("Enter explore")
        .snapshot("api")
        .key(Key::Char('\t'))
        .wait_for("Databases (1)")
        .expect_row_with(&["▸", "db", "app", "unknown"])
        .snapshot("db")
        .key(Key::Char('5'))
        .wait_for("Data workspaces (1)")
        .expect_row_with(&["▸", "data", "events", "not_checked"])
        .expect_footer_contains("Enter copy")
        .snapshot("data")
        .resize(tui::STANDARD)
        .wait_for("┏ Details ")
        .expect_text("Next        Enter copies kurama data events")
        .snapshot("data-detail")
        .key(Key::Char('\t'))
        .wait_for("S3 connections (0)")
        .expect_text("No [s3.<name>] section in config.toml.")
        .key(Key::Char('\t'))
        .wait_for("Profiles (3)")
        .expect_row_with(&["▸", "aws", "default"]);
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_token_grants(&[]);
    let calls = v.observed.last().api_calls.len();
    v.check("no API call", calls == 0, format!("{calls} calls"));
    v.finish();
}
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_home_api_tab_opens_the_explorer() {
    // Enter on an API leaves the home screen for its explorer, as
    // `kurama api github` would open it; quitting the explorer comes back
    // to the home screen on the API tab.
    let mut t = tui::launch(
        Scenario::tui("tui_home_api_tab_opens_the_explorer")
            .with_extra_config(SOURCE_TABS_CONFIG)
            .with_stored_token(
                "github",
                &stored_token_json(STORED_ACCESS_TOKEN, 3600, false),
            ),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .key(Key::Char('3'))
        .wait_for("APIs (1)")
        .expect_text("Next        Enter opens the explorer")
        .key(Key::Enter)
        .wait_for("Operations (5)")
        .expect_text("kurama api")
        .snapshot("explorer")
        .key(Key::Char('q'))
        .wait_for("APIs (1)")
        .expect_row_with(&["▸", "api", "github"])
        .snapshot("back-home");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_token_grants(&[]);
    let calls = v.observed.last().api_calls.len();
    v.check("no API call", calls == 0, format!("{calls} calls"));
    v.finish();
}
#[test]
fn tui_no_color_preserves_selection_and_badge_emphasis() {
    let mut t = tui::launch(
        Scenario::tui("tui_no_color_preserves_selection_and_badge_emphasis")
            .with_env("NO_COLOR", "1"),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .expect_default_colors()
        .expect_bold_text("kurama")
        .expect_bold_text("▸")
        .key(Key::Char('r'))
        .expect_default_colors()
        .expect_bold_text("readonly")
        .snapshot("normal")
        .resize(tui::SMALL)
        .expect_default_colors()
        .expect_bold_text("▸")
        .expect_footer_contains("q quit")
        .snapshot("small")
        .resize(tui::WIDE)
        .expect_default_colors()
        .expect_bold_text("▸")
        .snapshot("wide");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
#[test]
fn tui_empty_no_color_keeps_the_palette() {
    let mut t = tui::launch(
        Scenario::tui("tui_empty_no_color_keeps_the_palette").with_env("NO_COLOR", ""),
        tui::SMALL,
    );
    t.wait_for("Profiles (3)")
        .expect_colors()
        .expect_bold_text("▸")
        .snapshot("home");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
#[test]
fn tui_dumb_terminal_exits_2_without_control_sequences() {
    let mut t = tui::launch(
        Scenario::tui("tui_dumb_terminal_exits_2_without_control_sequences")
            .with_env("TERM", "dumb")
            .with_env("RUST_LOG", "kurama=debug"),
        tui::WIDE,
    );
    t.wait_for("error[TERMINAL_REQUIRED]")
        .expect_text("DEBUG Starting kurama")
        .expect_text("hint: run `kurama status`")
        .expect_plain_output()
        .snapshot("unsupported");
    let mut v = t.exit();
    v.expect_error("TERMINAL_REQUIRED", 2)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
/// A failure after raw mode and the alternate screen were entered, before
/// the screen exists, still gives the shell its terminal back.
#[test]
fn tui_terminal_initialization_failure_restores_terminal() {
    let mut t = tui::launch(
        Scenario::tui("tui_terminal_initialization_failure_restores_terminal")
            .with_env("KURAMA_TEST_TERMINAL_FAILS_AT", "terminal"),
        tui::SMALL,
    );
    t.wait_for("error[")
        .expect_text("injected failure at terminal")
        .expect_restored_terminal()
        .snapshot("failed");
    let mut v = t.exit();
    v.expect_exit_code(1)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_no_files_written();
    v.finish();
}
#[test]
fn tui_navigation_updates_the_detail_pane() {
    let mut t = tui::launch(
        Scenario::tui("tui_navigation_updates_the_detail_pane"),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .key(Key::Down)
        .expect_row_with(&["▸", "aws", "dev"])
        .expect_text("Role    arn:aws:iam::123456789012:role/Dev")
        .expect_text("Region  ap-northeast-1")
        .snapshot("dev-selected")
        .key(Key::End)
        .expect_row_with(&["▸", "aws", "ops-mfa"])
        .expect_text("MFA     arn:aws:iam::123456789012:mfa/agent")
        .expect_text("Next    Enter asks for the MFA code")
        .snapshot("ops-mfa-selected")
        .key(Key::Home)
        .expect_row_with(&["▸", "aws", "default"])
        .key(Key::Char('j'))
        .expect_row_with(&["▸", "aws", "dev"])
        .key(Key::Up)
        .expect_row_with(&["▸", "aws", "default"])
        .key(Key::PageDown)
        .expect_row_with(&["▸", "aws", "ops-mfa"])
        .key(Key::PageUp)
        .expect_row_with(&["▸", "aws", "default"]);
    let mut v = t.quit();
    v.expect_exit_code(0).expect_sts_actions(&[]);
    v.finish();
}
#[test]
fn tui_resize_keeps_the_selection_and_switches_panes() {
    let mut t = tui::launch(
        Scenario::tui("tui_resize_keeps_the_selection_and_switches_panes"),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .key(Key::Down)
        .expect_row_with(&["▸", "aws", "dev"])
        .expect_text("┏ Details ")
        .snapshot("120x40")
        .resize(tui::SMALL)
        .wait_until("the detail pane disappears", |screen| {
            !screen.contains("┏ Details ")
        })
        .expect_row_with(&["▸", "aws", "dev"])
        .expect_footer_contains("q quit")
        .snapshot("80x24")
        .resize(tui::WIDE)
        .wait_for("┏ Details ")
        .expect_row_with(&["▸", "aws", "dev"])
        .expect_text("Role    arn:aws:iam::123456789012:role/Dev")
        .expect_footer_contains("? help")
        .snapshot("160x50");
    let mut v = t.quit();
    v.expect_exit_code(0).expect_sts_actions(&[]);
    for step in ["120x40", "80x24", "160x50"] {
        let text = tui::read_screen(
            &tui::artifact_dir("tui_resize_keeps_the_selection_and_switches_panes").join(step),
        );
        let rows = step.split('x').nth(1).unwrap().parse::<usize>().unwrap();
        v.check(
            &format!("{step}: screen.txt has {rows} rows and every row fits the width"),
            text.lines().count() == rows
                && text.lines().all(|line| {
                    unicode_width::UnicodeWidthStr::width(line)
                        <= step.split('x').next().unwrap().parse().unwrap()
                }),
            format!("rows {}", text.lines().count()),
        );
    }
    v.finish();
}
#[test]
fn tui_search_filters_profiles_and_esc_clears() {
    let mut t = tui::launch(
        Scenario::tui("tui_search_filters_profiles_and_esc_clears"),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .key(Key::Char('/'))
        .expect_footer_contains("Esc clear")
        .type_text("ops")
        .wait_for("Profiles 1/3  /ops_")
        .expect_row_with(&["▸", "aws", "ops-mfa"])
        .expect_no_text("aws  dev")
        .snapshot("filtered")
        .type_text("zzz")
        .wait_for("No profile matches \"opszzz\"")
        .snapshot("empty")
        .key(Key::Backspace)
        .key(Key::Backspace)
        .key(Key::Backspace)
        .wait_for("Profiles 1/3  /ops_")
        .expect_row_with(&["▸", "aws", "ops-mfa"])
        .key(Key::Esc)
        .wait_for("Profiles (3)")
        .expect_row_with(&["▸", "aws", "default"])
        .expect_row_with(&["aws", "dev"]);
    let mut v = t.quit();
    v.expect_exit_code(0).expect_sts_actions(&[]);
    v.finish();
}
#[test]
fn tui_enter_assumes_the_role_and_hands_credentials_to_the_wrapper() {
    let mut t = tui::launch(
        Scenario::tui("tui_enter_assumes_the_role_and_hands_credentials_to_the_wrapper")
            .with_env_script(),
        tui::STANDARD,
    );
    // `r` first: the readonly toggle must reach the AssumeRole call and the
    // export script, like `kurama env --readonly` does.
    t.wait_for("Profiles (3)")
        .key(Key::Down)
        .expect_row_with(&["▸", "aws", "dev"])
        .key(Key::Char('r'))
        .key(Key::Enter)
        .wait_for("┏ Success ")
        .expect_text("Profile     dev")
        .expect_text(&format!("Access key  {RESULT_ACCESS_KEY}"))
        .expect_footer_contains("Enter exit")
        .snapshot("success")
        .key(Key::Enter);
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_env_script_exports(&[
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_READONLY_SESSION",
            "KURAMA_AWS",
        ]);
    let call = v.observed.last().sts_calls[0].clone();
    v.check(
        "AssumeRole targets the selected profile's role with the source key and the ReadOnlyAccess policy",
        call.role_arn.as_deref() == Some("arn:aws:iam::123456789012:role/Dev")
            && call.signing_access_key.as_deref() == Some(SOURCE_ACCESS_KEY)
            && call.policy_arns == ["arn:aws:iam::aws:policy/ReadOnlyAccess"],
        format!("{call:?}"),
    );
    v.finish();
}
#[test]
fn tui_console_toggle_generates_and_opens_federated_session() {
    let mut t = tui::launch(
        Scenario::tui("tui_console_toggle_generates_and_opens_federated_session")
            .with_env_script()
            .with_fake_browser(),
        tui::STANDARD,
    );
    // `c` turns console launch on: the assumed role's credentials go to the
    // federation endpoint once and the browser opens the login URL once,
    // like `kurama console`, and the shell still receives the credentials.
    t.wait_for("Profiles (3)")
        .key(Key::Down)
        .expect_row_with(&["▸", "aws", "dev"])
        .key(Key::Char('c'))
        .key(Key::Enter)
        .wait_for("┏ Success ")
        .expect_text("Profile     dev")
        .snapshot("success")
        .key(Key::Enter);
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_federation_calls(&[("getSigninToken", RESULT_ACCESS_KEY)])
        .expect_open_calls_containing(&[&["Action=login", &format!("SigninToken={SIGNIN_TOKEN}")]])
        .expect_env_script_exports(&["AWS_ACCESS_KEY_ID", "KURAMA_AWS"]);
    v.finish();
}
#[test]
fn tui_console_browser_failure_shows_an_error_and_keeps_the_credentials() {
    let mut t = tui::launch(
        Scenario::tui("tui_console_browser_failure_shows_an_error_and_keeps_the_credentials")
            .with_env_script()
            .with_fake_browser()
            .with_env("KURAMA_FAKE_OPEN_EXIT", "1"),
        tui::STANDARD,
    );
    // The URL opener exits 1 after the role was assumed: the TUI stays up
    // and says what failed, and quitting hands the credentials to the shell
    // and prints the URL, whole, to open by hand.
    t.wait_for("Profiles (3)")
        .key(Key::Down)
        .key(Key::Char('c'))
        .key(Key::Enter)
        .wait_for("┏ Error ")
        .expect_text("Failed to open the browser: `open` exited")
        .expect_text("and prints it.")
        .expect_text("Credentials are ready for the shell.")
        .snapshot("browser_failed")
        .key(Key::Enter)
        .wait_for("Profiles (3)")
        .expect_no_text("┏ Error ");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_federation_calls(&[("getSigninToken", RESULT_ACCESS_KEY)])
        .expect_open_calls_containing(&[&["Action=login", &format!("SigninToken={SIGNIN_TOKEN}")]])
        .expect_env_script_exports(&["AWS_ACCESS_KEY_ID", "KURAMA_AWS"]);
    let after = v.observed.last().stdout.replace(['\r', '\n'], "");
    v.check(
        "the terminal after the TUI shows the console URL to open",
        after.contains("# Console URL: https://signin.aws.amazon.com/federation?Action=login")
            && after.contains(&format!("SigninToken={SIGNIN_TOKEN}")),
        after.clone(),
    );
    v.finish();
}
#[test]
fn tui_mfa_prompt_accepts_a_code_and_assumes_the_role() {
    let mut t = tui::launch(
        Scenario::tui("tui_mfa_prompt_accepts_a_code_and_assumes_the_role").with_env_script(),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .key(Key::End)
        .expect_row_with(&["▸", "aws", "ops-mfa"])
        .key(Key::Enter)
        .wait_for("┏ MFA code ")
        .expect_text("Profile     ops-mfa")
        .expect_text("Device      arn:aws:iam::123456789012:mfa/agent")
        .expect_text("Code        ______")
        .expect_footer_contains("Enter submit")
        .snapshot("prompt")
        .type_text("123456")
        .expect_text("Code        ******")
        .snapshot("typed")
        .key(Key::Enter)
        .wait_for("┏ Success ")
        .expect_text("Profile     ops-mfa")
        .snapshot("success")
        .key(Key::Enter);
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_op_calls(0)
        .expect_env_script_exports(&["AWS_ACCESS_KEY_ID", "KURAMA_AWS"]);
    let call = v.observed.last().sts_calls[0].clone();
    v.check(
        "AssumeRole carries the typed code and the profile's MFA device",
        call.token_code.as_deref() == Some("123456")
            && call.serial_number.as_deref() == Some("arn:aws:iam::123456789012:mfa/agent"),
        format!("{call:?}"),
    );
    v.finish();
}
#[test]
fn tui_incomplete_mfa_code_does_not_submit() {
    let mut t = tui::launch(
        Scenario::tui("tui_incomplete_mfa_code_does_not_submit").with_env_script(),
        tui::STANDARD,
    );
    // Enter on five digits keeps the prompt and calls nothing; the sixth
    // digit makes the next Enter send the code, once.
    t.wait_for("Profiles (3)")
        .key(Key::End)
        .expect_row_with(&["▸", "aws", "ops-mfa"])
        .key(Key::Enter)
        .wait_for("┏ MFA code ")
        .type_text("12345")
        .expect_text("Code        *****_")
        .key(Key::Enter)
        .expect_text("┏ MFA code ")
        .expect_text("Code        *****_")
        .expect_footer_contains("Enter submit")
        .expect_no_text("┏ Success ")
        .snapshot("five-digits-after-enter")
        .type_text("6")
        .expect_text("Code        ******")
        .key(Key::Enter)
        .wait_for("┏ Success ")
        .expect_text("Profile     ops-mfa")
        .key(Key::Enter);
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_op_calls(0)
        .expect_env_script_exports(&["AWS_ACCESS_KEY_ID", "KURAMA_AWS"]);
    let calls = v.observed.last().sts_calls.clone();
    v.check(
        "the one AssumeRole carries the six typed digits",
        calls.len() == 1 && calls[0].token_code.as_deref() == Some("123456"),
        format!("{calls:?}"),
    );
    v.finish();
}
#[test]
fn tui_error_modal_returns_to_the_list() {
    let mut t = tui::launch(
        Scenario::tui("tui_error_modal_returns_to_the_list").sts(StsFake::Error {
            code: "AccessDenied",
            message: "User is not authorized to perform: sts:AssumeRole",
        }),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .key(Key::Down)
        .key(Key::Enter)
        .wait_for("┏ Error ")
        .expect_text("not authorized to perform: sts:AssumeRole")
        .expect_no_text("WARN")
        .expect_footer_contains("Enter back")
        .snapshot("error")
        .key(Key::Enter)
        .wait_until("the error modal closes", |screen| {
            !screen.contains("┏ Error ")
        })
        .expect_no_text("WARN")
        .expect_row_with(&["▸", "aws", "dev"])
        .expect_footer_contains("q quit")
        .snapshot("back")
        .key(Key::Ctrl('c'));
    let mut v = t.exit();
    v.expect_exit_code(0).expect_sts_actions(&["AssumeRole"]);
    v.finish();
}
#[test]
fn tui_help_modal_and_unicode_names_on_a_small_terminal() {
    let mut t = tui::launch(
        Scenario::tui("tui_help_modal_and_unicode_names_on_a_small_terminal")
            .with_aws_config(UNICODE_AWS_CONFIG),
        tui::SMALL,
    );
    t.wait_for("Profiles (4)")
        .expect_row_with(&["aws", "本番環境-管理者", "cache disabled"])
        .expect_row_with(&["aws", "開発環境", "-"])
        .expect_row_with(&["aws", "platform-engineering-production", "…", "-"])
        .expect_footer_contains("q quit")
        .snapshot("unicode-80x24")
        .key(Key::F1)
        .wait_for("┏ Help ")
        .expect_text("q or Ctrl-C  quit")
        .expect_footer_contains("Esc close")
        .snapshot("help")
        .key(Key::Esc)
        .wait_until("the help modal closes", |screen| {
            !screen.contains("┏ Help ")
        })
        .expect_footer_contains("q quit");
    let mut v = t.quit();
    v.expect_exit_code(0).expect_sts_actions(&[]);
    v.finish();
}
#[test]
fn tui_inherited_aws_env_never_paints_the_screen() {
    // A shell that already exports AWS_* (after `eval "$(kurama env ...)"`)
    // is the normal place to open the TUI. kurama clears the variables at the
    // SDK boundary and reports it; the report must wait until the terminal is
    // restored instead of being painted over the alternate screen.
    let mut t = tui::launch(
        Scenario::tui("tui_inherited_aws_env_never_paints_the_screen")
            .with_env_script()
            .with_env("AWS_ACCESS_KEY_ID", "ASIASTALE")
            .with_env("AWS_SECRET_ACCESS_KEY", "stale-secret")
            .with_env("AWS_SESSION_TOKEN", "stale-token"),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .key(Key::Down)
        .key(Key::Enter)
        .wait_for("┏ Success ")
        .expect_no_text("# Cleared")
        .snapshot("success")
        .key(Key::Enter);
    let mut v = t.exit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_stdout_contains("# Cleared 3 AWS environment variable(s)")
        .expect_env_script_exports(&["AWS_ACCESS_KEY_ID"]);
    let call = v.observed.last().sts_calls[0].clone();
    v.check(
        "AssumeRole is signed with the source profile key, not the inherited one",
        call.signing_access_key.as_deref() == Some(SOURCE_ACCESS_KEY),
        format!("signed with {:?}", call.signing_access_key),
    );
    v.finish();
}

/// `Ctrl-K` finds an operation of a description on disk by part of its
/// name and opens its form in the explorer; a URL description that is not
/// cached lists nothing and is not fetched. Leaving the explorer comes back
/// to the home screen.
#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_palette_finds_an_operation_and_opens_its_form() {
    let mut t = tui::launch(
        Scenario::tui("tui_palette_finds_an_operation_and_opens_its_form").with_extra_config(
            "[api.pets]\nbase_url = \"{server}/api\"\nopenapi = \"{fixtures}/petstore.json\"\n\n\
             [api.remote]\nbase_url = \"{server}/api\"\nopenapi = \"{server}/spec/petstore.json\"\n",
        ),
        tui::SMALL,
    );
    t.wait_for("Profiles (3)")
        .expect_footer_contains("^K go to")
        .key(Key::Ctrl('k'))
        .wait_for("┏ Go to ")
        .wait_for("op      pets/get")
        .type_text("petget")
        .expect_row_with(&["▸ op", "pets/get", "pets  GET /pets/{petId}"])
        .expect_no_text("remote  GET")
        .snapshot("palette")
        .key(Key::Enter)
        .wait_for("┏ pets/get  GET /pets/{petId} ")
        .expect_row_with(&["▸", "petId*"])
        .snapshot("form")
        .type_text("p1")
        .key(Key::Enter)
        .wait_for("HTTP 200 OK")
        .expect_text("\"path\": \"/api/pets/p1\"")
        .key(Key::Char('q'))
        .wait_for("APIs (2)")
        .snapshot("back-home");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_token_grants(&[]);
    let run = v.observed.last().clone();
    v.check(
        "one API call, the form's, and no description fetched",
        run.api_calls.len() == 1
            && run.api_calls[0].path == "/api/pets/p1"
            && run.spec_calls.is_empty(),
        format!("api {:?} spec {:?}", run.api_calls, run.spec_calls),
    );
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_activity_shows_audit_entries_as_they_are_appended() {
    // `kurama audit --watch` shows what the audit log holds, then each call
    // an agent makes while it is open, within a second of the call's end: a
    // write the [agent] policy refused reads `refused`. Neither the query
    // string nor a header the calls carried reaches the screen, `y` copies
    // the command that makes the call again under the API's base path, and
    // the monitor never writes the log it reads. The monitor is the
    // person's: KURAMA_AGENT marks only the calls, since an agent's run
    // opens no screen.
    let mut t = tui::launch_command_for_a_person(
        Scenario::tui("tui_activity_shows_audit_entries_as_they_are_appended")
            .with_extra_config("[api.public]\nbase_url = \"{server}/api\"\n")
            .with_env("KURAMA_AGENT", "1")
            .with_fake_tools()
            .then_run(&["api", "public", "/user?token=query-secret"]),
        tui::STANDARD,
        &["audit", "--watch"],
    );
    t.wait_for("Calls (1)")
        .expect_row_with(&["api", "public", "GET /api/user", "200"])
        .run_beside(&[
            "api",
            "public",
            "/items",
            "-d",
            "{\"name\":\"x\"}",
            "-H",
            "X-Trace: header-secret",
        ]);
    let appended = std::time::Instant::now();
    t.wait_for("Calls (2)");
    let shown_after = appended.elapsed();
    t.check(
        "an appended call is on screen within a second",
        shown_after < std::time::Duration::from_secs(1),
        format!("shown after {shown_after:?}"),
    )
    .expect_row_with(&["▸", "api", "public", "POST /api/items", "refused"])
    .expect_text("AGENT_POLICY_DENIED")
    .expect_no_text("query-secret")
    .expect_no_text("header-secret")
    .expect_no_text("X-Trace")
    .snapshot("appended");
    let log = t.home_file(".local/state/kurama/audit.jsonl");
    t.key(Key::Char('y'))
        .wait_for("Copied: kurama api public -X POST /items")
        .snapshot("copied");
    let after = t.home_file(".local/state/kurama/audit.jsonl");
    t.check(
        "the monitor never writes the audit log it reads",
        after == log && log.lines().count() == 2,
        format!("before: {log:?}\nafter: {after:?}"),
    );
    let mut v = t.quit();
    v.expect_exit_code(0);
    let copied = v.observed.last().clipboard_calls.clone();
    v.check(
        "y copies the command that makes the refused call again",
        copied == ["kurama api public -X POST /items"],
        format!("{copied:?}"),
    );
    v.finish();
}
