//! Connect clap's completion callbacks to silent adapters and reparse the completed argv prefix.

use std::ffi::{OsStr, OsString};

use clap_complete::engine::CompletionCandidate;

use crate::adapters::completion;
use crate::adapters::completion_protocol::CONTINUE_WORD;
use crate::domain::functions::completion_candidates::{self, Candidate, ProfileScope};
use crate::domain::functions::jq_completion;
use crate::domain::functions::operation_lookup::{
    OperationTarget, find_operation, parse_operation_target,
};
use crate::domain::types::json_shape::JsonShape;

fn candidates(values: Vec<Candidate>) -> Vec<CompletionCandidate> {
    values
        .into_iter()
        .map(|candidate| {
            CompletionCandidate::new(candidate.value).help(Some(candidate.help.into()))
        })
        .collect()
}

pub fn profiles(scope: ProfileScope) -> Vec<CompletionCandidate> {
    candidates(completion::profiles(scope))
}

pub fn databases() -> Vec<CompletionCandidate> {
    candidates(completion::databases())
}

pub fn s3_connections() -> Vec<CompletionCandidate> {
    candidates(completion::s3_connections())
}

pub fn config_sections() -> Vec<CompletionCandidate> {
    candidates(completion::config_sections())
}

pub fn config_keys() -> Vec<CompletionCandidate> {
    candidates(completion::config_keys())
}

pub fn apis() -> Vec<CompletionCandidate> {
    candidates(completion::apis())
}

pub fn operations() -> Vec<CompletionCandidate> {
    let Some(context) = read_completion_context() else {
        return Vec::new();
    };
    completion::read_api_spec(&context.api)
        .map(|spec| candidates(completion_candidates::operation_candidates(&spec)))
        .unwrap_or_default()
}

pub fn http_methods() -> Vec<CompletionCandidate> {
    candidates(completion_candidates::http_method_candidates())
}

pub fn parameters(current: &OsStr) -> Vec<CompletionCandidate> {
    let (
        Some(current),
        Some(ApiContext {
            api,
            target: Some(target),
            given,
            method: _,
            has_body: _,
            json: _,
        }),
    ) = (current.to_str(), read_completion_context())
    else {
        return Vec::new();
    };
    let Some(spec) = completion::read_api_spec(&api) else {
        return Vec::new();
    };
    let Some(operation) = find_operation(&spec, &target) else {
        return Vec::new();
    };
    let values = if let Some((name, _)) = current.split_once('=') {
        let Some(parameter) = operation.parameter(name) else {
            return Vec::new();
        };
        completion_candidates::parameter_value_candidates(parameter)
            .into_iter()
            .map(|candidate| Candidate {
                value: format!("{name}={}", candidate.value),
                help: candidate.help,
            })
            .collect()
    } else {
        completion_candidates::parameter_candidates(operation, &given)
    };
    candidates(
        values
            .into_iter()
            .filter(|candidate| candidate.value.starts_with(current))
            .collect(),
    )
}

pub fn jq(current: &OsStr) -> Vec<CompletionCandidate> {
    let (Some(current), Some(context)) = (current.to_str(), read_completion_context()) else {
        return Vec::new();
    };
    let Some(target) = context.target else {
        return Vec::new();
    };
    let Some(spec) = completion::read_api_spec(&context.api) else {
        return Vec::new();
    };
    let (method, path) = match parse_operation_target(&target) {
        OperationTarget::MethodPath { method, path } => (method, path),
        OperationTarget::Id(path) if path.starts_with('/') => {
            (if context.has_body { "POST" } else { "GET" }.into(), path)
        }
        OperationTarget::Id(id) => {
            let Some(operation) = find_operation(&spec, &id) else {
                return Vec::new();
            };
            (operation.method.clone(), operation.path.clone())
        }
    };
    let target = format!("{} {path}", context.method.as_deref().unwrap_or(&method));
    let Some(response) =
        find_operation(&spec, &target).and_then(|operation| operation.response.as_ref())
    else {
        return Vec::new();
    };
    let shape = JsonShape::from_schema(&response.schema);
    if shape == JsonShape::Unknown {
        return Vec::new();
    }
    let shape = if context.json {
        JsonShape::response_envelope(shape)
    } else {
        shape
    };
    jq_completion::complete(&shape, current, current.len())
        .candidates
        .into_iter()
        .map(|candidate| {
            let (value, _) = jq_completion::apply(current, current.len(), &candidate);
            CompletionCandidate::new(value)
                .help(Some(candidate.description.into()))
                // A scalar can still be inside select(...), a comparison or a pipe.
                // Leave the jq word (and an open shell quote) ready for more input.
                .tag(Some(CONTINUE_WORD.into()))
        })
        .collect()
}

#[derive(Debug)]
struct ApiContext {
    api: String,
    target: Option<String>,
    given: Vec<String>,
    method: Option<String>,
    has_body: bool,
    json: bool,
}

fn read_completion_context() -> Option<ApiContext> {
    let index = std::env::var("_CLAP_COMPLETE_INDEX").ok()?.parse().ok()?;
    parse_completion_context(std::env::args_os().collect(), index)
}

fn parse_completion_context(args: Vec<OsString>, index: usize) -> Option<ApiContext> {
    let words = args
        .into_iter()
        .skip_while(|word| word != "--")
        .skip(1)
        .take(index);
    let matches = super::args::build_command()
        .ignore_errors(true)
        .try_get_matches_from(words)
        .ok()?;
    let (command, args) = matches.subcommand()?;
    if command != "api" {
        return None;
    }
    Some(ApiContext {
        api: args.get_one::<String>("api")?.clone(),
        target: args.get_one::<String>("target").cloned(),
        method: args.get_one::<String>("method").cloned(),
        has_body: args.get_one::<String>("data").is_some(),
        json: args.get_flag("json"),
        given: args
            .get_many::<String>("param")
            .into_iter()
            .flatten()
            .cloned()
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_context_uses_clap_and_omits_the_current_word() {
        for (words, index, expected) in [
            (
                vec!["binary", "--", "kurama", "api", "pets", "pets/l"],
                3,
                Some("pets"),
            ),
            (
                vec![
                    "binary",
                    "--",
                    "kurama",
                    "api",
                    "pets",
                    "--describe",
                    "pets/l",
                ],
                4,
                Some("pets"),
            ),
            (vec!["binary", "--", "kurama", "api", "partial"], 2, None),
            (vec!["binary", "--", "kurama", "token", "partial"], 2, None),
        ] {
            assert_eq!(
                parse_completion_context(words.into_iter().map(OsString::from).collect(), index)
                    .map(|context| context.api)
                    .as_deref(),
                expected
            );
        }
    }
}

#[cfg(test)]
mod parameter_context_tests {
    use super::*;

    #[test]
    fn parameter_context_uses_clap_for_targets_options_and_assigned_values() {
        for (words, index, target, given) in [
            (vec!["kurama", "api", "pets", "-P", ""], 4, None, vec![]),
            (
                vec!["kurama", "api", "pets", "--ops", "pets", "-P", ""],
                6,
                None,
                vec![],
            ),
            (
                vec!["kurama", "api", "pets", "-X", "GET", "pets/get", "-P", ""],
                7,
                Some("pets/get"),
                vec![],
            ),
            (
                vec![
                    "kurama",
                    "api",
                    "pets",
                    "GET /pets/{petId}",
                    "-P",
                    "petId=42",
                    "-P",
                    "X-",
                ],
                7,
                Some("GET /pets/{petId}"),
                vec!["petId=42"],
            ),
            (
                vec![
                    "kurama",
                    "api",
                    "pets",
                    "pets/get",
                    "--param=petId=42",
                    "--param",
                    "",
                ],
                6,
                Some("pets/get"),
                vec!["petId=42"],
            ),
            (
                vec!["kurama", "api", "pets", "--", "GET /pets/{petId}", ""],
                5,
                Some("GET /pets/{petId}"),
                vec![],
            ),
        ] {
            let args = ["binary", "ignored", "--"]
                .into_iter()
                .chain(words)
                .map(OsString::from)
                .collect();
            let context = parse_completion_context(args, index).unwrap();
            assert_eq!(context.api, "pets");
            assert_eq!(context.target.as_deref(), target);
            assert_eq!(context.given, given);
        }
    }
}
