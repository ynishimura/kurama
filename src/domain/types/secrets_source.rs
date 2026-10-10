//! One `[auth.<name>]` credential source of kind `secrets`: a set of values
//! read from secret stores, each into a variable of its own, for the
//! command `kurama exec` runs (a login screen's username, password and
//! one-time password) -- no request, no grant, nothing stored.

use std::collections::BTreeMap;

use super::SecretRef;

/// Variables nobody but kurama may set: `exec` and the shell wrapper own
/// `AWS_*` and every `KURAMA_*` (`KURAMA_ENV_SCRIPT`, `KURAMA_AUTH`,
/// `KURAMA_AUTH_VAR`, `KURAMA_AGENT`, ...), and a source that wrote one
/// would replace the value kurama sets, or the one a child kurama reads.
const RESERVED_PREFIXES: [&str; 2] = ["AWS_", "KURAMA_"];

/// A validated `[auth.<name>]` entry of kind `secrets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretsSourceConfig {
    pub name: String,
    /// Variable name -> the reference its value is read from; never empty,
    /// always references, in name order.
    pub env: BTreeMap<String, SecretRef>,
}

impl SecretsSourceConfig {
    /// The rules a valid entry follows; the message names the offending
    /// variable and never a value.
    pub fn validate(&self) -> Result<(), String> {
        if self.env.is_empty() {
            return Err(
                "env must name at least one variable and the secret reference its value is read from"
                    .to_string(),
            );
        }
        for (var, reference) in &self.env {
            check_variable_name(var)?;
            if !reference.is_reference() {
                return Err(format!(
                    "env.{var} must be a secret reference, not the value itself: op://<vault>/<item>/<field>, \
                     aws-secrets://<aws-profile>/<secret-id> or aws-ssm://<aws-profile>/<parameter-name>"
                ));
            }
        }
        Ok(())
    }

    /// The variables, in the order `env` / `exec` set them.
    pub fn variables(&self) -> impl Iterator<Item = &str> {
        self.env.keys().map(String::as_str)
    }
}

/// `[A-Z_][A-Z0-9_]*`, and none of the names kurama sets itself.
fn check_variable_name(var: &str) -> Result<(), String> {
    let mut chars = var.chars();
    let shaped = chars
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !shaped {
        return Err(format!(
            "env.{var}: a variable name is upper-case letters, digits and _, not starting with a digit"
        ));
    }
    if let Some(prefix) = RESERVED_PREFIXES
        .iter()
        .find(|prefix| var.starts_with(*prefix))
    {
        return Err(format!(
            "env.{var}: variables starting with {prefix} are kurama's own; choose another name"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(pairs: &[(&str, &str)]) -> SecretsSourceConfig {
        SecretsSourceConfig {
            name: "site".into(),
            env: pairs
                .iter()
                .map(|(var, reference)| (var.to_string(), SecretRef::parse(reference).unwrap()))
                .collect(),
        }
    }

    #[test]
    fn references_under_shell_variable_names_are_valid() {
        let valid = source(&[
            ("SITE_USER", "op://Agent/site/username"),
            ("SITE_PASS", "aws-secrets://dev/site#password"),
            ("_OTP2", "aws-ssm://dev/site/otp"),
        ]);
        assert_eq!(valid.validate(), Ok(()));
        assert_eq!(
            valid.variables().collect::<Vec<_>>(),
            ["SITE_PASS", "SITE_USER", "_OTP2"]
        );
    }

    #[test]
    fn an_empty_table_is_refused() {
        let error = source(&[]).validate().unwrap_err();
        assert!(error.contains("at least one variable"), "{error}");
    }

    /// A literal would live in the file and every backup of it; the message
    /// names the variable and does not print the value back.
    #[test]
    fn a_literal_value_is_refused_without_echoing_it() {
        let error = source(&[("SITE_PASS", "hunter2")]).validate().unwrap_err();
        assert!(error.contains("env.SITE_PASS must be a secret reference"));
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[rstest::rstest]
    #[case("site_user", "upper-case")]
    #[case("1PASS", "upper-case")]
    #[case("SITE-PASS", "upper-case")]
    #[case("", "upper-case")]
    #[case("AWS_SECRET_ACCESS_KEY", "starting with AWS_")]
    #[case("KURAMA_ENV_SCRIPT", "starting with KURAMA_")]
    #[case("KURAMA_AUTH", "starting with KURAMA_")]
    fn a_name_that_is_not_a_variable_or_is_kuramas_own_is_refused(
        #[case] var: &str,
        #[case] expected: &str,
    ) {
        let error = source(&[(var, "op://Agent/site/password")])
            .validate()
            .unwrap_err();
        assert!(error.contains(expected), "{var}: {error}");
        assert!(error.contains(&format!("env.{var}")), "{error}");
    }
}
