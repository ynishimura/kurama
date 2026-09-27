//! Tests of the config.toml edits: only the target changes, and every refusal is typed; a refusal found before the edit starts leaves the document as it was.

use super::*;

const FILE: &str = "# my settings\n[core]\nlog_level = \"debug\" # loud\n\n\
    # the shared Google client\n[auth.google]\nkind = \"oauth\"\ngrant_type = \"authorization_code\"\n\
    auth_url = \"https://a/auth\"\ntoken_url = \"https://a/token\"\nclient_id = \"id\"\n\
    client_secret = \"op://Agent/google/client_secret\"\nscopes = [\"s1\"]\n\n\
    # sheets\n[api.sheets]\nbase_url = \"https://sheets\"\nauth = \"google\"\n\n\
    # docs\n[api.docs]\nbase_url = \"https://docs\"\nauth = \"google\"\n\n\
    [db.app]\nengine = \"postgres\"\npassword = \"plain-literal\" # old\n";

fn edit() -> Edit {
    Edit::new(Path::new("/c.toml"), FILE).unwrap()
}

/// `FILE` with `from` replaced by `to`, once.
fn file_with(from: &str, to: &str) -> String {
    assert_eq!(FILE.matches(from).count(), 1, "{from}");
    FILE.replacen(from, to, 1)
}

#[test]
fn a_set_changes_only_the_value_and_keeps_its_comment() {
    let mut edit = edit();
    edit.set("core.log_level", "\"info\"").unwrap();
    edit.set("auth.google.scopes", "[\"s1\", \"s2\"]").unwrap();
    assert_eq!(
        edit.text(),
        file_with("\"debug\" # loud", "\"info\" # loud").replacen(
            "[\"s1\"]",
            "[\"s1\", \"s2\"]",
            1
        )
    );
    assert_eq!(edit.units(), ["core", "auth.google"]);
    assert_eq!(
        edit.changes[0],
        Change {
            section: "core".into(),
            action: "set",
            key: Some("core.log_level".into())
        }
    );
}

#[test]
fn a_set_adds_a_key_to_an_entry_and_creates_an_omitted_fixed_table_at_the_end() {
    let mut edit = edit();
    edit.set("api.docs.description", "\"Docs\"").unwrap();
    edit.set("aws.session_cache.duration", "3600").unwrap();
    edit.set("openapi.revalidate_after", "0").unwrap();
    assert_eq!(
        edit.text(),
        file_with(
            "base_url = \"https://docs\"\nauth = \"google\"\n",
            "base_url = \"https://docs\"\nauth = \"google\"\ndescription = \"Docs\"\n"
        ) + "\n[aws.session_cache]\nduration = 3600\n\n[openapi]\nrevalidate_after = 0\n"
    );
}

#[test]
fn a_set_outside_an_existing_entry_or_of_a_whole_unit_is_refused() {
    let mut edit = edit();
    assert!(matches!(
        edit.set("api.new.base_url", "\"https://x\""),
        Err(InputError::Absent { units, .. }) if units == ["api.new"]
    ));
    assert!(matches!(
        edit.set("api.docs", "{ base_url = \"https://x\" }"),
        Err(InputError::WholeUnit { key }) if key == "api.docs"
    ));
    assert!(matches!(
        edit.set("nope.key", "1"),
        Err(InputError::NotAUnit { .. })
    ));
    assert!(matches!(
        edit.set("api", "1"),
        Err(InputError::NotAUnit { .. })
    ));
    assert!(matches!(
        edit.set("api.docs.base_url.deeper", "1"),
        Err(InputError::Invalid(message)) if message.contains("api.docs.base_url holds a value")
    ));
    assert_eq!(edit.text(), FILE);
    assert!(edit.changes.is_empty());
}

#[test]
fn a_value_that_is_not_toml_or_a_literal_secret_is_refused_and_not_echoed() {
    let mut edit = edit();
    match edit.set("core.log_level", "debug-unquoted") {
        Err(InputError::Invalid(message)) => {
            assert!(message.starts_with("the value for core.log_level is not a TOML value"));
            assert!(!message.contains("debug-unquoted"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    match edit.set("auth.google.client_secret", "\"hunter2\"") {
        Err(error @ InputError::LiteralSecret { .. }) => {
            assert!(!error.to_string().contains("hunter2"));
            assert!(
                error
                    .to_string()
                    .starts_with("auth.google.client_secret must be")
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        edit.set("db.app.password", "12345"),
        Err(InputError::LiteralSecret { .. })
    ));
    edit.set("db.app.password", "\"op://Agent/app/password\"")
        .unwrap();
    assert_eq!(
        edit.text(),
        file_with(
            "\"plain-literal\" # old",
            "\"op://Agent/app/password\" # old"
        )
    );
}

#[test]
fn a_replaced_unit_keeps_its_place_and_comment_and_loses_what_the_input_leaves_out() {
    let mut edit = edit();
    edit.replace(
        "[auth.google]\nkind = \"token\"\ntoken = \"op://Agent/google/token\"\n\n\
         [core]\nlog_level = \"warn\"\n",
    )
    .unwrap();
    let expected = FILE
        .replacen(
            "log_level = \"debug\" # loud\n",
            "log_level = \"warn\"\n",
            1,
        )
        .replacen(
            "kind = \"oauth\"\ngrant_type = \"authorization_code\"\n\
             auth_url = \"https://a/auth\"\ntoken_url = \"https://a/token\"\nclient_id = \"id\"\n\
             client_secret = \"op://Agent/google/client_secret\"\nscopes = [\"s1\"]\n",
            "kind = \"token\"\ntoken = \"op://Agent/google/token\"\n",
            1,
        );
    assert_eq!(edit.text(), expected);
    assert_eq!(
        edit.changes,
        [
            Change::of("auth.google", "replace"),
            Change::of("core", "replace")
        ]
    );
}

#[test]
fn a_replaced_unit_with_tables_of_its_own_stays_in_place() {
    let file = "[db.app]\nengine = \"postgres\"\n\n[db.app.iam]\naws_profile = \"old\"\n\n\
                [api.after]\nbase_url = \"https://after\"\n";
    let mut edit = Edit::new(Path::new("/c.toml"), file).unwrap();
    edit.replace("[db.app]\nengine = \"mysql\"\n[db.app.tunnel]\naws_profile = \"new\"\n")
        .unwrap();
    assert_eq!(
        edit.text(),
        "[db.app]\nengine = \"mysql\"\n[db.app.tunnel]\naws_profile = \"new\"\n\n\
         [api.after]\nbase_url = \"https://after\"\n"
    );
}

#[test]
fn a_replacement_adds_an_omitted_fixed_table_but_no_new_entry() {
    let mut edit = edit();
    assert!(matches!(
        edit.replace("[api.docs]\nbase_url = \"https://d\"\n[api.new]\nbase_url = \"https://n\"\n"),
        Err(InputError::Absent { units, .. }) if units == ["api.new"]
    ));
    assert!(matches!(
        edit.replace(
            "[api.docs]\nbase_url = \"https://d\"\nauth = 42\n[auth.x]\ntoken = \"plain\"\n"
        ),
        Err(InputError::LiteralSecret { .. })
    ));
    assert_eq!(edit.text(), FILE);
    edit.replace("[onepassword]\ntimeout = 5\n").unwrap();
    assert_eq!(edit.text(), format!("{FILE}\n[onepassword]\ntimeout = 5\n"));
    assert_eq!(edit.changes, [Change::of("onepassword", "add")]);
}

#[test]
fn an_unset_removes_each_key_with_its_comment() {
    let mut edit = edit();
    edit.unset(&["core.log_level".into(), "auth.google.scopes".into()])
        .unwrap();
    assert_eq!(
        edit.text(),
        file_with("log_level = \"debug\" # loud\n", "").replacen("scopes = [\"s1\"]\n", "", 1)
    );
    assert_eq!(
        edit.units(),
        ["core", "auth.google"],
        "each unit once, in order"
    );
}

#[test]
fn an_unset_refuses_a_missing_key_a_repeat_an_overlap_and_a_whole_unit() {
    let mut edit = edit();
    match edit.unset(&["core.log_level".into(), "core.nope".into(), "aws.x".into()]) {
        Err(error @ InputError::NoSuchKey { .. }) => {
            assert_eq!(error.to_string(), "/c.toml has no core.nope, aws.x")
        }
        other => panic!("{other:?}"),
    }
    match edit.unset(&["core.log_level".into(), "core.log_level".into()]) {
        Err(error @ InputError::Overlap { .. }) => assert_eq!(
            error.to_string(),
            "core.log_level is given more than once; give each once"
        ),
        other => panic!("{other:?}"),
    }
    match edit.unset(&["auth.google.scopes".into(), "auth.google".into()]) {
        Err(error @ InputError::Overlap { .. }) => assert_eq!(
            error.to_string(),
            "auth.google already holds auth.google.scopes; give one of them"
        ),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        edit.unset(&["core".into()]),
        Err(InputError::WholeUnit { .. })
    ));
    assert_eq!(edit.text(), FILE);
}

#[test]
fn a_removed_api_leaves_its_shared_auth_and_the_comments_of_the_rest() {
    let mut edit = edit();
    edit.remove(&["api.docs".into()]).unwrap();
    assert_eq!(
        edit.text(),
        file_with(
            "# docs\n[api.docs]\nbase_url = \"https://docs\"\nauth = \"google\"\n\n",
            ""
        )
    );
}

#[test]
fn an_auth_still_used_is_refused_with_every_api_that_uses_it() {
    let mut edit = edit();
    match edit.remove(&["auth.google".into(), "api.docs".into()]) {
        Err(error @ InputError::StillReferenced { .. }) => {
            assert_eq!(
                error.to_string(),
                "[auth.google] is still used by [api.sheets]; nothing was removed"
            )
        }
        other => panic!("{other:?}"),
    }
    // An API without `auth` uses the auth of its own name.
    let file = "[auth.gh]\nkind = \"token\"\ntoken = \"op://A/gh/t\"\n[api.gh]\nbase_url = \"https://gh\"\n";
    let mut defaulted = Edit::new(Path::new("/c.toml"), file).unwrap();
    assert!(matches!(
        defaulted.remove(&["auth.gh".into()]),
        Err(InputError::StillReferenced { referrers, .. }) if referrers == ["api.gh"]
    ));
}

#[test]
fn an_auth_removed_with_every_api_that_uses_it_is_one_edit() {
    let mut edit = edit();
    edit.remove(&[
        "api.docs".into(),
        "auth.google".into(),
        "api.sheets".into(),
        "db.app".into(),
    ])
    .unwrap();
    assert_eq!(
        edit.text(),
        "# my settings\n[core]\nlog_level = \"debug\" # loud\n"
    );
    assert!(matches!(
        edit.remove(&["api.docs".into(), "core.log_level".into()]),
        Err(InputError::InsideUnit { .. })
    ));
    assert!(matches!(
        edit.remove(&["api.docs".into()]),
        Err(InputError::Absent { units, .. }) if units == ["api.docs"]
    ));
}

#[test]
fn the_diff_shows_the_changed_lines_around_their_context_with_secrets_redacted() {
    let mut edit = edit();
    edit.set("api.docs.base_url", "\"https://docs2\"").unwrap();
    edit.set("db.app.engine", "\"mysql\"").unwrap();
    let diff = edit.diff();
    assert_eq!(
        diff,
        [
            "@@ -20,8 +20,8 @@",
            "# docs",
            "[api.docs]",
            "-base_url = \"https://docs\"",
            "+base_url = \"https://docs2\"",
            "auth = \"google\"",
            "",
            "[db.app]",
            "-engine = \"postgres\"",
            "+engine = \"mysql\"",
            "password = \"<redacted>\" # old",
        ]
        .iter()
        .map(|line| {
            if line.starts_with(['@', '-', '+']) {
                (*line).to_owned()
            } else {
                format!(" {line}")
            }
        })
        .collect::<Vec<_>>()
    );
    assert!(!diff.join("\n").contains("plain-literal"));
    assert!(edit.changed());
}

fn edited(file: &str, apply: impl FnOnce(&mut Edit)) -> String {
    let mut edit = Edit::new(Path::new("/c.toml"), file).unwrap();
    apply(&mut edit);
    edit.text()
}

/// A section owns the comment lines right above its header (after the last
/// blank line there) and the ones right after its last key (before the next
/// blank line); removing it removes those and keeps every other comment.
#[test]
fn a_removed_section_takes_its_own_comments_and_leaves_the_rest() {
    let file = "# my settings\n# about this file\n\n# the core\n[core]\nlog_level = \"debug\"\n\
                # old = 1\n\n# the api\n[api.x]\nbase_url = \"https://x\"\n";
    assert_eq!(
        edited(file, |edit| edit.remove(&["core".into()]).unwrap()),
        "# my settings\n# about this file\n\n# the api\n[api.x]\nbase_url = \"https://x\"\n"
    );
    // The first section without a comment of the file above it: no blank
    // line is left at the top.
    assert_eq!(
        edited(
            "[core]\nlog_level = \"debug\"\n\n[api.x]\nbase_url = \"https://x\"\n",
            |edit| { edit.remove(&["core".into()]).unwrap() }
        ),
        "[api.x]\nbase_url = \"https://x\"\n"
    );
    // The last section's commented-out lines go with it.
    assert_eq!(
        edited(
            "[core]\nlog_level = \"debug\"\n\n[api.x]\nbase_url = \"https://x\"\n# retry = 3\n",
            |edit| { edit.remove(&["api.x".into()]).unwrap() }
        ),
        "[core]\nlog_level = \"debug\"\n"
    );
    // A comment right above the next header, with no blank line, is the
    // next section's, and a blank line still separates it.
    assert_eq!(
        edited(
            "[api.a]\nbase_url = \"https://a\"\n\n[api.b]\nbase_url = \"https://b\"\n# about c\n[api.c]\nbase_url = \"https://c\"\n",
            |edit| edit.remove(&["api.b".into()]).unwrap()
        ),
        "[api.a]\nbase_url = \"https://a\"\n\n# about c\n[api.c]\nbase_url = \"https://c\"\n"
    );
}

/// `aws` written only as `[aws.session_cache]` has no header of its own:
/// the comment and blank line above its first header stay above the
/// replacement's.
#[test]
fn a_replaced_table_without_a_header_keeps_the_comment_above_its_first_one() {
    let file = "[core]\nlog_level = \"debug\"\n\n# session settings\n[aws.session_cache]\nduration = 3600\n";
    assert_eq!(
        edited(file, |edit| edit
            .replace("[aws.session_cache]\nduration = 7200\n")
            .unwrap()),
        "[core]\nlog_level = \"debug\"\n\n# session settings\n[aws.session_cache]\nduration = 7200\n"
    );
}

/// An API that signs with an AWS profile does not use the auth of its name.
#[test]
fn an_api_signing_with_an_aws_profile_does_not_hold_the_auth_of_its_name() {
    let file = "[auth.gh]\nkind = \"token\"\ntoken = \"op://A/gh/t\"\n\n\
                [api.gh]\nbase_url = \"https://gh\"\naws_profile = \"dev\"\n";
    assert_eq!(
        edited(file, |edit| edit.remove(&["auth.gh".into()]).unwrap()),
        "[api.gh]\nbase_url = \"https://gh\"\naws_profile = \"dev\"\n"
    );
}

/// Inside an inline table a new table is inline; a table given a value
/// becomes that value; inside a dotted table a key stays dotted.
#[test]
fn a_set_follows_the_form_of_the_table_it_writes_into() {
    assert_eq!(
        edited("[db]\napp = { engine = \"postgres\" }\n", |edit| edit
            .set("db.app.tunnel.aws_profile", "\"dev\"")
            .unwrap()),
        "[db]\napp = { engine = \"postgres\" , tunnel = { aws_profile = \"dev\" } }\n"
    );
    assert_eq!(
        edited(
            "[api.x]\nbase_url = \"https://x\"\n\n[api.x.headers]\nAccept = \"a\"\n",
            |edit| edit.set("api.x.headers", "{ Accept = \"b\" }").unwrap()
        ),
        "[api.x]\nbase_url = \"https://x\"\nheaders = { Accept = \"b\" }\n"
    );
    assert_eq!(
        edited("[aws]\nsession_cache.enabled = true\n", |edit| edit
            .set("aws.session_cache.duration", "3600")
            .unwrap()),
        "[aws]\nsession_cache.enabled = true\nsession_cache.duration = 3600\n"
    );
}

/// The next header is an `[[array]]` entry: its comment moves up like any
/// other header's.
#[test]
fn a_removed_section_before_an_array_entry_keeps_its_comment() {
    let file =
        "[api.a]\nbase_url = \"https://a\"\n\n# the lake\n[[data.lake.sources]]\nname = \"x\"\n";
    assert_eq!(
        edited(file, |edit| edit.remove(&["api.a".into()]).unwrap()),
        "# the lake\n[[data.lake.sources]]\nname = \"x\"\n"
    );
}
