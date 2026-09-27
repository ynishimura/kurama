//! The response as a collapsible JSON tree: the rows the expanded nodes
//! produce, each with the jq path that selects it, and the tree's keys.
//! Only expanded nodes are walked, and both the rows and the children one
//! node lists are bounded, so a deep or huge document costs what is shown.
//! Pure: no I/O.

use std::collections::BTreeSet;

use crossterm::event::KeyCode;
use serde_json::Value;

use crate::domain::functions::jq_completion::field_path;
use crate::shell::tui::components::list_navigation::moved_selection;

/// Rows one tree builds at most; the rest is one "not shown" row.
pub const MAX_TREE_ROWS: usize = 2_000;
/// Children one node lists at most; the rest is one "more" row.
pub const MAX_CHILDREN: usize = 500;

/// Which nodes are open (by jq path) and the selected row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeState {
    pub expanded: BTreeSet<String>,
    pub selected: usize,
}

impl Default for TreeState {
    /// The root open, its first row selected.
    fn default() -> Self {
        Self {
            expanded: BTreeSet::from([".".to_string()]),
            selected: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Leaf,
    Collapsed,
    Expanded,
    /// Children or rows left out by a bound; the count when known.
    Omitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    /// The jq path of the node (`.items[3].name`); empty on an omitted row.
    pub path: String,
    pub kind: RowKind,
    /// `name: "Rex"`, `items [40]`, `owner {2}`.
    pub text: String,
}

/// The visible rows of `value` with `expanded` open.
pub fn tree_rows(value: &Value, expanded: &BTreeSet<String>) -> Vec<TreeRow> {
    let mut walk = Walk {
        rows: Vec::new(),
        expanded,
        cut: false,
    };
    walk.push_node(value, ".".into(), String::new(), 0);
    let mut rows = walk.rows;
    if walk.cut {
        rows.truncate(MAX_TREE_ROWS);
        rows.push(TreeRow {
            depth: 0,
            path: String::new(),
            kind: RowKind::Omitted,
            text: format!("… rows after the first {MAX_TREE_ROWS} are not shown"),
        });
    }
    rows
}

struct Walk<'a> {
    rows: Vec<TreeRow>,
    expanded: &'a BTreeSet<String>,
    /// A node was left out because the rows reached the bound.
    cut: bool,
}

impl Walk<'_> {
    fn push_node(&mut self, value: &Value, path: String, label: String, depth: usize) {
        let rows = &mut self.rows;
        if rows.len() >= MAX_TREE_ROWS {
            self.cut = true;
            return;
        }
        let prefix = if label.is_empty() {
            String::new()
        } else {
            format!("{label} ")
        };
        let (children, summary): (Vec<(String, String, &Value)>, String) = match value {
            Value::Object(map) => (
                map.iter()
                    .take(MAX_CHILDREN)
                    .map(|(key, child)| (field_path(&path, key), key.clone(), child))
                    .collect(),
                format!("{{{}}}", map.len()),
            ),
            Value::Array(items) => (
                items
                    .iter()
                    .take(MAX_CHILDREN)
                    .enumerate()
                    .map(|(index, child)| (index_path(&path, index), format!("[{index}]"), child))
                    .collect(),
                format!("[{}]", items.len()),
            ),
            scalar => {
                let separator = if label.is_empty() { "" } else { ": " };
                rows.push(TreeRow {
                    depth,
                    path,
                    kind: RowKind::Leaf,
                    text: format!("{label}{separator}{scalar}"),
                });
                return;
            }
        };
        let total = match value {
            Value::Object(map) => map.len(),
            Value::Array(items) => items.len(),
            _ => 0,
        };
        let open = self.expanded.contains(&path);
        rows.push(TreeRow {
            depth,
            path,
            kind: if open {
                RowKind::Expanded
            } else {
                RowKind::Collapsed
            },
            text: format!("{prefix}{summary}"),
        });
        if !open {
            return;
        }
        for (child_path, child_label, child) in children {
            self.push_node(child, child_path, child_label, depth + 1);
        }
        if total > MAX_CHILDREN {
            self.rows.push(TreeRow {
                depth: depth + 1,
                path: String::new(),
                kind: RowKind::Omitted,
                text: format!("… {} more; jq reaches them", total - MAX_CHILDREN),
            });
        }
    }
}

/// `.[3]` at the root, `.items[3]` below it.
fn index_path(base: &str, index: usize) -> String {
    if base == "." {
        format!(".[{index}]")
    } else {
        format!("{base}[{index}]")
    }
}

/// Apply a key to the tree; whether it was one of the tree's keys.
pub fn handle_tree_key(state: &mut TreeState, code: KeyCode, value: &Value) -> bool {
    let rows = tree_rows(value, &state.expanded);
    let Some(row) = rows.get(state.selected.min(rows.len().saturating_sub(1))) else {
        return false;
    };
    match code {
        KeyCode::Right => {
            if row.kind == RowKind::Collapsed {
                state.expanded.insert(row.path.clone());
            }
        }
        KeyCode::Left => {
            if row.kind == RowKind::Expanded {
                state.expanded.remove(&row.path);
            } else if let Some(parent) = rows[..state.selected]
                .iter()
                .rposition(|above| above.depth + 1 == row.depth && above.kind == RowKind::Expanded)
            {
                state.selected = parent;
            }
        }
        code => match moved_selection(code, state.selected, rows.len()) {
            Some(index) => state.selected = index,
            None => return false,
        },
    }
    true
}

/// The jq path of the selected row, if it is a node.
pub fn selected_path(state: &TreeState, value: &Value) -> Option<String> {
    tree_rows(value, &state.expanded)
        .get(state.selected)
        .filter(|row| row.kind != RowKind::Omitted)
        .map(|row| row.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn texts(value: &Value, state: &TreeState) -> Vec<String> {
        tree_rows(value, &state.expanded)
            .iter()
            .map(|row| format!("{}{} {}", "  ".repeat(row.depth), row.path, row.text))
            .collect()
    }

    #[test]
    fn nodes_open_and_close_and_every_row_carries_its_jq_path() {
        let value = json!({"items": [{"name": "Rex", "a-b": 1}], "ok": true});
        let mut state = TreeState::default();
        assert_eq!(
            texts(&value, &state),
            [". {2}", "  .items items [1]", "  .ok ok: true"]
        );
        for code in [KeyCode::Down, KeyCode::Right, KeyCode::Down, KeyCode::Right] {
            assert!(handle_tree_key(&mut state, code, &value));
        }
        assert_eq!(
            texts(&value, &state),
            [
                ". {2}",
                "  .items items [1]",
                "    .items[0] [0] {2}",
                "      .items[0].name name: \"Rex\"",
                "      .items[0][\"a-b\"] a-b: 1",
                "  .ok ok: true",
            ]
        );
        state.selected = 3;
        assert_eq!(
            selected_path(&state, &value).as_deref(),
            Some(".items[0].name")
        );
        // Left on a leaf goes to its parent, and Left there closes it.
        handle_tree_key(&mut state, KeyCode::Left, &value);
        assert_eq!(state.selected, 2);
        handle_tree_key(&mut state, KeyCode::Left, &value);
        assert_eq!(texts(&value, &state).len(), 4);
        assert!(!handle_tree_key(&mut state, KeyCode::Char('x'), &value));

        let array = json!([[1]]);
        let mut state = TreeState::default();
        handle_tree_key(&mut state, KeyCode::Down, &array);
        handle_tree_key(&mut state, KeyCode::Right, &array);
        state.selected = 2;
        assert_eq!(selected_path(&state, &array).as_deref(), Some(".[0][0]"));
    }

    #[test]
    fn a_huge_array_and_a_deep_document_stay_within_the_bounds() {
        let huge = Value::Array((0..100_000).map(|index| json!({ "id": index })).collect());
        let rows = tree_rows(&huge, &TreeState::default().expanded);
        assert_eq!(rows.len(), 1 + MAX_CHILDREN + 1);
        assert_eq!(rows.last().unwrap().text, "… 99500 more; jq reaches them");
        assert_eq!(rows.last().unwrap().kind, RowKind::Omitted);

        // 120 levels (serde_json parses 128 at most) of 30 items, the
        // first of each the next level, all open: 3600 rows, cut at the bound.
        let mut deep = json!(1);
        let mut expanded = BTreeSet::from([".".to_string()]);
        let mut path = ".".to_string();
        for _ in 0..120 {
            let mut level = vec![json!(0); 30];
            level[0] = deep;
            deep = Value::Array(level);
            path = index_path(&path, 0);
            expanded.insert(path.clone());
        }
        let rows = tree_rows(&deep, &expanded);
        assert_eq!(rows.len(), MAX_TREE_ROWS + 1);
        assert_eq!(rows.last().unwrap().kind, RowKind::Omitted);
    }
}
