//! What a config command refuses in what a person gave it -- the TOML to add, a key to show -- as typed errors whose hint says what to fix, and the policy every piece of TOML written into config.toml passes.
//!
//! A refusal here leaves config.toml as it was, and the file is not what is
//! wrong, so none of these carries the "fix config.toml" hint a
//! `CoreError::Configuration` does.

use std::path::PathBuf;

use super::saved::Saved;
use crate::adapters::error::CoreError;

/// A refusal of what the person gave. Every variant is `CONFIG_INVALID`.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("cannot read the input {input}")]
    Unreadable {
        input: String,
        #[source]
        source: std::io::Error,
    },
    /// Not TOML, or a value `Config` refuses where the input has it; the
    /// message names the input's own line.
    #[error("{0}")]
    Invalid(String),
    /// A section the input adds is wrong as a configuration: a reference to
    /// nothing, a name an AWS profile has, an exclusive pair.
    #[error(transparent)]
    Section(CoreError),
    #[error(
        "the input has a key before its first table header; start it with a header such as [auth.<name>] or [api.<name>]"
    )]
    KeyOutsideTable,
    #[error("the input adds nothing; give it a section such as [auth.<name>] or [api.<name>]")]
    Empty,
    /// Sections the file already has, as `api.github`.
    #[error("{} already in {}; nothing was added", bracketed(units), path.display())]
    Taken { path: PathBuf, units: Vec<String> },
    /// Secret keys holding anything but a reference, as
    /// `auth.<name>.client_secret`. The value is never kept.
    #[error(
        "{} must be a secret reference, not the value itself: op://<vault>/<item>/<field>, \
         aws-secrets://<aws-profile>/<secret-id> or aws-ssm://<aws-profile>/<parameter-name>; the value is not shown",
        keys.join(", ")
    )]
    LiteralSecret { keys: Vec<String> },
    /// `config show` / `config list` asked for a key the file does not have.
    #[error("{} has no {key}", path.display())]
    NoSuchKey { path: PathBuf, key: String },
    /// Sections an edit names that the file does not have, as `api.github`.
    #[error("{} not in {}; nothing was changed", bracketed(units), path.display())]
    Absent { path: PathBuf, units: Vec<String> },
    /// A key under no section: not a fixed table, not an entry of a named one.
    #[error(
        "{key} is not in a section: a section is core, aws, onepassword, openapi, \
         or auth.<name>, api.<name>, data.<name>, db.<name>, s3.<name>"
    )]
    NotAUnit { key: String },
    /// `set` or `unset` given a whole section.
    #[error(
        "{key} is a whole section: `kurama config set --file` replaces it and `kurama config remove` removes it"
    )]
    WholeUnit { key: String },
    /// `remove` given a key inside a section.
    #[error("{key} is a key inside [{unit}], not a section: `kurama config unset` removes it")]
    InsideUnit { key: String, unit: String },
    /// The same key twice, or a key and a table that holds it.
    #[error("{}", overlap(outer, inner))]
    Overlap { outer: String, inner: String },
    /// `[auth.*]` sections an edit removes that APIs it keeps still use.
    #[error(
        "{} still used by {}; nothing was removed",
        bracketed(auths),
        listed(referrers)
    )]
    StillReferenced {
        auths: Vec<String>,
        /// The APIs that use them, as `api.github`.
        referrers: Vec<String>,
    },
}

impl InputError {
    /// What to do about it: the file was not changed.
    pub fn hint(&self) -> &'static str {
        match self {
            Self::Taken { .. } => {
                "choose another name for the new section, or leave it out of the input; the file was not changed"
            }
            Self::NoSuchKey { .. } => {
                "`kurama config list` shows the sections and keys the file has"
            }
            Self::Absent { .. } => {
                "`kurama config list` shows the sections the file has, and `kurama config add` adds a new one; the file was not changed"
            }
            Self::StillReferenced { .. } => {
                "remove those APIs in the same `kurama config remove`, or give them another auth with `kurama config set` first; the file was not changed"
            }
            _ => "fix the input; the file was not changed",
        }
    }
}

fn overlap(outer: &str, inner: &str) -> String {
    if outer == inner {
        format!("{outer} is given more than once; give each once")
    } else {
        format!("{outer} already holds {inner}; give one of them")
    }
}

/// `[a], [b]`.
fn listed(units: &[String]) -> String {
    let names: Vec<String> = units.iter().map(|unit| format!("[{unit}]")).collect();
    names.join(", ")
}

/// `[a] is` or `[a], [b] are`.
fn bracketed(units: &[String]) -> String {
    match units.len() {
        1 => format!("{} is", listed(units)),
        _ => format!("{} are", listed(units)),
    }
}

/// The policy every piece of TOML written into config.toml passes, whatever
/// the command: it parses, opens with a table header (a key before it would
/// land in the file's last table), adds at least one section, and holds a
/// reference in every secret key.
pub fn check_input(fragment: &str) -> Result<Saved, InputError> {
    let input =
        Saved::parse(fragment).map_err(|error| InputError::Invalid(error.describe("the input")))?;
    if input.has_top_level_values() {
        return Err(InputError::KeyOutsideTable);
    }
    if input.units().is_empty() {
        return Err(InputError::Empty);
    }
    let keys = input.literal_secrets();
    if !keys.is_empty() {
        return Err(InputError::LiteralSecret { keys });
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_of_the_policy_is_typed_and_names_no_value() {
        assert!(matches!(
            check_input("auth.x.kind = \"token\"\n"),
            Err(InputError::KeyOutsideTable)
        ));
        assert!(matches!(check_input("# nothing\n"), Err(InputError::Empty)));
        match check_input("[auth.x]\nkind = \"token\"\ntoken = 987654321\n") {
            Err(error @ InputError::LiteralSecret { .. }) => {
                assert!(error.to_string().starts_with("auth.x.token must be"));
                assert!(!error.to_string().contains("987654321"));
            }
            other => panic!("{:?}", other.err()),
        }
        match check_input("[auth.x\n") {
            Err(InputError::Invalid(message)) => {
                assert!(message.starts_with("the input line 1:"), "{message}")
            }
            other => panic!("{:?}", other.err()),
        }
        assert_eq!(
            check_input("[api.x]\nbase_url = \"https://x\"\n")
                .unwrap()
                .units(),
            ["api.x"]
        );
    }

    #[test]
    fn a_taken_section_names_every_one() {
        let error = InputError::Taken {
            path: "/c.toml".into(),
            units: vec!["auth.a".into(), "api.a".into()],
        };
        assert_eq!(
            error.to_string(),
            "[auth.a], [api.a] are already in /c.toml; nothing was added"
        );
    }
}
