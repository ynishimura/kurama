//! `SecretResolver` over every reference kurama reads: `op://` through the
//! 1Password CLI, `aws-secrets://` and `aws-ssm://` through AWS with the role
//! of an AWS profile, and a literal as itself.
//!
//! One port, not three: a caller asks for the value behind a `SecretRef` and
//! the reference itself says where it comes from. Each secret is read once per
//! process, whichever backend holds it: an RDS managed secret is one
//! `GetSecretValue` that two references (`#username`, `#password`) take their
//! fields from, and one 1Password item is one `op item get` -- one biometric
//! prompt -- however many fields of it the configuration names. With
//! `reporting_reads` each read that goes to a store says so on stderr, one
//! line per read, naming the secret and never its value.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::adapters::auth::OnePasswordSecrets;
use crate::adapters::auth::secret::{ItemField, item_field_value};
use crate::adapters::aws::secret_store;
use crate::adapters::config::OnePasswordConfig;
use crate::console::progress;
use crate::domain::types::SecretFailure;
use crate::domain::types::{AwsSecretRef, AwsSecretStore, Secret, SecretRef};
use crate::ports::{AwsProfileCredentials, SecretError, SecretResolver};

/// One secret in one place, as the cache keys it. Two AWS profiles can name
/// the same id and mean two different accounts, so the profile is part of it.
/// An `op://<vault>/<item>/<field>` reference is keyed by its item, whose
/// JSON answers every field of it: two fields of one 1Password item would be
/// two child processes and two biometric prompts otherwise, which is the same
/// thing the AWS side is keyed to avoid. Any other `op://` reference (a
/// section, an attribute) is `op read`'s and its own key.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ReadSecret {
    OnePassword(String),
    OnePasswordItem {
        vault: String,
        item: String,
    },
    Aws {
        store: AwsSecretStore,
        aws_profile: String,
        region: String,
        id: String,
    },
}

impl ReadSecret {
    /// Where this secret is read from: the store call, the id and the region,
    /// or the `op://` reference. The value is not part of it.
    fn describe(&self) -> String {
        match self {
            Self::OnePassword(reference) => format!("op read {reference}"),
            Self::OnePasswordItem { vault, item } => format!("op item get {item} --vault {vault}"),
            Self::Aws {
                store, region, id, ..
            } => format!("{} {id} ({region})", store.read_action()),
        }
    }
}

pub struct ConfiguredSecrets {
    onepassword: OnePasswordSecrets,
    /// The same AssumeRole path as `kurama env`; shared with the tunnel and
    /// the IAM token of a `kurama db` call, so one profile is assumed once.
    aws: Arc<dyn AwsProfileCredentials>,
    /// What AWS already answered in this process. Values never leave it.
    read: Mutex<HashMap<ReadSecret, Secret>>,
    /// Say each read that goes to a store on stderr (`kurama api -v`).
    report_reads: bool,
}

impl ConfiguredSecrets {
    pub fn new(onepassword: &OnePasswordConfig, aws: Arc<dyn AwsProfileCredentials>) -> Self {
        Self {
            onepassword: OnePasswordSecrets::new(onepassword),
            aws,
            read: Mutex::new(HashMap::new()),
            report_reads: false,
        }
    }

    /// Say on stderr each time a secret is read from its store; a secret the
    /// process already holds is not a read and says nothing.
    pub fn reporting_reads(self, report_reads: bool) -> Self {
        Self {
            report_reads,
            ..self
        }
    }

    fn report_read(&self, secret: &ReadSecret) {
        if self.report_reads {
            progress!("< secret read: {}", secret.describe());
        }
    }

    async fn resolve_onepassword(&self, reference: &str) -> Result<Secret, SecretError> {
        let Some(field) = ItemField::parse(reference) else {
            return self
                .read_once(ReadSecret::OnePassword(reference.into()), || {
                    self.onepassword.read(reference)
                })
                .await;
        };
        let key = ReadSecret::OnePasswordItem {
            vault: field.vault.into(),
            item: field.item.into(),
        };
        let item = self
            .read_once(key, || self.onepassword.read_item(field.vault, field.item))
            .await?;
        item_field_value(&field, item.expose())
    }

    /// What `read` answers, asked of 1Password once per process.
    async fn read_once<F>(
        &self,
        key: ReadSecret,
        read: impl FnOnce() -> F,
    ) -> Result<Secret, SecretError>
    where
        F: std::future::Future<Output = Result<Secret, SecretError>>,
    {
        if let Some(value) = self.read.lock().await.get(&key) {
            return Ok(value.clone());
        }
        self.report_read(&key);
        let value = read().await?;
        self.read.lock().await.insert(key, value.clone());
        Ok(value)
    }

    async fn resolve_aws(&self, reference: &AwsSecretRef) -> Result<Secret, SecretError> {
        let value = self.read_aws(reference).await?;
        match &reference.json_key {
            Some(key) => json_field(reference, value.expose(), key),
            None => Ok(value),
        }
    }

    /// The whole value AWS holds, read once per process.
    async fn read_aws(&self, reference: &AwsSecretRef) -> Result<Secret, SecretError> {
        let profile = self
            .aws
            .load_profile(&reference.aws_profile)
            .await
            .map_err(|error| {
                SecretError::unclassified(
                    // An AWS profile the file does not have is the
                    // configuration, which is what is left when the chain
                    // says nothing.
                    SecretFailure::Invalid,
                    format_args!("{reference}"),
                    error,
                )
            })?;
        let region = reference
            .region_in(profile.region_raw())
            .ok_or_else(|| SecretError::invalid(format!(
                "{reference}: the region cannot be told from the reference or from the AWS profile; \
                 add ?region=<region> or a region to the profile"
            )))?;
        let key = ReadSecret::Aws {
            store: reference.store,
            aws_profile: reference.aws_profile.clone(),
            region: region.clone(),
            id: reference.id.clone(),
        };
        if let Some(value) = self.read.lock().await.get(&key) {
            return Ok(value.clone());
        }
        let credentials = self.aws.assume_role(&profile).await.map_err(|error| {
            SecretError::unclassified(
                // Nothing was read, so a chain that says nothing is a retry.
                SecretFailure::Unreachable,
                format_args!(
                    "{reference}: the role of the AWS profile {} could not be assumed",
                    reference.aws_profile
                ),
                error,
            )
        })?;
        self.report_read(&key);
        let value = secret_store::read(reference.store, &credentials, &region, &reference.id)
            .await
            .map_err(|error| SecretError {
                message: format!("{reference}: {}", error.message),
                ..error
            })?;
        self.read.lock().await.insert(key, value.clone());
        Ok(value)
    }
}

/// One field of a JSON `SecretString`, which is the shape an RDS managed
/// secret has.
fn json_field(reference: &AwsSecretRef, value: &str, key: &str) -> Result<Secret, SecretError> {
    let document: serde_json::Value = serde_json::from_str(value).map_err(|_| {
        SecretError::invalid(format!(
            "{reference}: #{key} needs a JSON secret and this one is not JSON"
        ))
    })?;
    match document.get(key) {
        Some(serde_json::Value::String(field)) => Ok(Secret::new(field.as_str())),
        Some(other) => Err(SecretError::invalid(format!(
            "{reference}: the key {key} holds {}, not a string",
            kind_of(other)
        ))),
        None => Err(SecretError::invalid(format!(
            "{reference}: the secret has no key {key}"
        ))),
    }
}

fn kind_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

#[async_trait]
impl SecretResolver for ConfiguredSecrets {
    async fn resolve(&self, secret: &SecretRef) -> Result<Secret, SecretError> {
        match secret {
            SecretRef::Literal(value) => Ok(Secret::new(value.as_str())),
            SecretRef::OnePassword(reference) => self.resolve_onepassword(reference).await,
            SecretRef::Aws(reference) => self.resolve_aws(reference).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{Profile, SecretFailure};
    use crate::ports::aws_credentials::MockAwsProfileCredentials;

    fn reference(value: &str) -> Box<AwsSecretRef> {
        match SecretRef::parse(value).expect(value) {
            SecretRef::Aws(reference) => reference,
            other => panic!("{value} parsed as {other:?}"),
        }
    }

    fn secrets(aws: MockAwsProfileCredentials) -> ConfiguredSecrets {
        ConfiguredSecrets::new(&OnePasswordConfig::default(), Arc::new(aws))
    }

    fn profile(region: Option<&str>) -> Profile {
        let profile = Profile::new("dev");
        match region {
            Some(region) => profile.with_region_raw(region),
            None => profile,
        }
    }

    /// A role that could not be assumed is not a reference that is wrong.
    /// The chain reaches `ErrorCode::classify` intact, so a wrong TOTP stays
    /// exit 3 and a refused role stays exit 4 instead of becoming a usage
    /// error about the grammar of the reference.
    #[tokio::test]
    async fn a_role_that_cannot_be_assumed_keeps_its_own_error() {
        let mut aws = MockAwsProfileCredentials::new();
        aws.expect_load_profile()
            .returning(|_| Ok(profile(Some("ap-northeast-1"))));
        aws.expect_assume_role().returning(|_| {
            Err(anyhow::Error::from(
                crate::shell::executor::ExecutorError::StsFailed {
                    kind: crate::domain::functions::error_mapping::StsErrorKind::AccessDenied,
                    message: "not authorized to perform sts:AssumeRole".into(),
                },
            ))
        });

        let error = secrets(aws)
            .resolve(&SecretRef::Aws(reference("aws-ssm://dev/app/db")))
            .await
            .expect_err("the role was refused");

        let cause = error.cause.as_ref().expect("the AssumeRole chain is kept");
        assert!(
            cause.chain().any(|cause| cause
                .downcast_ref::<crate::shell::executor::ExecutorError>()
                .is_some()),
            "{cause:#}"
        );
        assert!(
            error.to_string().contains("could not be assumed"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_literal_reaches_no_backend() {
        let value = secrets(MockAwsProfileCredentials::new())
            .resolve(&SecretRef::Literal("plain".into()))
            .await
            .unwrap();
        assert_eq!(value.expose(), "plain");
    }

    /// The region the reference does not name comes from the AWS profile, and
    /// a reference nobody can place is refused before a role is assumed.
    #[tokio::test]
    async fn a_reference_without_a_region_anywhere_assumes_no_role() {
        let mut aws = MockAwsProfileCredentials::new();
        aws.expect_load_profile()
            .returning(|_| Ok(profile(None)))
            .times(1);
        aws.expect_assume_role().never();
        let error = secrets(aws)
            .resolve(&SecretRef::Aws(reference("aws-ssm://dev/app/db")))
            .await
            .expect_err("no region");
        assert_eq!(error.failure, SecretFailure::Invalid);
        assert!(
            error.to_string().contains("aws-ssm://dev/app/db"),
            "{error}"
        );
        assert!(error.to_string().contains("?region="), "{error}");
    }

    #[tokio::test]
    async fn an_aws_profile_that_does_not_exist_is_the_configuration_and_assumes_no_role() {
        let mut aws = MockAwsProfileCredentials::new();
        aws.expect_load_profile()
            .returning(|name| Err(anyhow::anyhow!("profile not found: {name}")));
        aws.expect_assume_role().never();
        let error = secrets(aws)
            .resolve(&SecretRef::Aws(reference("aws-ssm://missing/app/db")))
            .await
            .expect_err("no profile");
        assert_eq!(error.failure, SecretFailure::Invalid);
        assert!(error.to_string().contains("profile not found"), "{error}");
    }

    /// A managed RDS secret is one JSON document that two references read two
    /// fields of. A secret this process already read is not read again, and
    /// nothing on the way to reading it happens either: no role is assumed for
    /// a call nobody makes. The scenario
    /// `secrets_one_managed_secret_answers_username_and_password_in_one_call`
    /// is what pins the first read to one `GetSecretValue`.
    #[tokio::test]
    async fn a_secret_already_read_is_taken_from_the_process_and_assumes_no_role() {
        let mut aws = MockAwsProfileCredentials::new();
        aws.expect_load_profile()
            .returning(|_| Ok(profile(Some("ap-northeast-1"))));
        aws.expect_assume_role().never();
        let secrets = secrets(aws);
        let stored = ReadSecret::Aws {
            store: AwsSecretStore::SecretsManager,
            aws_profile: "dev".into(),
            region: "ap-northeast-1".into(),
            id: "app/db".into(),
        };
        // Stand in for the answer AWS would give, so the test says what the
        // cache does and not what the SDK does.
        secrets.read.lock().await.insert(
            stored,
            r#"{"username":"reader","password":"s3cret"}"#.into(),
        );
        for (written, expected) in [
            ("aws-secrets://dev/app/db#username", "reader"),
            ("aws-secrets://dev/app/db#password", "s3cret"),
        ] {
            let value = secrets
                .resolve(&SecretRef::Aws(reference(written)))
                .await
                .expect(written);
            assert_eq!(value.expose(), expected);
        }
    }

    /// Two AWS profiles naming one id in one region may be two accounts: the
    /// value one profile read answers that profile only, and the other goes
    /// to its own role. The role here is refused, so reaching it is an error
    /// and a value taken from the first profile would be a success. The
    /// scenario `secrets_cache_isolated_by_aws_profile` pins one read per
    /// profile against the fake store.
    #[tokio::test]
    async fn a_secret_one_aws_profile_read_is_not_the_answer_for_another() {
        let mut aws = MockAwsProfileCredentials::new();
        aws.expect_load_profile()
            .returning(|_| Ok(profile(Some("ap-northeast-1"))));
        aws.expect_assume_role()
            .times(1)
            .returning(|_| Err(anyhow::anyhow!("the role of other is refused")));
        let secrets = secrets(aws);
        secrets.read.lock().await.insert(
            ReadSecret::Aws {
                store: AwsSecretStore::ParameterStore,
                aws_profile: "dev".into(),
                region: "ap-northeast-1".into(),
                id: "/app/db".into(),
            },
            "value-of-dev".into(),
        );

        let dev = secrets
            .resolve(&SecretRef::Aws(reference("aws-ssm://dev/app/db")))
            .await
            .expect("dev already read it");
        assert_eq!(dev.expose(), "value-of-dev");
        let other = secrets
            .resolve(&SecretRef::Aws(reference("aws-ssm://other/app/db")))
            .await
            .expect_err("other has to assume its own role");
        assert!(
            other.to_string().contains("could not be assumed"),
            "{other}"
        );
    }

    /// One 1Password item is one `op item get`, the same way one managed
    /// secret is one `GetSecretValue`: two fields of the item the process
    /// already read come back without starting the CLI. The references name
    /// nothing that exists, so a cache miss reaches `op` and fails. The
    /// scenario `secrets_two_fields_of_one_1password_item_use_one_read` is
    /// what pins the first read to one `op item get`.
    #[tokio::test]
    async fn two_fields_of_a_1password_item_already_read_are_taken_from_the_process() {
        let secrets = secrets(MockAwsProfileCredentials::new());
        secrets.read.lock().await.insert(
            ReadSecret::OnePasswordItem {
                vault: "kurama-no-such-vault".into(),
                item: "kurama-no-such-item".into(),
            },
            r#"{"fields":[{"id":"username","value":"reader"},{"id":"password","value":"s3cret"}]}"#
                .into(),
        );
        for (field, expected) in [("username", "reader"), ("password", "s3cret")] {
            let reference = format!("op://kurama-no-such-vault/kurama-no-such-item/{field}");
            let value = secrets
                .resolve(&SecretRef::OnePassword(reference))
                .await
                .expect("this process already read the item");
            assert_eq!(value.expose(), expected);
        }
    }

    /// A reference with a section is not a field `op item get` answers by
    /// name, so it is `op read`'s, and its own key.
    #[tokio::test]
    async fn a_1password_reference_with_a_section_already_read_is_taken_from_the_process() {
        let reference = "op://kurama-no-such-vault/kurama-no-such-item/section/password";
        let secrets = secrets(MockAwsProfileCredentials::new());
        secrets
            .read
            .lock()
            .await
            .insert(ReadSecret::OnePassword(reference.into()), "s3cret".into());

        let value = secrets
            .resolve(&SecretRef::OnePassword(reference.into()))
            .await
            .expect("this process already read it");
        assert_eq!(value.expose(), "s3cret");
    }

    /// The `-v` line names the store call, the id and the region, or the
    /// `op://` reference, and not the AWS profile: the profile is how the
    /// role was chosen, not where the secret lives.
    #[test]
    fn a_read_is_described_by_its_store_id_and_region() {
        for (secret, expected) in [
            (
                ReadSecret::Aws {
                    store: AwsSecretStore::ParameterStore,
                    aws_profile: "dev".into(),
                    region: "ap-northeast-1".into(),
                    id: "/kurama/api-key".into(),
                },
                "ssm:GetParameter /kurama/api-key (ap-northeast-1)",
            ),
            (
                ReadSecret::Aws {
                    store: AwsSecretStore::SecretsManager,
                    aws_profile: "dev".into(),
                    region: "us-east-1".into(),
                    id: "app/db".into(),
                },
                "secretsmanager:GetSecretValue app/db (us-east-1)",
            ),
            (
                ReadSecret::OnePassword("op://Agent/Example/section/credential".into()),
                "op read op://Agent/Example/section/credential",
            ),
            (
                ReadSecret::OnePasswordItem {
                    vault: "Agent".into(),
                    item: "Example".into(),
                },
                "op item get Example --vault Agent",
            ),
        ] {
            assert_eq!(secret.describe(), expected);
        }
    }

    #[test]
    fn a_json_key_that_is_not_a_string_key_of_a_json_secret_is_refused() {
        let parsed = reference("aws-secrets://dev/app/db#password");
        for (value, says) in [
            ("not json at all", "not JSON"),
            (r#"{"username":"reader"}"#, "no key password"),
            (r#"{"password":5432}"#, "not a string"),
        ] {
            let error = json_field(&parsed, value, "password").expect_err(value);
            assert_eq!(error.failure, SecretFailure::Invalid);
            assert!(error.to_string().contains(says), "{value}: {error}");
            // The reference is named, and the secret itself never is.
            assert!(error.to_string().contains("aws-secrets://dev/app/db"));
            assert!(!error.to_string().contains("reader"), "{error}");
        }
        assert_eq!(
            json_field(&parsed, r#"{"password":"s3cret"}"#, "password")
                .unwrap()
                .expose(),
            "s3cret"
        );
    }

    /// The message names what the key holds, because "not a string" alone
    /// leaves the reader guessing at a document they cannot print.
    #[test]
    fn a_json_value_that_is_not_a_string_is_named_by_what_it_is() {
        use serde_json::json;
        for (value, name) in [
            (json!(null), "null"),
            (json!(true), "a boolean"),
            (json!(5432), "a number"),
            (json!("s3cret"), "a string"),
            (json!([1]), "an array"),
            (json!({"a": 1}), "an object"),
        ] {
            assert_eq!(kind_of(&value), name, "{value}");
        }
    }
}
