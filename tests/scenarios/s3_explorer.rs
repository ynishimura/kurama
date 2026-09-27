//! Runtime verification scenarios of the `s3-explorer` feature that a TOML
//! case cannot say: the explorer `kurama s3 <S3>` opens on a terminal,
//! driven on a pseudo terminal against the fake S3 of
//! `tests/support/s3_browse.rs`. `tests/scenarios/main.rs` holds the naming
//! rule and what each scenario writes.

use crate::support::s3_browse::{explorer_scenario as scenario, gets, listings};
use crate::support::tui::{self, Key};
use crate::support::{OnePassword, StsFake};

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_browses_buckets_prefixes_and_objects() {
    // From the home screen's S3 tab: the bucket root, a prefix, an object's
    // preview, back up past the root to the bucket list -- which this role
    // may not list, so the error box says so and the screen stays usable --
    // and back to the home screen. One AssumeRole for all of it.
    let mut t = tui::launch(
        scenario("tui_s3_browses_buckets_prefixes_and_objects"),
        tui::STANDARD,
    );
    t.wait_for("Profiles (")
        .key(Key::Char('6'))
        .wait_for("S3 connections (3)")
        .key(Key::Down)
        .key(Key::Down)
        .expect_row_with(&["▸", "s3", "walk"])
        .key(Key::Enter)
        .wait_for("鞍馬 kurama s3")
        .wait_for("List (2)")
        .expect_text("s3://browse-bucket/")
        .expect_row_with(&["▸", "other/"])
        .expect_row_with(&["reports/"])
        .expect_text("2 scanned · complete")
        .snapshot("root")
        .key(Key::Down)
        .key(Key::Enter)
        .wait_for("List (8)")
        .expect_text("s3://browse-bucket/reports/")
        .expect_row_with(&["2024/"])
        .expect_row_with(&["日本語 +%/"])
        .expect_row_with(&["a b/"])
        .expect_row_with(&["summary.txt", "19"])
        .snapshot("prefix")
        .key(Key::End)
        .key(Key::Enter)
        .wait_for("┏ Preview")
        .expect_text("Key         reports/summary.txt")
        .expect_text("bytes 0-19 (identity); the whole object")
        .snapshot("preview")
        .key(Key::Esc)
        .key(Key::Left)
        .wait_for("List (2)")
        .key(Key::Left)
        .wait_for("┏ Error ")
        .expect_text("AccessDenied")
        .snapshot("buckets-denied")
        .key(Key::Esc)
        .wait_for("S3 answered with an error")
        .expect_text("Buckets (0)")
        .resize(tui::SMALL)
        .expect_text("S3 answered with an error")
        .snapshot("error-small")
        .key(Key::Char('q'))
        .wait_for("S3 connections (3)");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_op_calls(0)
        .expect_no_files_written();
    let calls = &v.observed.last().api_calls;
    let (lists, reads) = (listings(calls), gets(calls));
    v.check(
        "three levels listed, one object read, one bucket list refused",
        lists == 3 && reads == 1,
        format!("{lists} listings, {reads} GETs"),
    );
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_preview_shows_the_fetched_range_and_its_limits() {
    // A JSON object read whole, then a binary one shown as a hex head, on a
    // compact terminal where the preview replaces the list, and wide.
    let mut t = tui::launch_command(
        scenario("tui_s3_preview_shows_the_fetched_range_and_its_limits"),
        tui::SMALL,
        &["s3", "assets", "s3://browse-objects/data/"],
    );
    t.wait_for("scanned · complete")
        .key(Key::Char('/'))
        .type_text("report")
        .wait_for("List 1/")
        .key(Key::Enter)
        .key(Key::Enter)
        .wait_for("┏ Preview")
        .expect_text("Size        33 bytes")
        .expect_text("Type        application/json")
        .expect_text("bytes 0-33 (identity); the whole object")
        .expect_text("{\"name\":\"βeta\",\"items\":[1,2,3]}")
        .expect_no_text("┏ List")
        .snapshot("json-small")
        .key(Key::Esc)
        .wait_for("┏ List")
        .key(Key::Esc)
        .key(Key::Char('/'))
        .type_text("image")
        .key(Key::Enter)
        .key(Key::Enter)
        .wait_for("binary: the first bytes as hex")
        .expect_text("89504e470d0a1a0a0000000d49484452")
        .expect_text("00000001")
        .resize(tui::WIDE)
        .wait_for("┏ List")
        .expect_text("┏ Preview")
        .snapshot("binary-wide");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_no_files_written();
    let calls = &v.observed.last().api_calls;
    let heads = calls.iter().filter(|call| call.method == "HEAD").count();
    let reads = gets(calls);
    v.check(
        "each preview is one HEAD and one ranged GET",
        heads == 2 && reads == 2,
        format!("{heads} HEADs, {reads} GETs"),
    );
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_content_search_starts_only_from_a_confirmed_form() {
    // `g` opens a form that names the prefix and the bound; typing and
    // cancelling it reads nothing, and only Enter starts the search. The
    // lines found are told apart from a key search and from the filter.
    let mut t = tui::launch_command(
        scenario("tui_s3_content_search_starts_only_from_a_confirmed_form"),
        tui::STANDARD,
        &["s3", "assets", "s3://browse-objects/logs/"],
    );
    t.wait_for("List (5)")
        .key(Key::Char('g'))
        .wait_for("┏ Content search")
        .expect_text("in           s3://browse-objects/logs/")
        .expect_text("max objects  100")
        .type_text("request-id-123")
        .snapshot("form")
        .key(Key::Esc)
        .wait_for("List (5)")
        .key(Key::Char('g'))
        .type_text("request-id-123")
        .key(Key::Enter)
        .wait_for("Content search \"request-id-123\" (2)")
        .wait_for("2 not read in full")
        .expect_row_with(&["▸", "a.log:2", "GET /x request-id-123 200"])
        .expect_row_with(&["b.log.gz:2", "POST /z request-id-123 500"])
        .snapshot("found")
        .key(Key::Enter)
        .wait_for("┏ Preview")
        .expect_text("Key         logs/a.log");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_no_files_written();
    let calls = &v.observed.last().api_calls;
    let (lists, reads) = (listings(calls), gets(calls));
    // The level, the search's listing; a GET for each of the five objects
    // and one for the preview. The cancelled form read nothing.
    v.check(
        "only the confirmed search read the objects",
        lists == 2 && reads == 6,
        format!("{lists} listings, {reads} GETs"),
    );
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_esc_stops_the_search_and_keeps_the_partial_result() {
    // The key search finds the first page; the second takes 30 s. Moving
    // and resizing work meanwhile, Esc stops it at once, what was found
    // stays with why it ended, and no further page is asked for.
    let mut t = tui::launch_command(
        scenario("tui_s3_esc_stops_the_search_and_keeps_the_partial_result"),
        tui::STANDARD,
        &["s3", "slow"],
    );
    t.wait_for("List (2+)")
        .key(Key::Char('s'))
        .wait_for("┏ Key search")
        .expect_text("max objects  1000")
        .type_text("k")
        .key(Key::Enter)
        .wait_for("Key search \"k\" (2)")
        .wait_for("searching the keys… 1 s")
        .expect_footer_contains("Esc stop")
        .key(Key::Down)
        .expect_row_with(&["▸", "k2.log"])
        .resize(tui::SMALL)
        .expect_text("searching the keys…")
        .snapshot("running")
        .key(Key::Esc)
        .wait_for("stopped by Esc; partial result")
        .expect_row_with(&["k1.log"])
        .expect_row_with(&["▸", "k2.log"])
        .expect_no_text("┏ Error")
        .snapshot("stopped")
        .key(Key::Enter)
        .wait_for("┏ Preview")
        .expect_text("Key         slow/k2.log");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_no_files_written();
    let calls = &v.observed.last().api_calls;
    let lists = listings(calls);
    // The level's first page, the search's first page and the slow second
    // one Esc dropped; nothing after it.
    v.check(
        "no listing started after Esc",
        lists == 3,
        format!("{lists} listings"),
    );
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_open_as_data_hands_the_uri_to_data() {
    // `d` on a previewed CSV shows the kurama data request -- the exact
    // URI, the [s3.*] and what was seen of the object -- and `y` copies its
    // command. Nothing runs it: no request follows the preview.
    let mut t = tui::launch_command(
        scenario("tui_s3_open_as_data_hands_the_uri_to_data").with_fake_tools(),
        tui::STANDARD,
        &["s3", "assets", "s3://browse-objects/data/"],
    );
    t.wait_for("scanned · complete")
        .key(Key::Char('/'))
        .type_text("orders")
        .key(Key::Enter)
        .key(Key::Enter)
        .wait_for("┏ Preview")
        .expect_text("id,amount")
        .key(Key::Char('d'))
        .wait_for("┏ Open as data")
        .expect_text("from      s3://browse-objects/data/orders.csv")
        .expect_text("s3_source assets")
        .expect_text("size      20 bytes")
        .expect_footer_contains("y copy command")
        .snapshot("handoff")
        .key(Key::Char('y'))
        .wait_for("copied to the clipboard")
        .key(Key::Esc)
        .wait_for("filter /orders")
        .key(Key::Esc)
        .wait_for("List (6)")
        .key(Key::Char('/'))
        .type_text("report")
        .key(Key::Enter)
        .key(Key::Char('d'))
        .wait_for("kurama data reads CSV, JSONL and Parquet; data/report.json is none of them");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_no_files_written();
    let run = v.observed.last().clone();
    v.check(
        "y copied the kurama data command",
        run.clipboard_calls
            == ["kurama data --from s3://browse-objects/data/orders.csv --s3-source assets --describe"],
        format!("{:?}", run.clipboard_calls),
    );
    let (lists, reads) = (listings(&run.api_calls), gets(&run.api_calls));
    v.check(
        "the handoff ran nothing: one listing and the preview's GET",
        lists == 1 && reads == 1,
        format!("{lists} listings, {reads} GETs"),
    );
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_no_color_keeps_the_selection() {
    let mut t = tui::launch_command(
        scenario("tui_s3_no_color_keeps_the_selection").with_env("NO_COLOR", "1"),
        tui::STANDARD,
        &["s3", "assets", "s3://browse-objects/data/"],
    );
    t.wait_for("scanned · complete")
        .expect_default_colors()
        .expect_bold_text("▸")
        .expect_bold_text("READ ONLY")
        .snapshot("list")
        .resize(tui::WIDE)
        .expect_default_colors()
        .expect_bold_text("▸")
        .snapshot("wide")
        .resize(tui::SMALL)
        .expect_default_colors()
        .expect_bold_text("▸")
        .snapshot("small");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole"])
        .expect_no_files_written();
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_renews_the_role_before_it_expires() {
    // The role's credentials end 30 seconds after each AssumeRole, inside the
    // minute kurama keeps in hand, so each request of the session assumes
    // the role again through the same path instead of signing with
    // credentials about to end: the level the screen opens on, then the
    // prefix Enter opens, after the one AssumeRole before the screen.
    let mut t = tui::launch_command(
        scenario("tui_s3_renews_the_role_before_it_expires").sts(StsFake::ExpiresIn(30)),
        tui::STANDARD,
        &["s3", "walk"],
    );
    t.wait_for("List (2)")
        .key(Key::Down)
        .key(Key::Enter)
        .wait_for("List (8)")
        .expect_text("s3://browse-bucket/reports/");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_sts_actions(&["AssumeRole", "AssumeRole", "AssumeRole"])
        .expect_op_calls(0)
        .expect_no_files_written();
    let lists = listings(&v.observed.last().api_calls);
    v.check("two levels listed", lists == 2, format!("{lists} listings"));
    v.finish();
}

#[test]
#[cfg_attr(not(feature = "test-fakes"), ignore = "needs --features test-fakes")]
fn tui_s3_tab_shows_the_session_of_its_aws_profile() {
    // `login` caches an MFA session for ops-mfa first. The S3 tab's STATE is
    // the SESSION the AWS tab shows for each connection's aws_profile -- a
    // valid session, `-` for a profile that needs no MFA -- and says so when
    // ~/.aws/config has no such profile. Nothing is assumed to show it.
    let mut t = tui::launch(
        scenario("tui_s3_tab_shows_the_session_of_its_aws_profile")
            .onepassword(OnePassword::Enabled)
            .with_session_cache()
            .with_extra_config(
                "[s3.mfa]\naws_profile = \"ops-mfa\"\n\n[s3.gone]\naws_profile = \"nope\"\n",
            )
            .then_run(&["login", "ops-mfa"]),
        tui::STANDARD,
    );
    t.wait_for("Profiles (3)")
        .expect_row_with(&["ops-mfa", "valid ("])
        .key(Key::Char('6'))
        .wait_for("S3 connections (5)")
        .expect_row_with(&["s3", "mfa", "valid ("])
        .expect_row_with(&["s3", "assets", "-"])
        .expect_row_with(&["s3", "gone", "unknown profile"])
        .expect_no_text("not_checked");
    let mut v = t.quit();
    v.expect_exit_code(0)
        .expect_run_sts_actions(0, &["GetSessionToken"])
        .expect_run_sts_actions(1, &[]);
    v.finish();
}
