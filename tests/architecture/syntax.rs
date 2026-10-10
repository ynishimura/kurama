//! The Rust sources read as syntax for the rules a text scan got wrong: the
//! paths a file names outside its test-only items, the child processes it
//! starts, and the errors it turns into text.
//!
//! A line scan misses `use crate::{adapters::x}`, `use std::fs as f`, a call
//! split over lines and `use std::process::{Command, Stdio}`, and it trips on
//! a comment or a string. Parsing sees none of those problems. It does not
//! resolve names: a path is what the file writes, with its `use` items
//! expanded, `super::` and `self::` stay relative, a glob import is the path
//! it names, and a macro body is read as tokens. `SYNTAX_LIMITS` is the one
//! statement of that, which the registry repeats for each rule that reads
//! through here.

use proc_macro2::{TokenStream, TokenTree};
use syn::spanned::Spanned;
use syn::visit::Visit;

/// What a syntax-based check cannot see, as `rules.toml` states it.
pub(crate) const SYNTAX_LIMITS: &str = "names are not resolved: super:: and self:: stay relative, a glob import is the path it names, a macro body is read as tokens, and a path built by a macro from pieces is not seen";

/// A path a source names, with the line it is on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedPath {
    pub(crate) segments: Vec<String>,
    pub(crate) line: usize,
}

impl NamedPath {
    pub(crate) fn text(&self) -> String {
        self.segments.join("::")
    }
}

/// Whether an item is test-only: `#[cfg(test)]` or a test attribute.
fn is_test_only(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        let path = attribute.path();
        if path.is_ident("test") {
            return true;
        }
        if path.is_ident("cfg") {
            return attribute
                .parse_args::<syn::Meta>()
                .is_ok_and(|meta| meta.path().is_ident("test"));
        }
        path.segments
            .last()
            .is_some_and(|last| last.ident == "test")
    })
}

fn item_attributes(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

/// `use a::{b, c::d as e}` as the full paths it brings in, with the name each
/// one is known by in the file.
fn use_paths(prefix: &[String], tree: &syn::UseTree, out: &mut Vec<(Vec<String>, String)>) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut prefix = prefix.to_vec();
            prefix.push(path.ident.to_string());
            use_paths(&prefix, &path.tree, out);
        }
        syn::UseTree::Name(name) => {
            let mut full = prefix.to_vec();
            let ident = name.ident.to_string();
            if ident != "self" {
                full.push(ident.clone());
            }
            let known_as = full.last().cloned().unwrap_or(ident);
            out.push((full, known_as));
        }
        syn::UseTree::Rename(rename) => {
            let mut full = prefix.to_vec();
            full.push(rename.ident.to_string());
            out.push((full, rename.rename.to_string()));
        }
        syn::UseTree::Glob(_) => {
            let mut full = prefix.to_vec();
            full.push("*".into());
            out.push((full, "*".into()));
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                use_paths(prefix, tree, out);
            }
        }
    }
}

/// The `a::b::c` sequences in a macro's tokens: a macro body is not parsed
/// as code, but a path in it is still a path.
fn token_paths(tokens: TokenStream, out: &mut Vec<NamedPath>) {
    let mut current: Vec<String> = Vec::new();
    let mut line = 0;
    let mut colons = 0;
    let flush = |current: &mut Vec<String>, line: usize, out: &mut Vec<NamedPath>| {
        if current.len() > 1 {
            out.push(NamedPath {
                segments: std::mem::take(current),
                line,
            });
        }
        current.clear();
    };
    for token in tokens {
        match token {
            TokenTree::Ident(ident) => {
                if current.is_empty() || colons != 2 {
                    flush(&mut current, line, out);
                    line = ident.span().start().line;
                }
                current.push(ident.to_string());
                colons = 0;
            }
            TokenTree::Punct(punct) if punct.as_char() == ':' => colons += 1,
            TokenTree::Group(group) => {
                flush(&mut current, line, out);
                colons = 0;
                token_paths(group.stream(), out);
            }
            _ => {
                flush(&mut current, line, out);
                colons = 0;
            }
        }
    }
    flush(&mut current, line, out);
}

#[derive(Default)]
struct Reader {
    paths: Vec<NamedPath>,
    /// Name in the file -> the full path a `use` item gave it.
    imports: Vec<(String, Vec<String>)>,
    spawns: Vec<usize>,
    flattened: Vec<usize>,
    calls: Vec<(String, usize)>,
    debug_derived: Vec<(String, usize)>,
}

impl Reader {
    /// A path as written, with its first segment replaced by what a `use`
    /// item imported under that name.
    fn resolved(&self, path: &syn::Path) -> Vec<String> {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let imported = segments.first().and_then(|first| {
            self.imports
                .iter()
                .find(|(name, _)| name == first)
                .map(|(_, full)| full)
        });
        match imported {
            Some(full) => full
                .iter()
                .cloned()
                .chain(segments.into_iter().skip(1))
                .collect(),
            None => segments,
        }
    }
}

impl<'ast> Visit<'ast> for Reader {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !is_test_only(item_attributes(item)) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        if derives_debug(&item.attrs) {
            self.debug_derived
                .push((item.ident.to_string(), item.ident.span().start().line));
        }
        syn::visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        if derives_debug(&item.attrs) {
            self.debug_derived
                .push((item.ident.to_string(), item.ident.span().start().line));
        }
        syn::visit::visit_item_enum(self, item);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let mut found = Vec::new();
        use_paths(&[], &item.tree, &mut found);
        let line = item.span().start().line;
        for (full, known_as) in found {
            self.paths.push(NamedPath {
                segments: full.clone(),
                line,
            });
            self.imports.push((known_as, full));
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments = self.resolved(path);
        if segments.len() > 1 {
            self.paths.push(NamedPath {
                segments,
                line: path.span().start().line,
            });
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        token_paths(mac.tokens.clone(), &mut self.paths);
        let name = mac
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        if name.as_deref() == Some("anyhow") && formats_only_a_placeholder(&mac.tokens) {
            self.flattened.push(mac.span().start().line);
        }
        syn::visit::visit_macro(self, mac);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(function) = &*call.func {
            let full = self.resolved(&function.path);
            if let Some(name) = full.last() {
                self.calls.push((name.clone(), call.span().start().line));
            }
            let spawn = full.len() >= 4
                && full[full.len() - 3..] == ["process", "Command", "new"]
                && ["std", "tokio"].contains(&full[full.len() - 4].as_str());
            if spawn {
                self.spawns.push(call.span().start().line);
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "map_err" && call.args.first().is_some_and(flattens_closure_error) {
            self.flattened.push(call.method.span().start().line);
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

/// Whether `#[derive(..)]`, or a `#[cfg_attr(.., derive(..))]`, names
/// `Debug` by itself or as the last segment of a path (`std::fmt::Debug`).
fn derives_debug(attributes: &[syn::Attribute]) -> bool {
    fn names_debug(tokens: TokenStream) -> bool {
        tokens.into_iter().any(|token| match token {
            TokenTree::Ident(ident) => ident == "Debug",
            TokenTree::Group(group) => names_debug(group.stream()),
            _ => false,
        })
    }
    attributes.iter().any(|attribute| {
        let path = attribute.path();
        (path.is_ident("derive") || path.is_ident("cfg_attr"))
            && matches!(&attribute.meta, syn::Meta::List(list) if names_debug(list.tokens.clone()))
    })
}

/// `|e| X(e.to_string())`: a constructor whose only argument is the text of
/// the closure's error. A tuple that keeps a typed kind next to the text, or
/// a `format!`, is not it.
fn flattens_closure_error(argument: &syn::Expr) -> bool {
    let syn::Expr::Closure(closure) = argument else {
        return false;
    };
    let [syn::Pat::Ident(parameter)] = closure.inputs.iter().collect::<Vec<_>>()[..] else {
        return false;
    };
    let mut body = &*closure.body;
    while let syn::Expr::Block(block) = body {
        match block.block.stmts.as_slice() {
            [syn::Stmt::Expr(expr, None)] => body = expr,
            _ => return false,
        }
    }
    let syn::Expr::Call(call) = body else {
        return false;
    };
    let [syn::Expr::MethodCall(text)] = call.args.iter().collect::<Vec<_>>()[..] else {
        return false;
    };
    text.method == "to_string"
        && text.args.is_empty()
        && matches!(&*text.receiver, syn::Expr::Path(receiver) if receiver.path.is_ident(&parameter.ident))
}

/// `anyhow!("{e}")` or `anyhow!("{}", e)`: a message that is nothing but
/// another error's text.
fn formats_only_a_placeholder(tokens: &TokenStream) -> bool {
    let Some(TokenTree::Literal(literal)) = tokens.clone().into_iter().next() else {
        return false;
    };
    let Ok(syn::Lit::Str(text)) = syn::parse_str::<syn::Lit>(&literal.to_string()) else {
        return false;
    };
    let text = text.value();
    text.starts_with('{') && text.ends_with('}') && text.matches('{').count() == 1
}

/// What one production source names, starts and flattens.
pub(crate) struct Reading {
    pub(crate) paths: Vec<NamedPath>,
    pub(crate) spawns: Vec<usize>,
    pub(crate) flattened: Vec<usize>,
    /// The last segment of every function a call names, with its line.
    pub(crate) calls: Vec<(String, usize)>,
    /// Every struct and enum that derives `Debug`, with the line of its name.
    pub(crate) debug_derived: Vec<(String, usize)>,
}

pub(crate) fn read_source(source: &str) -> Reading {
    let file = syn::parse_file(source).unwrap_or_else(|e| panic!("does not parse: {e}"));
    let mut reader = Reader::default();
    reader.visit_file(&file);
    Reading {
        paths: reader.paths,
        spawns: reader.spawns,
        flattened: reader.flattened,
        calls: reader.calls,
        debug_derived: reader.debug_derived,
    }
}

/// Whether a path is what a forbidden entry names. `x::` is anything in the
/// crate `x`; a bare `x` is a crate whose name starts with it (`aws_sdk`
/// covers `aws_sdk_sts`); `x::y` is those segments next to each other
/// anywhere in the path.
pub(crate) fn names(path: &NamedPath, forbidden: &str) -> bool {
    if let Some(krate) = forbidden.strip_suffix("::") {
        return path.segments.first().is_some_and(|first| first == krate);
    }
    if !forbidden.contains("::") {
        return path
            .segments
            .first()
            .is_some_and(|first| first.starts_with(forbidden));
    }
    let wanted: Vec<&str> = forbidden.split("::").collect();
    path.segments
        .windows(wanted.len())
        .any(|window| window.iter().map(String::as_str).eq(wanted.iter().copied()))
}

/// The registry states, for every rule whose checks read through here, what
/// this reading cannot see, and it is this statement.
#[test]
fn every_syntax_rule_states_what_the_reading_cannot_see() {
    let registry: toml::Table = toml::from_str(
        &std::fs::read_to_string(crate::support::root().join("tests/architecture/rules.toml"))
            .unwrap(),
    )
    .unwrap();
    let rules = registry["rule"].as_array().unwrap();
    let syntax: Vec<&toml::Value> = rules
        .iter()
        .filter(|rule| rule.get("method").and_then(|m| m.as_str()) == Some("syntax"))
        .collect();
    assert!(syntax.len() >= 8, "{} syntax rules", syntax.len());
    for rule in syntax {
        let limits = rule
            .get("limits")
            .and_then(|l| l.as_str())
            .unwrap_or_default();
        assert!(
            limits.starts_with(SYNTAX_LIMITS),
            "{}: limits do not state SYNTAX_LIMITS",
            rule["id"]
        );
    }
}
