//! The home screen's command palette (`Ctrl-K` or `:`): every AWS profile,
//! `[auth.*]`, `[api.*]`, `[db.*]` and `[data.*]` row, the operations of the
//! descriptions already on disk and the explorer's history, found by a fuzzy
//! query, and where choosing one goes. Pure: the runtime reads the
//! descriptions and the history files, and fetches nothing.

use crossterm::event::{KeyCode, KeyEvent};

use super::sources::{Enter, Tab};
use super::update::{Handoff, TuiModel};
use crate::domain::functions::fuzzy_match::fuzzy_score;
use crate::domain::types::request_history::HistoryEntry;
use crate::shell::tui::components::LineInput;
use crate::shell::tui::components::list_navigation::moved_selection;

/// Candidates the runtime collects at most (operations and history).
pub const MAX_ITEMS: usize = 20_000;
/// Matches ranked and kept for one query.
pub const MAX_MATCHES: usize = 200;

/// Where a candidate goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteTarget {
    /// Select the profile on the AWS tab.
    Profile(String),
    /// Select the row of this name on a source tab.
    Row { tab: Tab, name: String },
    /// Open the API explorer, on this entry's form when there is one.
    Explore {
        api: String,
        entry: Option<HistoryEntry>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    /// `aws`, `auth`, `api`, `db`, `data`, `op`, `history`.
    pub kind: &'static str,
    pub name: String,
    pub detail: String,
    pub target: PaletteTarget,
    /// What the query is matched against, in lower case: the name, then
    /// the detail.
    haystack: String,
    /// Bytes of `haystack` that are the name.
    name_len: usize,
}

/// What a match within the name alone adds: the name is what was asked for,
/// the detail only narrows it.
const NAME_BONUS: i64 = 1_000;

impl PaletteItem {
    pub fn new(kind: &'static str, name: String, detail: String, target: PaletteTarget) -> Self {
        let lower_name = name.to_lowercase();
        let name_len = lower_name.len();
        let haystack = format!("{lower_name} {}", detail.to_lowercase());
        Self {
            kind,
            name,
            detail,
            target,
            haystack,
            name_len,
        }
    }
}

/// The open palette: its candidates, the query, the ranked matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub items: Vec<PaletteItem>,
    pub input: LineInput,
    /// Indices into `items`, best first.
    pub matches: Vec<usize>,
    pub selected: usize,
}

impl Palette {
    pub fn new(items: Vec<PaletteItem>) -> Self {
        let mut palette = Self {
            items,
            input: LineInput::default(),
            matches: Vec::new(),
            selected: 0,
        };
        palette.rank();
        palette
    }

    /// Candidates that arrived after the palette opened.
    pub fn extend(&mut self, items: Vec<PaletteItem>) {
        self.items.extend(items);
        self.rank();
    }

    fn rank(&mut self) {
        self.matches = rank(&self.items, self.input.as_str());
        self.selected = 0;
    }

    pub fn selected_item(&self) -> Option<&PaletteItem> {
        self.items.get(*self.matches.get(self.selected)?)
    }
}

/// The best [`MAX_MATCHES`] items for `query`, best first; ties keep the
/// items' order.
pub fn rank(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(i64, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let score = fuzzy_score(query, &item.haystack[..item.name_len])
                .map(|score| score + NAME_BONUS)
                .or_else(|| fuzzy_score(query, &item.haystack))?;
            Some((score, index))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(MAX_MATCHES);
    scored.into_iter().map(|(_, index)| index).collect()
}

/// The candidates the model already holds: profiles and the tabs' rows.
pub fn model_items(model: &TuiModel) -> Vec<PaletteItem> {
    let profiles = model.profiles.all_profiles.iter().map(|row| {
        PaletteItem::new(
            "aws",
            row.name.clone(),
            row.role_arn.clone().unwrap_or_default(),
            PaletteTarget::Profile(row.name.clone()),
        )
    });
    let rows = [Tab::Auth, Tab::Api, Tab::Db, Tab::Data, Tab::S3]
        .into_iter()
        .flat_map(|tab| {
            let list = model.sources.list(tab).map(|list| list.rows.as_slice());
            list.unwrap_or_default().iter().map(move |row| {
                let target = match (&row.enter, tab) {
                    (Enter::Run(_), Tab::Api) => PaletteTarget::Explore {
                        api: row.name.clone(),
                        entry: None,
                    },
                    _ => PaletteTarget::Row {
                        tab,
                        name: row.name.clone(),
                    },
                };
                PaletteItem::new(row.kind, row.name.clone(), row.next.clone(), target)
            })
        });
    profiles.chain(rows).collect()
}

/// A key on the open palette.
pub fn update_palette_key(mut model: TuiModel, key: KeyEvent) -> (TuiModel, bool) {
    let Some(palette) = model.palette.as_mut() else {
        return (model, false);
    };
    match key.code {
        KeyCode::Esc => model.palette = None,
        KeyCode::Enter => {
            if let Some(item) = palette.selected_item() {
                let target = item.target.clone();
                model.palette = None;
                return (go_to(model, target), true);
            }
        }
        KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
            if let Some(index) = moved_selection(key.code, palette.selected, palette.matches.len())
            {
                palette.selected = index;
            }
        }
        _ => {
            if palette.input.handle_key(key) {
                palette.rank();
            }
        }
    }
    (model, false)
}

/// Show what `target` names; `model.handoff` is set for the explorer.
fn go_to(mut model: TuiModel, target: PaletteTarget) -> TuiModel {
    match target {
        PaletteTarget::Profile(name) => {
            model.tab = Tab::Aws;
            model.searching = false;
            model.search_query.clear();
            model.profiles.filtered_profiles = model.profiles.all_profiles.clone();
            model.profiles.selected_index = model
                .profiles
                .filtered_profiles
                .iter()
                .position(|row| row.name == name)
                .unwrap_or(0);
        }
        PaletteTarget::Row { tab, name } => {
            model.tab = tab;
            if let Some(list) = model.sources.list_mut(tab)
                && let Some(index) = list.rows.iter().position(|row| row.name == name)
            {
                list.selected = index;
            }
        }
        PaletteTarget::Explore { api, entry } => {
            model.tab = Tab::Api;
            model.handoff = Some(match entry {
                Some(entry) => Handoff::Explore { api, entry },
                None => Handoff::Run(vec!["api".into(), api]),
            });
            model.should_exit = true;
        }
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(api: &str, id: &str) -> PaletteItem {
        PaletteItem::new(
            "op",
            id.into(),
            format!("{api}  GET /{id}"),
            PaletteTarget::Explore {
                api: api.into(),
                entry: None,
            },
        )
    }

    #[test]
    fn the_query_ranks_the_closest_names_first_and_the_matches_are_bounded() {
        let items = vec![op("pets", "owners/list-pets"), op("pets", "pets/get")];
        let names = |query| -> Vec<String> {
            rank(&items, query)
                .into_iter()
                .map(|index| items[index].name.clone())
                .collect()
        };
        assert_eq!(names("pets/get"), ["pets/get"]);
        assert_eq!(names("pg"), ["pets/get", "owners/list-pets"]);
        assert_eq!(names(""), ["owners/list-pets", "pets/get"]);
        assert!(names("zzz").is_empty());
        // Both have p-e-t-g-e-t, but only one in its name.
        let items = vec![op("pets", "pets/list"), op("pets", "pets/get")];
        assert_eq!(rank(&items, "petget")[0], 1);

        let many: Vec<_> = (0..MAX_MATCHES * 3)
            .map(|index| op("pets", &format!("op{index}")))
            .collect();
        assert_eq!(rank(&many, "op").len(), MAX_MATCHES);
    }

    /// Thousands of operations: one keystroke ranks them all. The bound is
    /// generous for a debug build on a loaded machine; a ranking that went
    /// quadratic would take seconds.
    #[test]
    fn ranking_ten_thousand_operations_takes_well_under_a_frame_budget() {
        let items: Vec<_> = (0..10_000)
            .map(|index| op("github", &format!("repos/list-issues-{index}")))
            .collect();
        let started = std::time::Instant::now();
        for query in ["r", "re", "rep", "repos/li", "issues-99"] {
            rank(&items, query);
        }
        let per_query = started.elapsed() / 5;
        assert!(
            per_query < std::time::Duration::from_millis(250),
            "{per_query:?} per query"
        );
    }
}
