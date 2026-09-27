//! Find one operation of an `ApiSpec` by `operationId` or `METHOD /path`,
//! search the list, and suggest the closest ids for a target that matches
//! nothing.

use crate::domain::types::api_spec::{ApiSpec, Operation};

/// A TARGET that names an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationTarget {
    /// An `operationId`.
    Id(String),
    /// `METHOD /path/{param}`, the path template of the document.
    MethodPath { method: String, path: String },
}

pub fn parse_operation_target(target: &str) -> OperationTarget {
    let target = target.trim();
    match target.split_once(char::is_whitespace) {
        Some((verb, rest))
            if !verb.is_empty()
                && verb.chars().all(|c| c.is_ascii_alphabetic())
                && rest.trim_start().starts_with('/') =>
        {
            OperationTarget::MethodPath {
                method: verb.to_ascii_uppercase(),
                path: rest.trim().to_string(),
            }
        }
        _ => OperationTarget::Id(target.to_string()),
    }
}

/// The operation `target` names: an id (exact, then case-insensitive) or
/// a method and path template (a trailing slash is ignored).
pub fn find_operation<'a>(spec: &'a ApiSpec, target: &str) -> Option<&'a Operation> {
    match parse_operation_target(target) {
        OperationTarget::Id(id) => spec
            .operations
            .iter()
            .find(|operation| operation.id == id)
            .or_else(|| {
                spec.operations
                    .iter()
                    .find(|operation| operation.id.eq_ignore_ascii_case(&id))
            }),
        OperationTarget::MethodPath { method, path } => {
            let wanted = path.trim_end_matches('/');
            spec.operations.iter().find(|operation| {
                operation.method == method && operation.path.trim_end_matches('/') == wanted
            })
        }
    }
}

/// The operations every whitespace-separated term of `query` matches, in
/// the id, the method, the path, the summary or a tag (case-insensitive).
/// An empty query is every operation.
pub fn search_operations<'a>(spec: &'a ApiSpec, query: &str) -> Vec<&'a Operation> {
    search_operation_indices(spec, query)
        .into_iter()
        .map(|index| &spec.operations[index])
        .collect()
}

/// `search_operations` as indices into `spec.operations`.
pub fn search_operation_indices(spec: &ApiSpec, query: &str) -> Vec<usize> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    spec.operations
        .iter()
        .enumerate()
        .filter(|(_, operation)| terms.iter().all(|term| matches_term(operation, term)))
        .map(|(index, _)| index)
        .collect()
}

fn matches_term(operation: &Operation, term: &str) -> bool {
    operation.id.to_lowercase().contains(term)
        || operation.method.to_lowercase() == term
        || operation.path.to_lowercase().contains(term)
        || operation
            .summary
            .as_deref()
            .is_some_and(|summary| summary.to_lowercase().contains(term))
        || operation
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(term))
}

/// Up to `limit` operations close to `target`: for an id, the ids
/// containing it (shortest first), then the ids a few edits away from a
/// part of theirs (`issues/lst` finds `issues/list-for-repo`); for
/// `METHOD /path`, the operations of that method with a path that close.
pub fn suggest_operations<'a>(spec: &'a ApiSpec, target: &str, limit: usize) -> Vec<&'a Operation> {
    let (wanted, method) = match parse_operation_target(target) {
        OperationTarget::Id(id) => (id.to_lowercase(), None),
        OperationTarget::MethodPath { method, path } => (path.to_lowercase(), Some(method)),
    };
    if wanted.is_empty() {
        return Vec::new();
    }
    // A few typos are tolerated; a short target only matches as a substring
    // and a long path does not widen the net.
    let tolerance = (wanted.chars().count() / 3).min(3);
    let mut ranked: Vec<(usize, usize, &Operation)> = spec
        .operations
        .iter()
        .filter(|operation| {
            method
                .as_ref()
                .is_none_or(|method| operation.method == *method)
        })
        .filter_map(|operation| {
            let candidate = if method.is_some() {
                operation.path.to_lowercase()
            } else {
                operation.id.to_lowercase()
            };
            let distance = if candidate.contains(&wanted) {
                0
            } else {
                substring_distance(&wanted, &candidate)
            };
            (distance <= tolerance).then_some((distance, candidate.chars().count(), operation))
        })
        .collect();
    // Closest first, then the shortest candidate: `/x/issues` over
    // `/x/issues/{n}/comments`.
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.id.cmp(&b.2.id))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, _, operation)| operation)
        .collect()
}

/// The smallest edit distance between `query` and any substring of
/// `candidate`.
fn substring_distance(query: &str, candidate: &str) -> usize {
    let query: Vec<char> = query.chars().collect();
    let candidate: Vec<char> = candidate.chars().collect();
    // row[j]: distance between query[..i] and candidate[..j]
    let mut row: Vec<usize> = (0..=candidate.len()).map(|_| 0).collect();
    for (i, q) in query.iter().enumerate() {
        let mut previous = row.clone();
        previous[0] = i;
        row[0] = i + 1;
        for j in 1..=candidate.len() {
            let substitution = previous[j - 1] + usize::from(candidate[j - 1] != *q);
            row[j] = (previous[j] + 1).min(row[j - 1] + 1).min(substitution);
        }
    }
    row.into_iter().min().unwrap_or(query.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation(id: &str, method: &str, path: &str, summary: &str, tags: &[&str]) -> Operation {
        Operation {
            id: id.to_string(),
            has_operation_id: !id.contains(' '),
            method: method.to_string(),
            path: path.to_string(),
            summary: (!summary.is_empty()).then(|| summary.to_string()),
            description: None,
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            scopes: vec![],
            parameters: vec![],
            response: None,
            request_body: None,
            deprecated: false,
            external_docs: None,
            unsupported: vec![],
            graphql: None,
        }
    }

    fn spec() -> ApiSpec {
        ApiSpec {
            title: "GitHub".into(),
            version: "1".into(),
            description: None,
            server: None,
            operations: vec![
                operation(
                    "issues/list",
                    "GET",
                    "/issues",
                    "List issues assigned to the user",
                    &["issues"],
                ),
                operation(
                    "issues/list-for-repo",
                    "GET",
                    "/repos/{owner}/{repo}/issues",
                    "List repository issues",
                    &["issues"],
                ),
                operation(
                    "issues/create",
                    "POST",
                    "/repos/{owner}/{repo}/issues",
                    "Create an issue",
                    &["issues"],
                ),
                operation(
                    "pulls/list",
                    "GET",
                    "/repos/{owner}/{repo}/pulls",
                    "List pull requests",
                    &["pulls"],
                ),
                operation(
                    "DELETE /repos/{owner}/{repo}",
                    "DELETE",
                    "/repos/{owner}/{repo}",
                    "",
                    &["repos"],
                ),
            ],
            warnings: vec![],
        }
    }

    #[test]
    fn targets_are_ids_or_method_and_path_templates() {
        assert_eq!(
            parse_operation_target("issues/create"),
            OperationTarget::Id("issues/create".into())
        );
        assert_eq!(
            parse_operation_target("get /repos/{owner}/{repo}/issues"),
            OperationTarget::MethodPath {
                method: "GET".into(),
                path: "/repos/{owner}/{repo}/issues".into()
            }
        );
        assert_eq!(
            parse_operation_target("list issues"),
            OperationTarget::Id("list issues".into())
        );
    }

    #[test]
    fn find_by_id_case_insensitively_or_by_method_and_path() {
        let spec = spec();
        assert_eq!(
            find_operation(&spec, "issues/create").unwrap().method,
            "POST"
        );
        assert_eq!(
            find_operation(&spec, "ISSUES/CREATE").unwrap().id,
            "issues/create"
        );
        assert_eq!(
            find_operation(&spec, "POST /repos/{owner}/{repo}/issues/")
                .unwrap()
                .id,
            "issues/create"
        );
        assert_eq!(
            find_operation(&spec, "delete /repos/{owner}/{repo}")
                .unwrap()
                .id,
            "DELETE /repos/{owner}/{repo}"
        );
        assert!(find_operation(&spec, "issues/lst").is_none());
        assert!(find_operation(&spec, "PUT /repos/{owner}/{repo}/issues").is_none());
    }

    #[test]
    fn search_matches_every_term_in_id_path_summary_or_tag() {
        let spec = spec();
        let ids = |query: &str| -> Vec<String> {
            search_operations(&spec, query)
                .iter()
                .map(|o| o.id.clone())
                .collect()
        };
        assert_eq!(ids("").len(), 5);
        assert_eq!(ids("pull"), ["pulls/list"]);
        assert_eq!(
            ids("repo issues"),
            ["issues/list-for-repo", "issues/create"]
        );
        assert_eq!(ids("POST"), ["issues/create"]);
        assert_eq!(ids("Assigned"), ["issues/list"]);
        assert_eq!(
            ids("repos"),
            [
                "issues/list-for-repo",
                "issues/create",
                "pulls/list",
                "DELETE /repos/{owner}/{repo}"
            ]
        );
        assert!(ids("nothing-here").is_empty());
    }

    #[test]
    fn suggestions_rank_substrings_first_then_close_prefixes() {
        let spec = spec();
        let ids = |target: &str| -> Vec<String> {
            suggest_operations(&spec, target, 3)
                .iter()
                .map(|o| o.id.clone())
                .collect()
        };
        assert_eq!(
            ids("issues/lst"),
            ["issues/list", "issues/list-for-repo", "issues/create"]
        );
        assert_eq!(ids("pulls"), ["pulls/list"]);
        assert_eq!(
            ids("GET /repos/{owner}/{repo}/issue"),
            ["issues/list-for-repo"]
        );
        assert!(ids("zzzzzzzzzz").is_empty());
        assert!(ids("").is_empty());
        assert!(
            ids("zq").is_empty(),
            "a two-letter target matches as a substring only"
        );
        assert_eq!(ids("ull"), ["pulls/list"]);
        assert_eq!(suggest_operations(&spec, "issues", 1).len(), 1);
    }

    #[test]
    fn among_substring_hits_the_shortest_candidate_comes_first() {
        let mut spec = spec();
        spec.operations.insert(
            0,
            operation(
                "issues/get",
                "GET",
                "/repos/{owner}/{repo}/issues/{issue_number}",
                "Get an issue",
                &["issues"],
            ),
        );
        let ids: Vec<String> = suggest_operations(&spec, "GET /repos/{owner}/{repo}/issue", 3)
            .iter()
            .map(|o| o.id.clone())
            .collect();
        assert_eq!(ids, ["issues/list-for-repo", "issues/get"]);
    }

    #[test]
    fn substring_distance_matches_anywhere_in_the_candidate() {
        assert_eq!(substring_distance("issues/lst", "issues/list-for-repo"), 1);
        assert_eq!(substring_distance("lst", "issues/list-for-repo"), 1);
        assert_eq!(substring_distance("abc", "abc"), 0);
        assert_eq!(substring_distance("abc", "xyz"), 3);
        assert_eq!(substring_distance("abc", ""), 3);
        assert_eq!(substring_distance("", "abc"), 0);
    }

    #[test]
    fn search_indices_point_into_the_operations() {
        let spec = spec();
        assert_eq!(search_operation_indices(&spec, "list-for-repo"), vec![1]);
        let issues = search_operation_indices(&spec, "issues");
        assert!(issues.starts_with(&[0, 1]), "{issues:?}");
        assert!(search_operation_indices(&spec, "zzz").is_empty());
    }
}
