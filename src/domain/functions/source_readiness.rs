//! Whether each configured source can be used right now without a person, from what `status` already read; no I/O.
use crate::domain::functions::auth_status::{AuthStatus, TokenState};
use crate::domain::functions::profile_status::{ProfileStatus, SessionState, describe_remaining};
use crate::domain::types::{AuthSource, SecretRef};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// Whether a call through a source would go through now. Ordered from the
/// best to the worst: a source that depends on others is as ready as the
/// least ready of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    /// A call goes through without anyone.
    Ready,
    /// A call goes through, but 1Password asks a person to approve a read
    /// first; without one it fails once `[onepassword] timeout` passes.
    WillPrompt,
    /// A call stops with exit code 3 until a person runs `next_actions`.
    NeedsHuman,
    /// The configuration names what does not exist; no call can work.
    Misconfigured,
}

/// One row of `kurama agent ready`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceReadiness {
    pub name: String,
    /// `aws`, `auth`, `api` or `db`, as `status` names them.
    pub kind: &'static str,
    pub state: Readiness,
    pub reason: String,
    /// When the session or token the state rests on ends.
    pub expires_at: Option<String>,
    /// Commands a person runs to make the source ready.
    pub next_actions: Vec<String>,
}

/// Whether 1Password can be read without a person: a service account token
/// in the environment, or a keychain entry the configuration names.
#[derive(Clone, Copy, Debug)]
pub struct OnePassword {
    pub mfa_enabled: bool,
    pub headless: bool,
}

/// What a source rests on, as a state, a reason and what to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    pub state: Readiness,
    pub reason: String,
    pub next_actions: Vec<String>,
}

impl Condition {
    fn ready(reason: impl Into<String>) -> Self {
        Self {
            state: Readiness::Ready,
            reason: reason.into(),
            next_actions: vec![],
        }
    }

    fn of(row: &SourceReadiness) -> Self {
        Self {
            state: row.state,
            reason: format!("through {} {}: {}", row.kind, row.name, row.reason),
            next_actions: row.next_actions.clone(),
        }
    }

    /// The least ready of `self` and `other`; its reason explains the state,
    /// and every action either needs is kept once.
    fn and(self, other: Self) -> Self {
        let (mut worst, best) = if other.state > self.state {
            (other, self)
        } else {
            (self, other)
        };
        for action in best.next_actions {
            if !worst.next_actions.contains(&action) {
                worst.next_actions.push(action);
            }
        }
        worst
    }

    pub fn row(
        self,
        name: &str,
        kind: &'static str,
        expires_at: Option<String>,
    ) -> SourceReadiness {
        SourceReadiness {
            name: name.to_owned(),
            kind,
            state: self.state,
            reason: self.reason,
            expires_at,
            next_actions: self.next_actions,
        }
    }
}

fn login(name: &str) -> Vec<String> {
    vec![format!("kurama login {name}")]
}

fn format_time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn aws_readiness(
    status: &ProfileStatus,
    one_password: OnePassword,
    now: DateTime<Utc>,
) -> SourceReadiness {
    let name = &status.name;
    let why = match status.session {
        SessionState::Unreadable => "the cached MFA session cannot be read",
        SessionState::CacheDisabled => "the MFA session cache is disabled",
        _ => "no MFA session is cached",
    };
    let (condition, expires_at) = match &status.session {
        SessionState::NotRequired => (Condition::ready("no MFA: the role is assumed as is"), None),
        SessionState::Valid { expires_at } => (
            Condition::ready(format!(
                "MFA session valid for {}",
                describe_remaining(*expires_at, now)
            )),
            Some(format_time(*expires_at)),
        ),
        _ if !one_password.mfa_enabled => (
            Condition {
                state: Readiness::NeedsHuman,
                reason: format!("MFA is required, {why} and no MFA provider is enabled"),
                next_actions: login(name),
            },
            None,
        ),
        _ if one_password.headless => (
            Condition::ready(format!(
                "{why}; the TOTP comes from 1Password with the service account"
            )),
            None,
        ),
        _ => (
            Condition {
                state: Readiness::WillPrompt,
                reason: format!("{why}; reading the TOTP from 1Password asks a person to approve"),
                next_actions: login(name),
            },
            None,
        ),
    };
    condition.row(name, "aws", expires_at)
}

/// What reading a configured secret takes: nothing for a literal, 1Password
/// for `op://`, the role of an AWS profile for `aws-*://`.
pub fn secret_condition(
    secret: &SecretRef,
    one_password: OnePassword,
    aws: &dyn Fn(&str) -> Condition,
) -> Condition {
    match secret {
        SecretRef::Literal(_) => Condition::ready("the secret is written in the configuration"),
        SecretRef::OnePassword(_) if one_password.headless => {
            Condition::ready("the secret is read from 1Password with the service account")
        }
        SecretRef::OnePassword(_) => Condition {
            state: Readiness::WillPrompt,
            reason: "reading the secret from 1Password asks a person to approve".into(),
            next_actions: vec![],
        },
        SecretRef::Aws(reference) => aws(&reference.aws_profile),
    }
}

pub fn auth_readiness(
    source: &AuthSource,
    status: &AuthStatus,
    one_password: OnePassword,
    aws: &dyn Fn(&str) -> Condition,
    now: DateTime<Utc>,
) -> SourceReadiness {
    let name = source.name();
    let client = match source {
        AuthSource::Token(issued) => {
            return Condition::ready("the credential is read from its reference on each call")
                .and(secret_condition(&issued.token, one_password, aws))
                .row(name, "auth", None);
        }
        AuthSource::Secrets(secrets) => {
            return secrets
                .env
                .values()
                .fold(
                    Condition::ready("the secrets are read from their references on each exec"),
                    |condition, reference| {
                        condition.and(secret_condition(reference, one_password, aws))
                    },
                )
                .row(name, "auth", None);
        }
        AuthSource::OAuth(client) => client,
    };
    let secret = || match &client.client_secret {
        Some(secret) => secret_condition(secret, one_password, aws),
        None => Condition::ready("the client has no secret"),
    };
    let (condition, expires_at) = match &status.token {
        TokenState::Valid { expires_at } => (
            Condition::ready(match expires_at {
                Some(at) => format!("token valid for {}", describe_remaining(*at, now)),
                None => "token valid, with no expiration".into(),
            }),
            expires_at.map(format_time),
        ),
        TokenState::Expired { refreshable: true } => (
            Condition::ready("token expired; the refresh token renews it on the next call")
                .and(secret()),
            None,
        ),
        token => {
            let why = match token {
                TokenState::Expired { .. } => "the token expired and there is no refresh token",
                TokenState::Unreadable => "the stored token cannot be read",
                _ => "no token is stored",
            };
            let condition = if client.grant_type.needs_human() {
                Condition {
                    state: Readiness::NeedsHuman,
                    reason: format!(
                        "{why}; the {} grant needs a person to log in",
                        client.grant_type.as_str()
                    ),
                    next_actions: login(name),
                }
            } else {
                Condition::ready(format!(
                    "{why}; the {} grant gets one without a person",
                    client.grant_type.as_str()
                ))
                .and(secret())
            };
            (condition, None)
        }
    };
    condition.row(name, "auth", expires_at)
}

/// A source that runs through others (`[api.*]`, `[db.*]`): as ready as the
/// least ready of what it needs, or ready with `alone` when it needs nothing.
pub fn dependent_readiness(
    name: &str,
    kind: &'static str,
    needs: Vec<Condition>,
    alone: &str,
) -> SourceReadiness {
    needs
        .into_iter()
        .reduce(Condition::and)
        .unwrap_or_else(|| Condition::ready(alone))
        .row(name, kind, None)
}

/// The condition of a source another one names: its row, or misconfigured
/// when the configuration names what there is not.
pub fn named(rows: &[SourceReadiness], kind: &'static str, name: &str) -> Condition {
    match rows.iter().find(|row| row.kind == kind && row.name == name) {
        Some(row) => Condition::of(row),
        None => Condition {
            state: Readiness::Misconfigured,
            reason: format!("names the {kind} {name}, which is not configured"),
            next_actions: vec!["kurama config check".into()],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{GrantType, OAuthClientConfig};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()
    }

    fn profile(session: SessionState) -> ProfileStatus {
        ProfileStatus {
            name: "ops".into(),
            role_arn: None,
            region: None,
            mfa_serial: Some("arn:mfa".into()),
            active: false,
            session,
            needs_human: false,
        }
    }

    const HEADLESS: OnePassword = OnePassword {
        mfa_enabled: true,
        headless: true,
    };
    const PROMPTING: OnePassword = OnePassword {
        mfa_enabled: true,
        headless: false,
    };
    const NO_PROVIDER: OnePassword = OnePassword {
        mfa_enabled: false,
        headless: false,
    };

    #[test]
    fn source_readiness_of_a_profile_follows_its_mfa_session() {
        let valid = aws_readiness(
            &profile(SessionState::Valid {
                expires_at: now() + chrono::TimeDelta::minutes(90),
            }),
            NO_PROVIDER,
            now(),
        );
        assert_eq!(valid.state, Readiness::Ready);
        assert_eq!(valid.reason, "MFA session valid for 1h 30m");
        assert_eq!(valid.expires_at.as_deref(), Some("2030-01-01T01:30:00Z"));
        for (one_password, state, actions) in [
            (NO_PROVIDER, Readiness::NeedsHuman, 1),
            (PROMPTING, Readiness::WillPrompt, 1),
            (HEADLESS, Readiness::Ready, 0),
        ] {
            let row = aws_readiness(&profile(SessionState::Missing), one_password, now());
            assert_eq!(row.state, state, "{row:?}");
            assert_eq!(row.next_actions.len(), actions);
        }
        let without_mfa = aws_readiness(&profile(SessionState::NotRequired), NO_PROVIDER, now());
        assert_eq!(without_mfa.state, Readiness::Ready);
    }

    fn oauth(grant_type: GrantType, secret: Option<SecretRef>) -> AuthSource {
        AuthSource::OAuth(OAuthClientConfig {
            name: "gh".into(),
            grant_type,
            endpoints: crate::domain::types::EndpointSource::Issuer("https://id".into()),
            client_id: "id".into(),
            client_secret: secret,
            scopes: vec![],
            env_var: "GH".into(),
            redirect_port: None,
        })
    }

    fn status(token: TokenState) -> AuthStatus {
        AuthStatus {
            name: "gh".into(),
            detail: super::super::auth_status::AuthDetail::OAuth {
                grant_type: GrantType::AuthorizationCode,
            },
            env_vars: vec!["GH".into()],
            token,
            active: false,
            needs_human: false,
        }
    }

    fn no_aws(_: &str) -> Condition {
        unreachable!("no AWS reference here")
    }

    #[test]
    fn source_readiness_of_an_oauth_source_names_the_login_it_waits_for() {
        let code = oauth(GrantType::AuthorizationCode, None);
        for token in [
            TokenState::Missing,
            TokenState::Expired { refreshable: false },
            TokenState::Unreadable,
        ] {
            let row = auth_readiness(&code, &status(token), HEADLESS, &no_aws, now());
            assert_eq!(row.state, Readiness::NeedsHuman);
            assert_eq!(row.next_actions, ["kurama login gh"]);
            assert!(row.reason.contains("authorization_code"), "{}", row.reason);
        }
        let refreshable = auth_readiness(
            &code,
            &status(TokenState::Expired { refreshable: true }),
            HEADLESS,
            &no_aws,
            now(),
        );
        assert_eq!(refreshable.state, Readiness::Ready);
        let machine = oauth(
            GrantType::ClientCredentials,
            Some(SecretRef::OnePassword("op://v/i/f".into())),
        );
        let prompting = auth_readiness(
            &machine,
            &status(TokenState::Missing),
            PROMPTING,
            &no_aws,
            now(),
        );
        assert_eq!(prompting.state, Readiness::WillPrompt);
        assert!(!prompting.reason.contains("op://"), "{}", prompting.reason);
        let headless = auth_readiness(
            &machine,
            &status(TokenState::Missing),
            HEADLESS,
            &no_aws,
            now(),
        );
        assert_eq!(headless.state, Readiness::Ready);
    }

    #[test]
    fn source_readiness_of_a_secret_follows_its_reference_and_never_names_it() {
        let op = SecretRef::OnePassword("op://Agent/item/field".into());
        assert_eq!(
            secret_condition(&op, PROMPTING, &no_aws).state,
            Readiness::WillPrompt
        );
        let headless = secret_condition(&op, HEADLESS, &no_aws);
        assert_eq!(headless.state, Readiness::Ready);
        assert!(!headless.reason.contains("op://"), "{}", headless.reason);
        let literal = SecretRef::Literal("value".into());
        assert_eq!(
            secret_condition(&literal, PROMPTING, &no_aws).state,
            Readiness::Ready
        );
        let aws = SecretRef::parse("aws-ssm://ops/kurama/key").unwrap();
        let through = secret_condition(&aws, HEADLESS, &|profile| Condition {
            state: Readiness::NeedsHuman,
            reason: profile.into(),
            next_actions: vec![],
        });
        assert_eq!(
            (through.state, through.reason.as_str()),
            (Readiness::NeedsHuman, "ops")
        );
    }

    #[test]
    fn source_readiness_of_a_dependent_source_is_its_least_ready_need() {
        let rows = [
            aws_readiness(&profile(SessionState::Missing), NO_PROVIDER, now()),
            aws_readiness(&profile(SessionState::NotRequired), NO_PROVIDER, now()).row_named("dev"),
        ];
        let api = dependent_readiness(
            "svc",
            "api",
            vec![named(&rows, "aws", "dev"), named(&rows, "aws", "ops")],
            "no credential",
        );
        assert_eq!(api.state, Readiness::NeedsHuman);
        assert_eq!(api.next_actions, ["kurama login ops"]);
        assert!(api.reason.starts_with("through aws ops:"), "{}", api.reason);
        let missing = dependent_readiness("svc", "api", vec![named(&rows, "aws", "gone")], "-");
        assert_eq!(missing.state, Readiness::Misconfigured);
        let alone = dependent_readiness("svc", "api", vec![], "no credential");
        assert_eq!(
            (alone.state, alone.reason.as_str()),
            (Readiness::Ready, "no credential")
        );
    }

    impl SourceReadiness {
        fn row_named(self, name: &str) -> Self {
            Self {
                name: name.into(),
                ..self
            }
        }
    }
}
