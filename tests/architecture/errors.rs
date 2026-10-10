//! Errors stay typed, print their cause once, and every error code and derived
//! hint is pinned by a scenario.

use std::collections::{BTreeMap, BTreeSet};

use crate::support::*;

/// A hint that could not be built is no hint. Inventing a generic one -- every
/// action kurama knows, every store it reads -- reads like an answer, sends
/// the person nowhere, and passes any assertion that looks for a substring of
/// it, so the lookup behind it can break and stay green. A placeholder that
/// says `<...>` is not that: it cannot be mistaken for a real value.
#[test]
fn a_hint_is_never_invented_when_the_lookup_finds_nothing() {
    let path = root().join("src/shell/cli/error_code.rs");
    let source = std::fs::read_to_string(&path).expect("readable");
    let start = source.find("pub fn hint(").expect("the hint function");
    let end = start
        + source[start + 10..]
            .find("\n    pub fn ")
            .or_else(|| source[start + 10..].find("\n    fn "))
            .expect("the hint function ends")
        + 10;
    let mut invented = Vec::new();
    for (offset, line) in source[start..end].lines().enumerate() {
        let Some(rest) = line.split("unwrap_or").nth(1) else {
            continue;
        };
        let literal = rest
            .trim_start_matches("_else")
            .trim_start_matches('(')
            .trim_start_matches("|| ")
            .trim_start();
        if literal.starts_with('"') && !literal.starts_with("\"<") {
            invented.push(location(
                &path,
                source[..start].lines().count() + offset,
                line,
            ));
        }
    }
    assert!(
        invented.is_empty(),
        "return no hint instead of a general one when the lookup finds nothing:\n{}",
        invented.join("\n")
    );
}

/// A `Hint::Fixed` is the same text every time and cannot quietly stop being
/// right. A `Hint::Derived` is read out of the error, so both what it says and
/// what it does when the read finds nothing are behaviour -- and a scenario
/// that only checks the code cannot see either. Six of them, and each one is
/// pinned; the rule is worth having because the list is short enough to keep
/// at zero exemptions.
#[test]
fn every_derived_hint_is_pinned_by_a_scenario_that_reads_it() {
    let path = root().join("src/shell/cli/error_code.rs");
    let source = std::fs::read_to_string(&path).expect("readable");
    let codes: BTreeMap<String, String> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Self::"))
        .filter_map(|line| line.split_once(" => \""))
        .map(|(variant, code)| {
            (
                variant.to_string(),
                code.trim_end_matches("\",").to_string(),
            )
        })
        .collect();

    let start = source
        .find("    fn hint_of(")
        .expect("the hint_of function");
    let end = start
        + source[start..]
            .find("\n    /// Walk the error chain")
            .expect("hint_of ends");
    let mut derived: BTreeSet<String> = BTreeSet::new();
    let mut label: Vec<String> = Vec::new();
    let mut collecting = false;
    for line in source[start..end].lines() {
        let is_label = line.starts_with("            Self::") && !line.trim().starts_with("//");
        if is_label {
            if !collecting {
                label.clear();
            }
            for part in line.split("Self::").skip(1) {
                let variant: String = part
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !variant.is_empty() {
                    label.push(variant);
                }
            }
            collecting = !line.contains("=>");
        }
        if line.contains("Hint::derived(") || line.contains("Hint::Derived(") {
            derived.extend(label.iter().cloned());
        }
    }
    assert!(
        derived.len() >= 5,
        "the scan found {} derived hints, which is fewer than this file has: it stopped working",
        derived.len()
    );

    let scenarios = scenario_sources();
    let mut unread = Vec::new();
    for variant in &derived {
        let Some(code) = codes.get(variant) else {
            continue;
        };
        let read_by_a_scenario = scenarios.iter().any(|(file, source)| {
            // A case file is one scenario; a Rust file holds one per `fn`.
            let chunks: Vec<&str> = if file.ends_with(".toml") {
                vec![source.as_str()]
            } else {
                source.split("\nfn ").skip(1).collect()
            };
            chunks.iter().any(|scenario| {
                scenario.contains(&format!("\"{code}\"")) && scenario.contains("hint")
            })
        });
        if !read_by_a_scenario {
            unread.push(format!("{code} (ErrorCode::{variant})"));
        }
    }
    assert!(
        unread.is_empty(),
        "a hint built from the error needs a scenario that reads the hint, not only the code:\n{}",
        unread.join("\n")
    );
}

/// An adapter passes a chain on; it does not turn one into text. `{:#}` is
/// anyhow's whole chain as a string, and the moment an adapter writes one into
/// a message, `ErrorCode::classify` can no longer see what failed: that is how
/// a wrong TOTP under an `aws-*://` reference arrived as a malformed reference,
/// exit 2, with a hint about grammar. `SecretError::unclassified` is the shape
/// that keeps both -- a message and the chain that made it. Rendering a chain
/// for a person is the shell's job, in `error_code.rs`.
#[test]
fn an_adapter_never_turns_an_anyhow_chain_into_text() {
    let mut flattened = Vec::new();
    for path in rust_files(&root().join("src/adapters")) {
        let Some(code) = production_code(&path) else {
            continue;
        };
        for (number, line) in code.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains(":#}") {
                flattened.push(location(&path, number, line));
            }
        }
    }
    assert!(
        flattened.is_empty(),
        "pass the `anyhow::Error` on (`?`, `.context(...)`, or a constructor that keeps it \
         like `SecretError::unclassified`) instead of writing its chain into a message, \
         so `ErrorCode::classify` still sees what really failed:\n{}",
        flattened.join("\n")
    );
}

/// Error codes that no scenario triggers: the code, what it is, and why no
/// scenario can pin it.
pub(crate) const ERROR_CODES_WITHOUT_SCENARIO: [(&str, &str, &str); 1] = [(
    "INTERNAL",
    "the fallback for errors that no classification arm recognizes",
    "a failure a scenario can cause is one a classification arm names, so a scenario that reached INTERNAL would be a missing arm, not a pin",
)];

#[test]
fn every_error_code_is_pinned_by_a_scenario() {
    let codes = std::fs::read_to_string(root().join("src/shell/cli/error_code.rs")).unwrap();
    let scenarios: String = scenario_sources()
        .into_iter()
        .map(|(_, content)| content)
        .collect();
    let unpinned: Vec<&str> = codes
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Self::"))
        .filter_map(|arm| {
            arm.split_once(" => \"")
                .map(|(_, code)| code.trim_end_matches("\","))
        })
        .filter(|code| !scenarios.contains(&format!("\"{code}\"")))
        .filter(|code| {
            !ERROR_CODES_WITHOUT_SCENARIO
                .iter()
                .any(|(exempt, _, _)| exempt == code)
        })
        .collect();

    assert!(
        unpinned.is_empty(),
        "add a scenario under tests/scenarios/ that expects these error codes:\n{}",
        unpinned.join("\n")
    );
}

/// The layers whose errors carry codes: flattening one into a string hides
/// it from `ErrorCode::classify`. Adapters wrap foreign errors with
/// `#[source]` instead.
const TYPED_ERROR_LAYERS: [&str; 3] = ["src/shell", "src/workflows", "src/domain"];

/// The lines of `source` that flatten a typed error into its text
/// (`syntax.rs`): `map_err(|e| X(e.to_string()))`, however it is split over
/// lines, or `anyhow!` of a lone placeholder. A tuple that keeps a typed kind
/// next to the text, a `format!` that adds words, a comment, a string or test
/// code is not it.
fn flattened_lines(source: &str) -> Vec<usize> {
    crate::syntax::read_source(source).flattened
}

#[test]
fn typed_errors_are_never_flattened_into_strings() {
    let mut found = Vec::new();
    for dir in TYPED_ERROR_LAYERS {
        for path in rust_files(&root().join(dir)) {
            if production_code(&path).is_none() {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for line in flattened_lines(&source) {
                let text = source.lines().nth(line - 1).unwrap_or_default();
                found.push(location(&path, line - 1, text));
            }
        }
    }
    assert!(
        found.is_empty(),
        "keep the typed error in the chain (`?`, `.context(...)` or a `#[source]` field) instead of its text:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_flattened_error_on_one_line_or_several_is_found() {
    for source in [
        "fn f() { g().map_err(|e| Failure(e.to_string())); }",
        "fn f() {\n    g().map_err(|error| {\n        Failure::Io(\n            error.to_string(),\n        )\n    });\n}",
        "fn f() { g().map_err(|e| anyhow::anyhow!(\"{e}\")); }",
        "fn f() { let _ = anyhow!(\"{}\", e); }",
    ] {
        assert_detected("ARCH-006", !flattened_lines(source).is_empty(), source);
    }
}

#[test]
fn a_kept_error_a_comment_a_string_or_a_test_is_not_flattened() {
    for source in [
        "fn f() { g().map_err(|e| Failure(Kind::Io, e.to_string())); }",
        "fn f() { g().map_err(|e| Failure(format!(\"reading x: {e}\"))); }",
        "fn f() { g().map_err(|e| Failure(other.to_string())); }",
        "// g().map_err(|e| Failure(e.to_string()))\nfn f() {}",
        "fn f() -> &'static str { \"map_err(|e| X(e.to_string()))\" }",
        "fn f() { let _ = anyhow!(\"no {e} alone\"); }",
        "#[cfg(test)]\nmod tests {\n    fn f() { g().map_err(|e| Failure(e.to_string())); }\n}",
    ] {
        assert_allowed("ARCH-006", flattened_lines(source).is_empty(), source);
    }
}

/// The format string of a `#[error(...)]` attribute, if the line is one.
fn error_format(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix("#[error(")?
        .strip_suffix(")]")
}

/// Whether the variant after an `#[error(...)]` line names the cause as its
/// source. `#[from]` implies `#[source]`, and both make `{error:#}` append
/// the cause's own text after the variant's own.
fn variant_carries_a_source(rest: &[&str]) -> bool {
    let mut depth = 0i32;
    for line in rest {
        if line.contains("#[source]") || line.contains("#[from]") {
            return true;
        }
        if error_format(line).is_some() {
            return false;
        }
        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        let trimmed = line.trim_end();
        if depth <= 0 && (trimmed.ends_with(',') || trimmed.ends_with(';')) {
            return false;
        }
    }
    false
}

/// `main` renders the chain with `{error:#}`, which joins every cause with
/// `": "`. A variant whose own `Display` already embeds the cause -- through
/// `{source}` or the `{0}` of a `#[from]` field -- therefore prints that text
/// twice. `#[error(transparent)]` is the form that does not.
#[test]
fn error_variants_do_not_print_their_source_twice() {
    let mut found = Vec::new();
    for path in rust_files(&root().join("src")) {
        let Some(code) = production_code(&path) else {
            continue;
        };
        let lines: Vec<&str> = code.lines().collect();
        for (number, line) in lines.iter().enumerate() {
            let Some(format) = error_format(line) else {
                continue;
            };
            if !format.contains("{source}") && !format.contains("{0}") {
                continue;
            }
            if variant_carries_a_source(&lines[number + 1..]) {
                found.push(location(&path, number, line));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these variants print their cause twice under `{{error:#}}`; use `#[error(transparent)]` \
         or drop the cause from the format string:\n{}",
        found.join("\n")
    );
}

/// The files whose every `match` decides per variant: the error code and hint
/// of each failure, and what each command promises about its streams and the
/// audit log.
const CONTRACT_FILES: &[&str] = &["src/shell/cli/error_code.rs", "src/shell/cli/command.rs"];

/// The catch-all arms of a source's production `match`es, by line. A match on
/// literals (a string, a number) has to end in one, and is not counted.
fn catch_all_arms(source: &str) -> Vec<usize> {
    crate::syntax::read_matches(source)
        .into_iter()
        .filter(|reading| !reading.on_literals)
        .flat_map(|reading| reading.arms)
        .filter(|arm| arm.catch_all)
        .map(|arm| arm.line)
        .collect()
}

/// A new error variant or subcommand gets a code, a hint, an output kind and
/// an audit decision somebody chose: `ErrorCode::classify`, `hint_of` and
/// `CliCommand::contract` list every variant, so adding one stops the build
/// there. A `_ =>` gave a new `DataError` `DATA_FAILED` and a new
/// `CliExecutorError` `INTERNAL` without anyone deciding it, and a new
/// subcommand neither a JSON error contract nor an audit entry.
#[test]
fn a_contract_match_names_every_variant() {
    let mut found = Vec::new();
    for file in CONTRACT_FILES {
        let path = root().join(file);
        let code = production_code(&path).expect("a production file");
        let lines: Vec<&str> = code.lines().collect();
        for line in catch_all_arms(&code) {
            found.push(location(&path, line - 1, lines[line - 1]));
        }
    }
    assert!(
        found.is_empty(),
        "a contract match lists every variant, so a new one stops the build instead \
         of silently taking the catch-all's answer:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_catch_all_arm_in_any_form_is_found() {
    for source in [
        "fn f(e: &E) -> C { match e { E::A => C::A, _ => C::B } }",
        "fn f(e: &E) -> C { match e { E::A => C::A, _ if ready() => C::B, E::B => C::C } }",
        "fn f(e: &E) -> C { match e { E::A => C::A, other => C::from(other) } }",
        "fn f(e: Option<&E>) -> C { match e { Some(E::A) => C::A, Some(_) | None => C::B } }",
        "fn f(e: Option<&E>) -> C { match e { Some(E::A) => C::A, Some(x) => C::B, None => C::C } }",
        "fn f(r: Result<E, F>) -> C { match r { Err(F::A) => C::A, Err(e) => C::B, Ok(_) => C::C } }",
        "fn f(e: &E) -> C { match e { E::A | _ => C::A } }",
        "fn f(e: &E) -> C { let x = |c| match c.downcast_ref::<E>() {\n Some(E::A) => 1,\n _ => 2,\n }; x(e) }",
        "impl C { fn f(&self) -> u8 { match self { Self::A => 1, ref rest => 2 } } }",
    ] {
        assert!(!catch_all_arms(source).is_empty(), "{source}");
    }
}

#[test]
fn every_variant_named_a_literal_match_or_test_code_is_not_found() {
    for source in [
        "fn f(e: &E) -> C { match e { E::A => C::A, E::B { .. } | E::C(_) => C::B } }",
        "fn f(e: Option<&E>) -> C { match e { Some(E::A) => C::A, Some(E::B(_)) | None => C::B } }",
        "fn f(key: &str) -> u8 { match key { \"a\" | \"b\" => 1, \"c\" => 2, _ => 3 } }",
        "fn f(n: u8) -> u8 { match n { 0 => 1, 1..=9 => 2, _ => 3 } }",
        "fn f(e: &E) -> C { match e { E::A if ready() => C::A, E::A | E::B => C::B } }",
        "#[cfg(test)]\nmod tests { fn f(e: &E) -> u8 { match e { E::A => 1, _ => 2 } } }",
        "fn f(e: &E) -> bool { matches!(e, E::A) }",
        "fn f(e: Option<&E>) -> C { match e { None => C::A, Some(x) => C::B } }",
    ] {
        assert_eq!(catch_all_arms(source), Vec::<usize>::new(), "{source}");
    }
}
