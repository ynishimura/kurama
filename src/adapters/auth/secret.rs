//! Read an `op://` reference through the 1Password CLI: the whole item with
//! `op item get` when the reference names a field of it, `op read` otherwise.

use serde::Deserialize;
use tracing::debug;

use crate::adapters::auth::op_cli::OpCli;
use crate::adapters::config::OnePasswordConfig;
use crate::domain::functions::onepassword::{ItemGetOutput, build_item_get_args};
use crate::domain::types::Secret;
use crate::ports::SecretError;

/// The vault, the item and the field of `op://<vault>/<item>/<field>`: a
/// reference that one `op item get` of the item answers, together with every
/// other field of it the configuration names.
#[derive(Debug, PartialEq, Eq)]
pub struct ItemField<'a> {
    pub vault: &'a str,
    pub item: &'a str,
    pub field: &'a str,
}

impl<'a> ItemField<'a> {
    /// `None` for a reference that says more than the field -- a section
    /// (`op://v/i/section/field`) or a `?attribute=` -- which `op read` reads.
    pub fn parse(reference: &'a str) -> Option<Self> {
        let path = reference.strip_prefix("op://")?;
        if path.contains('?') {
            return None;
        }
        match path.split('/').collect::<Vec<_>>()[..] {
            [vault, item, field] if [vault, item, field].iter().all(|part| !part.is_empty()) => {
                Some(Self { vault, item, field })
            }
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct Item {
    fields: Vec<Field>,
}

#[derive(Deserialize)]
struct Field {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    value: Option<String>,
}

/// The value of the field an `op://` reference names, out of the JSON
/// `op item get` printed: matched on the field's id or its label, as
/// `op read` matches it. An empty field is the empty string.
pub fn item_field_value(reference: &ItemField, item_json: &str) -> Result<Secret, SecretError> {
    let item: Item = serde_json::from_str(item_json).map_err(|_| {
        SecretError::needs_a_person(format!(
            "op item get {} --vault {}: the 1Password CLI did not print an item",
            reference.item, reference.vault
        ))
    })?;
    let named = |name: &Option<String>| {
        name.as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(reference.field))
    };
    item.fields
        .into_iter()
        .find(|field| named(&field.id) || named(&field.label))
        .map(|field| Secret::new(field.value.unwrap_or_default()))
        .ok_or_else(|| {
            SecretError::invalid(format!(
                "op://{}/{}/{}: the item has no field {}",
                reference.vault, reference.item, reference.field, reference.field
            ))
        })
}

pub struct OnePasswordSecrets {
    op: std::sync::Arc<OpCli>,
}

impl OnePasswordSecrets {
    pub fn new(config: &OnePasswordConfig) -> Self {
        Self {
            op: std::sync::Arc::new(OpCli::from_config(config)),
        }
    }

    /// The value behind an `op://` reference, through `op read`.
    pub async fn read(&self, reference: &str) -> Result<Secret, SecretError> {
        debug!(reference, cli_path = %self.op.cli_path, "Reading a secret from 1Password");
        let what = format!("op read {reference}");
        let value = self
            .run(vec!["read".into(), reference.into()], &what)
            .await?;
        Ok(Secret::new(value.trim_end_matches(['\n', '\r'])))
    }

    /// The whole item, as the JSON `op item get --format json` prints: every
    /// field of it in one CLI run and one biometric prompt.
    pub async fn read_item(&self, vault: &str, item: &str) -> Result<Secret, SecretError> {
        debug!(vault, item, cli_path = %self.op.cli_path, "Reading an item from 1Password");
        let what = format!("op item get {item} --vault {vault}");
        self.run(
            build_item_get_args(item, Some(vault), ItemGetOutput::Json),
            &what,
        )
        .await
        .map(Secret::new)
    }

    /// stdout of one `op` run. Every failure here needs a person: signing
    /// in, approving the prompt, or fixing the reference.
    async fn run(&self, args: Vec<String>, what: &str) -> Result<String, SecretError> {
        let output = tokio::task::spawn_blocking({
            let op = self.op.clone();
            move || op.run(&args.iter().map(String::as_str).collect::<Vec<_>>())
        })
        .await
        .map_err(|error| SecretError::needs_a_person(error.to_string()))?
        .map_err(|error| match error.kind() {
            // The deadline message already says what to do about it.
            std::io::ErrorKind::TimedOut => SecretError::needs_a_person(error.to_string()),
            _ => SecretError::needs_a_person(format!(
                "could not run the 1Password CLI ({}): {error}",
                self.op.cli_path
            )),
        })?;
        if !output.status.success() {
            return Err(SecretError::needs_a_person(format!(
                "{what} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| SecretError::needs_a_person(format!("{what}: not UTF-8")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::SecretFailure;

    #[tokio::test]
    async fn a_missing_cli_is_a_failure_a_person_has_to_act_on() {
        let secrets = OnePasswordSecrets::new(&OnePasswordConfig {
            cli_path: "/nonexistent/op".into(),
            ..OnePasswordConfig::default()
        });
        let error = secrets
            .read("op://Agent/Item/secret")
            .await
            .expect_err("no CLI");
        assert!(error.to_string().contains("1Password CLI"), "{error}");
        assert_eq!(error.failure, SecretFailure::NeedsAPerson);
    }

    /// Only a reference that names a vault, an item and a field is read as
    /// a field of the item; a section or an attribute is `op read`'s.
    #[test]
    fn a_reference_is_a_field_of_an_item_only_when_it_names_nothing_more() {
        assert_eq!(
            ItemField::parse("op://Agent/db/password"),
            Some(ItemField {
                vault: "Agent",
                item: "db",
                field: "password"
            })
        );
        for reference in [
            "op://Agent/db/section/password",
            "op://Agent/db/one-time password?attribute=otp",
            "op://Agent/db",
            "op://Agent//password",
            "op://Agent/db/",
        ] {
            assert_eq!(ItemField::parse(reference), None, "{reference}");
        }
    }

    /// Two references to one item take two different fields out of the one
    /// JSON document, by id or by label, whatever the case.
    #[test]
    fn each_field_of_an_item_is_found_by_its_id_or_its_label() {
        let item = r#"{"fields":[
            {"id":"username","label":"username","value":"reader"},
            {"id":"password","label":"password","value":"s3cret"},
            {"id":"x1","label":"API Key","value":"key-value"},
            {"id":"notes","label":"notes"}
        ]}"#;
        for (reference, expected) in [
            ("op://Agent/db/username", "reader"),
            ("op://Agent/db/password", "s3cret"),
            ("op://Agent/db/api key", "key-value"),
            ("op://Agent/db/x1", "key-value"),
            ("op://Agent/db/notes", ""),
        ] {
            let field = ItemField::parse(reference).unwrap();
            assert_eq!(item_field_value(&field, item).unwrap().expose(), expected);
        }
        let missing = item_field_value(&ItemField::parse("op://Agent/db/token").unwrap(), item)
            .expect_err("no such field");
        assert_eq!(missing.failure, SecretFailure::Invalid);
        assert!(
            missing.to_string().contains("op://Agent/db/token"),
            "{missing}"
        );
        assert!(!missing.to_string().contains("s3cret"), "{missing}");
    }
}
