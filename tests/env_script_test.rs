//! Output contract of `kurama env <profile>`: the shell export script, the
//! `unset` script and the credential_process JSON are produced by pure
//! domain functions, so this test pins the exact text that reaches the shell.

use chrono::{TimeZone, Utc};
use kurama::domain::functions::export::{
    ACTIVE_AWS_PROFILE_VAR, AWS_ENV_VARS, generate_credential_json, generate_export_script,
    generate_unset_script,
};
use kurama::domain::{Credentials, Profile};
use std::process::Command;

fn credentials() -> Credentials {
    Credentials::new(
        "ASIAEXAMPLE".to_string(),
        "example-secret".to_string(),
        Some("example-token".to_string()),
        Some(Utc.with_ymd_and_hms(2030, 1, 1, 12, 0, 0).unwrap()),
    )
}

fn profile() -> Profile {
    let mut profile = Profile::new("dev");
    profile.set_role_arn("arn:aws:iam::123456789012:role/Dev");
    profile.set_source_profile("default");
    profile.set_region("ap-northeast-1");
    profile
}

#[test]
fn export_script_unsets_every_managed_variable_before_exporting() {
    let script = generate_export_script(&credentials(), &profile(), false);
    let lines: Vec<&str> = script.lines().collect();

    let managed: Vec<&str> = AWS_ENV_VARS
        .iter()
        .copied()
        .chain([ACTIVE_AWS_PROFILE_VAR])
        .collect();
    for (index, var) in managed.iter().enumerate() {
        assert_eq!(lines[index], format!("unset {var}"));
    }
    let exports = &lines[managed.len()..];
    assert_eq!(
        exports,
        [
            "export AWS_ACCESS_KEY_ID='ASIAEXAMPLE'",
            "export AWS_SECRET_ACCESS_KEY='example-secret'",
            "export AWS_SESSION_TOKEN='example-token'",
            "export AWS_SESSION_EXPIRATION='2030-01-01 12:00:00 UTC'",
            "export AWS_REGION='ap-northeast-1'",
            "export AWS_DEFAULT_REGION='ap-northeast-1'",
            "export KURAMA_AWS='dev'",
        ]
    );
}

#[test]
fn export_script_marks_readonly_sessions() {
    let script = generate_export_script(&credentials(), &profile(), true);
    assert!(script.ends_with("export AWS_READONLY_SESSION='true'"));
}

#[test]
fn export_script_clears_legacy_switching_vars_but_preserves_ca_bundle() {
    let script = generate_export_script(&credentials(), &profile(), false);

    assert!(script.contains("unset AWS_DEFAULT_PROFILE"));
    assert!(script.contains("unset AWS_CREDENTIAL_EXPIRATION"));
    assert!(!script.contains("unset AWS_CA_BUNDLE"));
}

#[test]
fn export_script_switches_the_parent_shell_without_stale_values() {
    let first = Credentials::new(
        "ASIADEV".to_string(),
        "dev-secret".to_string(),
        Some("dev-token".to_string()),
        Some(Utc.with_ymd_and_hms(2030, 1, 1, 12, 0, 0).unwrap()),
    );
    let first_profile = Profile::new("dev").with_region_raw("ap-northeast-1");
    let second = Credentials::new(
        "ASIAPROD".to_string(),
        "prod-secret".to_string(),
        None,
        None,
    );
    let second_profile = Profile::new("prod");
    let script = format!(
        "export AWS_DEFAULT_PROFILE='legacy'\nexport AWS_CREDENTIAL_EXPIRATION='legacy-expiration'\nexport AWS_CA_BUNDLE='custom-ca'\n{}\n{}\nprintf '%s|%s|%s|%s|%s|%s|%s|%s|%s\\n' \\
         \"$AWS_ACCESS_KEY_ID\" \"$AWS_SECRET_ACCESS_KEY\" \"${{AWS_SESSION_TOKEN-}}\" \\
         \"${{AWS_SESSION_EXPIRATION-}}\" \"${{AWS_REGION-}}\" \"${{AWS_READONLY_SESSION-}}\" \\
         \"${{AWS_DEFAULT_PROFILE-}}\" \"${{AWS_CREDENTIAL_EXPIRATION-}}\" \"$AWS_CA_BUNDLE\"",
        generate_export_script(&first, &first_profile, true),
        generate_export_script(&second, &second_profile, false),
    );

    let output = Command::new("sh")
        .args(["-c", &script])
        .output()
        .expect("POSIX shell is available");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "ASIAPROD|prod-secret|||||||custom-ca\n"
    );
}

#[test]
fn unset_script_clears_the_same_variables_the_export_sets() {
    let script = generate_unset_script(&[]);
    let expected: Vec<String> = AWS_ENV_VARS
        .iter()
        .chain([&ACTIVE_AWS_PROFILE_VAR])
        .map(|v| format!("unset {v}"))
        .collect();
    assert_eq!(script.lines().collect::<Vec<_>>(), expected);
}

#[test]
fn credential_json_matches_the_credential_process_schema() {
    let json: serde_json::Value =
        serde_json::from_str(&generate_credential_json(&credentials())).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "Version": 1,
            "AccessKeyId": "ASIAEXAMPLE",
            "SecretAccessKey": "example-secret",
            "SessionToken": "example-token",
            "Expiration": "2030-01-01T12:00:00Z",
        })
    );
}
