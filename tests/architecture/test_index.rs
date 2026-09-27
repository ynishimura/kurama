//! The test functions of the tree, read from the syntax: a function with a
//! `#[test]`, `#[tokio::test]` or `#[rstest]` attribute, with the file it is
//! in, and what a `Held by` cell resolves to among them.
//!
//! A text search for `fn <name>(` also finds a production function of that
//! name, one in a comment and one in a string. Parsing sees only functions
//! and their attributes; it resolves no names beyond the file, so two tests
//! of one name in different files are ambiguous unless the cell names the
//! file.

use std::path::Path;

use syn::visit::Visit;

use crate::support::*;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TestDefinition {
    pub(crate) file: String,
    pub(crate) name: String,
    pub(crate) kind: TestKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TestKind {
    /// Under `src/` or `xtask/src/`: a unit test next to the code.
    Unit,
    /// Under `tests/` or `xtask/tests/`: an integration test or a scenario.
    Integration,
}

/// Every test function under `src/`, `tests/` and xtask, and every case
/// under `tests/cases/`: `cargo xtask generate-cases` writes one `#[test]`
/// per case file, named after it, into `tests/scenarios/cases_generated.rs`,
/// so a claim a case holds resolves to the case's file and not to that one.
pub(crate) fn test_definitions() -> Vec<TestDefinition> {
    let mut definitions = Vec::new();
    for dir in ["src", "tests", "xtask/src", "xtask/tests"] {
        for path in rust_files(&root().join(dir)) {
            let file = relative_path(&path);
            if file == "tests/scenarios/cases_generated.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            definitions.extend(
                test_definitions_in(&file, &source).unwrap_or_else(|e| panic!("{file}: {e}")),
            );
        }
    }
    definitions.extend(
        case_claims()
            .into_iter()
            .map(|(name, feature)| TestDefinition {
                file: format!("tests/cases/{feature}/{name}.toml"),
                name,
                kind: TestKind::Integration,
            }),
    );
    definitions
}

fn relative_path(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

/// The test functions of one source file, nested modules included.
pub(crate) fn test_definitions_in(
    file: &str,
    source: &str,
) -> Result<Vec<TestDefinition>, syn::Error> {
    struct Collector<'a> {
        file: &'a str,
        found: Vec<TestDefinition>,
    }
    impl<'ast> Visit<'ast> for Collector<'_> {
        fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
            if item.attrs.iter().any(is_test_attribute) {
                self.found.push(TestDefinition {
                    file: self.file.to_string(),
                    name: item.sig.ident.to_string(),
                    kind: if self.file.starts_with("src/") || self.file.starts_with("xtask/src/") {
                        TestKind::Unit
                    } else {
                        TestKind::Integration
                    },
                });
            }
            syn::visit::visit_item_fn(self, item);
        }
    }
    let parsed = syn::parse_file(source)?;
    let mut collector = Collector {
        file,
        found: Vec::new(),
    };
    collector.visit_file(&parsed);
    Ok(collector.found)
}

fn is_test_attribute(attribute: &syn::Attribute) -> bool {
    let segments: Vec<String> = attribute
        .path()
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    matches!(
        segments
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
        ["test"] | ["tokio", "test"] | ["rstest"]
    )
}

/// The one test a `Held by` cell names: `name`, or `path/to/file.rs::name`
/// when the name alone is not enough.
pub(crate) fn resolve_claim<'a>(
    held_by: &str,
    definitions: &'a [TestDefinition],
) -> Result<&'a TestDefinition, String> {
    let (file, name) = match held_by.rsplit_once("::") {
        Some((file, name)) => (Some(file), name),
        None => (None, held_by),
    };
    let candidates: Vec<&TestDefinition> = definitions
        .iter()
        .filter(|test| test.name == name && file.is_none_or(|file| test.file == file))
        .collect();
    match candidates.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no test function named `{held_by}`")),
        many => Err(format!(
            "`{held_by}` names {} tests; write one of them with its file: {}",
            many.len(),
            many.iter()
                .map(|test| format!("{}::{}", test.file, test.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[test]
fn a_claim_on_a_function_without_a_test_attribute_does_not_resolve() {
    let source = "fn held() {}\n#[cfg(test)] mod tests { #[allow(dead_code)] fn also_held() {} }\n";
    let definitions = test_definitions_in("src/a.rs", source).unwrap();
    assert_detected(
        "ARCH-020",
        resolve_claim("held", &definitions) == Err("no test function named `held`".into())
            && resolve_claim("also_held", &definitions).is_err(),
        source,
    );
}

#[test]
fn a_claim_on_a_name_in_a_comment_or_a_string_does_not_resolve() {
    let source = "// fn held() {}\nconst S: &str = \"#[test] fn held() {}\";\n";
    let definitions = test_definitions_in("tests/a.rs", source).unwrap();
    assert_detected(
        "ARCH-020",
        resolve_claim("held", &definitions).is_err(),
        source,
    );
}

#[test]
fn a_claim_naming_two_tests_is_ambiguous_and_lists_both() {
    let mut definitions = test_definitions_in("src/a.rs", "#[test]\nfn held() {}\n").unwrap();
    definitions.extend(test_definitions_in("tests/b.rs", "#[test]\nfn held() {}\n").unwrap());
    let ambiguous = resolve_claim("held", &definitions)
        == Err("`held` names 2 tests; write one of them with its file: \
                src/a.rs::held, tests/b.rs::held"
            .into());
    assert_detected("ARCH-020", ambiguous, "two tests named `held`");
    assert_eq!(
        resolve_claim("tests/b.rs::held", &definitions).map(|test| test.kind),
        Ok(TestKind::Integration)
    );
}

#[test]
fn a_claim_resolves_sync_async_and_nested_tests() {
    // One line, so the text scan of `cargo xtask architecture-audit` does not
    // take these for tests of this file.
    let source = "#[cfg(test)] mod tests { #[test] fn plain() {} #[tokio::test] async fn awaited() {} \
                  #[rstest::rstest] fn parameterized() {} }";
    let definitions = test_definitions_in("src/a.rs", source).unwrap();
    assert_allowed(
        "ARCH-020",
        resolve_claim("plain", &definitions).map(|test| test.kind) == Ok(TestKind::Unit)
            && resolve_claim("awaited", &definitions).is_ok(),
        source,
    );
    // `rstest::rstest` is not the bare attribute; only `#[rstest]` counts.
    assert!(resolve_claim("parameterized", &definitions).is_err());
}

/// A case file is a test of its own name: what `cargo xtask generate-cases` writes from it.
#[test]
fn a_case_file_is_a_test_named_after_it() {
    let definitions = test_definitions();
    let (name, feature) = case_claims()
        .into_iter()
        .next()
        .expect("a case file in the tree");
    let found = resolve_claim(&name, &definitions).unwrap();
    assert_eq!(found.file, format!("tests/cases/{feature}/{name}.toml"));
    assert_eq!(found.kind, TestKind::Integration);
}
