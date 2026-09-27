//! The `[agent]` policy: which HTTP requests a run under `KURAMA_AGENT` may send without `--confirm`.
//!
//! A request passes when its method is one of `allow_methods`, its path
//! matches no `deny_paths` pattern, and `allow_paths` is empty or one of its
//! patterns matches. A pattern is a URL path in which `*` stands for any run
//! of characters, `/` included; the query is never part of the path.

/// The methods an agent run may send when the configuration names none.
pub const DEFAULT_ALLOW_METHODS: [&str; 2] = ["GET", "HEAD"];

/// The HTTP methods `allow_methods` may name.
pub const KNOWN_METHODS: [&str; 7] = ["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"];

/// The rules one request is held to, after `[api.<name>.agent]` has
/// replaced the `[agent]` keys it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPolicy {
    /// Upper case.
    pub allow_methods: Vec<String>,
    pub allow_paths: Vec<String>,
    pub deny_paths: Vec<String>,
}

/// Why a request is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Method,
    DeniedPath(String),
    PathNotAllowed,
}

impl AgentPolicy {
    /// Whether a run under an agent may send `method` to `path`.
    pub fn check(&self, method: &str, path: &str) -> Result<(), Refusal> {
        if !self
            .allow_methods
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(method))
        {
            return Err(Refusal::Method);
        }
        if let Some(pattern) = self
            .deny_paths
            .iter()
            .find(|pattern| path_matches(pattern, path))
        {
            return Err(Refusal::DeniedPath(pattern.clone()));
        }
        if !self.allow_paths.is_empty()
            && !self
                .allow_paths
                .iter()
                .any(|pattern| path_matches(pattern, path))
        {
            return Err(Refusal::PathNotAllowed);
        }
        Ok(())
    }
}

/// The refusal as the rest of one error line: what was asked and which rule
/// said no.
pub fn describe_refusal(refusal: &Refusal, method: &str, path: &str) -> String {
    match refusal {
        Refusal::Method => {
            format!("{method} {path}: {method} is not in allow_methods")
        }
        Refusal::DeniedPath(pattern) => {
            format!("{method} {path}: the path matches deny_paths {pattern:?}")
        }
        Refusal::PathNotAllowed => {
            format!("{method} {path}: the path matches no allow_paths pattern")
        }
    }
}

/// Whether `path` matches `pattern`, where `*` is any run of characters.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        // No `*`: the whole path is the pattern.
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// What is wrong with a method or a pattern the configuration names.
pub fn check_rules(methods: &[String], patterns: &[String]) -> Result<(), String> {
    if let Some(method) = methods
        .iter()
        .find(|method| !KNOWN_METHODS.contains(&method.to_ascii_uppercase().as_str()))
    {
        return Err(format!(
            "allow_methods names {method:?}, which is not one of {}",
            KNOWN_METHODS.join(", ")
        ));
    }
    if let Some(pattern) = patterns.iter().find(|pattern| !pattern.starts_with('/')) {
        return Err(format!(
            "a path pattern starts with /, got {pattern:?}; the query is never matched"
        ));
    }
    if let Some(pattern) = patterns.iter().find(|pattern| pattern.contains('?')) {
        return Err(format!(
            "a path pattern holds no query, got {pattern:?}; the query is never matched"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(methods: &[&str], allow: &[&str], deny: &[&str]) -> AgentPolicy {
        let owned = |items: &[&str]| items.iter().map(|item| item.to_string()).collect();
        AgentPolicy {
            allow_methods: owned(methods),
            allow_paths: owned(allow),
            deny_paths: owned(deny),
        }
    }

    #[test]
    fn a_method_outside_allow_methods_is_refused_and_case_does_not_matter() {
        let policy = policy(&DEFAULT_ALLOW_METHODS, &[], &[]);
        assert_eq!(policy.check("GET", "/pets"), Ok(()));
        assert_eq!(policy.check("head", "/pets"), Ok(()));
        assert_eq!(policy.check("POST", "/pets"), Err(Refusal::Method));
        assert_eq!(policy.check("DELETE", "/pets/1"), Err(Refusal::Method));
    }

    #[test]
    fn deny_paths_win_over_allow_paths() {
        let policy = policy(&["GET"], &["/repos/*"], &["/repos/*/secrets*"]);
        assert_eq!(policy.check("GET", "/repos/a/b"), Ok(()));
        assert_eq!(
            policy.check("GET", "/repos/a/b/secrets/x"),
            Err(Refusal::DeniedPath("/repos/*/secrets*".into()))
        );
        assert_eq!(policy.check("GET", "/user"), Err(Refusal::PathNotAllowed));
    }

    #[test]
    fn an_empty_allow_paths_allows_every_path() {
        let policy = policy(&["GET"], &[], &["/admin*"]);
        assert_eq!(policy.check("GET", "/anything/at/all"), Ok(()));
        assert!(policy.check("GET", "/admin/users").is_err());
    }

    #[rstest::rstest]
    #[case("/pets", "/pets", true)]
    #[case("/pets", "/pets/1", false)]
    #[case("/pets*", "/pets/1", true)]
    #[case("/pets/*", "/pets", false)]
    #[case("/pets/*/photos", "/pets/1/photos", true)]
    #[case("/pets/*/photos", "/pets/1/photos/2", false)]
    #[case("*", "/", true)]
    #[case("/a*b*c", "/aXbYc", true)]
    #[case("/a*b*c", "/aXcYb", false)]
    #[case("/v1/*", "/v2/x", false)]
    fn star_is_any_run_of_characters(
        #[case] pattern: &str,
        #[case] path: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(path_matches(pattern, path), expected, "{pattern} ~ {path}");
    }

    #[test]
    fn rules_name_known_methods_and_absolute_patterns_only() {
        let owned =
            |items: &[&str]| -> Vec<String> { items.iter().map(|s| s.to_string()).collect() };
        assert_eq!(
            check_rules(&owned(&["get", "POST"]), &owned(&["/x*"])),
            Ok(())
        );
        let error = check_rules(&owned(&["FETCH"]), &[]).unwrap_err();
        assert!(error.contains("\"FETCH\""), "{error}");
        let error = check_rules(&[], &owned(&["pets/*"])).unwrap_err();
        assert!(error.contains("starts with /"), "{error}");
        let error = check_rules(&[], &owned(&["/pets?x=1"])).unwrap_err();
        assert!(error.contains("no query"), "{error}");
    }

    #[test]
    fn the_refusal_names_the_request_and_the_rule() {
        assert_eq!(
            describe_refusal(&Refusal::Method, "POST", "/pets"),
            "POST /pets: POST is not in allow_methods"
        );
        assert!(
            describe_refusal(&Refusal::DeniedPath("/a*".into()), "GET", "/a/b")
                .contains("deny_paths \"/a*\"")
        );
    }
}
