//! What the development documents publish: tested domain functions, module
//! docs, the byte-budget table and every `Held by` claim.

use crate::support::*;

#[test]
fn domain_functions_are_tested_in_their_own_file() {
    let mut untested = Vec::new();
    for path in rust_files(&root().join("src/domain/functions")) {
        if path.file_name().unwrap() == "mod.rs" {
            continue;
        }
        let Some(code) = production_code(&path) else {
            continue;
        };
        let content = std::fs::read_to_string(&path).unwrap();
        let mut tests = content[code.len()..].to_string();
        let stem = path.file_stem().unwrap().to_string_lossy();
        if let Ok(next_to_it) =
            std::fs::read_to_string(path.with_file_name(format!("{stem}_tests.rs")))
        {
            tests.push_str(&next_to_it);
        }
        for line in code.lines() {
            let Some(name) = line
                .strip_prefix("pub fn ")
                .or_else(|| line.strip_prefix("pub(crate) fn "))
                .and_then(|rest| rest.split(['(', '<']).next())
            else {
                continue;
            };
            if !contains_word(&tests, name) {
                untested.push(format!(
                    "{}: {name}",
                    path.strip_prefix(root()).unwrap().display()
                ));
            }
        }
    }
    assert!(
        untested.is_empty(),
        "a public domain function is tested in its own file or its `<name>_tests.rs`; tests move with the code:\n{}",
        untested.join("\n")
    );
}

#[test]
fn every_production_file_starts_with_a_module_doc() {
    let missing: Vec<String> = rust_files(&root().join("src"))
        .into_iter()
        .filter(|path| production_code(path).is_some())
        .filter(|path| {
            let content = std::fs::read_to_string(path).unwrap();
            !content
                .lines()
                .find(|line| !line.trim().is_empty())
                .is_some_and(|line| line.starts_with("//!"))
        })
        .map(|path| path.strip_prefix(root()).unwrap().display().to_string())
        .collect();
    assert!(
        missing.is_empty(),
        "start the file with a `//!` line that says what it is for; `cargo xtask map` prints it:\n{}",
        missing.join("\n")
    );
}

/// The byte-budget table in `docs/development/data.md` is the published form of
/// `DataRequest::spends_byte_budget`. Two statements of one rule drift, so the
/// table is parsed and checked against the function that decides.
#[test]
fn the_byte_budget_table_matches_the_rule_it_publishes() {
    use kurama::domain::types::dataset::{DataFormat, DataRequest};

    let page = std::fs::read_to_string(root().join("docs/development/data.md")).unwrap();
    let marker =
        "<!-- checked against `DataRequest::spends_byte_budget` by tests/architecture/ -->";
    let table: Vec<&str> = page
        .split(marker)
        .nth(1)
        .expect("the table is marked as checked here")
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .collect();
    let rows: Vec<Vec<&str>> = table
        .iter()
        .skip(2) // header and separator
        .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
        .collect();
    assert!(!rows.is_empty(), "the table has no rows: {table:?}");

    // One representative request per phrase the table uses to name a column.
    let request = |phrase: &str| -> DataRequest {
        let args = match phrase {
            "--tables" => serde_json::json!({"operation": "tables", "args": {}}),
            "--describe" => serde_json::json!({"operation": "describe", "args": {"table": "t"}}),
            "--explain" => {
                serde_json::json!({"operation": "query", "args": {"sql": "SELECT 1", "explain": true}})
            }
            "--preview TABLE --columns NAME" => {
                serde_json::json!({"operation": "preview", "args": {"table": "t", "columns": ["a"]}})
            }
            "--preview TABLE without --columns" => {
                serde_json::json!({"operation": "preview", "args": {"table": "t"}})
            }
            "--summary" => serde_json::json!({"operation": "summary", "args": {"table": "t"}}),
            "--query" => serde_json::json!({"operation": "query", "args": {"sql": "SELECT 1"}}),
            "--export" => {
                serde_json::json!({"operation": "query", "args": {"sql": "SELECT 1", "export": "out.csv"}})
            }
            other => panic!("the table names `{other}`, which this test cannot build"),
        };
        serde_json::from_value(args).unwrap()
    };

    let mut checked = 0;
    for row in &rows {
        let [operations, csv, parquet] = row[..] else {
            panic!("a table row does not have three cells: {row:?}");
        };
        for phrase in operations.split(", ") {
            let phrase = phrase.replace('`', "");
            let request = request(&phrase);
            for (cell, format) in [(csv, DataFormat::Csv), (parquet, DataFormat::Parquet)] {
                let published = match cell {
                    "yes" => true,
                    "no" => false,
                    other => panic!("a cell says `{other}`, not yes or no"),
                };
                assert_eq!(
                    request.spends_byte_budget(format),
                    published,
                    "{phrase} on {format:?}: the table says {cell}"
                );
                checked += 1;
            }
        }
    }
    // JSONL behaves as CSV does; check it here rather than widening the table.
    assert_eq!(
        request("--query").spends_byte_budget(DataFormat::Jsonl),
        request("--query").spends_byte_budget(DataFormat::Csv),
        "JSONL and CSV are both row formats"
    );
    assert!(checked >= 12, "only {checked} cells were checked");
}

/// A development document that promises behavior -- "once", "before",
/// "only" -- lists the promise in a `| Claim | Held by |` table, and the cell
/// names one test function: a function with a test attribute, found by
/// parsing, not a string that happens to read `fn <name>(`. The IAM chapter
/// published three such sentences that no test would have noticed breaking;
/// a renamed or deleted test now fails here instead of leaving the sentence
/// standing alone. Whether the test can fail for the claim is what `cargo
/// xtask mutate` and the Definition of Done ask.
#[test]
fn every_published_claim_names_a_test_that_exists() {
    let definitions = crate::test_index::test_definitions();
    let mut claims = 0;
    let mut missing = Vec::new();
    // Recursive: a claims table in a subdirectory is checked like any other.
    let mut pending = vec![root().join("docs/development")];
    let mut pages = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                pages.push(path);
            }
        }
    }
    pages.sort();
    for path in pages {
        let text = std::fs::read_to_string(&path).unwrap();
        let mut in_table = false;
        for (number, line) in text.lines().enumerate() {
            let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
            if !line.trim_start().starts_with('|') {
                in_table = false;
                continue;
            }
            if cells
                .iter()
                .map(|cell| cell.trim())
                .eq(["Claim", "Held by"])
            {
                in_table = true;
                continue;
            }
            if !in_table
                || cells
                    .iter()
                    .all(|cell| cell.trim().trim_matches('-').is_empty())
            {
                continue;
            }
            claims += 1;
            let held_by = cells.last().unwrap().trim().trim_matches('`');
            if let Err(reason) = crate::test_index::resolve_claim(held_by, &definitions) {
                missing.push(format!("{}: {reason}", location(&path, number, line)));
            }
        }
    }
    assert!(claims > 0, "no `| Claim | Held by |` table was found");
    assert!(
        missing.is_empty(),
        "a published claim does not name exactly one test function:\n{}",
        missing.join("\n")
    );
}
