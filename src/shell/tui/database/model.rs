//! The database explorer's model: the table list with its filter, the four
//! tabs (columns, preview, SQL, result), the request that is running, and
//! what the view reads off them. No I/O, no clock.

use super::effects::DbAsk;
use crate::domain::types::database::DbResult;
use crate::shell::tui::components::LineInput;

/// What the header says about the open database.
pub struct DbSummary {
    /// The `[db.*]` name, or the file.
    pub name: String,
    /// The engine and the version the server reported.
    pub engine: String,
    /// The database (or file) the connection reads.
    pub database: String,
    /// `via ssm <instance>` when a tunnel carries the connection.
    pub tunnel: Option<String>,
}

/// One row of the table list.
pub struct TableEntry {
    pub schema: String,
    pub name: String,
    /// `table` or `view`, as the catalog said.
    pub kind: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::VariantArray)]
pub enum Tab {
    Columns,
    Preview,
    Sql,
    Result,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Self::Columns => "Columns",
            Self::Preview => "Preview",
            Self::Sql => "SQL",
            Self::Result => "Result",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Columns => Self::Preview,
            Self::Preview => Self::Sql,
            Self::Sql => Self::Result,
            Self::Result => Self::Columns,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Columns => Self::Result,
            Self::Preview => Self::Columns,
            Self::Sql => Self::Preview,
            Self::Result => Self::Sql,
        }
    }
}

/// Which side has the keys. On a compact terminal it is also the side that
/// is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Tables,
    Detail,
}

/// A result on one tab, with the selected cell.
pub struct Grid {
    /// What it is the result of: a table name, or `query`.
    pub source: String,
    pub result: DbResult,
    pub elapsed_ms: u64,
    pub row: usize,
    pub column: usize,
}

/// The request that is running.
pub struct Running {
    pub ask: DbAsk,
    pub elapsed_secs: u64,
    /// `Esc` was pressed; the answer is on its way.
    pub stopping: bool,
}

pub enum DbModal {
    Help,
    /// The whole of one cell.
    Cell(CellDetail),
    Error(String),
}

pub struct CellDetail {
    pub column: String,
    pub data_type: Option<String>,
    pub base64: bool,
    /// `None` for NULL.
    pub value: Option<String>,
}

pub struct DbModel {
    pub summary: DbSummary,
    pub tables: Vec<TableEntry>,
    /// Where the next page of the list starts; `None` once the list is whole.
    pub more_tables: Option<(String, String)>,
    /// The first page has arrived.
    pub tables_loaded: bool,
    /// Indices into `tables` that match the filter.
    pub filtered: Vec<usize>,
    /// Index into `filtered`.
    pub selected: usize,
    pub search: LineInput,
    pub searching: bool,
    pub focus: Focus,
    pub tab: Tab,
    pub columns: Option<Grid>,
    pub preview: Option<Grid>,
    pub result: Option<Grid>,
    pub sql: LineInput,
    pub running: Option<Running>,
    /// How the last request ended when it did not bring a result: a stop.
    pub last_stop: Option<String>,
    pub modal: Option<DbModal>,
    /// One line in the header until the next key.
    pub notice: Option<String>,
    pub should_exit: bool,
}

impl DbModel {
    pub fn new(summary: DbSummary) -> Self {
        Self {
            summary,
            tables: Vec::new(),
            more_tables: None,
            tables_loaded: false,
            filtered: Vec::new(),
            selected: 0,
            search: LineInput::default(),
            searching: false,
            focus: Focus::Tables,
            tab: Tab::Columns,
            columns: None,
            preview: None,
            result: None,
            sql: LineInput::default(),
            running: None,
            last_stop: None,
            modal: None,
            notice: None,
            should_exit: false,
        }
    }

    pub fn selected_table(&self) -> Option<&TableEntry> {
        self.tables.get(*self.filtered.get(self.selected)?)
    }

    /// The grid of the current tab; the SQL tab has none.
    pub fn grid(&self) -> Option<&Grid> {
        match self.tab {
            Tab::Columns => self.columns.as_ref(),
            Tab::Preview => self.preview.as_ref(),
            Tab::Result => self.result.as_ref(),
            Tab::Sql => None,
        }
    }

    pub fn grid_mut(&mut self) -> Option<&mut Grid> {
        match self.tab {
            Tab::Columns => self.columns.as_mut(),
            Tab::Preview => self.preview.as_mut(),
            Tab::Result => self.result.as_mut(),
            Tab::Sql => None,
        }
    }

    /// The filter is on the names already fetched; the server is not asked.
    pub(super) fn apply_search(&mut self) {
        let query = self.search.as_str().to_lowercase();
        self.filtered = self
            .tables
            .iter()
            .enumerate()
            .filter(|(_, table)| {
                query.is_empty()
                    || format!("{}.{}", table.schema, table.name)
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }
}

/// The line under a result: how many rows, how long, and whether the
/// result itself was cut -- which is different from a cell cut to fit.
pub fn result_status(grid: &Grid) -> String {
    let result = &grid.result;
    let mut line = format!(
        "{} row{}  {} ms",
        result.row_count,
        if result.row_count == 1 { "" } else { "s" },
        grid.elapsed_ms
    );
    if let Some(reason) = result.stop_reason() {
        let bound = match reason {
            crate::domain::types::database::DbStopReason::MaxRows => "max_rows",
            crate::domain::types::database::DbStopReason::MaxResultBytes => "max_result_bytes",
        };
        line.push_str(&format!("  truncated at {bound}, more rows exist"));
    }
    line
}
