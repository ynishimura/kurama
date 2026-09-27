//! The allowlists themselves: every entry names something that exists, once,
//! with a reason, and is still needed; a file allowed across a boundary
//! names the test that exercises it.
//!
//! A rule that allows a list of files goes quiet when an entry goes stale: a
//! renamed file leaves its old path allowed and its new one refused, a file
//! that stopped spawning keeps its permission for the next change, and an
//! entry without a reason cannot be judged by whoever finds it.

use std::collections::BTreeSet;

use crate::database::{
    BRIDGE_ENTRY_POINTS, BRIDGE_EXPORTS, RAW_TYPES_AND_CONSTANTS, RUST_ENTRY_POINTS,
    SQLITE_ENTRY_POINTS,
};
use crate::error_code_exemptions::{self as exemptions, error_code_exemption_problems};
use crate::errors::ERROR_CODES_WITHOUT_SCENARIO;
use crate::layers::{
    BLOCKING_LOCK_FILES, FILES_THAT_REWRITE_A_LINE, FILES_THAT_SIGN, FILES_THAT_SPAWN,
    FILES_THAT_WAIT_ON_CTRL_C, rewrites_a_line, signs, spawns_a_child, takes_the_blocking_lock,
    waits_on_ctrl_c,
};
use crate::support::*;
use crate::test_index::{TestDefinition, resolve_claim, test_definitions};
use crate::xtask::{XTASK_GH_FILES, starts_gh};

/// What is wrong with a file allowlist, given what each path holds (`None`
/// for a path that does not exist), what the rule looks for, and the tests.
fn allowlist_problems(
    list_name: &str,
    entries: &[Allowed],
    source: &dyn Fn(&str) -> Option<String>,
    still_needed: fn(&str) -> bool,
    tests: &[TestDefinition],
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for (path, reason, held_by) in entries {
        if !seen.insert(path) {
            problems.push(format!("{list_name}: {path} is listed twice"));
        }
        if reason.trim().is_empty() {
            problems.push(format!("{list_name}: {path} gives no reason"));
        }
        match source(path) {
            None => problems.push(format!("{list_name}: {path} does not exist")),
            Some(text) if !still_needed(&text) => problems.push(format!(
                "{list_name}: {path} no longer does what the entry allows; remove it"
            )),
            Some(_) => {}
        }
        if let Err(reason) = resolve_claim(held_by, tests) {
            problems.push(format!("{list_name}: {path}: {reason}"));
        }
    }
    problems
}

/// A file allowlist, its entries, and what an allowed file must still do.
type AllowlistCheck = (&'static str, &'static [Allowed], fn(&str) -> bool);

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(root().join(path)).ok()
}

#[test]
fn every_allowed_file_exists_once_with_a_reason_a_test_and_a_need() {
    let tests = test_definitions();
    let lists: [AllowlistCheck; 6] = [
        ("FILES_THAT_SPAWN", &FILES_THAT_SPAWN, spawns_a_child),
        ("FILES_THAT_SIGN", &FILES_THAT_SIGN, signs),
        (
            "FILES_THAT_REWRITE_A_LINE",
            &FILES_THAT_REWRITE_A_LINE,
            rewrites_a_line,
        ),
        (
            "BLOCKING_LOCK_FILES",
            &BLOCKING_LOCK_FILES,
            takes_the_blocking_lock,
        ),
        ("XTASK_GH_FILES", &XTASK_GH_FILES, starts_gh),
        (
            "FILES_THAT_WAIT_ON_CTRL_C",
            &FILES_THAT_WAIT_ON_CTRL_C,
            waits_on_ctrl_c,
        ),
    ];
    let problems: Vec<String> = lists
        .iter()
        .flat_map(|(name, entries, needed)| {
            allowlist_problems(name, entries, &read, *needed, &tests)
        })
        .collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The names of a reviewed list that repeat or that nothing uses any more.
fn entry_point_problems(list_name: &str, entries: &[&str], used_in: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in entries {
        if !seen.insert(entry) {
            problems.push(format!("{list_name}: {entry} is listed twice"));
        }
        if !contains_word(used_in, entry) {
            problems.push(format!(
                "{list_name}: {entry} is no longer used; remove it from the reviewed list"
            ));
        }
    }
    problems
}

#[test]
fn every_reviewed_entry_point_is_listed_once_and_still_used() {
    let duckdb: String = rust_files(&root().join("src/adapters/duckdb"))
        .iter()
        .filter_map(|path| production_code(path))
        .collect();
    let bridge = read("src/adapters/duckdb/arrow_bridge.cpp").expect("the Arrow bridge exists");
    let sqlite =
        read("src/adapters/database/sqlite_statements.rs").expect("the SQLite statement file");
    let problems: Vec<String> = [
        ("RUST_ENTRY_POINTS", RUST_ENTRY_POINTS, &duckdb),
        ("RAW_TYPES_AND_CONSTANTS", RAW_TYPES_AND_CONSTANTS, &duckdb),
        ("BRIDGE_ENTRY_POINTS", BRIDGE_ENTRY_POINTS, &bridge),
        ("BRIDGE_EXPORTS", BRIDGE_EXPORTS, &bridge),
        ("SQLITE_ENTRY_POINTS", SQLITE_ENTRY_POINTS, &sqlite),
    ]
    .iter()
    .flat_map(|(name, entries, used_in)| entry_point_problems(name, entries, used_in))
    .collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_error_code_exemption_is_a_real_code_with_its_reasons() {
    let codes = read("src/shell/cli/error_code.rs").expect("error_code.rs exists");
    let scenarios: String = scenario_sources()
        .into_iter()
        .map(|(_, content)| content)
        .collect();
    let problems = error_code_exemption_problems(&ERROR_CODES_WITHOUT_SCENARIO, &codes, &scenarios);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_sound_error_code_exemption_passes() {
    let problems = error_code_exemption_problems(
        &exemptions::SOUND,
        exemptions::FIXTURE_ERROR_CODES,
        exemptions::FIXTURE_SCENARIOS,
    );
    assert_allowed("ARCH-040", problems.is_empty(), &format!("{problems:?}"));
}

#[test]
fn an_error_code_exemption_that_repeats_is_unexplained_unknown_or_pinned_is_refused() {
    let problems = error_code_exemption_problems(
        &exemptions::UNSOUND,
        exemptions::FIXTURE_ERROR_CODES,
        exemptions::FIXTURE_SCENARIOS,
    );
    assert_detected(
        "ARCH-040",
        problems == exemptions::UNSOUND_PROBLEMS,
        &format!("{problems:?}"),
    );
}

fn fixture_tests() -> Vec<TestDefinition> {
    crate::test_index::test_definitions_in("tests/x.rs", "#[test] fn it_is_tested() {}").unwrap()
}

fn fixture_source(path: &str) -> Option<String> {
    match path {
        "src/spawns.rs" => {
            Some("use std::process::Command; fn f() { Command::new(\"x\"); }".into())
        }
        "src/idle.rs" => Some("fn nothing() {}".into()),
        _ => None,
    }
}

#[test]
fn an_allowlist_entry_that_is_sound_passes() {
    let entries = [("src/spawns.rs", "a child", "it_is_tested")];
    let problems = allowlist_problems(
        "L",
        &entries,
        &fixture_source,
        spawns_a_child,
        &fixture_tests(),
    );
    assert_allowed("ARCH-040", problems.is_empty(), &format!("{problems:?}"));
}

#[test]
fn an_allowlist_entry_that_is_missing_repeated_idle_or_unexplained_is_refused() {
    let entries = [
        ("src/gone.rs", "renamed away", "it_is_tested"),
        ("src/spawns.rs", "a child", "it_is_tested"),
        ("src/spawns.rs", " ", "it_is_tested"),
        ("src/idle.rs", "stopped spawning", "it_is_tested"),
        ("src/spawns.rs", "a child", "no_such_test"),
    ];
    let problems = allowlist_problems(
        "L",
        &entries,
        &fixture_source,
        spawns_a_child,
        &fixture_tests(),
    );
    let expected = [
        "L: src/gone.rs does not exist",
        "L: src/spawns.rs is listed twice",
        "L: src/spawns.rs gives no reason",
        "L: src/idle.rs no longer does what the entry allows; remove it",
        "L: src/spawns.rs is listed twice",
        "L: src/spawns.rs: no test function named `no_such_test`",
    ];
    assert_detected("ARCH-040", problems == expected, &format!("{problems:?}"));
}

#[test]
fn a_reviewed_entry_point_that_repeats_or_is_unused_is_refused() {
    let problems = entry_point_problems("E", &["used", "used", "gone"], "call(used)");
    let expected = [
        "E: used is listed twice",
        "E: gone is no longer used; remove it from the reviewed list",
    ];
    assert_detected("ARCH-040", problems == expected, &format!("{problems:?}"));
    assert!(entry_point_problems("E", &["used"], "call(used)").is_empty());
}
