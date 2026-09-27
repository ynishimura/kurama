//! Session name generation functions
//!
//! Pure functions for generating AWS session names from a user-configurable
//! template. Supported placeholders: `{prefix}`, `{profile}`, `{readonly}`,
//! `{role}`, `{account}`.

/// Configuration for session name generation
#[derive(Debug, Clone)]
pub struct SessionNameConfig {
    /// Template with {prefix}, {profile}, {readonly}, {role}, {account}
    /// placeholders (default: "{prefix}-{readonly}-{profile}")
    pub template: String,
    /// Prefix for session names (default: "kurama")
    pub prefix: String,
    /// Indicator for readonly sessions (default: "ro")
    pub readonly_indicator: String,
    /// Maximum length for session name (default: 64, AWS limit)
    pub max_length: usize,
}

impl SessionNameConfig {
    /// Create a new config with defaults
    pub fn new() -> Self {
        Self {
            template: "{prefix}-{readonly}-{profile}".to_string(),
            prefix: "kurama".to_string(),
            readonly_indicator: "ro".to_string(),
            max_length: 64,
        }
    }

    /// Set custom template
    #[must_use]
    pub fn with_template(mut self, template: impl Into<String>) -> Self {
        self.template = template.into();
        self
    }

    /// Set custom prefix
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set custom readonly indicator
    #[must_use]
    pub fn with_readonly_indicator(mut self, indicator: impl Into<String>) -> Self {
        self.readonly_indicator = indicator.into();
        self
    }
}

impl Default for SessionNameConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate a session name template
///
/// Rejects empty templates, unmatched `{`, and placeholders other than
/// `{prefix}`, `{profile}`, `{readonly}`, `{role}`, `{account}`.
pub fn validate_session_name_template(template: &str) -> Result<(), String> {
    if template.trim().is_empty() {
        return Err("session name template must not be empty".to_string());
    }
    tokenize_session_name_template(template).map(|_| ())
}

/// A single token of a parsed session name template
///
/// Splitting the template into tokens up front lets each placeholder be
/// substituted exactly once, so a substituted value that happens to contain
/// placeholder-like text (e.g. a user-supplied prefix of `"team-{profile}-x"`)
/// is never re-scanned as template syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionNameToken {
    /// Literal text, copied verbatim into the output.
    Literal(String),
    /// A placeholder to be substituted with a value.
    Placeholder(PlaceholderKind),
}

/// The placeholders a session name template may contain
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaceholderKind {
    Prefix,
    Profile,
    Readonly,
    Role,
    Account,
}

/// Values available to session name templates
///
/// `role_name` and `account_id` are derived from the profile's role ARN and
/// may be absent when the ARN is malformed; templates that reference them
/// then fail to render.
#[derive(Debug, Clone, Default)]
pub struct SessionNameContext<'a> {
    pub profile_name: &'a str,
    pub role_name: Option<&'a str>,
    pub account_id: Option<&'a str>,
}

/// Render a session name from a template
///
/// Rendering rules:
/// 1. The template is parsed into literal and placeholder tokens.
/// 2. `{readonly}` is resolved first: replaced with `readonly_indicator` in
///    readonly mode; otherwise removed together with exactly one adjacent
///    hyphen (the left one is preferred).
/// 3. `{prefix}` and `{profile}` are replaced with their values. `{role}`
///    and `{account}` are replaced with `ctx.role_name` / `ctx.account_id`
///    when present; when absent (a malformed role ARN), rendering fails.
/// 4. The result is sanitized (invalid characters become `-`) and truncated
///    to `max_length`.
/// 5. Results shorter than 2 characters (AWS minimum) are an error.
///
/// # Example
///
/// ```
/// use kurama::domain::functions::session::{render_session_name, SessionNameConfig, SessionNameContext};
///
/// let config = SessionNameConfig::new().with_template("claude-{profile}");
/// let ctx = SessionNameContext {
///     profile_name: "dev",
///     ..Default::default()
/// };
/// assert_eq!(
///     render_session_name(&config, &ctx, false).unwrap(),
///     "claude-dev"
/// );
///
/// let config = SessionNameConfig::new().with_template("{prefix}-{role}");
/// let ctx = SessionNameContext {
///     profile_name: "dev",
///     role_name: Some("MyRole"),
///     account_id: None,
/// };
/// assert_eq!(render_session_name(&config, &ctx, false).unwrap(), "kurama-MyRole");
/// ```
pub fn render_session_name(
    config: &SessionNameConfig,
    ctx: &SessionNameContext<'_>,
    readonly: bool,
) -> Result<String, String> {
    validate_session_name_template(&config.template)?;

    let tokens = tokenize_session_name_template(&config.template)?;
    let tokens = resolve_readonly_tokens(tokens, &config.readonly_indicator, readonly);
    let rendered = render_session_name_tokens(&tokens, &config.prefix, ctx, &config.template)?;

    let name = truncate_session_name(&sanitize_session_name(&rendered), config.max_length);

    if name.len() < 2 {
        return Err(format!(
            "rendered session name '{}' is shorter than 2 characters (template: '{}')",
            name, config.template
        ));
    }

    Ok(name)
}

/// Parse a session name template into literal and placeholder tokens, or say
/// why it is not one: an unmatched `{` or a placeholder kurama does not know.
fn tokenize_session_name_template(template: &str) -> Result<Vec<SessionNameToken>, String> {
    let mut tokens = Vec::new();
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        if start > 0 {
            tokens.push(SessionNameToken::Literal(rest[..start].to_string()));
        }

        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            return Err("unmatched '{' in session name template".to_string());
        };
        let name = &after[..end];
        let kind = match name {
            "prefix" => PlaceholderKind::Prefix,
            "profile" => PlaceholderKind::Profile,
            "readonly" => PlaceholderKind::Readonly,
            "role" => PlaceholderKind::Role,
            "account" => PlaceholderKind::Account,
            _ => {
                return Err(format!(
                    "unknown placeholder '{{{}}}' in session name template \
                     (valid: {{prefix}}, {{profile}}, {{readonly}}, {{role}}, {{account}})",
                    name
                ));
            }
        };
        tokens.push(SessionNameToken::Placeholder(kind));

        rest = &after[end + 1..];
    }

    if !rest.is_empty() {
        tokens.push(SessionNameToken::Literal(rest.to_string()));
    }

    Ok(tokens)
}

/// Resolve `{readonly}` tokens, per the "readonly first" rule
///
/// In readonly mode every `{readonly}` token becomes a literal holding
/// `indicator`. Otherwise each `{readonly}` token is dropped together with
/// exactly one adjacent hyphen: a trailing `-` on the preceding literal
/// token is preferred, falling back to a leading `-` on the following
/// literal token.
fn resolve_readonly_tokens(
    mut tokens: Vec<SessionNameToken>,
    indicator: &str,
    readonly: bool,
) -> Vec<SessionNameToken> {
    if readonly {
        for token in &mut tokens {
            if let SessionNameToken::Placeholder(PlaceholderKind::Readonly) = token {
                *token = SessionNameToken::Literal(indicator.to_string());
            }
        }
        return tokens;
    }

    let mut i = 0;
    while i < tokens.len() {
        if !matches!(
            tokens[i],
            SessionNameToken::Placeholder(PlaceholderKind::Readonly)
        ) {
            i += 1;
            continue;
        }

        let mut absorbed = false;
        if i > 0
            && let SessionNameToken::Literal(prev) = &mut tokens[i - 1]
            && prev.ends_with('-')
        {
            prev.pop();
            absorbed = true;
        }
        if !absorbed
            && let Some(SessionNameToken::Literal(next)) = tokens.get_mut(i + 1)
            && next.starts_with('-')
        {
            next.remove(0);
        }

        tokens.remove(i);
        // Do not advance `i`: removal shifted the following token into this slot.
    }

    tokens
}

/// Render tokens into the final string, substituting each placeholder once
///
/// `{role}` / `{account}` require `ctx.role_name` / `ctx.account_id` to be
/// `Some`; when the value is absent (a malformed role ARN), rendering fails
/// with an error naming the unresolvable placeholder.
fn render_session_name_tokens(
    tokens: &[SessionNameToken],
    prefix: &str,
    ctx: &SessionNameContext<'_>,
    template: &str,
) -> Result<String, String> {
    let mut result = String::new();
    for token in tokens {
        match token {
            SessionNameToken::Literal(text) => result.push_str(text),
            SessionNameToken::Placeholder(PlaceholderKind::Prefix) => result.push_str(prefix),
            SessionNameToken::Placeholder(PlaceholderKind::Profile) => {
                result.push_str(ctx.profile_name);
            }
            SessionNameToken::Placeholder(PlaceholderKind::Readonly) => {
                unreachable!("{{readonly}} tokens must be resolved before rendering")
            }
            SessionNameToken::Placeholder(PlaceholderKind::Role) => {
                let role_name = ctx.role_name.ok_or_else(|| {
                    format!(
                        "cannot resolve {{role}} in session name template '{}': role ARN missing or malformed",
                        template
                    )
                })?;
                result.push_str(role_name);
            }
            SessionNameToken::Placeholder(PlaceholderKind::Account) => {
                let account_id = ctx.account_id.ok_or_else(|| {
                    format!(
                        "cannot resolve {{account}} in session name template '{}': role ARN missing or malformed",
                        template
                    )
                })?;
                result.push_str(account_id);
            }
        }
    }
    Ok(result)
}

/// Truncate session name to fit AWS limits
///
/// AWS session names must be:
/// - 2-64 characters
/// - Match pattern [\w+=,.@-]*
fn truncate_session_name(name: &str, max_length: usize) -> String {
    if name.len() <= max_length {
        name.to_string()
    } else {
        name[..max_length].to_string()
    }
}

/// Sanitize a string for use in session name
///
/// Replaces invalid characters with hyphens.
/// Valid characters: alphanumeric, =, +, ,, ., @, -, _
pub fn sanitize_session_name(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "=+,.@-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_session_name() {
        let long_name = "a".repeat(100);
        let truncated = truncate_session_name(&long_name, 64);
        assert_eq!(truncated.len(), 64);
    }

    #[test]
    fn test_sanitize_session_name() {
        assert_eq!(sanitize_session_name("valid-name_123"), "valid-name_123");
        assert_eq!(sanitize_session_name("invalid name!"), "invalid-name-");
        assert_eq!(sanitize_session_name("test@email.com"), "test@email.com");
    }

    mod render_session_name_tests {
        use super::*;

        fn ctx(profile_name: &str) -> SessionNameContext<'_> {
            SessionNameContext {
                profile_name,
                ..Default::default()
            }
        }

        #[test]
        fn default_template_renders_prefix_readonly_and_profile() {
            let config = SessionNameConfig::default();
            assert_eq!(
                render_session_name(&config, &ctx("prod"), false).unwrap(),
                "kurama-prod"
            );
            assert_eq!(
                render_session_name(&config, &ctx("prod"), true).unwrap(),
                "kurama-ro-prod"
            );
        }

        #[test]
        fn custom_template_renders_placeholders() {
            let config = SessionNameConfig::new().with_template("claude-{profile}");
            assert_eq!(
                render_session_name(&config, &ctx("kurama-real"), false).unwrap(),
                "claude-kurama-real"
            );
        }

        #[test]
        fn fixed_template_ignores_profile() {
            let config = SessionNameConfig::new().with_template("my-fixed-name");
            assert_eq!(
                render_session_name(&config, &ctx("prod"), false).unwrap(),
                "my-fixed-name"
            );
        }

        #[test]
        fn readonly_at_start_absorbs_trailing_hyphen() {
            let config = SessionNameConfig::new().with_template("{readonly}-{profile}");
            assert_eq!(
                render_session_name(&config, &ctx("prod"), false).unwrap(),
                "prod"
            );
            assert_eq!(
                render_session_name(&config, &ctx("prod"), true).unwrap(),
                "ro-prod"
            );
        }

        #[test]
        fn readonly_at_end_absorbs_leading_hyphen() {
            let config = SessionNameConfig::new().with_template("{profile}-{readonly}");
            assert_eq!(
                render_session_name(&config, &ctx("prod"), false).unwrap(),
                "prod"
            );
            assert_eq!(
                render_session_name(&config, &ctx("prod"), true).unwrap(),
                "prod-ro"
            );
        }

        #[test]
        fn custom_readonly_indicator_is_used() {
            let config = SessionNameConfig::new().with_readonly_indicator("readonly");
            assert_eq!(
                render_session_name(&config, &ctx("prod"), true).unwrap(),
                "kurama-readonly-prod"
            );
        }

        #[test]
        fn profile_hyphens_are_preserved() {
            let config = SessionNameConfig::default();
            assert_eq!(
                render_session_name(&config, &ctx("my--profile"), false).unwrap(),
                "kurama-my--profile"
            );
        }

        #[test]
        fn invalid_characters_are_sanitized() {
            let config = SessionNameConfig::default();
            assert_eq!(
                render_session_name(&config, &ctx("my profile!"), false).unwrap(),
                "kurama-my-profile-"
            );
        }

        #[test]
        fn result_is_truncated_to_max_length() {
            let config = SessionNameConfig::default();
            let long_profile = "p".repeat(100);
            let name = render_session_name(&config, &ctx(&long_profile), false).unwrap();
            assert_eq!(name.len(), 64);
        }

        #[test]
        fn non_ascii_profile_does_not_panic_on_truncation() {
            let config = SessionNameConfig::default();
            let profile = "\u{3042}".repeat(100); // 100 x Japanese 'a'
            let name = render_session_name(&config, &ctx(&profile), false).unwrap();
            assert_eq!(name.len(), 64);
        }

        #[test]
        fn too_short_result_is_an_error() {
            let config = SessionNameConfig::new().with_template("{readonly}");
            let result = render_session_name(&config, &ctx("prod"), false);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("2 characters"));
        }

        #[test]
        fn unknown_placeholder_is_an_error() {
            let config = SessionNameConfig::new().with_template("{prefix}-{user}");
            assert!(render_session_name(&config, &ctx("prod"), false).is_err());
        }

        #[test]
        fn placeholder_like_text_in_values_is_not_reinterpreted() {
            let config = SessionNameConfig::new().with_prefix("team-{profile}-x");
            let name = render_session_name(&config, &ctx("dev"), false).unwrap();
            // The literal "{profile}" inside prefix must not be substituted;
            // braces are outside the allowed charset and sanitize to '-'.
            assert_eq!(name, "team--profile--x-dev");
        }

        #[test]
        fn role_placeholder_renders_role_name() {
            let config = SessionNameConfig::new().with_template("{prefix}-{role}");
            let ctx = SessionNameContext {
                profile_name: "dev",
                role_name: Some("MyRole"),
                account_id: None,
            };
            assert_eq!(
                render_session_name(&config, &ctx, false).unwrap(),
                "kurama-MyRole"
            );
        }

        #[test]
        fn account_placeholder_renders_account_id() {
            let config = SessionNameConfig::new().with_template("{account}-{profile}");
            let ctx = SessionNameContext {
                profile_name: "prod",
                role_name: None,
                account_id: Some("123456789012"),
            };
            assert_eq!(
                render_session_name(&config, &ctx, false).unwrap(),
                "123456789012-prod"
            );
        }

        #[test]
        fn unresolvable_role_placeholder_is_an_error() {
            let config = SessionNameConfig::new().with_template("{prefix}-{role}");
            let ctx = SessionNameContext {
                profile_name: "dev",
                ..Default::default()
            };
            let result = render_session_name(&config, &ctx, false);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("{role}"));
        }

        #[test]
        fn unresolvable_account_placeholder_is_an_error() {
            let config = SessionNameConfig::new().with_template("{account}");
            let ctx = SessionNameContext {
                profile_name: "dev",
                ..Default::default()
            };
            let result = render_session_name(&config, &ctx, false);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("{account}"));
        }
    }

    mod validate_template_tests {
        use super::*;

        #[test]
        fn accepts_default_template() {
            assert!(validate_session_name_template("{prefix}-{readonly}-{profile}").is_ok());
        }

        #[test]
        fn accepts_template_without_placeholders() {
            assert!(validate_session_name_template("fixed-name").is_ok());
        }

        #[test]
        fn rejects_unknown_placeholder_and_lists_valid_ones() {
            let result = validate_session_name_template("{prefix}-{user}");
            assert!(result.is_err());
            let message = result.unwrap_err();
            assert!(message.contains("{user}"));
            assert!(message.contains("{prefix}"));
            assert!(message.contains("{profile}"));
            assert!(message.contains("{readonly}"));
            assert!(message.contains("{role}"));
            assert!(message.contains("{account}"));
        }

        #[test]
        fn accepts_role_and_account_placeholders() {
            assert!(validate_session_name_template("{prefix}-{role}-{account}").is_ok());
        }

        #[test]
        fn rejects_empty_template() {
            assert!(validate_session_name_template("").is_err());
            assert!(validate_session_name_template("   ").is_err());
        }

        #[test]
        fn rejects_unclosed_brace() {
            assert!(validate_session_name_template("{prefix-{profile}").is_err());
        }
    }
}
