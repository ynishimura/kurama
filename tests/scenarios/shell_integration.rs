//! Runtime verification scenarios of the `shell-integration` feature.
//! `tests/scenarios/main.rs` holds the naming rule and what each
//! scenario writes.

use crate::support::tui::{self, Key};
use crate::support::{COMPLETION_CONFIG, Scenario, SpecFake, completion_values, run};

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_completes_a_subcommand() {
    let id = "completion_zsh_completes_a_subcommand";
    let mut t = tui::launch_zsh(
        Scenario::tui(id).for_feature("shell-integration"),
        tui::STANDARD,
    );
    // Correct a typo through the PTY before completing the resulting `kurama sta`.
    t.type_text("kurama stax")
        .key(Key::Left)
        .key(Key::Delete)
        .key(Key::Left)
        .key(Key::Right)
        .expect_command_line("kurama sta")
        .key(Key::Tab)
        .expect_command_line("kurama status ")
        .snapshot("subcommand");
    let mut v = t.exit_zsh();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_api_calls(0, None);
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_lists_flags_with_their_descriptions() {
    let id = "completion_zsh_lists_flags_with_their_descriptions";
    let mut t = tui::launch_zsh(
        Scenario::tui(id).for_feature("shell-integration"),
        tui::STANDARD,
    );
    t.type_text("kurama api x --")
        .key(Key::Tab)
        .wait_for("--ops")
        .expect_row_with(&["--ops", "List the operations"])
        .snapshot("flags");
    let mut v = t.exit_zsh();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_api_calls(0, None);
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_completes_the_shell_name() {
    let id = "completion_zsh_completes_the_shell_name";
    let mut t = tui::launch_zsh(
        Scenario::tui(id).for_feature("shell-integration"),
        tui::STANDARD,
    );
    t.type_text("kurama init ")
        .key(Key::Tab)
        .expect_command_line("kurama init zsh ")
        .snapshot("shell");
    let mut v = t.exit_zsh();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_api_calls(0, None);
    v.finish();
}
#[test]
fn completion_profiles_are_scoped_and_sorted() {
    let mut v = run(Scenario::completion(
        "completion_profiles_are_scoped_and_sorted",
        &["--", "kurama", "env", ""],
        "2",
    )
    .with_config(COMPLETION_CONFIG)
    .then_run(&["--", "kurama", "exec", ""])
    .then_run(&["--", "kurama", "login", ""])
    .then_run(&["--", "kurama", "logout", ""])
    .then_run(&["--", "kurama", "status", ""])
    .then_run(&["--", "kurama", "console", ""])
    .then_run(&["--", "kurama", "token", ""]));
    let runs = v.observed.runs.clone();
    for (i, run) in runs.iter().enumerate() {
        let expected = match i {
            5 => vec!["default", "dev", "ops-mfa"],
            6 => vec!["agent", "worker"],
            _ => vec!["agent", "default", "dev", "ops-mfa", "worker"],
        };
        v.check(
            &format!("run {i} completes only its source namespace in name order"),
            completion_values(&run.stdout) == expected,
            run.stdout.clone(),
        );
    }
    v.check(
        "profile help includes the AWS account and OAuth grant/host",
        runs[0].stdout.contains("123456789012")
            && runs[0]
                .stdout
                .contains("agent:client_credentials 127.0.0.1"),
        runs[0].stdout.clone(),
    );
    v.finish();
}
#[test]
fn completion_api_names_include_descriptions() {
    let mut v = run(Scenario::completion(
        "completion_api_names_include_descriptions",
        &["--", "kurama", "api", ""],
        "2",
    )
    .with_config(COMPLETION_CONFIG));
    v.expect_stdout_contains("pets:Pets: dev")
        .expect_stdout_contains("remote:http://127.0.0.1:");
    let stdout = v.observed.last().stdout.clone();
    v.check(
        "API names are sorted and exclude credential sources and AWS profiles",
        completion_values(&stdout) == ["cached", "pets", "remote"],
        stdout,
    );
    v.finish();
}
/// `config show`, `config list` and `config remove` complete the sections
/// the file saves, and `config set` and `config unset` its keys, read for
/// syntax only: a file whose `[api.*]` names an auth nobody wrote does not
/// load, and its sections and keys are still offered.
#[test]
fn completion_config_sections_come_from_the_saved_file() {
    let mut v = run(Scenario::completion(
        "completion_config_sections_come_from_the_saved_file",
        &["--", "kurama", "config", "show", ""],
        "3",
    )
    .with_config(
        "[core]\nlog_level = \"info\"\n\n[api.orphan]\nbase_url = \"https://x\"\nauth = \"missing\"\n",
    )
    .then_run(&["--", "kurama", "config", "list", ""])
    .then_run(&["--", "kurama", "config", "remove", ""])
    .then_run(&["--", "kurama", "config", "set", ""])
    .then_run(&["--", "kurama", "config", "unset", ""]));
    for index in 0..5 {
        let stdout = v.observed.runs[index].stdout.clone();
        let expected: &[&str] = if index < 3 {
            &["api.orphan", "core"]
        } else {
            &["api.orphan.auth", "api.orphan.base_url", "core.log_level"]
        };
        v.check(
            &format!("run {index}: the saved sections or keys, sorted"),
            completion_values(&stdout) == expected,
            stdout,
        );
    }
    v.finish();
}
/// `preset setup` completes the preset ids from the catalog compiled into
/// the binary: a config.toml that is not TOML, which empties every
/// completion that reads it, leaves them all, each with its title.
#[test]
fn completion_preset_completes_ids_without_reading_config() {
    let mut v = run(Scenario::completion(
        "completion_preset_completes_ids_without_reading_config",
        &["--", "kurama", "preset", "setup", ""],
        "3",
    )
    .with_config("[invalid TOML")
    .then_run(&["--", "kurama", "preset", "setup", "goo"])
    .then_run(&["--", "kurama", "preset", "setup", "lin"]));
    let all = v.observed.runs[0].stdout.clone();
    v.check(
        "an id carries its title",
        all.contains("linear:Linear GraphQL API with a personal API key"),
        all.clone(),
    );
    v.check(
        "every preset id, sorted by the catalog's order",
        completion_values(&all)
            == [
                "github",
                "github-oauth",
                "google-sheets",
                "google-docs",
                "google-drive",
                "linear",
                "linear-oauth",
                "elevenlabs",
                "openai",
                "slack",
                "contentful",
                "fireworks",
                "jira",
                "zendesk",
                "backlog",
            ],
        all,
    );
    let prefixed = v.observed.runs[1].stdout.clone();
    v.check(
        "the ids that start with the word",
        completion_values(&prefixed) == ["google-sheets", "google-docs", "google-drive"],
        prefixed,
    );
    let added = v.observed.last().stdout.clone();
    v.check(
        "another word, another set of ids",
        completion_values(&added) == ["linear", "linear-oauth"],
        added,
    );
    v.finish();
}
#[test]
fn completion_invalid_config_and_missing_aws_file_are_silent() {
    let mut v = run(Scenario::completion(
        "completion_invalid_config_and_missing_aws_file_are_silent",
        &["--", "kurama", "env", ""],
        "2",
    )
    .with_config("[invalid TOML")
    .with_env(
        "AWS_CONFIG_FILE",
        "/nonexistent/kurama-completion-aws-config",
    )
    .then_run(&["--", "kurama", "api", ""])
    .then_run(&["--", "kurama", "token", ""])
    .then_run(&["--", "kurama", "console", ""]));
    let outputs: Vec<_> = v
        .observed
        .runs
        .iter()
        .map(|run| run.stdout.clone())
        .collect();
    v.check(
        "every malformed/missing-source completion has zero candidates",
        outputs.iter().all(String::is_empty),
        format!("{outputs:?}"),
    );
    v.finish();
}
#[test]
fn completion_missing_explicit_config_is_silent() {
    let mut v = run(Scenario::completion(
        "completion_missing_explicit_config_is_silent",
        &["--", "kurama", "env", ""],
        "2",
    )
    .with_env(
        "KURAMA_CONFIG_PATH",
        "/nonexistent/kurama-completion-explicit-config.toml",
    ));
    let stdout = v.observed.last().stdout.clone();
    v.check(
        "a missing explicitly named config declines even available AWS profiles",
        stdout.is_empty(),
        stdout,
    );
    v.finish();
}
#[test]
fn completion_missing_aws_file_keeps_auth_sources() {
    let mut v = run(Scenario::completion(
        "completion_missing_aws_file_keeps_auth_sources",
        &["--", "kurama", "env", ""],
        "2",
    )
    .with_config(COMPLETION_CONFIG)
    .with_env(
        "AWS_CONFIG_FILE",
        "/nonexistent/kurama-completion-aws-config",
    ));
    let stdout = v.observed.last().stdout.clone();
    v.check(
        "missing AWS input still completes configured auth sources",
        completion_values(&stdout) == ["agent", "worker"],
        stdout,
    );
    v.finish();
}
#[test]
fn completion_absent_default_config_still_lists_aws_profiles() {
    let mut v = run(Scenario::completion(
        "completion_absent_default_config_still_lists_aws_profiles",
        &["--", "kurama", "env", ""],
        "2",
    )
    .with_env("KURAMA_CONFIG_PATH", ""));
    let stdout = v.observed.last().stdout.clone();
    v.check(
        "a missing optional default config still permits AWS completion",
        completion_values(&stdout) == ["default", "dev", "ops-mfa"],
        stdout,
    );
    v.finish();
}
#[test]
fn completion_operations_use_only_files_and_cached_urls() {
    let mut v = run(Scenario::completion(
        "completion_operations_use_only_files_and_cached_urls",
        &["--", "kurama", "api", "pets", "pets/"],
        "3",
    )
    .with_config(COMPLETION_CONFIG)
    .spec(SpecFake::Down)
    .with_seeded_spec_cache("/spec/petstore.json", "petstore.json", 86400)
    .then_run(&["--", "kurama", "api", "cached", "pets/"])
    .then_run(&["--", "kurama", "api", "remote", "pets/"]));
    let runs = v.observed.runs.clone();
    v.check(
        "file and expired cached descriptions complete the same matching operations",
        completion_values(&runs[0].stdout) == ["pets/create", "pets/get", "pets/list"]
            && runs[0].stdout == runs[1].stdout,
        format!("{:?} / {:?}", runs[0].stdout, runs[1].stdout),
    );
    v.check(
        "a URL without a cached description returns no candidates",
        runs[2].stdout.is_empty(),
        runs[2].stdout.clone(),
    );
    v.expect_run_spec_calls(0, 0, false, 0)
        .expect_run_spec_calls(1, 0, false, 0)
        .expect_run_spec_calls(2, 0, false, 0);
    v.finish();
}
#[test]
fn completion_describe_uses_operation_candidates() {
    let mut v = run(Scenario::completion(
        "completion_describe_uses_operation_candidates",
        &["--", "kurama", "api", "pets", "--describe", "pets/l"],
        "4",
    )
    .with_config(COMPLETION_CONFIG));
    v.expect_stdout_contains("pets/list:GET /pets");
    let stdout = v.observed.last().stdout.clone();
    v.check(
        "--describe completes the matching operation",
        completion_values(&stdout) == ["pets/list"],
        stdout,
    );
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_api_and_operation_names_bypass_the_wrapper() {
    let id = "completion_zsh_api_and_operation_names_bypass_the_wrapper";
    let mut t = tui::launch_zsh(
        Scenario::tui(id)
            .for_feature("shell-integration")
            .with_config(COMPLETION_CONFIG),
        tui::STANDARD,
    );
    t.type_text("kurama() { print -r -- called > wrapper-called; }")
        .key(Key::Enter)
        .expect_command_line("");
    t.type_text("kurama api pe")
        .key(Key::Tab)
        .expect_command_line("kurama api pets ")
        .snapshot("api-name");
    t.type_text("pets/li")
        .key(Key::Tab)
        .expect_command_line("kurama api pets pets/list ")
        .snapshot("operation-name");
    let mut v = t.exit_zsh();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_api_calls(0, None)
        .expect_run_spec_calls(0, 0, false, 0);
    let files = v.observed.files_written.clone();
    v.check(
        "completion bypasses the shell wrapper without writing files",
        files.is_empty(),
        format!("{files:?}"),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_preserves_data_file_and_directory_completion() {
    let id = "completion_zsh_preserves_data_file_and_directory_completion";
    let mut t = tui::launch_zsh(
        Scenario::tui(id)
            .for_feature("shell-integration")
            .with_home_file("inputs/request.json", "{}\n"),
        tui::STANDARD,
    );
    for prefix in [
        "kurama data ",
        "kurama data --from ",
        "kurama data --request ",
        "kurama data --file ",
        "kurama data --export ",
    ] {
        t.key(Key::Ctrl('u'))
            .type_text(prefix)
            .type_text("inp")
            .key(Key::Tab)
            .expect_command_line(&format!("{prefix}inputs/"));
        t.type_text("req")
            .key(Key::Tab)
            .expect_command_line(&format!("{prefix}inputs/request.json "));
    }
    t.snapshot("data-file");
    let mut v = t.exit_zsh();
    v.expect_exit_code(0)
        .expect_sts_actions(&[])
        .expect_op_calls(0)
        .expect_api_calls(0, None)
        .expect_run_spec_calls(0, 0, false, 0);
    let files = v.observed.files_written.clone();
    v.check(
        "completion leaves the shell setup and input fixture unchanged",
        files.is_empty(),
        format!("{files:?}"),
    );
    v.finish();
}
#[test]
fn completion_parameters_use_the_target() {
    let mut v = run(Scenario::completion(
        "completion_parameters_use_the_target",
        &["--", "kurama", "api", "pets", "pets/get", "-P", ""],
        "5",
    )
    .with_config(COMPLETION_CONFIG)
    .then_run(&["--", "kurama", "api", "pets", "GET /pets/{petId}", "-P", ""])
    .then_run(&["--", "kurama", "api", "pets", "unknown", "-P", ""])
    .then_run(&["--", "kurama", "api", "remote", "pets/get", "-P", ""])
    .then_run(&["--", "kurama", "api", "pets", "--dry-run", "-P", ""]));
    let runs = v.observed.runs.clone();
    v.check(
        "id and method/path select required petId before header X-Trace",
        completion_values(&runs[0].stdout) == ["petId=", "X-Trace="]
            && runs[0].stdout == runs[1].stdout,
        format!("{:?}", runs.iter().map(|r| &r.stdout).collect::<Vec<_>>()),
    );
    v.check(
        "name help includes location, type and required",
        runs[0].stdout.contains("petId=:path string (required)"),
        runs[0].stdout.clone(),
    );
    v.check(
        "unknown operation, uncached URL and missing target have zero candidates",
        runs[2..].iter().all(|r| r.stdout.is_empty()),
        format!("{:?}", &runs[2..]),
    );
    v.finish();
}
#[test]
fn completion_parameters_omit_given_names() {
    let mut v = run(Scenario::completion(
        "completion_parameters_omit_given_names",
        &[
            "--", "kurama", "api", "pets", "pets/get", "-P", "petId=42", "-P", "",
        ],
        "7",
    )
    .with_config(COMPLETION_CONFIG)
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "pets/get",
        "-P",
        "x-trace=t1",
        "-P",
        "",
    ]));
    let runs = v.observed.runs.clone();
    v.check(
        "an already assigned name is excluded",
        completion_values(&runs[0].stdout) == ["X-Trace="],
        runs[0].stdout.clone(),
    );
    v.check(
        "assigned headers match without regard to ASCII case",
        completion_values(&runs[1].stdout) == ["petId="],
        runs[1].stdout.clone(),
    );
    v.finish();
}
#[test]
fn completion_parameter_values_filter_the_current_value_prefix() {
    let mut v = run(Scenario::completion(
        "completion_parameter_values_filter_the_current_value_prefix",
        &["--", "kurama", "api", "pets", "pets/list", "-P", "status="],
        "5",
    )
    .with_config(COMPLETION_CONFIG)
    .then_run(&["--", "kurama", "api", "pets", "pets/list", "-P", "status=s"])
    .then_run(&["--", "kurama", "api", "pets", "pets/list", "-P", "limit="])
    .then_run(&[
        "--",
        "kurama",
        "api",
        "pets",
        "owners/list-pets",
        "-P",
        "adopted=",
    ])
    .then_run(&["--", "kurama", "api", "pets", "pets/get", "-P", "petId="])
    .then_run(&["--", "kurama", "api", "pets", "pets/get", "-P", "missing="]));
    let expected = [
        vec!["status=available", "status=sold"],
        vec!["status=sold"],
        vec!["limit=20"],
        vec!["adopted=true", "adopted=false"],
        vec![],
        vec![],
    ];
    let outputs: Vec<_> = v.observed.runs.iter().map(|r| r.stdout.clone()).collect();
    for (i, (stdout, expected)) in outputs.iter().zip(expected).enumerate() {
        v.check(
            &format!("run {i} returns only matching values with the name retained"),
            completion_values(stdout) == expected,
            stdout.clone(),
        );
    }
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn completion_zsh_inserts_a_parameter_name_without_a_space() {
    let id = "completion_zsh_inserts_a_parameter_name_without_a_space";
    let mut t = tui::launch_zsh(
        Scenario::tui(id)
            .for_feature("shell-integration")
            .with_config(COMPLETION_CONFIG),
        tui::STANDARD,
    );
    t.type_text("kurama api pets pets/get -P pet")
        .key(Key::Tab)
        .expect_command_line("kurama api pets pets/get -P petId=")
        .snapshot("parameter-name");
    t.type_text("42")
        .expect_command_line("kurama api pets pets/get -P petId=42");
    t.key(Key::Ctrl('u'))
        .type_text("kurama api pets pets/list -P status=s")
        .key(Key::Tab)
        .expect_command_line("kurama api pets pets/list -P status=sold ")
        .snapshot("parameter-value");
    for target in [
        r#""GET /pets/{petId}""#,
        "'GET /pets/{petId}'",
        r"GET\ /pets/\{petId\}",
    ] {
        t.key(Key::Ctrl('u'))
            .type_text(&format!("kurama api pets {target} -P pet"))
            .key(Key::Tab)
            .expect_command_line(&format!("kurama api pets {target} -P petId="));
    }
    t.key(Key::Ctrl('u'))
        .type_text(r#"kurama api pets pets/list -P "status=s"#)
        .key(Key::Tab)
        .expect_command_line("kurama api pets pets/list -P \"status=sold\" ")
        .snapshot("quoted-parameter-value");
    t.key(Key::Ctrl('u'))
        .type_text("setopt COMPLETE_IN_WORD")
        .key(Key::Enter)
        .expect_command_line("")
        .type_text("kurama api pets pets/list -P status=sd")
        .key(Key::Left)
        .key(Key::Tab)
        .wait_for("kurama api pets pets/list -P status=sold");
    for quote in ['\"', '\''] {
        t.key(Key::Ctrl('u'))
            .type_text(&format!(
                "kurama api pets pets/list -P {quote}status=s{quote}"
            ))
            .key(Key::Tab)
            .expect_command_line(&format!(
                "kurama api pets pets/list -P {quote}status=sold{quote}"
            ));
    }
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

/// A `kind = "token"` source alongside an OAuth one. Completion reads the
/// configuration and nothing else, so what it says about each has to come
/// from the file it just parsed.
const TOKEN_SOURCE_COMPLETION_CONFIG: &str = r#"
[auth.agent]
kind = "oauth"
grant_type = "client_credentials"
token_url = "{server}/oauth/token"
client_id = "agent-client"
[auth.example]
kind = "token"
token = "op://Agent/Example/credential"
header = "xi-api-key"
format = "{token}"
env_var = "EXAMPLE_TOKEN"
[api.example]
base_url = "{server}/api"
"#;

/// Completion must describe a `kind = "token"` source without resolving it:
/// it runs on every Tab, and its description must not be the reference the
/// credential is kept behind.
#[test]
fn completion_lists_a_token_source_by_its_header() {
    let mut v = run(Scenario::completion(
        "completion_lists_a_token_source_by_its_header",
        &["--", "kurama", "token", ""],
        "2",
    )
    .with_config(TOKEN_SOURCE_COMPLETION_CONFIG)
    .then_run(&["--", "kurama", "env", ""])
    .then_run(&["--", "kurama", "api", ""]));
    let runs = v.observed.runs.clone();
    v.check(
        "both kinds of source are offered, in name order",
        completion_values(&runs[0].stdout) == ["agent", "example"],
        runs[0].stdout.clone(),
    );
    v.check(
        "the token source is described by the header it sends, not by its reference",
        runs[0].stdout.contains("example:token xi-api-key") && !runs[0].stdout.contains("op://"),
        runs[0].stdout.clone(),
    );
    v.check(
        "env offers it too, and the API that uses it is a separate namespace",
        completion_values(&runs[1].stdout).contains(&"example")
            && completion_values(&runs[2].stdout) == ["example"],
        format!("env {:?} api {:?}", runs[1].stdout, runs[2].stdout),
    );
    v.finish();
}
#[test]
fn completion_parameter_candidates_cannot_break_candidate_protocol() {
    let config = format!(
        "[api.items]\nbase_url = \"{{server}}/api\"\nopenapi = \"{}/tests/fixtures/shell-integration/completion-newlines.json\"\n",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut v = run(Scenario::completion(
        "completion_parameter_candidates_cannot_break_candidate_protocol",
        &["--", "kurama", "api", "items", "items/list", "-P", ""],
        "5",
    )
    .with_config(&config)
    .then_run(&["--", "kurama", "api", "items", "items/list", "-P", "limit="])
    .then_run(&["--", "kurama", "api", "items", "items/list", "-P", "order="]));
    let runs = v.observed.runs.clone();
    // The description drops every control character from the document, so
    // each parameter and each value stays one record of the protocol, and a
    // line break never adds a candidate of its own.
    let records = |i: usize| runs[i].stdout.lines().collect::<Vec<_>>();
    v.check(
        "a name, a type and a default with a line break are each one record",
        records(0)
            == [
                "limit=:query ten|twentyinjected=value|thirty (default forty) (required)",
                "evilinjected==:query string",
                "order=:query stringinjected (default ascdesc)",
            ],
        runs[0].stdout.clone(),
    );
    v.check(
        "enum, default and example values with a control character are one record each",
        records(1)
            == [
                "limit=ten:enum",
                "limit=twentyinjected=value:enum",
                "limit=thirty:enum",
                "limit=forty:default",
                "limit=fifty:example",
            ],
        runs[1].stdout.clone(),
    );
    v.check(
        "a default made of two lines is one value",
        records(2) == ["order=ascdesc:default"],
        runs[2].stdout.clone(),
    );
    v.finish();
}
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
fn init_zsh_wrapper_sources_exports_and_preserves_exit() {
    let id = "init_zsh_wrapper_sources_exports_and_preserves_exit";
    let mut t = tui::launch_zsh_wrapper(
        Scenario::tui(id).for_feature("shell-integration"),
        tui::STANDARD,
    );
    t.type_text("kurama env dev; print -r -- \"exit=$? profile=$KURAMA_AWS region=$AWS_REGION\"")
        .key(Key::Enter)
        .wait_for("exit=0 profile=dev region=ap-northeast-1");
    t.type_text("kurama env missing 2>/dev/null; print -r -- \"failed=$?\"")
        .key(Key::Enter)
        .wait_until("the failed exit code is printed", |screen| {
            screen.lines().any(|line| line.starts_with("failed="))
        });
    t.type_text("left=($TMPDIR/kurama_env.*(N)); print -r -- \"left=${#left}\"")
        .key(Key::Enter)
        .wait_for("left=0");
    let screen = t.screen_text();
    let mut v = t.exit_zsh();
    v.expect_exit_code(0).expect_sts_actions(&["AssumeRole"]);
    v.check(
        "the failed run returns the binary's own exit code, not the wrapper's",
        screen.lines().any(|line| line == "failed=2"),
        screen.clone(),
    );
    v.finish();
}
