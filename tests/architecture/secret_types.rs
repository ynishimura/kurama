//! Whether a type whose name says it holds a credential, a token or a secret
//! prints its value through a derived `Debug`.
//!
//! A derived `Debug` prints every field, so one `debug!(?value)` or `{:?}` in
//! an error would log the value. A type named for a secret writes its `Debug`
//! out (or holds a `Secret`, whose `Debug` is the redaction marker); the ones
//! that hold only where a secret is, or how reading it failed, are listed in
//! `DEBUG_DERIVED_ALLOWED` with the reason.

use std::collections::BTreeSet;

use crate::support::*;
use crate::syntax::read_source;

/// The directories whose types are held to the rule: the value objects and
/// the plain data of the ports, where a credential type is declared.
const DIRS: [&str; 2] = ["src/domain/types", "src/ports"];

/// A type whose name holds one of these says it carries a secret.
const SECRET_WORDS: [&str; 3] = ["Credential", "Token", "Secret"];

/// Types named for a secret that derive `Debug` because they hold none.
pub(crate) const DEBUG_DERIVED_ALLOWED: [(&str, &str); 8] = [
    (
        "SecretError",
        "a failure kind and a message that names the reference, never the value",
    ),
    (
        "TokenStoreError",
        "a keychain or file failure; the stored token is never part of it",
    ),
    (
        "SecretFailure",
        "how a read failed: a kind and a store, no text",
    ),
    (
        "AwsSecretStore",
        "which AWS store a reference names: Secrets Manager or Parameter Store",
    ),
    (
        "AwsSecretRef",
        "an AWS profile, an id, a region and a JSON key: where a secret is, not what it is",
    ),
    (
        "TokenSourceConfig",
        "the reference a token is read from (`SecretRef` redacts a literal), the header and the variable",
    ),
    (
        "TokenPlacement",
        "where a token goes: a header name and the format around it",
    ),
    (
        "SecretsSourceConfig",
        "variable names and the references they are read from (`SecretRef` redacts a literal)",
    ),
];

fn named_for_a_secret(name: &str) -> bool {
    SECRET_WORDS.iter().any(|word| name.contains(word))
}

/// The types of `source` named for a secret that derive `Debug`, with the
/// line of the name. Test-only items are not read.
pub(crate) fn secret_types_deriving_debug(source: &str) -> Vec<(String, usize)> {
    read_source(source)
        .debug_derived
        .into_iter()
        .filter(|(name, _)| named_for_a_secret(name))
        .collect()
}

#[test]
fn a_type_named_for_a_secret_does_not_derive_debug() {
    let allowed: BTreeSet<&str> = DEBUG_DERIVED_ALLOWED
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let mut found = BTreeSet::new();
    let mut problems = Vec::new();
    for dir in DIRS {
        for path in rust_files(&root().join(dir)) {
            if production_code(&path).is_none() {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for (name, line) in secret_types_deriving_debug(&source) {
                if !allowed.contains(name.as_str()) {
                    problems.push(format!(
                        "{}:{line}: {name} derives Debug; write it out with the value redacted, \
                         or hold a `Secret`",
                        path.strip_prefix(root()).unwrap().display()
                    ));
                }
                found.insert(name);
            }
        }
    }
    let mut seen = BTreeSet::new();
    for (name, reason) in DEBUG_DERIVED_ALLOWED {
        if !seen.insert(name) {
            problems.push(format!("DEBUG_DERIVED_ALLOWED: {name} is listed twice"));
        }
        if reason.trim().is_empty() {
            problems.push(format!("DEBUG_DERIVED_ALLOWED: {name} gives no reason"));
        }
        if !found.contains(name) {
            problems.push(format!(
                "DEBUG_DERIVED_ALLOWED: {name} no longer derives Debug in {DIRS:?}; remove it"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_secret_named_type_deriving_debug_in_any_form_is_found() {
    for (source, fixture) in [
        (
            "#[derive(Debug, Clone)]\npub struct ApiToken(String);\n",
            "derive(Debug) on a struct",
        ),
        (
            "#[derive(\n    Clone,\n    std::fmt::Debug,\n)]\npub enum IssuedSecret { Value(String) }\n",
            "a path to Debug in a derive split over lines, on an enum",
        ),
        (
            "#[cfg_attr(feature = \"x\", derive(Debug))]\nstruct Credentials { key: String }\n",
            "derive(Debug) inside cfg_attr",
        ),
        (
            "mod inner {\n    #[derive(Debug)]\n    pub(crate) struct RefreshToken(String);\n}\n",
            "a type in a nested module",
        ),
    ] {
        assert_detected(
            "ARCH-046",
            !secret_types_deriving_debug(source).is_empty(),
            fixture,
        );
    }
}

#[test]
fn a_written_out_debug_another_name_or_a_test_type_is_not_found() {
    for (source, fixture) in [
        (
            "#[derive(Clone)]\npub struct Secret(String);\nimpl std::fmt::Debug for Secret {\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(\"[REDACTED]\") }\n}\n",
            "a Debug written out",
        ),
        (
            "#[derive(Debug, Clone)]\npub struct Profile { name: String }\n",
            "a derived Debug on a type not named for a secret",
        ),
        (
            "#[cfg(test)]\nmod tests {\n    #[derive(Debug)]\n    struct FakeToken(String);\n}\n",
            "a test-only type",
        ),
        (
            "// #[derive(Debug)] struct Token(String);\n#[derive(Clone)]\nstruct Token(String);\n",
            "a derive in a comment",
        ),
    ] {
        assert_allowed(
            "ARCH-046",
            secret_types_deriving_debug(source).is_empty(),
            fixture,
        );
    }
}
