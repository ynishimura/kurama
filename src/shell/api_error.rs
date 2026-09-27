//! `ApiError`: the failures of `kurama api` that are not OAuth failures, and
//! the text each one prints.

use crate::domain::functions::operation_request::OperationRequestError;
use crate::ports::HttpError;

/// Failures of `kurama api` that are not OAuth failures.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("API not found: {0}")]
    NotFound(String),
    #[error("TARGET is required: a path such as /user, a URL, or `METHOD /path`")]
    TargetRequired,
    #[error("{0}")]
    ArgumentInvalid(String),
    /// The API answered with a status outside 2xx.
    #[error("HTTP {status}{}", format_reason(.reason, .excerpt))]
    HttpStatus {
        status: u16,
        reason: String,
        excerpt: String,
    },
    #[error("request failed")]
    RequestFailed(#[source] HttpError),
    #[error("{0}")]
    Jq(String),
    /// `--output PATH` could not be written; whatever was at the path is
    /// left as it was.
    #[error("cannot write the response body to {path}")]
    OutputFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// Neither the profile, the host nor the AWS profile say what to sign
    /// for; `api` names the `[api.*]` section for the hint.
    #[error("[api.{api}] {message}")]
    SigningTargetRequired { api: String, message: String },
    /// `--ops`, `--describe`, `--refresh-spec`, an operation target or the
    /// explorer on an API without `openapi`.
    #[error("[api.{api}] has no openapi description; {needed} needs one")]
    SpecRequired { api: String, needed: String },
    /// The description could not be read or fetched (and nothing is cached).
    #[error("cannot load the API description {location}: {message}")]
    SpecUnavailable {
        api: String,
        /// The `[api.*]` key the description is configured under.
        key: &'static str,
        location: String,
        message: String,
    },
    /// A dry run found no cached copy of a description that is fetched with
    /// the API's credential (`openapi_auth`), and a dry run uses none.
    #[error(
        "the API description {location} is fetched with the API's credential (openapi_auth) and a dry run uses none; no copy is cached"
    )]
    SpecNeedsCredential { api: String, location: String },
    /// The description parsed or normalized with an error.
    #[error("the API description {location} is not usable: {message}")]
    SpecInvalid {
        api: String,
        /// The `[api.*]` key the description is configured under.
        key: &'static str,
        location: String,
        message: String,
    },
    /// No operation of the description matches the TARGET or `--describe`.
    #[error("operation {target:?} is not in the API description{}", format_candidates(.candidates))]
    OperationNotFound {
        api: String,
        target: String,
        candidates: Vec<String>,
    },
    /// The `-P` parameters or the body do not fit the operation.
    #[error("{error}")]
    OperationInput {
        api: String,
        error: OperationRequestError,
    },
    /// A GraphQL endpoint answered the introspection query with errors only.
    #[error("the GraphQL endpoint {location} refuses introspection: {message}")]
    IntrospectionRefused {
        api: String,
        location: String,
        message: String,
    },
    /// A GraphQL operation was answered with `errors`: the first one, its
    /// path and how many more there are.
    #[error("the GraphQL answer carries errors: {0}")]
    GraphQl(String),
}

fn format_candidates(candidates: &[String]) -> String {
    if candidates.is_empty() {
        String::new()
    } else {
        format!("; did you mean {}?", candidates.join(", "))
    }
}

fn format_reason(reason: &str, excerpt: &str) -> String {
    let mut text = String::new();
    if !reason.is_empty() {
        text.push(' ');
        text.push_str(reason);
    }
    if !excerpt.is_empty() {
        text.push_str(": ");
        text.push_str(excerpt);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_http_status_names_its_reason_and_excerpt_only_when_there_are_any() {
        let status = |reason: &str, excerpt: &str| {
            ApiError::HttpStatus {
                status: 404,
                reason: reason.into(),
                excerpt: excerpt.into(),
            }
            .to_string()
        };
        assert_eq!(
            status("Not Found", "no such pet"),
            "HTTP 404 Not Found: no such pet"
        );
        assert_eq!(status("Not Found", ""), "HTTP 404 Not Found");
        assert_eq!(status("", "no such pet"), "HTTP 404: no such pet");
        assert_eq!(status("", ""), "HTTP 404");
    }

    #[test]
    fn an_unknown_operation_names_the_candidates_only_when_there_are_any() {
        let error = |candidates: Vec<String>| {
            ApiError::OperationNotFound {
                api: "pets".into(),
                target: "pets/creat".into(),
                candidates,
            }
            .to_string()
        };
        assert_eq!(
            error(vec!["pets/create".into(), "pets/get".into()]),
            r#"operation "pets/creat" is not in the API description; did you mean pets/create, pets/get?"#
        );
        assert_eq!(
            error(vec![]),
            r#"operation "pets/creat" is not in the API description"#
        );
    }
}
