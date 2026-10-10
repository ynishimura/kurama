//! What `kurama status` reports for an `[auth.*]` credential source: the
//! token in the store, whether the shell holds it, and whether getting a
//! token would need a person.

use chrono::{DateTime, Utc};

use super::oauth::TOKEN_REUSE_MARGIN;
use super::profile_status::describe_remaining;
use crate::domain::types::{AuthKind, AuthSource, GrantType};

/// What the token store holds for one credential source, without the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenLookup {
    Stored {
        expires_at: Option<DateTime<Utc>>,
        refreshable: bool,
    },
    /// The store could not be read (for example a locked keychain).
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenState {
    /// A usable token is stored; `None` means it carries no expiration.
    Valid {
        expires_at: Option<DateTime<Utc>>,
    },
    /// The stored token is expired; `refreshable` when a refresh token exists.
    Expired {
        refreshable: bool,
    },
    Missing,
    Unreadable,
    /// Nothing was looked up: a `kind = "token"` or `kind = "secrets"`
    /// source keeps its values in secret stores, which `status` does not read.
    NotChecked,
}

impl TokenState {
    /// Stable identifier for JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Valid { .. } => "valid",
            Self::Expired { .. } => "expired",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
            Self::NotChecked => "not_checked",
        }
    }
}

/// What differs between the kinds of source, one variant per kind, so a
/// row cannot claim a grant for a source that runs none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDetail {
    /// The grant an OAuth source gets its first token from.
    OAuth { grant_type: GrantType },
    /// The header a `kind = "token"` source presents its credential in;
    /// `None` when it goes in a query parameter.
    Token { header: Option<String> },
    /// A `kind = "secrets"` source: its variables say all there is.
    Secrets,
}

impl AuthDetail {
    pub fn kind(&self) -> AuthKind {
        match self {
            Self::OAuth { .. } => AuthKind::OAuth,
            Self::Token { .. } => AuthKind::Token,
            Self::Secrets => AuthKind::Secrets,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub name: String,
    pub detail: AuthDetail,
    /// The variables `env` / `exec` set: one for a token, one per secret.
    pub env_vars: Vec<String>,
    pub token: TokenState,
    /// The shell holds this source's values (`KURAMA_AUTH`).
    pub active: bool,
    /// `kurama token` / `kurama api` would stop with exit code 3 without a
    /// terminal: no usable token, nothing to refresh, and the grant needs a
    /// person.
    pub needs_human: bool,
}

/// What `status` reports for one source. A `kind = "token"` or `kind =
/// "secrets"` source is reported without a lookup: its values are in secret
/// stores, and reading them would be the AWS or 1Password call `status`
/// promises not to make.
pub fn auth_status(
    source: &AuthSource,
    lookup: Option<&TokenLookup>,
    active_auth: Option<&str>,
    now: DateTime<Utc>,
) -> AuthStatus {
    let active = active_auth == Some(source.name());
    let env_vars = source.env_vars().into_iter().map(str::to_string).collect();
    let not_checked = |detail| AuthStatus {
        name: source.name().to_string(),
        detail,
        env_vars,
        token: TokenState::NotChecked,
        active,
        needs_human: false,
    };
    let client = match source {
        AuthSource::Token(issued) => {
            return not_checked(AuthDetail::Token {
                header: issued.header_name().map(str::to_string),
            });
        }
        AuthSource::Secrets(_) => return not_checked(AuthDetail::Secrets),
        AuthSource::OAuth(client) => client,
    };
    let token = match lookup {
        None => TokenState::Missing,
        Some(TokenLookup::Unreadable) => TokenState::Unreadable,
        Some(TokenLookup::Stored {
            expires_at,
            refreshable,
        }) => match expires_at {
            Some(expires_at) if *expires_at - now <= TOKEN_REUSE_MARGIN => TokenState::Expired {
                refreshable: *refreshable,
            },
            _ => TokenState::Valid {
                expires_at: *expires_at,
            },
        },
    };
    let headless = matches!(
        token,
        TokenState::Valid { .. } | TokenState::Expired { refreshable: true }
    );
    AuthStatus {
        name: client.name.clone(),
        detail: AuthDetail::OAuth {
            grant_type: client.grant_type,
        },
        env_vars: vec![client.env_var.clone()],
        active,
        needs_human: !headless && client.grant_type.needs_human(),
        token,
    }
}

/// Short text for a session column: `valid (1h 52m)`, `valid`, `expired
/// (refreshable)`, `expired`, `none`, `unreadable`, `not_checked`.
pub fn describe_token(state: &TokenState, now: DateTime<Utc>) -> String {
    match state {
        TokenState::Valid {
            expires_at: Some(expires_at),
        } => format!("valid ({})", describe_remaining(*expires_at, now)),
        TokenState::Valid { expires_at: None } => "valid".to_string(),
        TokenState::Expired { refreshable: true } => "expired (refreshable)".to_string(),
        TokenState::Expired { refreshable: false } => "expired".to_string(),
        TokenState::Missing => "none".to_string(),
        TokenState::Unreadable => "unreadable".to_string(),
        TokenState::NotChecked => "not_checked".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{
        DEFAULT_TOKEN_ENV_VAR, EndpointSource, OAuthClientConfig, OAuthEndpoints, SecretRef,
        TokenSourceConfig,
    };
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn client(grant: GrantType) -> AuthSource {
        AuthSource::OAuth(OAuthClientConfig {
            name: "github".into(),
            grant_type: grant,
            endpoints: EndpointSource::Explicit(OAuthEndpoints {
                auth_url: Some("https://as/auth".into()),
                token_url: "https://as/token".into(),
                device_auth_url: Some("https://as/device".into()),
            }),
            client_id: "id".into(),
            client_secret: None,
            scopes: vec![],
            env_var: DEFAULT_TOKEN_ENV_VAR.into(),
            redirect_port: None,
        })
    }

    fn issued() -> AuthSource {
        AuthSource::Token(TokenSourceConfig {
            name: "example".into(),
            token: SecretRef::parse("aws-secrets://dev/example/api-key").unwrap(),
            placement: crate::domain::types::TokenPlacement::Header {
                name: "X-API-Key".into(),
                format: "{token}".into(),
            },
            env_var: "EXAMPLE_TOKEN".into(),
        })
    }

    /// The credential of a `kind = "token"` source is in a secret store, so
    /// `status` reports it as unchecked and never as missing: "missing" is
    /// a state a person would try to fix with a login that does not exist.
    #[test]
    fn a_token_source_is_reported_without_a_lookup() {
        let status = auth_status(&issued(), None, Some("example"), now());
        assert_eq!(
            status.detail,
            AuthDetail::Token {
                header: Some("X-API-Key".into())
            }
        );
        assert_eq!(status.detail.kind(), AuthKind::Token);
        assert_eq!(status.env_vars, ["EXAMPLE_TOKEN"]);
        assert_eq!(status.token, TokenState::NotChecked);
        assert_eq!(status.token.as_str(), "not_checked");
        assert_eq!(describe_token(&status.token, now()), "not_checked");
        assert!(status.active);
        assert!(!status.needs_human);
    }

    /// A `kind = "secrets"` source is reported by its variable names, never
    /// a value, and like a token source without a lookup.
    #[test]
    fn a_secrets_source_is_reported_by_its_variables_without_a_lookup() {
        let source = AuthSource::Secrets(crate::domain::types::SecretsSourceConfig {
            name: "site".into(),
            env: [
                ("SITE_USER", "op://Agent/site/username"),
                ("SITE_OTP", "op://Agent/site/otp"),
            ]
            .into_iter()
            .map(|(var, reference)| (var.into(), SecretRef::parse(reference).unwrap()))
            .collect(),
        });
        let status = auth_status(&source, None, Some("site"), now());
        assert_eq!(status.detail, AuthDetail::Secrets);
        assert_eq!(status.detail.kind(), AuthKind::Secrets);
        assert_eq!(status.env_vars, ["SITE_OTP", "SITE_USER"]);
        assert_eq!(status.token, TokenState::NotChecked);
        assert!(status.active);
        assert!(!status.needs_human);
    }

    /// A stored token belongs to no `kind = "token"` source: were one ever
    /// passed in, reporting it would say a credential is cached that nothing
    /// wrote and `logout` cannot remove.
    #[test]
    fn a_token_source_ignores_a_lookup_it_was_handed() {
        let status = auth_status(
            &issued(),
            Some(&TokenLookup::Stored {
                expires_at: Some(now() + Duration::hours(2)),
                refreshable: true,
            }),
            None,
            now(),
        );
        assert_eq!(status.token, TokenState::NotChecked);
        assert!(!status.active);
    }

    #[test]
    fn valid_token_needs_nobody_and_marks_the_active_source() {
        let status = auth_status(
            &client(GrantType::AuthorizationCode),
            Some(&TokenLookup::Stored {
                expires_at: Some(now() + Duration::hours(2)),
                refreshable: false,
            }),
            Some("github"),
            now(),
        );
        assert_eq!(
            status.token,
            TokenState::Valid {
                expires_at: Some(now() + Duration::hours(2))
            }
        );
        assert!(status.active);
        assert!(!status.needs_human);
        assert_eq!(status.env_vars, ["KURAMA_TOKEN"]);
        assert_eq!(
            status.detail,
            AuthDetail::OAuth {
                grant_type: GrantType::AuthorizationCode
            }
        );
    }

    #[rstest::rstest]
    #[case(None, GrantType::AuthorizationCode, TokenState::Missing, true)]
    #[case(None, GrantType::DeviceCode, TokenState::Missing, true)]
    #[case(None, GrantType::ClientCredentials, TokenState::Missing, false)]
    #[case(
        Some(TokenLookup::Unreadable),
        GrantType::AuthorizationCode,
        TokenState::Unreadable,
        true
    )]
    #[case(Some(TokenLookup::Stored { expires_at: Some(now() - Duration::hours(1)), refreshable: true }), GrantType::AuthorizationCode, TokenState::Expired { refreshable: true }, false)]
    #[case(Some(TokenLookup::Stored { expires_at: Some(now() + Duration::seconds(30)), refreshable: false }), GrantType::AuthorizationCode, TokenState::Expired { refreshable: false }, true)]
    #[case(Some(TokenLookup::Stored { expires_at: None, refreshable: false }), GrantType::AuthorizationCode, TokenState::Valid { expires_at: None }, false)]
    fn token_state_and_human_need(
        #[case] lookup: Option<TokenLookup>,
        #[case] grant: GrantType,
        #[case] expected: TokenState,
        #[case] needs_human: bool,
    ) {
        let status = auth_status(&client(grant), lookup.as_ref(), None, now());
        assert_eq!(status.token, expected);
        assert_eq!(status.needs_human, needs_human);
        assert!(!status.active);
    }

    #[rstest::rstest]
    #[case(TokenState::Valid { expires_at: Some(now() + Duration::minutes(112)) }, "valid (1h 52m)")]
    #[case(TokenState::Valid { expires_at: None }, "valid")]
    #[case(TokenState::Expired { refreshable: true }, "expired (refreshable)")]
    #[case(TokenState::Expired { refreshable: false }, "expired")]
    #[case(TokenState::Missing, "none")]
    #[case(TokenState::Unreadable, "unreadable")]
    fn token_descriptions(#[case] state: TokenState, #[case] expected: &str) {
        assert_eq!(describe_token(&state, now()), expected);
        assert!(!state.as_str().is_empty());
    }
}
