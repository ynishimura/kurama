//! Export script generation functions
//!
//! Pure functions for generating shell export scripts.

use sha2::{Digest, Sha256};

use crate::domain::types::{Credentials, Profile, RequestAuth, SourceCredential, check_env_var};

/// Environment variables that hold AWS credentials or select an AWS profile.
///
/// kurama removes them from its own environment before any STS call, and the
/// export script unsets them before exporting new values.
pub const AWS_ENV_VARS: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_SECURITY_TOKEN",
    "AWS_SESSION_EXPIRATION",
    "AWS_CREDENTIAL_EXPIRATION",
    "AWS_PROFILE",
    "AWS_DEFAULT_PROFILE",
    "AWS_READONLY_SESSION",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
];

/// Name of the AWS profile whose credentials the shell holds, exported with
/// the credentials. `kurama status` marks that profile as active. It carries
/// no credentials, so the SDK-boundary environment cleanup keeps it out of
/// AWS's credential provider chain.
pub const ACTIVE_AWS_PROFILE_VAR: &str = "KURAMA_AWS";

/// Name of the `[auth.*]` source whose token the shell holds, exported with
/// the token by `kurama env <auth>`.
pub const ACTIVE_AUTH_VAR: &str = "KURAMA_AUTH";

/// Names of the variables `kurama env <auth>` exported into, space
/// separated (one for a token, one per secret of a `secrets` source),
/// exported with them, so `unset` clears those variables after the
/// configuration stopped naming them.
pub const ACTIVE_AUTH_ENV_VAR: &str = "KURAMA_AUTH_VAR";

/// Every variable the export script sets or unsets.
fn managed_vars() -> impl Iterator<Item = &'static str> {
    AWS_ENV_VARS
        .iter()
        .copied()
        .chain(std::iter::once(ACTIVE_AWS_PROFILE_VAR))
}

/// The variables that carry a profile's role credentials, in export order.
pub fn credential_env_vars(
    credentials: &Credentials,
    profile: &Profile,
    readonly: bool,
) -> Vec<(&'static str, String)> {
    let mut vars = vec![
        ("AWS_ACCESS_KEY_ID", credentials.access_key_id().to_string()),
        (
            "AWS_SECRET_ACCESS_KEY",
            credentials.secret_access_key().to_string(),
        ),
    ];
    if let Some(token) = credentials.session_token() {
        vars.push(("AWS_SESSION_TOKEN", token.to_string()));
    }
    if let Some(expiration) = credentials.expiration() {
        vars.push(("AWS_SESSION_EXPIRATION", expiration.to_string()));
    }
    if let Some(region) = profile.region_raw() {
        vars.push(("AWS_REGION", region.to_string()));
        vars.push(("AWS_DEFAULT_REGION", region.to_string()));
    }
    vars.push((ACTIVE_AWS_PROFILE_VAR, profile.name().to_string()));
    if readonly {
        vars.push(("AWS_READONLY_SESSION", "true".to_string()));
    }
    vars
}

/// Shell script that unsets every managed variable, then exports the
/// profile's credentials.
///
/// ```ignore
/// unset AWS_ACCESS_KEY_ID
/// ...
/// export AWS_ACCESS_KEY_ID='ASIA...'
/// export KURAMA_AWS='prod'
/// ```
pub fn generate_export_script(
    credentials: &Credentials,
    profile: &Profile,
    readonly: bool,
) -> String {
    managed_vars()
        .map(|var| format!("unset {var}"))
        .chain(
            credential_env_vars(credentials, profile, readonly)
                .into_iter()
                .map(|(name, value)| export_var(name, &value)),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate credential_process compatible JSON from credentials
///
/// Output format follows AWS credential_process specification:
/// https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-sourcing-external.html
pub fn generate_credential_json(credentials: &Credentials) -> String {
    let mut map = serde_json::Map::new();
    map.insert("Version".to_string(), serde_json::Value::from(1));
    map.insert(
        "AccessKeyId".to_string(),
        serde_json::Value::from(credentials.access_key_id()),
    );
    map.insert(
        "SecretAccessKey".to_string(),
        serde_json::Value::from(credentials.secret_access_key()),
    );
    if let Some(token) = credentials.session_token() {
        map.insert("SessionToken".to_string(), serde_json::Value::from(token));
    }
    if let Some(expiration) = credentials.expiration() {
        map.insert(
            "Expiration".to_string(),
            serde_json::Value::from(expiration.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        );
    }
    serde_json::Value::Object(map).to_string()
}

/// Shell script that unsets every variable kurama exports: the AWS set and
/// the token variables of every `[auth.*]` source (`token_managed_vars`).
pub fn generate_unset_script(token_vars: &[String]) -> String {
    managed_vars()
        .map(str::to_string)
        .chain(token_vars.iter().cloned())
        .map(|var| format!("unset {var}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every variable `kurama env <auth>` manages: each configured variable
/// once, in order, then the names in `previous` -- the shell's
/// `KURAMA_AUTH_VAR`, which the configuration may no longer name -- when
/// every one of them is a variable name (the value comes from the shell and
/// ends up in a script the shell runs, so a value that is not a list of
/// names is not read in part), then `KURAMA_AUTH` and `KURAMA_AUTH_VAR`.
pub fn token_managed_vars<'a>(
    env_vars: impl IntoIterator<Item = &'a str>,
    previous: Option<&'a str>,
) -> Vec<String> {
    let mut previous: Vec<&str> = previous
        .map(|names| names.split_whitespace().collect())
        .unwrap_or_default();
    if !previous.iter().all(|var| check_env_var(var).is_ok()) {
        previous.clear();
    }
    let mut vars: Vec<String> = Vec::new();
    for var in env_vars.into_iter().chain(previous) {
        if !vars.iter().any(|known| known == var) {
            vars.push(var.to_string());
        }
    }
    vars.push(ACTIVE_AUTH_VAR.to_string());
    vars.push(ACTIVE_AUTH_ENV_VAR.to_string());
    vars
}

/// The variables `env` / `exec` set for the source `name`, in export order:
/// each of `values`, then the source name in `KURAMA_AUTH` and the names of
/// `values`, space separated, in `KURAMA_AUTH_VAR`.
pub fn auth_env_vars(name: &str, values: Vec<(String, String)>) -> Vec<(String, String)> {
    let names = values
        .iter()
        .map(|(var, _)| var.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    values
        .into_iter()
        .chain([
            (ACTIVE_AUTH_VAR.to_string(), name.to_string()),
            (ACTIVE_AUTH_ENV_VAR.to_string(), names),
        ])
        .collect()
}

/// The variables that carry a source's one credential, in export order.
pub fn token_env_vars(
    credential: &SourceCredential,
    source: &RequestAuth,
) -> Vec<(String, String)> {
    auth_env_vars(
        source.name(),
        vec![(source.env_var().to_string(), credential.value().to_string())],
    )
}

/// Shell script that unsets every managed token variable, then exports
/// `vars` (from `auth_env_vars`).
pub fn generate_auth_export_script(vars: Vec<(String, String)>, managed: &[String]) -> String {
    managed
        .iter()
        .map(|var| format!("unset {var}"))
        .chain(
            vars.into_iter()
                .map(|(name, value)| export_var(&name, &value)),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

/// The values of a `secrets` source as `kurama env <source> --json` prints
/// them: `{"env": {"<VAR>": "<value>", ...}}`, the shape of the `env` table
/// they are configured in.
pub fn generate_secrets_json(values: &[(String, String)]) -> String {
    let env: serde_json::Map<String, serde_json::Value> = values
        .iter()
        .map(|(var, value)| (var.clone(), value.as_str().into()))
        .collect();
    serde_json::json!({ "env": env }).to_string()
}

/// The credential as `kurama token --json` and `kurama env <auth> --json`
/// print it. The four keys are the same for both kinds of source: a
/// credential that was issued elsewhere says nothing about its type, its
/// expiry or its scope, and `null` is what kurama knows about them.
pub fn generate_token_json(credential: &SourceCredential) -> String {
    let (token_type, expires_at, scope) = match credential {
        SourceCredential::OAuth(token) => (
            Some(token.token_type.clone()),
            token.expires_at.map(format_expiry),
            token.scope.clone(),
        ),
        SourceCredential::Issued(_) => (None, None, None),
    };
    serde_json::json!({
        "access_token": credential.value(),
        "token_type": token_type,
        "expires_at": expires_at,
        "scope": scope,
    })
    .to_string()
}

/// `sha256:<hex>` of the credential: says whether two sources hold one
/// value without saying anything else about it -- not even its length.
pub fn token_fingerprint(credential: &SourceCredential) -> String {
    let hex: String = Sha256::digest(credential.value().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// `kurama token --fingerprint --json`: the fingerprint and nothing else.
pub fn generate_fingerprint_json(credential: &SourceCredential) -> String {
    serde_json::json!({ "fingerprint": token_fingerprint(credential) }).to_string()
}

fn format_expiry(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Generate export script for a single variable
pub fn export_var(name: &str, value: &str) -> String {
    format!("export {}='{}'", name, escape_shell_value(value))
}

/// Escape a value for safe shell usage
fn escape_shell_value(value: &str) -> String {
    // Single quotes in shell need special handling
    value.replace('\'', "'\"'\"'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::OAuthToken;
    use chrono::{DateTime, Utc};

    fn create_test_credentials() -> Credentials {
        Credentials::new(
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            Some("token123".to_string()),
            None,
        )
    }

    fn create_test_profile() -> Profile {
        Profile::new("test").with_region_raw("us-east-1")
    }

    #[test]
    fn test_generate_export_script() {
        let creds = create_test_credentials();
        let profile = create_test_profile();

        let script = generate_export_script(&creds, &profile, false);

        // Should first unset all AWS env vars to clear stale values
        assert!(script.contains("unset AWS_ACCESS_KEY_ID"));
        assert!(script.contains("unset AWS_SESSION_TOKEN"));
        assert!(script.contains("unset KURAMA_AWS"));

        // Then export new values
        assert!(script.contains("export AWS_ACCESS_KEY_ID='AKIAIOSFODNN7EXAMPLE'"));
        assert!(
            script.contains(
                "export AWS_SECRET_ACCESS_KEY='wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY'"
            )
        );
        assert!(script.contains("export AWS_SESSION_TOKEN='token123'"));
        assert!(script.contains("export AWS_REGION='us-east-1'"));
        assert!(script.contains("export KURAMA_AWS='test'"));
        assert!(!script.contains("export AWS_READONLY_SESSION"));
    }

    #[test]
    fn test_generate_export_script_readonly() {
        let creds = create_test_credentials();
        let profile = create_test_profile();

        let script = generate_export_script(&creds, &profile, true);

        assert!(script.contains("export AWS_READONLY_SESSION='true'"));
    }

    #[test]
    fn credential_env_vars_skip_what_the_profile_does_not_have() {
        let creds = Credentials::new("AKIA".to_string(), "secret".to_string(), None, None);
        let names: Vec<&str> = credential_env_vars(&creds, &Profile::new("plain"), false)
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(
            names,
            ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "KURAMA_AWS"]
        );
    }

    #[test]
    fn active_profile_var_is_separate_from_aws_cleanup() {
        assert!(!AWS_ENV_VARS.contains(&ACTIVE_AWS_PROFILE_VAR));
    }

    #[test]
    fn test_generate_unset_script() {
        let script = generate_unset_script(&["GITHUB_TOKEN".to_string()]);

        assert!(script.contains("unset AWS_ACCESS_KEY_ID"));
        assert!(script.contains("unset AWS_SECRET_ACCESS_KEY"));
        assert!(script.contains("unset AWS_SESSION_TOKEN"));
        assert!(script.contains("unset KURAMA_AWS"));
        assert!(script.ends_with("unset GITHUB_TOKEN"));
    }

    fn oauth_client() -> RequestAuth {
        use crate::domain::types::{EndpointSource, GrantType, OAuthClientConfig, OAuthEndpoints};
        RequestAuth::OAuth(OAuthClientConfig {
            name: "github".into(),
            grant_type: GrantType::AuthorizationCode,
            endpoints: EndpointSource::Explicit(OAuthEndpoints {
                auth_url: Some("https://as/auth".into()),
                token_url: "https://as/token".into(),
                device_auth_url: None,
            }),
            client_id: "id".into(),
            client_secret: None,
            scopes: vec![],
            env_var: "GITHUB_TOKEN".into(),
            redirect_port: None,
        })
    }

    fn issued_source() -> RequestAuth {
        use crate::domain::types::{SecretRef, TokenSourceConfig};
        RequestAuth::Token(TokenSourceConfig {
            name: "example".into(),
            token: SecretRef::parse("op://Agent/Example/credential").unwrap(),
            placement: crate::domain::types::TokenPlacement::Header {
                name: "X-API-Key".into(),
                format: "{token}".into(),
            },
            env_var: "EXAMPLE_TOKEN".into(),
        })
    }

    #[test]
    fn token_vars_are_deduplicated_and_end_with_the_active_markers() {
        assert_eq!(
            token_managed_vars(["GITHUB_TOKEN", "KURAMA_TOKEN", "GITHUB_TOKEN"], None),
            [
                "GITHUB_TOKEN",
                "KURAMA_TOKEN",
                "KURAMA_AUTH",
                "KURAMA_AUTH_VAR"
            ]
        );
        assert_eq!(
            token_managed_vars([], None),
            ["KURAMA_AUTH", "KURAMA_AUTH_VAR"]
        );
    }

    /// The variable an earlier `env` exported into is cleared even when no
    /// source names it any more, and only when it is a variable name: the
    /// value comes from the shell and ends up in a script the shell runs.
    #[test]
    fn the_previously_exported_variable_is_cleared_when_it_is_a_name() {
        assert_eq!(
            token_managed_vars(["GITHUB_TOKEN"], Some("OLD_TOKEN")),
            [
                "GITHUB_TOKEN",
                "OLD_TOKEN",
                "KURAMA_AUTH",
                "KURAMA_AUTH_VAR"
            ]
        );
        assert_eq!(
            token_managed_vars(["GITHUB_TOKEN"], Some("GITHUB_TOKEN")),
            ["GITHUB_TOKEN", "KURAMA_AUTH", "KURAMA_AUTH_VAR"]
        );
        // A `secrets` source recorded several names; each is cleared.
        assert_eq!(
            token_managed_vars(["GITHUB_TOKEN"], Some("SITE_USER SITE_PASS")),
            [
                "GITHUB_TOKEN",
                "SITE_USER",
                "SITE_PASS",
                "KURAMA_AUTH",
                "KURAMA_AUTH_VAR"
            ]
        );
        assert_eq!(
            token_managed_vars([], Some("SITE_USER $(rm -rf ~) 1X")),
            ["KURAMA_AUTH", "KURAMA_AUTH_VAR"]
        );
        for hostile in ["X; rm -rf ~", "", "1X"] {
            assert_eq!(
                token_managed_vars(["GITHUB_TOKEN"], Some(hostile)),
                ["GITHUB_TOKEN", "KURAMA_AUTH", "KURAMA_AUTH_VAR"],
                "{hostile:?}"
            );
        }
    }

    #[test]
    fn token_export_script_unsets_every_token_variable_then_exports_the_source() {
        let token = SourceCredential::OAuth(OAuthToken::bearer("gho_secret'quote"));
        let managed = token_managed_vars(["GITHUB_TOKEN", "OTHER_TOKEN"], None);
        let script = generate_auth_export_script(token_env_vars(&token, &oauth_client()), &managed);
        assert_eq!(
            script,
            "unset GITHUB_TOKEN\nunset OTHER_TOKEN\nunset KURAMA_AUTH\nunset KURAMA_AUTH_VAR\n\
             export GITHUB_TOKEN='gho_secret'\"'\"'quote'\nexport KURAMA_AUTH='github'\n\
             export KURAMA_AUTH_VAR='GITHUB_TOKEN'"
        );
        assert_eq!(token_env_vars(&token, &oauth_client())[0].0, "GITHUB_TOKEN");
    }

    /// A source of either kind exports the same three variables, so the shell
    /// wrapper and `kurama unset` have one shape to clear.
    #[test]
    fn an_issued_credential_exports_the_same_three_variables() {
        let credential = SourceCredential::Issued("api-key".into());
        let script = generate_auth_export_script(
            token_env_vars(&credential, &issued_source()),
            &token_managed_vars(["EXAMPLE_TOKEN"], None),
        );
        assert_eq!(
            script,
            "unset EXAMPLE_TOKEN\nunset KURAMA_AUTH\nunset KURAMA_AUTH_VAR\n\
             export EXAMPLE_TOKEN='api-key'\nexport KURAMA_AUTH='example'\n\
             export KURAMA_AUTH_VAR='EXAMPLE_TOKEN'"
        );
    }

    /// A `secrets` source exports every variable, its name, and the
    /// variables' names, so `unset` clears all of them after a rename.
    #[test]
    fn a_secrets_source_exports_every_variable_and_records_their_names() {
        let vars = auth_env_vars(
            "site",
            vec![
                ("SITE_PASS".into(), "p'w".into()),
                ("SITE_USER".into(), "me".into()),
            ],
        );
        assert_eq!(
            generate_auth_export_script(
                vars,
                &token_managed_vars(["SITE_PASS", "SITE_USER"], None)
            ),
            "unset SITE_PASS\nunset SITE_USER\nunset KURAMA_AUTH\nunset KURAMA_AUTH_VAR\n\
             export SITE_PASS='p'\"'\"'w'\nexport SITE_USER='me'\nexport KURAMA_AUTH='site'\n\
             export KURAMA_AUTH_VAR='SITE_PASS SITE_USER'"
        );
        let json: serde_json::Value = serde_json::from_str(&generate_secrets_json(&[
            ("SITE_PASS".into(), "pw".into()),
            ("SITE_USER".into(), "me".into()),
        ]))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"env": {"SITE_PASS": "pw", "SITE_USER": "me"}})
        );
    }

    #[test]
    fn token_json_carries_expiration_and_scope() {
        let mut token = OAuthToken::bearer("at");
        token.expires_at = Some(
            DateTime::parse_from_rfc3339("2026-09-17T03:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        token.scope = Some("repo".into());
        let json: serde_json::Value =
            serde_json::from_str(&generate_token_json(&SourceCredential::OAuth(token))).unwrap();
        assert_eq!(json["access_token"], "at");
        assert_eq!(json["token_type"], "Bearer");
        assert_eq!(json["expires_at"], "2026-09-17T03:00:00Z");
        assert_eq!(json["scope"], "repo");
        let bare: serde_json::Value = serde_json::from_str(&generate_token_json(
            &SourceCredential::OAuth(OAuthToken::bearer("x")),
        ))
        .unwrap();
        assert!(bare["expires_at"].is_null());
        assert!(bare["scope"].is_null());
    }

    /// An issued credential has no type, expiry or scope; saying `"Bearer"`
    /// would be a claim about a value that may go in `X-API-Key`.
    #[test]
    fn an_issued_credential_reports_only_its_value() {
        let json: serde_json::Value = serde_json::from_str(&generate_token_json(
            &SourceCredential::Issued("api-key".into()),
        ))
        .unwrap();
        assert_eq!(json["access_token"], "api-key");
        assert!(json["token_type"].is_null());
        assert!(json["expires_at"].is_null());
        assert!(json["scope"].is_null());
    }

    /// The SHA-256 of the value, and nothing else about it: two sources that
    /// resolve to one value print one line whichever kind they are.
    #[test]
    fn a_fingerprint_is_the_sha256_of_the_value_alone() {
        let issued = SourceCredential::Issued("abc".into());
        assert_eq!(
            token_fingerprint(&issued),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            token_fingerprint(&SourceCredential::OAuth(OAuthToken::bearer("abc"))),
            token_fingerprint(&issued)
        );
        assert_ne!(
            token_fingerprint(&SourceCredential::Issued("abd".into())),
            token_fingerprint(&issued)
        );
    }

    /// `--fingerprint --json` names the fingerprint and nothing a person could
    /// narrow the value down with.
    #[test]
    fn fingerprint_json_carries_only_the_fingerprint() {
        let mut token = OAuthToken::bearer("abc");
        token.scope = Some("repo".into());
        let json: serde_json::Value =
            serde_json::from_str(&generate_fingerprint_json(&SourceCredential::OAuth(token)))
                .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "fingerprint": "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            })
        );
    }

    #[test]
    fn test_export_var() {
        assert_eq!(export_var("FOO", "bar"), "export FOO='bar'");
    }

    #[test]
    fn test_escape_shell_value() {
        assert_eq!(escape_shell_value("simple"), "simple");
        assert_eq!(escape_shell_value("it's"), "it'\"'\"'s");
    }

    #[test]
    fn test_generate_credential_json_with_session_token() {
        let expiration = DateTime::parse_from_rfc3339("2025-12-31T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc);
        let creds = Credentials::new(
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            Some("token123".to_string()),
            Some(expiration),
        );

        let json_str = generate_credential_json(&creds);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["Version"], 1);
        assert_eq!(parsed["AccessKeyId"], "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(
            parsed["SecretAccessKey"],
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        );
        assert_eq!(parsed["SessionToken"], "token123");
        assert_eq!(parsed["Expiration"], "2025-12-31T23:59:59Z");
    }

    #[test]
    fn test_generate_credential_json_without_optional_fields() {
        let creds = Credentials::new(
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            None,
            None,
        );

        let json_str = generate_credential_json(&creds);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["Version"], 1);
        assert_eq!(parsed["AccessKeyId"], "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(
            parsed["SecretAccessKey"],
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        );
        assert!(parsed.get("SessionToken").is_none());
        assert!(parsed.get("Expiration").is_none());
    }
}
