//! Public configuration API: defaults, the `[core]` / `[aws]` / `[onepassword]`
//! layout and the rejection of unknown keys.

use kurama::adapters::config::Config;

#[test]
fn default_config_caches_sessions_and_uses_onepassword() {
    let config = Config::default();

    assert_eq!(config.core.log_level, None);
    assert!(config.aws.session_cache.enabled);
    assert_eq!(config.aws.session_cache.duration, 43200);
    assert_eq!(
        config.aws.session_name.template,
        "{prefix}-{readonly}-{profile}"
    );
    assert!(config.onepassword.enabled);
    assert_eq!(config.onepassword.cli_path, "op");
}

#[test]
fn default_config_round_trips_through_toml() {
    let config = Config::default();
    let toml_str = toml::to_string(&config).unwrap();
    let parsed = Config::parse(&toml_str).unwrap();

    assert_eq!(
        parsed.aws.session_cache.duration,
        config.aws.session_cache.duration
    );
    assert_eq!(
        parsed.aws.session_name.prefix,
        config.aws.session_name.prefix
    );
    assert_eq!(parsed.onepassword.cli_path, config.onepassword.cli_path);
}

#[test]
fn onepassword_section_with_mappings_parses() {
    let config = Config::parse(
        r#"
        [onepassword]
        enabled = true
        item_name = "aws-credentials"

        [onepassword.mappings]
        default = "personal-aws"
        company = "work-aws"
    "#,
    )
    .unwrap();

    assert!(config.onepassword.enabled);
    assert_eq!(config.onepassword.item_name, "aws-credentials");
    assert_eq!(config.onepassword.mappings.len(), 2);
    assert_eq!(
        config.onepassword.mappings.get("default"),
        Some(&"personal-aws".to_string())
    );
}

#[test]
fn unknown_keys_are_rejected_with_their_line() {
    let error = Config::parse("[core]\nrole_duration = 7200\n").unwrap_err();
    let message = error.to_string();

    assert!(
        message.contains("config.toml line 2: unknown field `role_duration`"),
        "{message}"
    );
}

#[test]
fn config_openapi_revalidation_interval_defaults_and_round_trips_integer_seconds() {
    use kurama::adapters::config::Config;
    for (input, expected) in [
        ("", 3600),
        ("[openapi]\n", 3600),
        ("[openapi]\nrevalidate_after = 0\n", 0),
        ("[openapi]\nrevalidate_after = 60\n", 60),
    ] {
        let config = Config::parse(input).unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert!(
            encoded.contains(&format!("revalidate_after = {expected}")),
            "{encoded}"
        );
        let round_trip = Config::parse(&encoded).unwrap();
        assert_eq!(toml::to_string(&round_trip).unwrap(), encoded);
    }
    for value in ["-1", "1.5", "\"1h\"", "true"] {
        let error = Config::parse(&format!("[openapi]\nrevalidate_after = {value}\n")).unwrap_err();
        assert!(error.to_string().contains("revalidate_after"), "{error}");
    }
}
