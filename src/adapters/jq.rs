//! `--jq`: run a jq filter on a JSON value, in process, through jaq.

use std::sync::atomic::{AtomicBool, Ordering};

use jaq_core::load::{self, Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, compile, data, unwrap_valr};
use jaq_json::{Val, read};

/// A response body parsed once for repeated jq previews.
pub struct ParsedInput {
    value: Val,
}

/// Convert a JSON value to jaq's input representation once.
pub fn parse_input(input: &serde_json::Value) -> Result<ParsedInput, String> {
    let encoded = serde_json::to_vec(input).map_err(|error| error.to_string())?;
    let value = read::parse_single(&encoded).map_err(|error| format!("input: {error}"))?;
    Ok(ParsedInput { value })
}

/// Every output of `filter` on `input`, one line each: strings raw, other
/// values as compact JSON (like `jq -r` for strings and `jq -c` otherwise).
pub fn apply_filter(
    filter: &str,
    input: &serde_json::Value,
    max_outputs: Option<usize>,
) -> Result<Vec<String>, String> {
    let input = parse_input(input)?;
    apply_filter_parsed(filter, &input, max_outputs)
}

/// Run a filter against an already parsed input value.
pub fn apply_filter_parsed(
    filter: &str,
    input: &ParsedInput,
    max_outputs: Option<usize>,
) -> Result<Vec<String>, String> {
    run_filter(filter, input, max_outputs, &AtomicBool::new(false))
}

/// Every output of `filter` on `input`, ending early once `stop` is raised:
/// the flag is read between outputs, so an endless filter ends when it is
/// set, while one computing a single output runs to its end.
pub fn apply_filter_until(
    filter: &str,
    input: &serde_json::Value,
    stop: &AtomicBool,
) -> Result<Vec<String>, String> {
    run_filter(filter, &parse_input(input)?, None, stop)
}

fn run_filter(
    filter: &str,
    input: &ParsedInput,
    max_outputs: Option<usize>,
    stop: &AtomicBool,
) -> Result<Vec<String>, String> {
    let program = File {
        code: filter,
        path: (),
    };
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());
    let loader = Loader::new(defs);
    let arena = Arena::default();
    let modules = loader.load(&arena, program).map_err(|errors| {
        format!(
            "invalid jq filter {filter:?}: {}",
            describe_load_errors(errors)
        )
    })?;
    let compiled = Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errors| {
            format!(
                "invalid jq filter {filter:?}: {}",
                describe_compile_errors(errors)
            )
        })?;

    let ctx = Ctx::<data::JustLut<Val>>::new(&compiled.lut, Vars::new([]));
    compiled
        .id
        .run((ctx, input.value.clone()))
        .take(max_outputs.unwrap_or(usize::MAX))
        .take_while(|_| !stop.load(Ordering::Relaxed))
        .map(unwrap_valr)
        .map(|result| {
            result
                .map(|value| match value {
                    Val::TStr(text) => String::from_utf8_lossy(text.as_ref()).into_owned(),
                    other => other.to_string(),
                })
                .map_err(|error| format!("jq: {error}"))
        })
        .collect()
}

/// jaq's load errors only derive `Debug`; say what was expected and where.
fn describe_load_errors(errors: load::Errors<&str, ()>) -> String {
    let mut parts = Vec::new();
    for (_, error) in errors {
        match error {
            load::Error::Io(failures) => {
                parts.extend(failures.into_iter().map(|(path, e)| format!("{path}: {e}")))
            }
            load::Error::Lex(failures) => parts.extend(
                failures
                    .into_iter()
                    .map(|(expect, at)| format!("expected {} {}", expect.as_str(), location(at))),
            ),
            load::Error::Parse(failures) => parts.extend(
                failures
                    .into_iter()
                    .map(|(expect, at)| format!("expected {} {}", expect.as_str(), location(at))),
            ),
        }
    }
    parts.join("; ")
}

fn describe_compile_errors(errors: compile::Errors<&str, ()>) -> String {
    errors
        .into_iter()
        .flat_map(|(_, failures)| failures)
        .map(|(name, undefined)| format!("undefined {} {name}", undefined.as_str()))
        .collect::<Vec<_>>()
        .join("; ")
}

fn location(at: &str) -> String {
    if at.is_empty() {
        "at the end of the filter".to_string()
    } else {
        format!("at {at:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_raw_and_other_values_are_json() {
        let input = serde_json::json!({"login": "octocat", "id": 1, "tags": ["a", "b"]});
        assert_eq!(apply_filter(".login", &input, None).unwrap(), ["octocat"]);
        assert_eq!(apply_filter(".id", &input, None).unwrap(), ["1"]);
        assert_eq!(
            apply_filter(".tags", &input, None).unwrap(),
            ["[\"a\",\"b\"]"]
        );
        assert_eq!(apply_filter(".tags[]", &input, None).unwrap(), ["a", "b"]);
        assert_eq!(
            apply_filter("{name: .login}", &input, None).unwrap(),
            ["{\"name\":\"octocat\"}"]
        );
        assert_eq!(apply_filter(".missing", &input, None).unwrap(), ["null"]);
    }

    #[test]
    fn standard_library_filters_are_available() {
        let input = serde_json::json!([3, 1, 2]);
        assert_eq!(
            apply_filter("sort | map(. * 2)", &input, None).unwrap(),
            ["[2,4,6]"]
        );
        assert_eq!(apply_filter("length", &input, None).unwrap(), ["3"]);
    }

    #[test]
    fn filter_and_runtime_errors_are_reported_in_words() {
        let input = serde_json::json!({"a": 1});
        let unterminated = apply_filter(".[", &input, None).unwrap_err();
        assert!(
            unterminated.starts_with("invalid jq filter \".[\": expected ")
                && unterminated.ends_with("at the end of the filter")
                && !unterminated.contains("File {"),
            "{unterminated}"
        );
        assert!(
            apply_filter(".a[]", &input, None)
                .unwrap_err()
                .starts_with("jq:")
        );
        let undefined = apply_filter("nosuchfilter", &input, None).unwrap_err();
        assert_eq!(
            undefined,
            "invalid jq filter \"nosuchfilter\": undefined filter nosuchfilter"
        );
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use crate::domain::functions::jq_completion::examples;
    use crate::domain::types::json_shape::JsonShape;

    #[test]
    fn a_raised_stop_flag_ends_an_endless_filter() {
        let stop = std::sync::atomic::AtomicBool::new(true);
        let input = serde_json::json!(1);
        assert!(
            apply_filter_until("repeat(.)", &input, &stop)
                .unwrap()
                .is_empty()
        );
        let running = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(apply_filter_until(".", &input, &running).unwrap(), ["1"]);
    }

    #[test]
    fn jq_preview_limits_outputs_using_the_same_evaluator_as_apply() {
        let input = serde_json::json!({});
        assert_eq!(
            apply_filter("range(0; 6)", &input, Some(4)).unwrap(),
            ["0", "1", "2", "3"]
        );
        assert_eq!(apply_filter("range(0; 6)", &input, None).unwrap().len(), 6);
    }

    #[test]
    fn jq_examples_run_on_the_response_that_produced_them() {
        for input in [
            serde_json::json!({"content": [{"id": 7, "name": "日本語", "enabled": true}]}),
            serde_json::json!([{"weird.key": "a", "with space": false}]),
            serde_json::json!({"id": 1, "name": "Rex"}),
            serde_json::json!([]),
            serde_json::json!(null),
        ] {
            for example in examples(&JsonShape::from_value(&input)) {
                assert!(
                    apply_filter(&example.filter, &input, None).is_ok(),
                    "{} on {input}",
                    example.filter
                );
            }
        }
    }
}
