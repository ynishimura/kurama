//! The dependency and purity rules of the layers, and the one-place rules:
//! which files may spawn, sign, rewrite a line, take the blocking lock, or
//! print a progress line.

use crate::sigint_listener::{LATE_LISTENERS, REGISTERED_WITH_THE_WORK, late_ctrl_c_listeners};
use crate::support::*;

const PURE_LAYERS: [&str; 2] = ["src/domain", "src/workflows"];

const FORBIDDEN_IN_PURE_LAYERS: [&str; 17] = [
    "crate::adapters",
    "crate::shell",
    "tokio::",
    "std::fs",
    "std::env",
    "std::process",
    "std::net",
    "reqwest",
    "aws_config",
    "aws_sdk",
    "keyring",
    "tracing::",
    "crossterm",
    "ratatui",
    "Utc::now",
    "SystemTime::now",
    "Instant::now",
];

/// The TUI's pure files: they draw a model and change state, nothing else.
const TUI_PURE_PATHS: [&str; 24] = [
    "src/shell/tui/components",
    "src/shell/tui/layout.rs",
    "src/shell/tui/theme.rs",
    "src/shell/tui/tea/update.rs",
    "src/shell/tui/tea/view.rs",
    "src/shell/tui/explorer/update.rs",
    "src/shell/tui/explorer/jq_input.rs",
    "src/shell/tui/explorer/jq_view.rs",
    "src/shell/tui/explorer/view.rs",
    "src/shell/tui/explorer/messages.rs",
    "src/shell/tui/explorer/model.rs",
    "src/shell/tui/explorer/effects.rs",
    "src/shell/tui/database/update.rs",
    "src/shell/tui/database/view.rs",
    "src/shell/tui/database/messages.rs",
    "src/shell/tui/database/model.rs",
    "src/shell/tui/database/effects.rs",
    "src/shell/tui/s3/update.rs",
    "src/shell/tui/s3/view.rs",
    "src/shell/tui/s3/messages.rs",
    "src/shell/tui/s3/model.rs",
    "src/shell/tui/s3/effects.rs",
    "src/shell/tui/activity/update.rs",
    "src/shell/tui/activity/view.rs",
];

/// What the TUI's pure files may not touch: everything the domain may not,
/// except the terminal crates they draw with and their sibling modules.
const FORBIDDEN_IN_TUI_PURE_PATHS: [&str; 14] = [
    "crate::adapters",
    "tokio::",
    "std::fs",
    "std::env",
    "std::process",
    "std::net",
    "reqwest",
    "aws_config",
    "aws_sdk",
    "keyring",
    "tracing::",
    "Utc::now",
    "SystemTime::now",
    "Instant::now",
];

const FORBIDDEN_IN_PORTS: [&str; 5] = [
    "crate::adapters",
    "crate::shell",
    "std::fs",
    "std::process",
    "reqwest",
];

#[test]
fn domain_and_workflows_do_no_io_and_read_no_clock() {
    let found: Vec<String> = PURE_LAYERS
        .iter()
        .flat_map(|dir| violations(dir, &FORBIDDEN_IN_PURE_LAYERS))
        .collect();
    assert!(
        found.is_empty(),
        "pure layers must not perform I/O, read the clock, log, or depend on adapters/shell:\n{}",
        found.join("\n")
    );
}

#[test]
fn tui_view_update_layout_theme_and_components_are_pure() {
    let found: Vec<String> = TUI_PURE_PATHS
        .iter()
        .flat_map(|path| violations(path, &FORBIDDEN_IN_TUI_PURE_PATHS))
        .collect();
    assert!(
        found.is_empty(),
        "the TUI's view, update, layout, theme and components must not perform I/O, read the clock, log, or depend on adapters:\n{}",
        found.join("\n")
    );
}

/// Spawning a child process is where stdio, reaping and deadlines go wrong:
/// a pipe left undrained deadlocks, an inherited stdin steals the terminal,
/// an uncaptured stderr loses the child's own message. Each of these files
/// owns one child and names the test that exercises it; another must be a
/// deliberate decision, not a stray `Command::new`. `allowlists.rs` holds
/// each entry to an existing file that still spawns, a reason and that test.
pub(crate) const FILES_THAT_SPAWN: [Allowed; 7] = [
    (
        "src/adapters/own_command.rs",
        "kurama itself for `kurama mcp` and the Obsidian CLI, with stdin fed or null, both streams captured and a deadline",
        "a_call_that_does_not_end_is_killed_at_the_deadline",
    ),
    (
        "src/adapters/auth/op_cli.rs",
        "the 1Password CLI, with no stdin, captured stderr and a deadline",
        "a_read_nobody_answers_is_killed_at_the_deadline",
    ),
    (
        "src/adapters/aws/ssm_tunnel.rs",
        "session-manager-plugin, which holds a port forward open while a query runs",
        "db_tunnel_reads_the_port_out_of_the_line_the_plugin_prints",
    ),
    (
        "src/adapters/browser.rs",
        "the browser opener, detached with every stream closed",
        "oauth_login_authorization_code_opens_the_browser_and_stores_the_token",
    ),
    (
        "src/adapters/clipboard.rs",
        "pbcopy / xclip, fed the text on stdin and waited for",
        "tui_explorer_edits_the_body_in_the_editor_and_copies_the_command",
    ),
    (
        "src/adapters/editor.rs",
        "$EDITOR, which takes the terminal while the TUI is suspended",
        "the_editor_sees_the_text_in_a_private_file_that_is_removed_afterwards",
    ),
    (
        "src/shell/cli/executor.rs",
        "the command `kurama exec` runs with the role credentials in its environment",
        "exec_child_gets_the_credentials_but_not_the_wrapper_script",
    ),
];

/// Whether a source calls `Command::new` of `std::process` or
/// `tokio::process`, however it was imported (`syntax.rs`). `clap::Command`
/// shares the name, so the import decides.
pub(crate) fn spawns_a_child(source: &str) -> bool {
    !crate::syntax::read_source(source).spawns.is_empty()
}

#[test]
fn a_spawn_through_any_import_or_across_lines_is_found() {
    for source in [
        "use std::process::{Command, Stdio};\nfn f() { Command::new(\"x\"); }",
        "use std::process::Command;\nfn f() { Command::new(\"x\"); }",
        "use tokio::process::{Child, Command};\nfn f() { Command::new(\"x\"); }",
        "use std::process::Command as Child;\nfn f() { Child::new(\"x\"); }",
        "use std::{io, process::{Command, Stdio}};\nfn f() { Command::new(\"x\"); }",
        "fn f() {\n    std::process::Command\n        ::new(\"x\");\n}",
    ] {
        assert_detected("ARCH-025", spawns_a_child(source), source);
    }
}

#[test]
fn a_clap_command_a_comment_a_string_or_a_test_is_not_a_spawn() {
    for source in [
        "use clap::Command;\nuse std::process::{ExitCode, Stdio};\nfn f() { Command::new(\"kurama\"); }",
        "// std::process::Command::new(\"x\")\nfn f() {}",
        "fn f() -> &'static str { \"std::process::Command::new\" }",
        "#[cfg(test)]\nmod tests {\n    use std::process::Command;\n    fn f() { Command::new(\"x\"); }\n}",
    ] {
        assert_allowed("ARCH-025", !spawns_a_child(source), source);
    }
}

#[test]
fn a_forbidden_import_nested_aliased_or_split_is_found() {
    let forbidden = [
        "crate::adapters",
        "std::fs",
        "tokio::",
        "aws_sdk",
        "Utc::now",
    ];
    for source in [
        "use crate::{domain::x, adapters::aws::sts};",
        "use std::fs as filesystem;",
        "use std::{\n    collections::BTreeMap,\n    fs,\n};",
        "fn f() { let _ = tokio::time::sleep; }",
        "use aws_sdk_sts::Client;",
        "fn f() { chrono::Utc\n    ::now(); }",
        "fn f() { tracing::info!(\"{}\", crate::adapters::x()); }",
    ] {
        assert_detected(
            "ARCH-001",
            !violations_in(source, &forbidden).is_empty(),
            source,
        );
    }
}

#[test]
fn a_forbidden_path_in_a_comment_a_string_or_test_code_is_not_found() {
    let forbidden = ["crate::adapters", "std::fs", "tokio::"];
    for source in [
        "// use crate::adapters::x;\nfn f() {}",
        "/// Reads with `std::fs` in the shell.\nfn f() {}",
        "fn f() -> &'static str { \"crate::adapters\" }",
        "#[cfg(test)]\nmod tests {\n    use std::fs;\n    #[tokio::test]\n    async fn t() {}\n}",
        "#[test]\nfn t() { let _ = std::fs::read(\"x\"); }",
        "use crate::domain::adapters_note;",
    ] {
        assert_allowed(
            "ARCH-001",
            violations_in(source, &forbidden).is_empty(),
            source,
        );
    }
}

#[test]
fn only_named_files_start_a_child_process() {
    let found: Vec<String> = rust_files(&root().join("src"))
        .into_iter()
        .filter(|path| spawns_a_child(&std::fs::read_to_string(path).unwrap()))
        .map(|path| {
            path.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|path| !allows(&FILES_THAT_SPAWN, path))
        .collect();
    assert!(
        found.is_empty(),
        "a child process is started outside the files that own one; run it through \
         `OpCli` when it is `op`, or add the file to FILES_THAT_SPAWN with tests for \
         its stdio, reaping and deadline:\n{}",
        found.join("\n")
    );
}

#[test]
fn adapters_do_not_depend_on_the_shell() {
    let found = violations("src/adapters", &["crate::shell"]);
    assert!(
        found.is_empty(),
        "adapters must not depend on the shell:\n{}",
        found.join("\n")
    );
}

#[test]
fn ports_hold_traits_and_data_only() {
    let found = violations("src/ports", &FORBIDDEN_IN_PORTS);
    assert!(
        found.is_empty(),
        "ports must not depend on adapters or perform I/O:\n{}",
        found.join("\n")
    );
}

#[test]
fn path_attributes_load_test_files_only() {
    let mut found = Vec::new();
    for path in rust_files(&root().join("src")) {
        let content = std::fs::read_to_string(&path).unwrap();
        for (number, line) in content.lines().enumerate() {
            let Some(target) = line.trim().strip_prefix("#[path = \"") else {
                continue;
            };
            if !target.trim_end_matches("\"]").ends_with("tests.rs") {
                found.push(location(&path, number, line));
            }
        }
    }
    assert!(
        found.is_empty(),
        "`#[path]` is for `<name>_tests.rs` only; a production submodule is a file under its parent's directory:\n{}",
        found.join("\n")
    );
}

/// Choosing which media type of a description to read is one rule. It was
/// once written three times -- for the request body, and again for each
/// document format's response -- and the response copies quietly dropped
/// `application/vnd.api+json` bodies on the floor. Outside `pick_media_type`
/// the literal may only be the fallback of an `unwrap_or_else`.
#[test]
fn media_type_selection_has_one_implementation() {
    let path = root().join("src/domain/functions/openapi.rs");
    let code = production_code(&path).expect("openapi.rs is production code");
    let picker = code
        .find("fn pick_media_type")
        .expect("`pick_media_type` is the one selection rule");
    let picker_line = code[..picker].lines().count();
    let mut found = Vec::new();
    for (number, line) in code.lines().enumerate() {
        if number >= picker_line || !line.contains("\"application/json\"") {
            continue;
        }
        if !line.contains("unwrap_or_else") {
            found.push(location(&path, number, line));
        }
    }
    assert!(
        found.is_empty(),
        "select the media type through `pick_media_type`; a second rule goes \
         stale on its own:\n{}",
        found.join("\n")
    );
}

/// Progress lines a person reads to decide something. Printed from one
/// place, so every command that can print it prints the same thing -- and so
/// a command that should print it and does not is visible as a missing call,
/// not as a line nobody copied.
const SINGLE_SOURCE_MESSAGES: [&str; 1] = ["# API description: "];

#[test]
fn a_progress_line_is_printed_from_one_place() {
    let mut found = Vec::new();
    for message in SINGLE_SOURCE_MESSAGES {
        let mut sites = Vec::new();
        for path in rust_files(&root().join("src")) {
            let Some(code) = production_code(&path) else {
                continue;
            };
            for (number, line) in code.lines().enumerate() {
                if line.contains(message) {
                    sites.push(location(&path, number, line));
                }
            }
        }
        if sites.len() != 1 {
            found.push(format!(
                "`{message}` is printed from {} places:",
                sites.len()
            ));
            found.extend(sites);
        }
    }
    assert!(
        found.is_empty(),
        "give the message one home and call it from every path:\n{}",
        found.join("\n")
    );
}

/// Taking the file lock blocks the calling thread until the other process
/// lets go. On the async runtime that stalls every other task sharing the
/// worker, with nothing on screen to say why, so the blocking call lives
/// only in these files -- each of which offers an `_async` entry point that
/// moves it to `spawn_blocking` and says something while it waits.
pub(crate) const BLOCKING_LOCK_FILES: [Allowed; 2] = [
    (
        "src/shell/api_runtime.rs",
        "the token refresh lock, so parallel calls refresh an expired token once",
        "oauth_refresh_replaces_an_expired_token_once_across_parallel_calls",
    ),
    (
        "src/adapters/openapi/cache.rs",
        "the per-entry write lock of the description cache",
        "revalidation_updates_only_matching_metadata_and_never_the_body",
    ),
];

/// Whether a source takes the blocking file lock.
pub(crate) fn takes_the_blocking_lock(source: &str) -> bool {
    source.contains("ProfileLock::acquire")
}

/// The synchronous cache writers behind those async entry points. Calling
/// one from `src/shell` puts the blocking lock back on the runtime.
const BLOCKING_CACHE_WRITERS: [&str; 2] = ["store_with_wait(", "update_fetched_at_with_wait("];

#[test]
fn the_blocking_lock_is_taken_only_behind_an_async_entry_point() {
    let mut found = Vec::new();
    for path in rust_files(&root().join("src")) {
        let Some(code) = production_code(&path) else {
            continue;
        };
        let relative = path.strip_prefix(root()).unwrap().display().to_string();
        let allowed = allows(&BLOCKING_LOCK_FILES, &relative);
        for (number, line) in code.lines().enumerate() {
            if takes_the_blocking_lock(line) && !allowed {
                found.push(location(&path, number, line));
            }
            if relative.starts_with("src/shell/")
                && BLOCKING_CACHE_WRITERS
                    .iter()
                    .any(|writer| line.contains(writer))
            {
                found.push(location(&path, number, line));
            }
        }
        if allowed && !code.contains("spawn_blocking") {
            found.push(format!(
                "{relative}: takes the blocking lock without a `spawn_blocking` entry point"
            ));
        }
    }
    assert!(
        found.is_empty(),
        "take the lock through the `_async` entry point, which runs it on a \
         blocking worker and says it is waiting:\n{}",
        found.join("\n")
    );
}

/// `tokio::signal::ctrl_c()` registers its listener when it is first polled,
/// and a SIGINT reaches only the listeners that exist when it arrives. A
/// command that starts its work and polls `ctrl_c()` later loses a SIGINT
/// sent in between: `kurama data` ran a scan on until it was killed.
/// A command registers `signal(SignalKind::interrupt())` before it starts
/// anything; `ctrl_c()` stays only in the files listed here, and there only
/// in the shape `sigint_listener.rs` accepts: bound, then first polled by the
/// `select!` that polls the work.
pub(crate) const FILES_THAT_WAIT_ON_CTRL_C: [Allowed; 0] = [];

/// Whether production code waits on `tokio::signal::ctrl_c`.
pub(crate) fn waits_on_ctrl_c(source: &str) -> bool {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .any(|line| line.contains("signal::ctrl_c"))
}

#[test]
fn a_sigint_listener_is_registered_before_the_work_it_stops() {
    let mut found = Vec::new();
    for path in rust_files(&root().join("src")) {
        let source = std::fs::read_to_string(&path).unwrap();
        if !waits_on_ctrl_c(&source) {
            continue;
        }
        let relative = path
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !allows(&FILES_THAT_WAIT_ON_CTRL_C, &relative) {
            found.push(format!("{relative}: not in FILES_THAT_WAIT_ON_CTRL_C"));
        }
        for line in late_ctrl_c_listeners(&source) {
            found.push(format!(
                "{relative}:{line}: polled first after the work may have started"
            ));
        }
    }
    assert!(
        found.is_empty(),
        "ARCH-043: `ctrl_c()` registers only when first polled, so a SIGINT sent before \
         then is lost; register `tokio::signal::unix::signal(SignalKind::interrupt())` \
         before the work starts:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_ctrl_c_in_code_is_found() {
    assert_detected(
        "ARCH-043",
        waits_on_ctrl_c("    _ = tokio::signal::ctrl_c() => stop(),\n"),
        "a select! arm on ctrl_c()",
    );
}

#[test]
fn a_ctrl_c_named_in_a_comment_beside_an_eager_listener_is_not() {
    assert_allowed(
        "ARCH-043",
        !waits_on_ctrl_c(
            "    // `tokio::signal::ctrl_c()` registers late\n    let mut interrupt = signal(SignalKind::interrupt())?;\n",
        ),
        "a comment naming ctrl_c() and an eager listener",
    );
}

#[test]
fn a_ctrl_c_listener_polled_after_the_work_started_is_found() {
    for fixture in LATE_LISTENERS {
        assert_detected(
            "ARCH-043",
            !late_ctrl_c_listeners(fixture).is_empty(),
            fixture,
        );
    }
}

#[test]
fn a_ctrl_c_listener_first_polled_with_the_work_is_not() {
    let found = late_ctrl_c_listeners(REGISTERED_WITH_THE_WORK);
    assert_allowed("ARCH-043", found.is_empty(), &format!("{found:?}"));
}

/// Only `console.rs` rewrites a line in place. A carriage return is cursor
/// addressing: it needs the terminal check `terminal::rewrites_stderr_lines`
/// makes, and a second place writing one would be a second, unchecked rule.
pub(crate) const FILES_THAT_REWRITE_A_LINE: [Allowed; 1] = [(
    "src/console.rs",
    "ProgressLine, gated on terminal::rewrites_stderr_lines",
    "a_progress_line_is_rewritten_in_place_and_ended_only_when_written",
)];

/// Whether a source writes a string that opens with a carriage return,
/// which returns the cursor and writes over what is there. CRLF, an escaped
/// `\r` and a trimmed one are not that.
pub(crate) fn rewrites_a_line(source: &str) -> bool {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .any(|line| line.contains("\"\\r") && !line.contains("\\r\\n"))
}

#[test]
fn only_named_files_rewrite_a_line_in_place() {
    let found: Vec<String> = rust_files(&root().join("src"))
        .into_iter()
        .filter(|path| rewrites_a_line(&std::fs::read_to_string(path).unwrap()))
        .map(|path| {
            path.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|path| !allows(&FILES_THAT_REWRITE_A_LINE, path))
        .collect();
    assert!(
        found.is_empty(),
        "a carriage return is written outside the file that owns line rewriting; \
         go through `console::ProgressLine`, which is gated on \
         `terminal::rewrites_stderr_lines`:\n{}",
        found.join("\n")
    );
}

/// `aws_sigv4` is named in one file. The IAM database token was a second
/// signer for a while, with the identity and the signing parameters copied
/// and the error text already drifting: one described its causes, the other
/// stopped at "failed to create canonical request".
pub(crate) const FILES_THAT_SIGN: [Allowed; 1] = [(
    "src/adapters/sigv4.rs",
    "sign_request and presign_query, the one SigV4 signer",
    "get_vanilla",
)];

/// Whether a source names the SigV4 crate in a path or a `use`.
pub(crate) fn signs(source: &str) -> bool {
    crate::syntax::read_source(source).paths.iter().any(|path| {
        path.segments
            .first()
            .is_some_and(|first| first == "aws_sigv4")
    })
}

#[test]
fn a_sigv4_import_path_or_alias_is_a_signature() {
    for source in [
        "use aws_sigv4::sign::v4;",
        "use aws_sigv4 as signer;",
        "fn f() { let _ = aws_sigv4::http_request::sign(a, b); }",
        "use aws_sigv4::{\n    http_request::SignableRequest,\n    sign::v4,\n};",
    ] {
        assert_detected("ARCH-019", signs(source), source);
    }
}

#[test]
fn a_sigv4_mention_in_a_comment_a_string_or_a_test_is_not_a_signature() {
    for source in [
        "// aws_sigv4 is used by adapters/sigv4.rs only\nfn f() {}",
        "fn f() -> &'static str { \"aws_sigv4::sign\" }",
        "#[cfg(test)]\nmod tests {\n    use aws_sigv4::sign::v4;\n}",
    ] {
        assert_allowed("ARCH-019", !signs(source), source);
    }
}

#[test]
fn only_the_sigv4_adapter_signs() {
    let found: Vec<String> = rust_files(&root().join("src"))
        .into_iter()
        .filter(|path| signs(&std::fs::read_to_string(path).unwrap()))
        .map(|path| {
            path.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|path| !allows(&FILES_THAT_SIGN, path))
        .collect();
    assert!(
        found.is_empty(),
        "a SigV4 signature is made outside `adapters/sigv4.rs`; call `sign_request` or \
         `presign_query` instead of building another signer:\n{}",
        found.join("\n")
    );
}

/// The one file that asks whether a stream is a terminal, and so the one
/// place that weighs the streams against a run declared non-interactive
/// (`KURAMA_AGENT`): what a run shows -- a screen, a laid-out body, a
/// rewritten line -- is decided there. The adapters are outside the rule on
/// purpose: whether the keychain or `op` may prompt is how a run
/// authenticates, which the declaration does not change.
const TERMINAL_DECISION: &str = "src/shell/tui/terminal.rs";

/// Whether production code asks a stream whether it is a terminal: every
/// form of the question -- a method call or `IsTerminal::is_terminal(&s)` --
/// names the trait.
pub(crate) fn asks_for_a_terminal(source: &str) -> bool {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .any(|line| line.contains("IsTerminal"))
}

#[test]
fn only_terminal_rs_decides_whether_a_run_is_interactive() {
    let mut found = Vec::new();
    for path in rust_files(&root().join("src")) {
        let relative = path
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative == TERMINAL_DECISION || relative.starts_with("src/adapters/") {
            continue;
        }
        let Some(code) = production_code(&path) else {
            continue;
        };
        if asks_for_a_terminal(&code) {
            found.push(relative);
        }
    }
    assert!(
        found.is_empty(),
        "ARCH-044: ask `{TERMINAL_DECISION}` whether this run is interactive, so a run \
         declared non-interactive is one on every path:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_stream_asked_whether_it_is_a_terminal_is_found() {
    assert_detected(
        "ARCH-044",
        asks_for_a_terminal(
            "use std::io::IsTerminal;\n    let tty = std::io::stdout().is_terminal();\n",
        ),
        "stdout().is_terminal() outside terminal.rs",
    );
    assert_detected(
        "ARCH-044",
        asks_for_a_terminal(
            "    let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());\n",
        ),
        "IsTerminal::is_terminal(&stdout()) outside terminal.rs",
    );
}

#[test]
fn a_terminal_named_in_a_comment_is_not() {
    assert_allowed(
        "ARCH-044",
        !asks_for_a_terminal(
            "    // IsTerminal is terminal.rs's question\n    let tty = terminal::stdout_is_interactive();\n",
        ),
        "a comment naming is_terminal() beside the call to terminal.rs",
    );
}
