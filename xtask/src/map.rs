//! `cargo xtask map` and `cargo xtask search`: the feature map with each
//! file's purpose line, and the public symbols of `src/` by name.

use std::collections::BTreeSet;
use std::path::Path;

use crate::features::{Feature, load_features};
use crate::impact::test_filters;
use crate::verify::{list_scenarios, scenario_name};
use crate::{root, rust_files};

/// `  - <purpose>`: the first sentence of the `//!` paragraph that opens a
/// Rust file (of `mod.rs` for a directory entry), empty for anything else.
fn module_doc(path: &str) -> String {
    let file = match path.strip_suffix('/') {
        Some(dir) => format!("{dir}/mod.rs"),
        None if path.ends_with(".rs") => path.to_string(),
        None => return String::new(),
    };
    let Ok(content) = std::fs::read_to_string(root().join(file)) else {
        return String::new();
    };
    let paragraph = content
        .lines()
        .map_while(|line| line.strip_prefix("//!"))
        .map(str::trim)
        .take_while(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let sentence = match paragraph.find(". ") {
        Some(end) => &paragraph[..=end],
        None => paragraph.as_str(),
    };
    if sentence.is_empty() {
        String::new()
    } else {
        format!("  - {sentence}")
    }
}

pub(crate) fn map(feature: Option<&str>) -> Result<(), String> {
    let features = load_features()?;
    let scenarios = list_scenarios()?;
    let selected: Vec<(&String, &Feature)> = match feature {
        Some(name) => vec![features.get_key_value(name).ok_or_else(|| {
            format!(
                "unknown feature `{name}`; known: {:?}",
                features.keys().collect::<Vec<_>>()
            )
        })?],
        None => features.iter().collect(),
    };
    for (name, feature) in selected {
        println!("[{name}] {}", feature.summary);
        println!(
            "  entry:     {}{}",
            feature.entry,
            module_doc(&feature.entry)
        );
        for file in &feature.files {
            println!("  file:      {file}{}", module_doc(file));
        }
        println!("  tests:     {}", test_command(&test_filters(feature)));
        if !feature.depends_on.is_empty() {
            println!("  depends:   {}", feature.depends_on.join(", "));
        }
        let used_by: Vec<&str> = features
            .iter()
            .filter(|(_, other)| other.depends_on.contains(name))
            .map(|(other, _)| other.as_str())
            .collect();
        if !used_by.is_empty() {
            println!("  used by:   {}", used_by.join(", "));
        }
        for scenario in &feature.scenarios {
            let known = if scenarios.iter().any(|e| scenario_name(e) == scenario) {
                ""
            } else {
                "  (MISSING)"
            };
            println!("  scenario:  {scenario}{known}");
        }
        if let Some(notes) = &feature.notes {
            println!("  notes:     {notes}");
        }
        println!();
    }
    if feature.is_none() {
        let mapped: BTreeSet<&String> = features.values().flat_map(|f| &f.scenarios).collect();
        let unmapped: Vec<&str> = scenarios
            .iter()
            .map(|s| scenario_name(s))
            .filter(|s| !mapped.contains(&s.to_string()))
            .collect();
        println!("scenarios: {} (unmapped: {unmapped:?})", scenarios.len());
    }
    Ok(())
}

pub(crate) fn test_command(filters: &[String]) -> String {
    format!(
        "cargo test --locked --features test-fakes -- {}",
        filters.join(" ")
    )
}

#[derive(Debug)]
struct Symbol {
    kind: &'static str,
    name: String,
    path: String,
    line: usize,
    doc: String,
}

/// Public items declared in `src/` (production code only), by scanning
/// declarations. Enough to answer "does something like this already exist?".
pub(crate) fn search(pattern: Option<&str>) -> Result<(), String> {
    let pattern = pattern
        .ok_or("usage: cargo xtask search <PATTERN>")?
        .to_lowercase();
    let mut symbols: Vec<Symbol> = rust_files(&root().join("src"))
        .iter()
        .flat_map(|path| declared_symbols(path))
        .filter(|symbol| symbol.name.to_lowercase().contains(&pattern))
        .collect();
    symbols.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    for symbol in &symbols {
        println!(
            "{:<6} {:<40} {}:{}  {}",
            symbol.kind, symbol.name, symbol.path, symbol.line, symbol.doc
        );
    }
    println!("{} symbol(s) match `{pattern}`", symbols.len());
    Ok(())
}

fn declared_symbols(path: &Path) -> Vec<Symbol> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let production = &content[..content.find("#[cfg(test)]").unwrap_or(content.len())];
    let relative = path
        .strip_prefix(root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let mut doc_lines: Vec<String> = Vec::new();
    let mut symbols = Vec::new();
    for (index, line) in production.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(doc) = trimmed.strip_prefix("///") {
            doc_lines.push(doc.trim().to_string());
            continue;
        }
        if trimmed.starts_with("#[") {
            continue;
        }
        if let Some((kind, name)) = declaration(trimmed) {
            symbols.push(Symbol {
                kind,
                name,
                path: relative.clone(),
                line: index + 1,
                doc: doc_lines.first().cloned().unwrap_or_default(),
            });
        }
        doc_lines.clear();
    }
    symbols
}

fn declaration(line: &str) -> Option<(&'static str, String)> {
    let rest = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub "))?;
    let rest = rest.strip_prefix("async ").unwrap_or(rest);
    let rest = rest.strip_prefix("unsafe ").unwrap_or(rest);
    for (keyword, kind) in [
        ("fn ", "fn"),
        ("struct ", "struct"),
        ("enum ", "enum"),
        ("trait ", "trait"),
        ("type ", "type"),
        ("const ", "const"),
    ] {
        if let Some(after) = rest.strip_prefix(keyword) {
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some((kind, name));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_recognizes_public_items() {
        assert_eq!(
            declaration("pub fn generate_export_script("),
            Some(("fn", "generate_export_script".to_string()))
        );
        assert_eq!(
            declaration("pub async fn bootstrap() -> Result<Config> {"),
            Some(("fn", "bootstrap".to_string()))
        );
        assert_eq!(
            declaration("pub(crate) struct Foo {"),
            Some(("struct", "Foo".to_string()))
        );
        assert_eq!(declaration("fn private() {}"), None);
        assert_eq!(declaration("pub use foo::bar;"), None);
    }
}
