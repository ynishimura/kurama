//! Pure jq path completion and response-shaped examples shared by CLI and TUI.

use crate::domain::types::json_shape::JsonShape;

const MAX_CANDIDATES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub label: String,
    pub description: String,
    pub continues: bool,
    start: usize,
    cursor_back: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completion {
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Example {
    pub filter: String,
    pub description: String,
}

const BUILTINS: &[(&str, &str, usize)] = &[
    ("length", "Count items or characters", 0),
    ("keys", "List object keys or array indices", 0),
    ("map()", "Transform each array item", 1),
    ("select()", "Keep matching values", 1),
    ("to_entries", "Turn an object into key/value entries", 0),
    ("sort_by()", "Sort an array by a field", 1),
    ("group_by()", "Group array items by a field", 1),
    ("unique", "Remove duplicate array values", 0),
    ("add", "Combine array values", 0),
    ("first", "Take the first item", 0),
    ("last", "Take the last item", 0),
    ("has(\"\")", "Check for a key or index", 2),
    ("type", "Show the JSON type", 0),
    ("@csv", "Format an array as CSV", 0),
    ("@tsv", "Format an array as TSV", 0),
];

pub fn complete(shape: &JsonShape, text: &str, cursor: usize) -> Completion {
    let prefix = &text[..cursor];
    let start = token_start(prefix);
    let token = &prefix[start..];
    let candidates = if token.starts_with('.') {
        context(shape, &prefix[..start])
            .map(|shape| path_candidates(shape, token, !text[cursor..].starts_with(['.', '['])))
            .unwrap_or_default()
    } else {
        BUILTINS
            .iter()
            .filter(|(value, _, _)| value.starts_with(token))
            .map(|(value, description, cursor_back)| Candidate {
                value: (*value).into(),
                label: (*value).into(),
                description: (*description).into(),
                continues: *cursor_back > 0,
                start: 0,
                cursor_back: *cursor_back,
            })
            .collect()
    };
    Completion {
        candidates: candidates
            .into_iter()
            .map(|candidate| Candidate { start, ..candidate })
            .collect(),
    }
}

/// Replace the current path/name, preserving following expressions and arguments.
pub fn apply(text: &str, cursor: usize, candidate: &Candidate) -> (String, usize) {
    let mut result = String::with_capacity(text.len() + candidate.value.len());
    result.push_str(&text[..candidate.start]);
    let suffix = &text[continuation_end(text, candidate.start, cursor)..];
    if !candidate.value.starts_with('.')
        && let Some((name, _)) = candidate.value.split_once('(')
        && suffix.starts_with('(')
    {
        result.push_str(name);
        result.push_str(suffix);
        return (result, candidate.start + name.len() + 1);
    }
    result.push_str(&candidate.value);
    result.push_str(suffix);
    (
        result,
        candidate.start + candidate.value.len() - candidate.cursor_back,
    )
}

/// Where a character of a jq program stands against its JSON strings.
#[derive(Default)]
struct Quotes {
    quoted: bool,
    escaped: bool,
}

impl Quotes {
    /// A string that is already open, for a scan that starts after its quote.
    fn opened() -> Self {
        Self {
            quoted: true,
            escaped: false,
        }
    }

    /// Take `ch`; `true` when it belongs to a string, its quotes included,
    /// so the caller does not read it as syntax.
    fn consume(&mut self, ch: char) -> bool {
        if self.quoted {
            if self.escaped {
                self.escaped = false;
            } else if ch == '\\' {
                self.escaped = true;
            } else if ch == '"' {
                self.quoted = false;
            }
            true
        } else if ch == '"' {
            self.quoted = true;
            true
        } else {
            false
        }
    }
}

/// The remainder of this identifier or open key/index, excluding outer delimiters.
fn continuation_end(text: &str, start: usize, cursor: usize) -> usize {
    let mut quotes = Quotes::default();
    let mut brackets = 0usize;
    let mut advance = |ch| {
        if !quotes.consume(ch) {
            match ch {
                '[' => brackets += 1,
                ']' => brackets = brackets.saturating_sub(1),
                _ => {}
            }
        }
        !quotes.quoted && brackets == 0
    };
    let mut closed = true;
    for ch in text[start..cursor].chars() {
        closed = advance(ch);
    }
    if closed {
        return cursor
            + text[cursor..]
                .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
                .unwrap_or(text.len() - cursor);
    }
    for (index, ch) in text[cursor..].char_indices() {
        if advance(ch) {
            return cursor + index + ch.len_utf8();
        }
    }
    text.len()
}

fn token_start(text: &str) -> usize {
    let mut start = 0;
    let mut quotes = Quotes::default();
    for (index, ch) in text.char_indices() {
        if quotes.consume(ch) {
            continue;
        }
        if ch.is_whitespace()
            || matches!(ch, '|' | '(' | ',' | ';' | ':' | '{' | '}')
            || ch == '[' && !text[start..index].starts_with('.')
        {
            start = index + ch.len_utf8();
        }
    }
    start
}

/// Follow simple pipeline paths only. Do not guess the output of arbitrary jq.
fn context<'a>(shape: &'a JsonShape, prefix: &str) -> Option<&'a JsonShape> {
    let mut current = shape;
    let mut start = 0;
    let mut quotes = Quotes::default();
    // A `(` or `|` inside an index ends here or leaves a path `resolve_path`
    // cannot follow, so neither needs the bracket depth.
    for (index, ch) in prefix.char_indices() {
        if quotes.consume(ch) {
            continue;
        }
        match ch {
            '(' => {
                match prefix[start..index].trim() {
                    "map" | "sort_by" | "group_by" => current = current.item()?,
                    "select" | "" => {}
                    _ => return None,
                }
                start = index + 1;
            }
            '|' => {
                current = resolve_path(current, prefix[start..index].trim())?;
                start = index + 1;
            }
            ')' | '{' | '}' | ',' => return None,
            _ => {}
        }
    }
    Some(current)
}

fn resolve_path<'a>(mut shape: &'a JsonShape, path: &str) -> Option<&'a JsonShape> {
    let mut offset = usize::from(path.starts_with('.'));
    if offset == 0 {
        return None;
    }
    while offset < path.len() {
        let (step, end) = step(path, offset)?;
        shape = match step {
            Step::Field(name) => shape.field(&name)?,
            Step::Item => shape.item()?,
        };
        offset = end;
    }
    Some(shape)
}

enum Step {
    Field(String),
    Item,
}

fn step(path: &str, mut offset: usize) -> Option<(Step, usize)> {
    if path[offset..].starts_with('.') {
        offset += 1;
    }
    let rest = &path[offset..];
    if rest.starts_with('"') {
        let mut quotes = Quotes::opened();
        let end = rest.char_indices().skip(1).find_map(|(index, ch)| {
            quotes.consume(ch);
            (!quotes.quoted).then_some(index)
        })?;
        let name = serde_json::from_str::<String>(&rest[..=end]).ok()?;
        return Some((Step::Field(name), offset + end + 1));
    }
    if rest.starts_with('[') {
        let mut quotes = Quotes::default();
        let end = rest
            .char_indices()
            .skip(1)
            .find_map(|(index, ch)| (!quotes.consume(ch) && ch == ']').then_some(index))?;
        let contents = &rest[1..end];
        let index = contents.strip_prefix('-').unwrap_or(contents);
        let step = if contents.is_empty()
            || (!index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
        {
            Step::Item
        } else {
            Step::Field(serde_json::from_str::<String>(contents).ok()?)
        };
        return Some((step, offset + end + 1));
    }
    let end = rest
        .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some((Step::Field(rest[..end].into()), offset + end))
}

fn path_candidates(mut shape: &JsonShape, path: &str, expand_children: bool) -> Vec<Candidate> {
    let mut offset = 1;
    let mut base = ".";
    loop {
        if offset == path.len() {
            return if expand_children {
                children(shape, base)
            } else {
                Vec::new()
            };
        }
        let Some((next, end)) = step(path, offset) else {
            return children(shape, base)
                .into_iter()
                .filter(|candidate| candidate.value.starts_with(path))
                .collect();
        };
        if end == path.len() && !path.ends_with(']') {
            let Step::Field(ref prefix) = next else {
                unreachable!()
            };
            let candidates: Vec<_> = match shape {
                JsonShape::Object(fields) => fields
                    .iter()
                    .filter(|(name, _)| name.starts_with(prefix.as_str()))
                    .take(MAX_CANDIDATES)
                    .map(|(name, child)| Candidate {
                        value: field_path(base, name),
                        label: field_label(name),
                        description: child.description(),
                        continues: child.continues(),
                        start: 0,
                        cursor_back: 0,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            if expand_children
                && candidates.len() == 1
                && candidates[0].value == path
                && candidates[0].continues
                && let Step::Field(name) = next
            {
                return children(shape.field(&name).unwrap(), path);
            }
            return candidates;
        }
        let child = match next {
            Step::Field(name) => shape.field(&name),
            Step::Item => shape.item(),
        };
        let Some(child) = child else {
            return Vec::new();
        };
        shape = child;
        base = &path[..end];
        offset = end;
        if &path[offset..] == "." {
            return if expand_children {
                children(shape, base)
            } else {
                Vec::new()
            };
        }
    }
}

pub fn field_path(base: &str, name: &str) -> String {
    let identifier = name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    let base = if base == "." { "" } else { base };
    if identifier {
        format!("{base}.{name}")
    } else {
        format!(
            "{base}{}[{}]",
            if base.is_empty() { "." } else { "" },
            serde_json::to_string(name).unwrap()
        )
    }
}

fn field_label(name: &str) -> String {
    let quoted = serde_json::to_string(name).unwrap();
    quoted[1..quoted.len() - 1].into()
}

fn children(shape: &JsonShape, base: &str) -> Vec<Candidate> {
    let entries: Vec<_> = match shape {
        JsonShape::Object(fields) => fields
            .iter()
            .take(MAX_CANDIDATES)
            .map(|(name, shape)| (field_path(base, name), field_label(name), shape))
            .collect(),
        JsonShape::Array { item, .. } => ["[]", "[0]"]
            .into_iter()
            .map(|suffix| (format!("{base}{suffix}"), suffix.into(), item.as_ref()))
            .collect(),
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .map(|(value, label, shape)| Candidate {
            value,
            label,
            description: shape.description(),
            continues: shape.continues(),
            start: 0,
            cursor_back: 0,
        })
        .collect()
}

/// Small, runnable starting points using fields in the current response.
pub fn examples(shape: &JsonShape) -> Vec<Example> {
    let mut examples = Vec::new();
    let mut add = |filter: String, description: &str| {
        examples.push(Example {
            filter,
            description: description.into(),
        })
    };
    let array = match shape {
        JsonShape::Array { item, .. } => Some((".".to_string(), item.as_ref())),
        JsonShape::Object(fields) => fields
            .iter()
            .find_map(|(name, shape)| shape.item().map(|item| (field_path(".", name), item))),
        _ => None,
    };
    if let Some((path, item)) = array {
        add(format!("{path} | length"), "Count items");
        add(format!("{path}[]"), "Show each item");
        if let JsonShape::Object(fields) = item {
            let projection = fields
                .iter()
                .take(2)
                .map(|(name, _)| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(name).unwrap(),
                        field_path(".", name)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            add(
                format!("{path}[] | {{{projection}}}"),
                "Keep selected fields",
            );
            if let Some((name, _)) = fields.iter().find(|(_, shape)| {
                matches!(
                    shape,
                    JsonShape::Scalar {
                        kind: "boolean",
                        ..
                    }
                )
            }) {
                add(
                    format!("{path}[] | select({})", field_path(".", name)),
                    &format!("Keep items where {} is true", field_path(".", name)),
                );
            }
            if let Some((name, _)) = fields
                .iter()
                .find(|(_, shape)| matches!(shape, JsonShape::Scalar { kind: "string", .. }))
            {
                add(
                    format!("{path} | map({}) | sort", field_path(".", name)),
                    "Sort field values",
                );
            }
            add(format!("{path}[0] | keys"), "List the first item's fields");
        }
    } else if let JsonShape::Object(fields) = shape {
        add("keys".into(), "List fields");
        add("to_entries".into(), "Show key/value pairs");
        for (name, _) in fields.iter().take(2) {
            add(field_path(".", name), "Show a field");
        }
    }
    for (filter, description) in [
        (".", "Show the complete value"),
        ("type", "Show the JSON type"),
        ("[.]", "Wrap the value in an array"),
        ("{value: .}", "Wrap the value in an object"),
        ("tostring", "Convert the value to text"),
        (". == null", "Check whether the value is null"),
    ] {
        if examples.len() >= 8 {
            break;
        }
        examples.push(Example {
            filter: filter.into(),
            description: description.into(),
        });
    }
    examples
}

#[cfg(test)]
#[path = "jq_completion_tests.rs"]
mod tests;
