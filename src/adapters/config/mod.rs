//! `~/.config/kurama/config.toml`.
//!
//! ```toml
//! [core]
//! log_level = "info"
//!
//! [aws.session_name]
//! template = "{prefix}-{readonly}-{profile}"
//!
//! [aws.session_cache]
//! enabled = true
//! duration = 43200
//!
//! [onepassword]
//! item_name = "aws_cm"
//! vault = "Agent"
//!
//! [auth.github]          # an OAuth credential source, see `api`
//! kind = "oauth"
//! grant_type = "authorization_code"
//! auth_url = "https://github.com/login/oauth/authorize"
//! token_url = "https://github.com/login/oauth/access_token"
//! client_id = "Iv1.xxxxxxxx"
//!
//! [api.github]           # an API `kurama api` calls with that source
//! base_url = "https://api.github.com"
//! ```
//!
//! `[core]` holds settings that do not depend on a provider, `[aws]` the AWS
//! settings, `[onepassword]` the 1Password CLI, `[auth.*]` the credential
//! sources that are not AWS profiles and `[api.*]` the APIs. An unknown key
//! is a `CONFIG_INVALID` error that names the key and its line, so a
//! misspelled or outdated key never goes unnoticed. A key this kurama reads
//! in other sections also names those sections, so a key written for a newer
//! kurama reads as an old binary and not only as a typo.

pub mod agent;
pub mod api;
pub mod audit;
pub mod aws;
pub mod check;
pub mod constants;
pub mod data;
pub mod db;
mod diff;
pub mod edit;
pub mod input;
pub mod inventory;
mod known_keys;
mod layout;
pub use db::{
    DbAuth, DbConnection, DbEngine, DbTls, IamSection, InstanceRef, ServerDatabase, SqliteDatabase,
};
pub mod onepassword;
pub mod openapi;
pub mod references;
pub mod saved;
pub mod writer;

pub use api::{ApiDescription, ApiProfile, ApiToml, AuthToml, SpecSource};
pub use aws::AwsConfig;
pub use onepassword::OnePasswordConfig;

use crate::adapters::error::CoreError;
use crate::adapters::utils::path;
use crate::domain::types::AuthSource;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub core: CoreConfig,
    #[serde(default)]
    pub openapi: openapi::OpenApiConfig,
    #[serde(default)]
    pub aws: AwsConfig,
    #[serde(default)]
    pub onepassword: OnePasswordConfig,
    /// Credential sources that are not AWS profiles, by name.
    #[serde(default)]
    pub auth: BTreeMap<String, AuthToml>,
    /// APIs `kurama api` calls, by name.
    #[serde(default)]
    pub api: BTreeMap<String, ApiToml>,
    #[serde(default)]
    pub data: BTreeMap<String, data::DataWorkspace>,
    /// Databases `kurama db` reads, by name.
    #[serde(default)]
    pub db: BTreeMap<String, db::DbSection>,
    #[serde(default)]
    pub s3: BTreeMap<String, data::S3Connection>,
    /// What a run under `KURAMA_AGENT` may do without `--confirm`.
    #[serde(default)]
    pub agent: agent::AgentConfig,
    /// Which calls the audit log records.
    #[serde(default)]
    pub audit: audit::AuditConfig,
}

/// Settings that do not depend on a provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CoreConfig {
    /// Log level for kurama's own events; `RUST_LOG` wins.
    #[serde(default)]
    pub log_level: Option<String>,
}

impl Config {
    pub async fn load() -> Result<Self> {
        let (path, named) = Self::config_source()?;
        Self::load_from(&path, named).await
    }

    /// The configuration at `path`. `named` says the user pointed at it, and
    /// only a named file has to exist.
    async fn load_from(path: &Path, named: bool) -> Result<Self> {
        debug!(path = ?path, "Loading kurama configuration");
        let Some(content) = Self::read_source(path, named).await? else {
            debug!(path = ?path, "Config file not found, using default configuration");
            return Ok(Self::default());
        };
        let config = Self::parse(&content)?;
        debug!(
            session_cache_enabled = config.aws.session_cache.enabled,
            onepassword_enabled = config.onepassword.enabled,
            "Configuration loaded"
        );
        Ok(config)
    }

    /// The text of the file at `path`; `None` when it is the default location
    /// and absent, which is the configuration of someone who has not written
    /// one. A file the user named has to exist.
    pub(crate) async fn read_source(path: &Path, named: bool) -> Result<Option<String>> {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !named => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(CoreError::config(
                format!("configuration file does not exist: {}", path.display()),
            )
            .into()),
            // A directory or a permission error is the same class of mistake,
            // so it must not reach the caller as an untyped I/O error.
            Err(error) => {
                Err(CoreError::config(format!("cannot read {}: {error}", path.display())).into())
            }
        }
    }

    /// Parse and validate the content of a config file.
    ///
    /// Syntax errors and unknown keys become one line naming the line number.
    pub fn parse(content: &str) -> Result<Self> {
        let config: Config =
            toml::from_str(content).map_err(|error| read_error(content, &error))?;
        config.validate()?;
        Ok(config)
    }

    pub fn config_path() -> Result<PathBuf> {
        Self::config_source().map(|(path, _)| path)
    }

    /// The file kurama reads, and whether the user named it. Only a named file
    /// must exist; the default location may be absent. Both answers come from
    /// here so no caller can re-derive the precedence and disagree.
    pub(crate) fn config_source() -> Result<(PathBuf, bool)> {
        // Priority: named path > default XDG config directory.
        // A named file must exist; the default location may be absent.
        match std::env::var("KURAMA_CONFIG_PATH") {
            // An exported but empty variable names nothing.
            Ok(custom) if !custom.is_empty() => Ok((PathBuf::from(custom), true)),
            // Force XDG config directory for all platforms (~/.config/)
            _ => Ok((path::get_kurama_config_dir()?.join("config.toml"), false)),
        }
    }

    /// Validate the configuration values. Failures are `CONFIG_INVALID` (exit 2).
    pub fn validate(&self) -> Result<()> {
        match self.problems().into_iter().next() {
            Some((_, error)) => Err(error.into()),
            None => Ok(()),
        }
    }

    /// Every value the types accepted but kurama cannot use, each with the
    /// section it is in (`api.github`), in the order `validate` reports the
    /// first. A section yields one problem at most: what it checks later may
    /// depend on what it checked first.
    pub fn problems(&self) -> Vec<(String, CoreError)> {
        let mut problems = Vec::new();
        let duration = self.aws.session_cache.duration;
        if !(900..=129600).contains(&duration) {
            problems.push((
                "aws.session_cache".to_owned(),
                CoreError::config(format!(
                    "[aws.session_cache] duration must be between 900 and 129600 seconds, got: {duration}"
                )),
            ));
        }
        if self.onepassword.timeout == 0 {
            problems.push((
                "onepassword".to_owned(),
                CoreError::config("[onepassword] timeout must be at least 1 second"),
            ));
        }
        if let Err(error) = self.agent.validate() {
            problems.push((
                "agent".to_owned(),
                CoreError::config(format!("[agent] {error}")),
            ));
        }
        if let Err(error) = self.audit.validate() {
            problems.push((
                "audit".to_owned(),
                CoreError::config(format!("[audit] {error}")),
            ));
        }
        let mut check = |kind: &str, name: &str, result: std::result::Result<(), String>| {
            if let Err(error) = result {
                problems.push((format!("{kind}.{name}"), in_section(kind, name)(error)));
            }
        };
        for (name, auth) in &self.auth {
            check("auth", name, auth.typed(name).map(drop));
        }
        for (name, api) in &self.api {
            check("api", name, api.typed(name, &self.auth).map(drop));
        }
        for (name, workspace) in &self.data {
            let result = workspace
                .validate()
                .map_err(|error| error.to_string())
                .and_then(|()| match &workspace.s3_source {
                    Some(source) if !self.s3.contains_key(source) => {
                        Err("unknown s3_source".to_owned())
                    }
                    _ => Ok(()),
                });
            check("data", name, result);
        }
        for (name, connection) in &self.s3 {
            check(
                "s3",
                name,
                connection.validate().map_err(|error| error.to_string()),
            );
        }
        for (name, section) in &self.db {
            check(
                "db",
                name,
                section
                    .connection()
                    .map(drop)
                    .map_err(|error| error.to_string()),
            );
        }
        problems
    }

    /// The `[auth.<name>]` source, validated; `None` when there is none.
    pub fn auth_source(&self, name: &str) -> Result<Option<AuthSource>> {
        self.auth
            .get(name)
            .map(|auth| self.typed_auth(name, auth))
            .transpose()
    }

    /// Every `[auth.*]` source, validated, sorted by name.
    pub fn auth_sources(&self) -> Result<Vec<AuthSource>> {
        self.auth
            .iter()
            .map(|(name, auth)| self.typed_auth(name, auth))
            .collect()
    }

    /// The `[api.<name>]` profile, validated; `None` when there is none.
    pub fn api_profile(&self, name: &str) -> Result<Option<ApiProfile>> {
        self.api
            .get(name)
            .map(|api| self.typed_api(name, api))
            .transpose()
    }

    /// Every `[api.*]` profile, validated, sorted by name.
    pub fn api_profiles(&self) -> Result<Vec<ApiProfile>> {
        self.api
            .iter()
            .map(|(name, api)| self.typed_api(name, api))
            .collect()
    }

    fn typed_auth(&self, name: &str, auth: &AuthToml) -> Result<AuthSource> {
        Ok(auth.typed(name).map_err(in_section("auth", name))?)
    }

    fn typed_api(&self, name: &str, api: &ApiToml) -> Result<ApiProfile> {
        Ok(api
            .typed(name, &self.auth)
            .map_err(in_section("api", name))?)
    }
}

/// A validation failure inside `[<kind>.<name>]`, prefixed with the section.
fn in_section<E: std::fmt::Display>(kind: &str, name: &str) -> impl FnOnce(E) -> CoreError {
    let section = format!("[{kind}.{name}]");
    move |error| CoreError::config(format!("{section} {error}"))
}

/// A document serde could not read, as one line naming the line number; an
/// unknown key that another section reads says which sections do.
pub(crate) fn read_error(content: &str, error: &toml::de::Error) -> CoreError {
    let message = match line_of(content, error.span()) {
        Some(line) => format!("config.toml line {line}: {}", error.message()),
        None => format!("config.toml: {}", error.message()),
    };
    let message = message.replace('\n', " ");
    let Some(key) = unknown_key(error.message()) else {
        return CoreError::config(message);
    };
    match known_keys::sections_reading(key) {
        sections if sections.is_empty() => CoreError::config(message),
        sections => CoreError::KeyOfAnotherSection {
            message,
            key: key.to_owned(),
            sections,
        },
    }
}

/// The 1-based line a byte span of `content` starts on.
pub(crate) fn line_of(content: &str, span: Option<std::ops::Range<usize>>) -> Option<usize> {
    span.map(|span| content[..span.start].matches('\n').count() + 1)
}

/// The key serde refused as unknown (`unknown field `x`, expected ...`).
fn unknown_key(message: &str) -> Option<&str> {
    message
        .strip_prefix("unknown field `")?
        .split_once('`')
        .map(|(key, _)| key)
}

pub(super) const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;

    fn configuration_error(result: Result<Config>) -> String {
        let error = result.expect_err("configuration accepted");
        assert!(
            matches!(
                error.downcast_ref::<CoreError>(),
                Some(CoreError::Configuration(_))
            ),
            "not a configuration error: {error:#}"
        );
        error.to_string()
    }

    mod unknown_keys {
        use super::*;

        #[rstest::rstest]
        #[case(
            "[core]\nlog_level = \"info\"\n\n[mfa.onepassword]\nvault = \"Agent\"",
            "config.toml line 4: unknown field `mfa`"
        )]
        #[case(
            "[core]\nrole_duration = 3600",
            "config.toml line 2: unknown field `role_duration`"
        )]
        #[case(
            "[aws.session_cache]\nenable = false",
            "config.toml line 2: unknown field `enable`"
        )]
        #[case(
            "[onepassword]\nvalut = \"Agent\"",
            "config.toml line 2: unknown field `valut`"
        )]
        #[case("[ui]\ntheme = \"dark\"", "config.toml line 1: unknown field `ui`")]
        fn unknown_key_is_a_one_line_configuration_error(
            #[case] content: &str,
            #[case] expected: &str,
        ) {
            let message = configuration_error(Config::parse(content));
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains('\n'), "{message}");
        }

        /// `format` is a key of `[auth.*]`, so under `[api.*]` it is misplaced
        /// or written for a newer kurama; the error keeps the key and where
        /// this kurama reads it, and never the value.
        #[test]
        fn a_key_of_another_section_names_the_sections_that_read_it() {
            let error = Config::parse(
                "[api.example]\nbase_url = \"https://api.example.com\"\nformat = \"Bearer {token}\"\n",
            )
            .expect_err("configuration accepted");
            match error.downcast_ref::<CoreError>() {
                Some(CoreError::KeyOfAnotherSection {
                    message,
                    key,
                    sections,
                }) => {
                    assert!(
                        message.contains("config.toml line 3: unknown field `format`"),
                        "{message}"
                    );
                    assert_eq!(key, "format");
                    assert!(sections.iter().any(|section| section == "[auth.*]"));
                    assert!(!format!("{error:#}").contains("Bearer"), "{error:#}");
                }
                _ => panic!("not a key of another section: {error:#}"),
            }
        }

        #[test]
        fn current_layout_parses() {
            let config = Config::parse(
                "[core]\nlog_level = \"debug\"\n\n\
                 [aws.session_name]\nprefix = \"team\"\n\n\
                 [aws.session_cache]\nenabled = false\n\n\
                 [onepassword]\nvault = \"Agent\"\n\n\
                 [onepassword.mappings]\ndefault = \"personal\"\n",
            )
            .unwrap();
            assert_eq!(config.core.log_level.as_deref(), Some("debug"));
            assert_eq!(config.aws.session_name.prefix, "team");
            assert!(!config.aws.session_cache.enabled);
            assert_eq!(config.onepassword.vault.as_deref(), Some("Agent"));
            assert_eq!(config.onepassword.mappings.len(), 1);
        }
    }

    mod session_name {
        use super::*;

        #[test]
        fn parses_full_section() {
            let config = Config::parse(
                "[aws.session_name]\ntemplate = \"claude-{profile}\"\nprefix = \"myteam\"\nreadonly_indicator = \"readonly\"\n",
            )
            .unwrap();
            assert_eq!(config.aws.session_name.template, "claude-{profile}");
            assert_eq!(config.aws.session_name.prefix, "myteam");
            assert_eq!(config.aws.session_name.readonly_indicator, "readonly");
        }

        #[test]
        fn defaults_apply_when_section_is_absent() {
            let config = Config::parse("").unwrap();
            assert_eq!(
                config.aws.session_name.template,
                "{prefix}-{readonly}-{profile}"
            );
            assert_eq!(config.aws.session_name.prefix, "kurama");
            assert_eq!(config.aws.session_name.readonly_indicator, "ro");
        }

        #[test]
        fn partial_section_fills_missing_keys_with_defaults() {
            let config = Config::parse("[aws.session_name]\ntemplate = \"fixed-name\"\n").unwrap();
            assert_eq!(config.aws.session_name.template, "fixed-name");
            assert_eq!(config.aws.session_name.prefix, "kurama");
        }

        /// Each key a section leaves out gets its own default, not the
        /// default of the whole section.
        #[test]
        fn a_section_naming_only_the_prefix_keeps_the_other_defaults() {
            let config = Config::parse("[aws.session_name]\nprefix = \"team\"\n").unwrap();
            assert_eq!(
                config.aws.session_name.template,
                "{prefix}-{readonly}-{profile}"
            );
            assert_eq!(config.aws.session_name.prefix, "team");
            assert_eq!(config.aws.session_name.readonly_indicator, "ro");
        }

        #[test]
        fn session_name_is_typed_when_read() {
            let config =
                Config::parse("[aws.session_name]\ntemplate = \"claude-{profile}\"\n").unwrap();
            let domain = &config.aws.session_name;
            assert_eq!(domain.template, "claude-{profile}");
            assert_eq!(domain.prefix, "kurama");
            assert_eq!(domain.max_length, 64);
        }

        #[test]
        fn unknown_placeholder_is_a_configuration_error() {
            let message = configuration_error(Config::parse(
                "[aws.session_name]\ntemplate = \"{prefix}-{user}\"\n",
            ));
            assert!(message.contains("{user}"), "{message}");
        }
    }

    mod session_cache {
        use super::*;

        #[rstest::rstest]
        #[case("")]
        #[case("[aws.session_cache]")]
        fn defaults(#[case] content: &str) {
            let config = Config::parse(content).unwrap();
            assert!(config.aws.session_cache.enabled);
            assert_eq!(config.aws.session_cache.duration, 43200);
        }

        #[rstest::rstest]
        #[case(899)]
        #[case(129601)]
        fn rejects_invalid_duration(#[case] duration: u64) {
            let message = configuration_error(Config::parse(&format!(
                "[aws.session_cache]\nduration = {duration}"
            )));
            assert!(message.contains("[aws.session_cache]"), "{message}");
        }

        #[rstest::rstest]
        #[case(900)]
        #[case(129600)]
        fn accepts_limits_and_disable(#[case] duration: u64) {
            let config = Config::parse(&format!(
                "[aws.session_cache]\nenabled = false\nduration = {duration}"
            ))
            .unwrap();
            assert!(!config.aws.session_cache.enabled);
            assert_eq!(config.aws.session_cache.duration, duration);
        }
    }

    mod onepassword {
        use super::*;

        #[test]
        fn vault_parses() {
            let config =
                Config::parse("[onepassword]\nitem_name = \"aws_cm\"\nvault = \"Agent\"\n")
                    .unwrap();
            assert_eq!(config.onepassword.vault, Some("Agent".to_string()));
        }

        #[test]
        fn vault_defaults_to_none() {
            let config = Config::parse("[onepassword]\nitem_name = \"aws_cm\"\n").unwrap();
            assert_eq!(config.onepassword.vault, None);
        }

        /// A section that names only the item still gets the CLI, the
        /// deadline and the field labels kurama reads by default.
        #[test]
        fn a_section_naming_only_the_item_keeps_every_other_default() {
            let config = Config::parse(
                "[onepassword]\nitem_name = \"aws_cm\"\n\n[onepassword.field_names]\naccess_key_id = \"key\"\n",
            )
            .unwrap();
            let onepassword = &config.onepassword;
            assert_eq!(onepassword.cli_path, "op");
            assert_eq!(onepassword.timeout, 30);
            assert_eq!(onepassword.field_names.access_key_id, "key");
            assert_eq!(
                onepassword.field_names.secret_access_key,
                "secret_access_key"
            );

            let bare = Config::parse("[onepassword]\nitem_name = \"aws_cm\"\n").unwrap();
            assert_eq!(bare.onepassword.field_names.access_key_id, "access_key_id");
        }

        /// The item for a profile or an MFA serial: a mapping of the key
        /// itself, else of the user the serial names, else the default item.
        #[test]
        fn the_item_for_a_key_follows_the_mappings_then_the_default() {
            let config = Config::parse(
                "[onepassword]\nitem_name = \"shared\"\n\n[onepassword.mappings]\n\
                 dev = \"dev-item\"\nalice = \"alice-item\"\n",
            )
            .unwrap();
            let onepassword = &config.onepassword;

            assert_eq!(
                onepassword.get_item_for_key("dev").as_deref(),
                Some("dev-item")
            );
            assert_eq!(
                onepassword
                    .get_item_for_key("arn:aws:iam::123456789012:mfa/alice")
                    .as_deref(),
                Some("alice-item")
            );
            assert_eq!(
                onepassword.get_item_for_key("prod").as_deref(),
                Some("shared")
            );

            let unnamed = Config::parse("[onepassword]\nitem_name = \"\"\n").unwrap();
            assert_eq!(unnamed.onepassword.get_item_for_key("prod"), None);
        }
    }

    mod auth_and_api {
        use super::*;

        const SECTIONS: &str = "[auth.github]\nkind = \"oauth\"\ngrant_type = \"authorization_code\"\n\
             auth_url = \"https://github.com/login/oauth/authorize\"\n\
             token_url = \"https://github.com/login/oauth/access_token\"\nclient_id = \"Iv1.abc\"\n\n\
             [api.github]\nbase_url = \"https://api.github.com\"\n";

        #[test]
        fn sections_parse_and_resolve_each_other() {
            let config = Config::parse(SECTIONS).unwrap();
            let AuthSource::OAuth(client) = config.auth_source("github").unwrap().unwrap() else {
                panic!("expected an oauth source");
            };
            assert_eq!(client.client_id, "Iv1.abc");
            let api = config.api_profile("github").unwrap().unwrap();
            assert_eq!(api.auth.as_deref(), Some("github"));
            assert!(config.auth_source("missing").unwrap().is_none());
            assert!(config.api_profile("missing").unwrap().is_none());
            assert_eq!(config.auth_sources().unwrap().len(), 1);
            assert_eq!(config.api_profiles().unwrap().len(), 1);
        }

        #[test]
        fn invalid_sections_are_configuration_errors_naming_the_section() {
            let message = configuration_error(Config::parse(
                "[auth.svc]\nkind = \"oauth\"\ngrant_type = \"client_credentials\"\nclient_id = \"id\"\n",
            ));
            assert!(
                message.contains("[auth.svc] issuer or token_url is required"),
                "{message}"
            );
            let message = configuration_error(Config::parse(
                "[api.svc]\nbase_url = \"https://x\"\nauth = \"nope\"\n",
            ));
            assert!(message.contains("[api.svc] auth = \"nope\""), "{message}");
            let message = configuration_error(Config::parse(
                "[auth.svc]\nkind = \"oauth\"\nclient_id = \"id\"\n",
            ));
            assert!(
                message.contains("[auth.svc] grant_type is required"),
                "{message}"
            );
        }
    }

    /// Sets `KURAMA_CONFIG_PATH` and puts it back when it drops, so an async
    /// test can await in between.
    struct PathEnv {
        previous: Option<std::ffi::OsString>,
    }

    impl PathEnv {
        fn set(custom: Option<&str>) -> Self {
            let previous = std::env::var_os("KURAMA_CONFIG_PATH");
            test_env::set_or_remove("KURAMA_CONFIG_PATH", custom);
            Self { previous }
        }
    }

    impl Drop for PathEnv {
        fn drop(&mut self) {
            test_env::set_or_remove("KURAMA_CONFIG_PATH", self.previous.as_ref());
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_named_file_that_does_not_exist_is_a_configuration_error() {
        let _env = PathEnv::set(Some("/nonexistent/kurama/config.toml"));
        let message = configuration_error(Config::load().await);
        assert!(
            message.contains("does not exist: /nonexistent/kurama/config.toml"),
            "{message}"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_path_that_names_a_directory_is_a_configuration_error() {
        let dir = tempfile::tempdir().unwrap();
        let _env = PathEnv::set(Some(dir.path().to_str().unwrap()));
        let message = configuration_error(Config::load().await);
        assert!(message.contains("cannot read "), "{message}");
    }

    #[test]
    #[serial_test::serial]
    fn a_custom_path_names_the_file() {
        let _env = PathEnv::set(Some("/somewhere/else.toml"));
        let (path, named) = Config::config_source().unwrap();
        assert_eq!(path, PathBuf::from("/somewhere/else.toml"));
        assert!(named, "KURAMA_CONFIG_PATH names the file it points at");
    }

    #[test]
    #[serial_test::serial]
    fn an_exported_but_empty_path_names_nothing() {
        let _env = PathEnv::set(Some(""));
        let (path, named) = Config::config_source().unwrap();
        assert!(path.ends_with("config.toml"), "{path:?}");
        assert!(!named, "the default location may be absent");
    }

    /// The default location may be absent: that is the configuration of
    /// someone who has not written one, not an error. A file the user named
    /// has to exist.
    #[tokio::test]
    async fn only_a_named_file_has_to_exist() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("config.toml");

        let config = Config::load_from(&absent, false)
            .await
            .expect("an absent default file is the default configuration");
        assert!(config.aws.session_cache.enabled);
        assert!(config.data.is_empty());

        let message = configuration_error(Config::load_from(&absent, true).await);
        assert!(message.contains("does not exist"), "{message}");
    }

    /// A workspace reads S3 through an `[s3.*]` section that has to exist.
    #[test]
    fn a_workspace_names_an_s3_source_that_exists() {
        let workspace = "[data.lake]\nroot_dir = \"/data\"\ns3_source = \"assets\"\n\n\
                         [[data.lake.sources]]\nname = \"orders\"\npath = \"s3://bucket/o.parquet\"\n";
        let assets = "[s3.assets]\naws_profile = \"dev\"\nregion = \"ap-northeast-1\"\n\n";

        assert!(Config::parse(&format!("{assets}{workspace}")).is_ok());
        let message = configuration_error(Config::parse(workspace));
        assert!(
            message.contains("[data.lake] unknown s3_source"),
            "{message}"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn load_rejects_an_invalid_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "[aws.session_cache]\nduration = 899").unwrap();
        let _env = PathEnv::set(Some(file.path().to_str().unwrap()));
        let message = configuration_error(Config::load().await);
        assert!(message.contains("[aws.session_cache]"), "{message}");
    }
}
