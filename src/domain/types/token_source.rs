//! One `[auth.<name>]` credential source of kind `token`: a credential that
//! was issued elsewhere (an API key), read from a secret store when it is
//! needed and presented where the configuration says -- a header, HTTP Basic
//! or a query parameter.

use base64::Engine;

use super::SecretRef;
use super::http::{HttpRequest, is_header_name};

/// The only placeholder `format` substitutes the resolved value for.
pub const TOKEN_PLACEHOLDER: &str = "{token}";

/// Header a source presents its credential in when it names none.
pub const DEFAULT_TOKEN_HEADER: &str = "Authorization";

/// Value the header carries when the source names no `format`.
pub const DEFAULT_TOKEN_FORMAT: &str = "Bearer {token}";

/// A validated `[auth.<name>]` entry of kind `token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSourceConfig {
    pub name: String,
    /// Always a reference: a literal would live in the configuration file and
    /// in every backup of it.
    pub token: SecretRef,
    /// Where the credential goes in a request.
    pub placement: TokenPlacement,
    /// Variable `kurama env` / `kurama exec` put the credential in.
    pub env_var: String,
}

/// Where a `kind = "token"` source puts its credential. One enum, because a
/// source that named a header and a query parameter would send the
/// credential twice or to one place nobody reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenPlacement {
    /// In the header `name`, as `format` with `{token}` replaced.
    Header { name: String, format: String },
    /// `Authorization: Basic`, with `username` and the credential as the
    /// password (Jira's email and API token, Zendesk's `<email>/token`).
    Basic { username: String },
    /// In the query parameter `param` (Backlog's `apiKey`).
    Query { param: String },
}

impl TokenPlacement {
    /// Where the credential goes, in a few words: the header name, `basic`,
    /// or `query <param>`.
    pub fn summary(&self) -> String {
        match self {
            Self::Header { name, .. } => name.clone(),
            Self::Basic { .. } => "basic".to_string(),
            Self::Query { param } => format!("query {param}"),
        }
    }
}

impl TokenSourceConfig {
    /// The rules a valid entry follows; the message names the offending key.
    pub fn validate(&self) -> Result<(), String> {
        if !self.token.is_reference() {
            return Err(
                "token must be a secret reference, not the value itself: op://<vault>/<item>/<field>, \
                 aws-secrets://<aws-profile>/<secret-id> or aws-ssm://<aws-profile>/<parameter-name>"
                    .to_string(),
            );
        }
        match &self.placement {
            TokenPlacement::Header { name, format } => {
                check_header_name(name)?;
                check_format(format)
            }
            // RFC 7617: the user-id ends at the first colon, so a colon in
            // it would move part of the username into the password.
            TokenPlacement::Basic { username } if username.is_empty() || username.contains(':') => {
                Err(format!(
                    "username must be non-empty and carry no colon, got {username:?}"
                ))
            }
            TokenPlacement::Basic { .. } => Ok(()),
            TokenPlacement::Query { param } if param.is_empty() => {
                Err("query must name the parameter the credential goes in".to_string())
            }
            TokenPlacement::Query { .. } => Ok(()),
        }
    }

    /// The header this source sends its credential in, if it uses one; the
    /// header `--dry-run` and `-v` mask.
    pub fn header_name(&self) -> Option<&str> {
        match &self.placement {
            TokenPlacement::Header { name, .. } => Some(name),
            TokenPlacement::Basic { .. } => Some(DEFAULT_TOKEN_HEADER),
            TokenPlacement::Query { .. } => None,
        }
    }

    /// `request` with `value` where this source puts it, as the only value
    /// there: a header written with `-H`, or a parameter already in the URL,
    /// is replaced rather than added to.
    #[must_use]
    pub fn apply(&self, mut request: HttpRequest, value: &str) -> HttpRequest {
        let header = match &self.placement {
            TokenPlacement::Header { name, format } => {
                (name.clone(), format.replace(TOKEN_PLACEHOLDER, value))
            }
            TokenPlacement::Basic { username } => {
                let pair = format!("{username}:{value}");
                let encoded = base64::engine::general_purpose::STANDARD.encode(pair);
                (DEFAULT_TOKEN_HEADER.to_string(), format!("Basic {encoded}"))
            }
            TokenPlacement::Query { param } => {
                request.url = with_query_param(&request.url, param, value);
                return request;
            }
        };
        request
            .headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case(&header.0));
        request.headers.push(header);
        request
    }
}

/// `url` with `param=value` as the only value of `param`. A URL that does
/// not parse is returned as it is: the request fails on it either way.
fn with_query_param(url: &str, param: &str, value: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(name, _)| name != param)
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    parsed
        .query_pairs_mut()
        .clear()
        .extend_pairs(kept)
        .append_pair(param, value);
    parsed.into()
}

fn check_header_name(header: &str) -> Result<(), String> {
    if !is_header_name(header) {
        return Err(format!(
            "header must be a header name such as Authorization or X-API-Key, got {header:?}"
        ));
    }
    Ok(())
}

/// `format` carries `{token}` and no other brace: a second placeholder
/// would be a value kurama has no way to fill in, and leaving it as written
/// would send `{api_key}` to the API as if it were the credential.
fn check_format(format: &str) -> Result<(), String> {
    if !format.contains(TOKEN_PLACEHOLDER) {
        return Err(format!(
            "format must carry the credential as {TOKEN_PLACEHOLDER}, got {format:?}"
        ));
    }
    // What is left once every placeholder is taken out is literal text, and
    // a brace in literal text is either another placeholder or one that was
    // never closed.
    let literal = format.replace(TOKEN_PLACEHOLDER, "");
    if literal.contains('{') || literal.contains('}') {
        return Err(format!(
            "format takes no placeholder besides {TOKEN_PLACEHOLDER}, and {format:?} has another"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(token: SecretRef, header: &str, format: &str) -> TokenSourceConfig {
        placed(
            token,
            TokenPlacement::Header {
                name: header.into(),
                format: format.into(),
            },
        )
    }

    fn placed(token: SecretRef, placement: TokenPlacement) -> TokenSourceConfig {
        TokenSourceConfig {
            name: "example".into(),
            token,
            placement,
            env_var: "EXAMPLE_TOKEN".into(),
        }
    }

    fn header_value(config: &TokenSourceConfig, token: &str) -> String {
        let request = config.apply(HttpRequest::new("GET", "https://api.example.com/x"), token);
        request.headers[0].1.clone()
    }

    fn reference() -> SecretRef {
        SecretRef::parse("aws-secrets://dev/example/api-key").unwrap()
    }

    #[test]
    fn the_default_shape_is_a_bearer_authorization_header() {
        let config = source(reference(), DEFAULT_TOKEN_HEADER, DEFAULT_TOKEN_FORMAT);
        assert!(config.validate().is_ok());
        assert_eq!(header_value(&config, "abc123"), "Bearer abc123");
    }

    #[test]
    fn another_header_and_format_carry_the_bare_credential() {
        let config = source(reference(), "xi-api-key", "{token}");
        assert!(config.validate().is_ok());
        assert_eq!(header_value(&config, "abc123"), "abc123");
        let prefixed = source(reference(), "X-API-Key", "token {token}");
        assert!(prefixed.validate().is_ok());
        assert_eq!(header_value(&prefixed, "abc123"), "token abc123");
    }

    #[test]
    fn a_literal_token_is_refused_because_it_would_live_in_the_file() {
        let message = source(
            SecretRef::parse("plain-api-key").unwrap(),
            DEFAULT_TOKEN_HEADER,
            DEFAULT_TOKEN_FORMAT,
        )
        .validate()
        .unwrap_err();
        assert!(
            message.contains("token must be a secret reference"),
            "{message}"
        );
        assert!(message.contains("op://"), "{message}");
        assert!(message.contains("aws-secrets://"), "{message}");
        assert!(message.contains("aws-ssm://"), "{message}");
        assert!(!message.contains("plain-api-key"), "{message}");
    }

    #[rstest::rstest]
    #[case(
        "Authorization",
        "ApiKey {api_key}",
        "format must carry the credential"
    )]
    #[case("Authorization", "Bearer {token", "format must carry the credential")]
    #[case("Authorization", "Bearer token", "format must carry the credential")]
    // A second placeholder is found wherever it sits, and an open brace and
    // a close brace are each one on their own.
    #[case("Authorization", "{token}{api_key}", "takes no placeholder besides")]
    #[case("Authorization", "{token} {x}", "takes no placeholder besides")]
    #[case("Authorization", "{token}{", "takes no placeholder besides")]
    #[case("Authorization", "{token}}", "takes no placeholder besides")]
    #[case("Authorization: Bearer", "{token}", "header must be a header name")]
    #[case("", "{token}", "header must be a header name")]
    fn an_unusable_header_or_format_names_the_key(
        #[case] header: &str,
        #[case] format: &str,
        #[case] expected: &str,
    ) {
        let message = source(reference(), header, format).validate().unwrap_err();
        assert!(message.contains(expected), "{message}");
        // Whatever was written is quoted back, so the line to fix is visible.
        assert!(
            !message.contains("{api_key}") || message.contains(format),
            "{message}"
        );
    }

    /// Two `{token}` are two copies of one value, not a second placeholder.
    #[test]
    fn the_same_placeholder_may_appear_twice() {
        let config = source(reference(), "X-Auth", "{token}:{token}");
        assert!(config.validate().is_ok());
        assert_eq!(header_value(&config, "k"), "k:k");
    }

    /// Jira takes `email:api-token`, Zendesk `email/token:api-token`, both
    /// base64 in `Authorization: Basic`, replacing an `Authorization` given
    /// with `-H`.
    #[test]
    fn basic_sends_the_username_and_the_credential_as_the_password() {
        let config = placed(
            reference(),
            TokenPlacement::Basic {
                username: "me@example.com/token".into(),
            },
        );
        assert!(config.validate().is_ok());
        let request = HttpRequest::new("GET", "https://x.zendesk.com/api/v2/users/me")
            .with_header("authorization", "Bearer other");

        let sent = config.apply(request, "s3cret");

        assert_eq!(
            sent.headers,
            vec![(
                "Authorization".to_string(),
                "Basic bWVAZXhhbXBsZS5jb20vdG9rZW46czNjcmV0".to_string()
            )]
        );
        assert_eq!(config.header_name(), Some("Authorization"));
    }

    #[test]
    fn a_basic_username_with_a_colon_or_none_is_refused() {
        for username in ["", "me:you"] {
            let message = placed(
                reference(),
                TokenPlacement::Basic {
                    username: username.into(),
                },
            )
            .validate()
            .unwrap_err();
            assert!(message.contains("username must be"), "{message}");
        }
    }

    /// Backlog takes `apiKey` in the query; the other parameters keep their
    /// order and a value already there is replaced, not repeated.
    #[test]
    fn query_puts_the_credential_in_its_parameter_only() {
        let config = placed(
            reference(),
            TokenPlacement::Query {
                param: "apiKey".into(),
            },
        );
        assert!(config.validate().is_ok());

        let sent = config.apply(
            HttpRequest::new(
                "GET",
                "https://x.backlog.com/api/v2/issues?count=1&apiKey=old&a=b",
            ),
            "k&y",
        );

        assert_eq!(
            sent.url,
            "https://x.backlog.com/api/v2/issues?count=1&a=b&apiKey=k%26y"
        );
        assert!(sent.headers.is_empty());
        assert_eq!(config.header_name(), None);
        let bare = config.apply(
            HttpRequest::new("GET", "https://x.backlog.com/api/v2/users/myself"),
            "k",
        );
        assert_eq!(
            bare.url,
            "https://x.backlog.com/api/v2/users/myself?apiKey=k"
        );
    }

    #[test]
    fn a_query_without_a_parameter_name_is_refused() {
        let message = placed(
            reference(),
            TokenPlacement::Query {
                param: String::new(),
            },
        )
        .validate()
        .unwrap_err();
        assert!(message.contains("query must name"), "{message}");
    }
}
