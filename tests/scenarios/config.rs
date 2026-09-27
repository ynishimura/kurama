//! Runtime verification scenarios of the `config` feature that a TOML case
//! cannot state: the bytes and mode of the file, a symbolic link, a directory
//! that refuses the write, and every run of a sequence checked for calls.
//! `tests/scenarios/main.rs` holds the naming rule and what each scenario
//! writes.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::support::{OnePassword, Sandbox, Scenario};

const FRAGMENT: &str = "[auth.svc]\nkind = \"token\"\ntoken = \"op://Agent/svc/credential\"\n\n\
                        [api.svc]\nbase_url = \"https://svc.example.com\"\nauth = \"svc\"\n";

/// The permission bits of `path`.
const MODE: fn(&Path) -> u32 = |path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

/// Give `path` these permission bits.
const SET_MODE: fn(&Path, u32) =
    |path, mode| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();

/// The file's own bytes -- a comment, a key with its trailing comment, no
/// final newline -- are the start of the new file, byte for byte, and its
/// mode 0640 stays. A file that does not exist yet is created 0600.
#[test]
fn config_add_preserves_existing_bytes_and_permissions() {
    const EXISTING: &str = "# written by hand\n[core]\nlog_level = \"debug\"    # loud";
    let scenario = Scenario::new(
        "config_add_preserves_existing_bytes_and_permissions",
        "config",
        &[],
    )
    .with_config(EXISTING)
    .with_stdin(FRAGMENT.to_owned());
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    SET_MODE(&config, 0o640);
    let runs = vec![sandbox.run_cli(&["config", "add", "--file", "-"])];
    let first = std::fs::read_to_string(&config).unwrap();
    let first_mode = MODE(&config);
    std::fs::remove_file(&config).unwrap();
    let mut runs = runs;
    runs.push(sandbox.run_cli(&["config", "add", "--file", "-"]));
    let created = std::fs::read_to_string(&config).unwrap_or_default();
    let created_mode = MODE(&config);

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    for index in 0..2 {
        v.keyed_run(index, "add", |v| {
            v.expect_exit_code(0)
                .expect_stdout_empty()
                .expect_stderr_contains("# added [auth.svc], [api.svc] to ")
                .expect_sts_actions(&[])
                .expect_op_calls(0)
        });
    }
    v.check(
        "the existing bytes are the start of the file, then a newline, a blank line and the input",
        first == format!("{EXISTING}\n\n{FRAGMENT}"),
        format!("observed {first:?}"),
    )
    .check(
        "the existing mode 0640 is kept",
        first_mode == 0o640,
        format!("observed {first_mode:o}"),
    )
    .check(
        "a file that did not exist is the input alone",
        created == FRAGMENT,
        format!("observed {created:?}"),
    )
    .check(
        "a new file is readable and writable by its owner only",
        created_mode == 0o600,
        format!("observed {created_mode:o}"),
    )
    .expect_files_written(&["kurama-config.toml"]);
    v.finish();
}

/// A config file that is a symbolic link into a dotfiles directory stays a
/// link: the file it points at gets the new sections, and nothing else is
/// written beside either of them. A link whose file does not exist yet (a
/// fresh dotfiles checkout) is not replaced either: the file is created where
/// it points.
#[test]
fn config_add_preserves_a_config_symlink() {
    let scenario = Scenario::new("config_add_preserves_a_config_symlink", "config", &[])
        .with_home_file("dotfiles/kurama.toml", "[core]\nlog_level = \"info\"\n")
        .with_stdin(FRAGMENT.to_owned());
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    let target = sandbox.home.join("dotfiles/kurama.toml");
    std::fs::remove_file(&config).unwrap();
    std::os::unix::fs::symlink(&target, &config).unwrap();
    let runs = vec![
        sandbox.run_cli(&["config", "add", "--file", "-"]),
        sandbox.run_cli(&["config", "show", "api.svc"]),
    ];
    let still_a_link = std::fs::symlink_metadata(&config)
        .unwrap()
        .file_type()
        .is_symlink();
    let points_at = std::fs::read_link(&config).ok();
    let saved = std::fs::read_to_string(&target).unwrap();
    let beside: Vec<String> = std::fs::read_dir(target.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    std::fs::remove_file(&target).unwrap();
    let mut runs = runs;
    runs.push(sandbox.run_cli(&["config", "add", "--file", "-"]));
    let dangling_kept = std::fs::symlink_metadata(&config)
        .unwrap()
        .file_type()
        .is_symlink();
    let created = std::fs::read_to_string(&target).unwrap_or_default();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "add", |v| {
        v.expect_exit_code(0)
            .expect_stdout_empty()
            .expect_stderr_contains("/kurama-config.toml")
    });
    v.keyed_run(2, "dangling", |v| {
        v.expect_exit_code(0).expect_stdout_empty()
    });
    v.keyed_run(1, "show", |v| {
        v.expect_exit_code(0).expect_stdout_equals(
            "[api.svc]\nbase_url = \"https://svc.example.com\"\nauth = \"svc\"\n",
        )
    });
    v.check(
        "the config path is still a symbolic link",
        still_a_link,
        "a regular file replaced the link",
    )
    .check(
        "the link still points at the dotfiles file",
        points_at.as_deref() == Some(target.as_path()),
        format!("observed {points_at:?}"),
    )
    .check(
        "the file the link points at holds the new sections after its own bytes",
        saved == format!("[core]\nlog_level = \"info\"\n\n{FRAGMENT}"),
        format!("observed {saved:?}"),
    )
    .check(
        "no temporary file is left beside it",
        beside == ["kurama.toml"],
        format!("observed {beside:?}"),
    )
    .check(
        "a link to a missing file is still a link after the add",
        dangling_kept,
        "a regular file replaced the link",
    )
    .check(
        "the missing file is created where the link points, holding the input",
        created == FRAGMENT,
        format!("observed {created:?}"),
    )
    .expect_files_written(&["home/dotfiles/kurama.toml", "kurama-config.toml"]);
    v.finish();
}

/// A directory the new file cannot be written into (mode 0500) is
/// CONFIG_WRITE_FAILED, exit 1, with the path in the message and in the
/// hint. The file keeps every byte, including the literal secret it already
/// held, and that secret appears nowhere in the output.
#[test]
fn config_add_reports_write_failure_without_exposing_secrets() {
    const SECRET: &str = "literal-secret-already-in-the-file";
    let existing = format!(
        "[auth.legacy]\nkind = \"oauth\"\ngrant_type = \"client_credentials\"\n\
         token_url = \"https://as.example.com/token\"\nclient_id = \"legacy\"\nclient_secret = \"{SECRET}\"\n"
    );
    let scenario = Scenario::new(
        "config_add_reports_write_failure_without_exposing_secrets",
        "config",
        &[],
    )
    .with_config(&existing)
    .with_stdin(FRAGMENT.to_owned());
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    let before = std::fs::read(&config).unwrap();
    let directory = config.parent().unwrap().to_path_buf();
    SET_MODE(&directory, 0o500);
    let runs = vec![
        sandbox.run_cli(&["config", "add", "--file", "-"]),
        sandbox.run_cli(&["config", "add", "--file", "-", "--json"]),
    ];
    SET_MODE(&directory, 0o700);
    let path = config.display().to_string();
    let after = std::fs::read(&config).unwrap_or_default();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "text", |v| {
        v.expect_error("CONFIG_WRITE_FAILED", 1)
            .expect_stdout_empty()
            .expect_stderr_contains(&format!("cannot save {path}: Permission denied"))
            .expect_stderr_contains(&format!(
                "hint: check that the directory of {path} exists and is writable"
            ))
            .expect_stderr_excludes(SECRET)
            .expect_stderr_excludes("op://Agent/svc/credential")
    });
    v.keyed_run(1, "json", |v| {
        v.expect_json_error("CONFIG_WRITE_FAILED", 1)
            .expect_stdout_empty()
            .expect_stderr_excludes(SECRET)
    });
    v.check(
        "the file is byte for byte what it was",
        after == before,
        format!("observed {} bytes of {}", after.len(), before.len()),
    )
    .expect_sts_actions(&[])
    .expect_op_calls(0)
    .expect_no_files_written();
    v.finish();
}

/// `config path`, `list`, `show` and `add`, reading and writing a file whose
/// sources would each reach something when used -- an OAuth grant,
/// 1Password, Secrets Manager, an AssumeRole, a description URL -- reach none
/// of them. The edit commands are checked the same way by
/// `config_edit_commands_do_not_call_external_services`.
#[test]
fn config_operations_do_not_call_external_services() {
    let scenario = Scenario::new(
        "config_operations_do_not_call_external_services",
        "config",
        &[],
    )
    .onepassword(OnePassword::Enabled)
    .with_extra_config(
        "[auth.browser]\nkind = \"oauth\"\ngrant_type = \"authorization_code\"\n\
         auth_url = \"{server}/oauth/authorize\"\ntoken_url = \"{server}/oauth/token\"\n\
         client_id = \"browser\"\nclient_secret = \"op://Agent/browser/client_secret\"\n\n\
         [auth.stored]\nkind = \"token\"\ntoken = \"aws-secrets://dev/example/api-key\"\n\n\
         [api.browser]\nbase_url = \"{server}/api\"\nopenapi = \"{server}/spec/petstore.json\"\n\n\
         [api.signed]\nbase_url = \"https://abc.execute-api.ap-northeast-1.amazonaws.com/prod\"\n\
         aws_profile = \"dev\"\n",
    )
    .with_stdin(FRAGMENT.to_owned());
    let commands: [&[&str]; 7] = [
        &["config", "path"],
        &["config", "list"],
        &["config", "show"],
        &["config", "show", "auth.browser", "--json"],
        &["config", "add", "--file", "-", "--dry-run"],
        &["config", "add", "--file", "-"],
        &["config", "show", "api.svc"],
    ];
    let mut sandbox = Sandbox::create(&scenario);
    let runs = commands
        .iter()
        .map(|args| sandbox.run_cli(args))
        .collect::<Vec<_>>();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    for (index, args) in commands.iter().enumerate() {
        v.keyed_run(index, &args.join(" "), |v| {
            v.expect_exit_code(0)
                .expect_sts_actions(&[])
                .expect_op_calls(0)
                .expect_token_grants(&[])
                .expect_open_calls(&[])
                .expect_api_call_count(0)
                .expect_run_spec_calls(index, 0, false, 0)
        });
    }
    v.expect_secret_reads(&[])
        .expect_files_written(&["kurama-config.toml"]);
    v.finish();
}

/// `config set`, `unset` and `remove` edit the credentials and description
/// URLs of sources that would each reach something when used -- an OAuth
/// grant with a 1Password secret, a Secrets Manager token, a description on
/// the fake server, an AssumeRole -- and reach none of them: every run is
/// checked for STS, `op`, grants, the browser, description fetches and API
/// calls, the whole sequence for secret reads, and the file holds exactly
/// the edits.
#[test]
fn config_edit_commands_do_not_call_external_services() {
    const EXISTING: &str = "[auth.browser]\nkind = \"oauth\"\ngrant_type = \"authorization_code\"\n\
                            auth_url = \"{server}/oauth/authorize\"\ntoken_url = \"{server}/oauth/token\"\n\
                            client_id = \"browser\"\nclient_secret = \"op://Agent/browser/client_secret\"\n\n\
                            [auth.stored]\nkind = \"token\"\ntoken = \"aws-secrets://dev/example/api-key\"\n\n\
                            [api.browser]\nbase_url = \"{server}/api\"\nauth = \"browser\"\n\
                            openapi = \"{server}/spec/petstore.json\"\n\n\
                            [api.stored]\nbase_url = \"{server}/stored\"\nauth = \"stored\"\n\n\
                            [api.signed]\nbase_url = \"https://abc.execute-api.ap-northeast-1.amazonaws.com/prod\"\n\
                            aws_profile = \"dev\"\n";
    const SIGNED: &str = "\n[api.signed]\n\
                          base_url = \"https://abc.execute-api.ap-northeast-1.amazonaws.com/prod\"\n\
                          aws_profile = \"dev\"\n";
    let scenario = Scenario::new(
        "config_edit_commands_do_not_call_external_services",
        "config",
        &[],
    )
    .onepassword(OnePassword::Enabled)
    .with_extra_config(EXISTING);
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    let before = std::fs::read_to_string(&config).unwrap();
    let server = before
        .split("token_url = \"")
        .nth(1)
        .and_then(|rest| rest.split("/oauth/token").next())
        .unwrap()
        .to_owned();
    let spec = format!("\"{server}/spec/v2.json\"");
    let commands: [&[&str]; 6] = [
        &[
            "config",
            "set",
            "auth.browser.client_secret",
            "\"op://Agent/browser/rotated\"",
        ],
        &[
            "config",
            "set",
            "auth.stored.token",
            "\"aws-ssm://dev/example/api-key\"",
        ],
        &["config", "set", "api.stored.openapi", &spec, "--json"],
        &["config", "unset", "api.browser.openapi"],
        &["config", "remove", "api.signed", "--dry-run"],
        &["config", "remove", "api.signed"],
    ];
    let runs = commands
        .iter()
        .map(|args| sandbox.run_cli(args))
        .collect::<Vec<_>>();
    let after = std::fs::read_to_string(&config).unwrap();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    for (index, args) in commands.iter().enumerate() {
        v.keyed_run(index, &args.join(" "), |v| {
            v.expect_exit_code(0)
                .expect_sts_actions(&[])
                .expect_op_calls(0)
                .expect_token_grants(&[])
                .expect_open_calls(&[])
                .expect_api_call_count(0)
                .expect_run_spec_calls(index, 0, false, 0)
        });
    }
    let expected = before
        .replace(
            "op://Agent/browser/client_secret",
            "op://Agent/browser/rotated",
        )
        .replace(
            "aws-secrets://dev/example/api-key",
            "aws-ssm://dev/example/api-key",
        )
        .replace(
            "auth = \"stored\"\n",
            &format!("auth = \"stored\"\nopenapi = {spec}\n"),
        )
        .replace(&format!("openapi = \"{server}/spec/petstore.json\"\n"), "")
        .replace(SIGNED, "");
    v.check(
        "the file holds the new credentials, the description URLs and the removal, nothing else",
        after == expected,
        format!("expected {expected:?}, observed {after:?}"),
    )
    .expect_secret_reads(&[])
    .expect_files_written(&["kurama-config.toml"]);
    v.finish();
}

/// A hand-written file with comments beside its values and between its
/// sections, which the edits below must keep.
const COMMENTED: &str = "# written by hand\n[core]\nlog_level = \"debug\"    # loud\n\n\
                         # my service\n[auth.svc]\nkind = \"token\"\n\
                         token = \"op://Agent/svc/credential\"   # in 1Password\n\n\
                         [api.svc]\nbase_url = \"https://svc.example.com\"  # prod\nauth = \"svc\"\n";

/// `config set` changes the one value it names and keeps the comment beside
/// it; every other byte of the file stays. A fixed table the file omits is
/// created at the end, a value the file already holds writes nothing (JSON:
/// not changed, not applied, no changes), and a file that is not TOML is
/// not edited at all.
#[test]
fn config_set_updates_a_key_and_preserves_unrelated_content() {
    const BROKEN: &str = "[core\nlog_level = \"debug\"\n";
    let scenario = Scenario::new(
        "config_set_updates_a_key_and_preserves_unrelated_content",
        "config",
        &[],
    )
    .with_config(COMMENTED);
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    let mut runs = vec![sandbox.run_cli(&[
        "config",
        "set",
        "api.svc.base_url",
        "\"https://staging.example.com\"",
    ])];
    let first = std::fs::read_to_string(&config).unwrap();
    runs.push(sandbox.run_cli(&["config", "set", "core.log_level", "\"info\"", "--json"]));
    runs.push(sandbox.run_cli(&["config", "set", "aws.session_cache.duration", "3600"]));
    runs.push(sandbox.run_cli(&[
        "config",
        "set",
        "api.svc.base_url",
        "\"https://staging.example.com\"",
    ]));
    runs.push(sandbox.run_cli(&["config", "set", "core.log_level", "\"info\"", "--json"]));
    let last = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, BROKEN).unwrap();
    runs.push(sandbox.run_cli(&["config", "set", "core.log_level", "\"info\""]));
    let broken = std::fs::read_to_string(&config).unwrap();
    let path = config.display().to_string();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "set a value", |v| {
        v.expect_exit_code(0)
            .expect_stdout_empty()
            .expect_stderr_contains(&format!("# set api.svc.base_url in {path}\n"))
    });
    v.keyed_run(1, "json", |v| {
        v.expect_exit_code(0)
            .expect_stderr_empty()
            .expect_stdout_contains("\"changed\": true")
            .expect_stdout_contains("\"applied\": true")
            .expect_stdout_contains(
                "\"section\": \"core\",\n      \"action\": \"set\",\n      \"key\": \"core.log_level\"",
            )
    });
    v.keyed_run(2, "an omitted fixed table", |v| {
        v.expect_exit_code(0)
            .expect_stderr_contains("# set aws.session_cache.duration in ")
    });
    v.keyed_run(3, "the same value again", |v| {
        v.expect_exit_code(0)
            .expect_stderr_contains(&format!("# {path} already holds that; nothing was written"))
    });
    v.keyed_run(4, "the same value again, as JSON", |v| {
        v.expect_exit_code(0).expect_stdout_contains(
            "\"changed\": false,\n  \"applied\": false,\n  \"changes\": [],",
        )
    });
    v.keyed_run(5, "not TOML", |v| {
        v.expect_error("CONFIG_INVALID", 2)
            .expect_stdout_empty()
            .expect_stderr_contains("config.toml line 1:")
    });
    v.check(
        "only the value changed, with the comment beside it kept",
        first
            == COMMENTED.replace(
                "\"https://svc.example.com\"  # prod",
                "\"https://staging.example.com\"  # prod",
            ),
        format!("observed {first:?}"),
    )
    .check(
        "every edit changed only its own key; the new table is at the end",
        last == COMMENTED
            .replace(
                "\"https://svc.example.com\"  # prod",
                "\"https://staging.example.com\"  # prod",
            )
            .replace("\"debug\"    # loud", "\"info\"    # loud")
            + "\n[aws.session_cache]\nduration = 3600\n",
        format!("observed {last:?}"),
    )
    .check(
        "a file that is not TOML is left as it was",
        broken == BROKEN,
        format!("observed {broken:?}"),
    )
    .expect_sts_actions(&[])
    .expect_op_calls(0)
    .expect_files_written(&["kurama-config.toml"]);
    v.finish();
}

/// `config set --file` replaces each section the input names whole, in its
/// place and under its comment: a key the input leaves out is gone, nothing
/// is merged, and the sections it does not name keep every byte. Two
/// sections that only make sense together -- an auth changing its kind and
/// the API naming it -- change in one write, and an input with one bad
/// section changes neither. `--dry-run` shows the whole replacement.
#[test]
fn config_set_file_replaces_related_sections_atomically() {
    const EXISTING: &str = "# written by hand\n[core]\nlog_level = \"debug\"\n\n\
                            # the machine client\n[auth.svc]\nkind = \"oauth\"\n\
                            grant_type = \"client_credentials\"\ntoken_url = \"https://as.example.com/token\"\n\
                            client_id = \"svc\"\nclient_secret = \"op://Agent/svc/client_secret\"\n\n\
                            [api.svc]\ndescription = \"old\"\nbase_url = \"https://svc.example.com/v1\"\nauth = \"svc\"\n\n\
                            # untouched\n[api.other]\nbase_url = \"https://other.example.com\"\n";
    const REPLACEMENT: &str = "[auth.svc]\nkind = \"token\"\ntoken = \"op://Agent/svc/api-key\"\n\
                               header = \"X-Api-Key\"\nformat = \"{token}\"\n\n\
                               [api.svc]\nbase_url = \"https://svc.example.com/v2\"\nauth = \"svc\"\n";
    const HALF_BAD: &str = "[auth.svc]\nkind = \"token\"\ntoken = \"op://Agent/svc/api-key\"\n\n\
                            [api.svc]\nbase_url = \"https://svc.example.com/v2\"\nauth = \"nope\"\n";
    let scenario = Scenario::new(
        "config_set_file_replaces_related_sections_atomically",
        "config",
        &[],
    )
    .with_config(EXISTING)
    .with_home_file("edit/replacement.toml", REPLACEMENT)
    .with_home_file("edit/half-bad.toml", HALF_BAD);
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    let replacement = sandbox.home.join("edit/replacement.toml");
    let half_bad = sandbox.home.join("edit/half-bad.toml");
    let (replacement, half_bad) = (
        replacement.to_str().unwrap().to_owned(),
        half_bad.to_str().unwrap().to_owned(),
    );
    let mut runs = vec![sandbox.run_cli(&["config", "set", "--file", &half_bad])];
    let after_refusal = std::fs::read_to_string(&config).unwrap();
    runs.push(sandbox.run_cli(&["config", "set", "--file", &replacement, "--dry-run"]));
    runs.push(sandbox.run_cli(&["config", "set", "--file", &replacement, "--json"]));
    let saved = std::fs::read_to_string(&config).unwrap();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "one bad section", |v| {
        v.expect_error("CONFIG_INVALID", 2)
            .expect_stderr_contains("[api.svc]")
            .expect_stderr_contains("names no [auth.nope]")
            .expect_stderr_contains("hint: fix the input; the file was not changed")
    });
    v.keyed_run(1, "dry run", |v| {
        v.expect_exit_code(0)
            .expect_stdout_contains("-description = \"old\"\n")
            .expect_stdout_contains("-grant_type = \"client_credentials\"\n")
            .expect_stdout_contains("+header = \"X-Api-Key\"\n")
            .expect_stderr_contains(
                "# dry run: replaced [auth.svc] whole, replaced [api.svc] whole in ",
            )
    });
    v.keyed_run(2, "replace", |v| {
        v.expect_exit_code(0)
            .expect_stdout_contains("\"section\": \"auth.svc\",\n      \"action\": \"replace\"")
            .expect_stdout_contains("\"section\": \"api.svc\",\n      \"action\": \"replace\"")
    });
    let expected = format!(
        "# written by hand\n[core]\nlog_level = \"debug\"\n\n# the machine client\n{REPLACEMENT}\n\
         # untouched\n[api.other]\nbase_url = \"https://other.example.com\"\n"
    );
    v.check(
        "a refused input leaves the file byte for byte",
        after_refusal == EXISTING,
        format!("observed {after_refusal:?}"),
    )
    .check(
        "both sections are replaced whole in their place, the rest keeps every byte",
        saved == expected,
        format!("observed {saved:?}"),
    )
    .expect_sts_actions(&[])
    .expect_op_calls(0)
    .expect_files_written(&["kurama-config.toml"]);
    v.finish();
}

/// The preset flow the edit commands exist for: two Google presets share
/// one `[auth.google]`; the scope the second lacks is added with the
/// `config set` its step names; adding that preset again is refused as a
/// collision; removing its API keeps the shared auth, with the scope, for
/// the API still using it, and the auth itself cannot be removed while that
/// API is there.
#[test]
fn config_remove_api_keeps_a_shared_auth() {
    const SCOPES: &str = "[\"https://www.googleapis.com/auth/spreadsheets.readonly\",\"https://www.googleapis.com/auth/documents.readonly\"]";
    let scenario = Scenario::new("config_remove_api_keeps_a_shared_auth", "config", &[])
        .with_config("# mine\n[core]\nlog_level = \"info\"\n");
    let mut sandbox = Sandbox::create(&scenario);
    let config = sandbox.kurama_config().to_path_buf();
    let mut runs = vec![
        sandbox.run_cli(&[
            "preset",
            "add",
            "google-sheets",
            "--set",
            "client_id=id-1",
            "--set",
            "client_secret=op://Agent/kurama-google/client_secret",
        ]),
        sandbox.run_cli(&["preset", "add", "google-docs"]),
        sandbox.run_cli(&["config", "set", "auth.google.scopes", SCOPES]),
        sandbox.run_cli(&["preset", "add", "google-docs"]),
        sandbox.run_cli(&["config", "show", "auth.google"]),
    ];
    let before_remove = std::fs::read_to_string(&config).unwrap();
    runs.push(sandbox.run_cli(&["config", "remove", "api.google-docs"]));
    runs.push(sandbox.run_cli(&["config", "show", "auth.google"]));
    runs.push(sandbox.run_cli(&["config", "remove", "auth.google"]));
    runs.push(sandbox.run_cli(&["config", "list"]));
    let after = std::fs::read_to_string(&config).unwrap();

    let mut v = sandbox.finish(scenario.id, scenario.feature, runs, vec![]);
    v.keyed_run(0, "first preset", |v| {
        v.expect_exit_code(0)
            .expect_stderr_contains("# added [auth.google], [api.google-sheets] to ")
    });
    v.keyed_run(1, "second preset", |v| {
        v.expect_exit_code(0)
            .expect_stderr_contains("# added [api.google-docs] to ")
            .expect_stderr_contains(&format!("kurama config set auth.google.scopes '{SCOPES}'"))
    });
    v.keyed_run(2, "add the scope", |v| {
        v.expect_exit_code(0)
            .expect_stderr_contains("# set auth.google.scopes in ")
    });
    v.keyed_run(3, "the preset again", |v| {
        v.expect_error("ARGUMENT_INVALID", 2)
            .expect_stderr_contains("[api.google-docs]")
            .expect_stderr_contains("--as")
    });
    v.keyed_run(4, "the auth before", |v| {
        v.expect_exit_code(0)
            .expect_stdout_contains(&format!("scopes = {SCOPES}\n"))
    });
    v.keyed_run(5, "remove the API", |v| {
        v.expect_exit_code(0)
            .expect_stderr_contains("# removed [api.google-docs] in ")
    });
    let shown_before = v.observed.runs[4].stdout.clone();
    v.keyed_run(6, "the auth after", |v| {
        v.expect_exit_code(0).expect_stdout_equals(&shown_before)
    });
    v.keyed_run(7, "remove the auth", |v| {
        v.expect_error("CONFIG_INVALID", 2).expect_stderr_contains(
            "[auth.google] is still used by [api.google-sheets]; nothing was removed",
        )
    });
    v.keyed_run(8, "what is left", |v| {
        v.expect_exit_code(0)
            .expect_stdout_contains("auth.google: ")
            .expect_stdout_contains("api.google-sheets: ")
    });
    // The docs API was appended last, under its preset comment: removing it
    // removes that block, blank line and comment included, and nothing else.
    let kept = before_remove
        .find("\n# kurama preset: google-docs")
        .map(|cut| &before_remove[..cut]);
    v.check(
        "the docs API and its comment are gone, every other byte is kept",
        kept == Some(after.as_str()) && after.starts_with("# mine\n[core]\n"),
        format!("before {before_remove:?}, after {after:?}"),
    )
    .expect_sts_actions(&[])
    .expect_op_calls(0)
    .expect_api_call_count(0)
    .expect_files_written(&["kurama-config.toml"]);
    v.finish();
}
