//! Virtualized results grid: only visible cells are rendered, so
//! multi-million-row results scroll smoothly with flat memory. Started as
//! the M2 spike; findings are in docs/devlog.md.

use crate::theme;
use gpui::Entity;
use gpui::{
    actions, div, point, prelude::*, px, rgb, uniform_list, Action, App, ClipboardItem, Context,
    EventEmitter, FocusHandle, Focusable, KeyBinding, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, UniformListScrollHandle, Window,
};
use gpui_component::input::{Input, InputState};
use gpui_component::menu::ContextMenuExt as _;
use gpui_component::Sizable as _;
use std::collections::HashMap;
use zedb_core::{ColumnMeta, Value};

actions!(grid_spike, [Copy, CopyAsCsv, SelectAll]);

/// Key context for the results grid. Scoping cmd-c / cmd-a to this
/// (rather than binding them globally) keeps them from shadowing the
/// SQL editor's own cmd-c / cmd-a, which live in the "Input" context.
const CONTEXT: &str = "DataGrid";

/// A header interaction asking the owning tab to rewrite the query.
pub enum GridEvent {
    /// The complete desired sort, in priority order; empty clears it.
    SortRequested { sort: Vec<(String, bool)> },
    /// A full managed WHERE conjunct for the column, or None to clear.
    FilterRequested {
        column: String,
        predicate: Option<String>,
    },
}

/// Header context-menu choice; routed back via the workspace so it
/// lands on the visible grid regardless of focus.
#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct HeaderSort {
    pub column: String,
    /// 0 clears this column's sort, 1 ascending, 2 descending.
    pub direction: u8,
    /// Merge into the existing sort instead of replacing it.
    pub multi: bool,
}

/// Header context-menu request to open the filter panel; routed via
/// the workspace, which owns the statement the prefill comes from.
#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct HeaderFilter {
    pub column: String,
}

/// The open filter popover under one header.
struct FilterPanel {
    column: String,
    numeric: bool,
    prefill: Option<String>,
    mode: FilterMode,
}

enum FilterMode {
    /// A distinct-values probe is in flight.
    Loading,
    /// Distinct dictionary values with their checked state, plus an
    /// optional NULL entry when the column can hold one.
    Checkboxes {
        values: Vec<(String, bool)>,
        null: Option<bool>,
    },
    Text(Entity<InputState>),
}

const ROW_HEIGHT: f32 = 24.0;
const COL_WIDTH: f32 = 120.0;

/// Resolved tree-sitter highlight runs over a displayed string.
type HighlightRuns = Vec<(std::ops::Range<usize>, gpui::HighlightStyle)>;

/// A rectangular cell selection: an anchor (where it started) and a
/// focus (where it currently reaches). A single click has anchor ==
/// focus; drag and shift-click move the focus.
#[derive(Clone, Copy)]
struct Selection {
    anchor: (usize, usize),
    focus: (usize, usize),
}

impl Selection {
    fn cell(pos: (usize, usize)) -> Self {
        Self {
            anchor: pos,
            focus: pos,
        }
    }

    fn rows(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor.0.min(self.focus.0)..=self.anchor.0.max(self.focus.0)
    }

    fn cols(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor.1.min(self.focus.1)..=self.anchor.1.max(self.focus.1)
    }

    fn contains(&self, row: usize, col: usize) -> bool {
        self.rows().contains(&row) && self.cols().contains(&col)
    }

    fn is_single(&self) -> bool {
        self.anchor == self.focus
    }
}

pub struct GridSpike {
    columns: Vec<ColumnMeta>,
    rows: Vec<Vec<Value>>,
    requested_rows: Option<usize>,
    result_complete: bool,
    result_capped: bool,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    selected: Option<Selection>,
    /// A left-drag selection is in progress (extends focus on hover).
    selecting: bool,
    /// Per-column widths, drag-resizable from the header dividers.
    col_widths: Vec<f32>,
    /// An active header-divider drag: (column, start width, start mouse x).
    resizing_column: Option<(usize, f32, f32)>,
    /// A resize drag just ended; swallow the click it would produce.
    just_resized: bool,
    /// The sort the displayed result actually ran with, by column name.
    sort: Vec<(String, bool)>,
    /// A result whose header arrived but whose rows have not: the old
    /// rows stay visible until the replacement starts streaming.
    pending: Option<(Vec<ColumnMeta>, Option<usize>)>,
    /// Column-attributable filters (column, conjunct) from the SQL.
    filters: Vec<(String, String)>,
    /// Remembered widths per column-name set, so re-running the same
    /// shape of query keeps your resizes.
    width_memory: HashMap<String, Vec<f32>>,
    filter_panel: Option<FilterPanel>,
    /// Cell whose full value is open in the inspector overlay.
    inspected: Option<(usize, usize)>,
    /// Lazily-filled tree-sitter runs for visible composite cells;
    /// None entries mean "computed, nothing to color". Cleared on new
    /// results and on theme change.
    highlight_cache: HashMap<(usize, usize), Option<HighlightRuns>>,
    /// The theme mode the cache was computed under.
    cache_dark: bool,
    /// Long-lived highlighters (query compilation is ~10ms; parsing a
    /// cell is microseconds), one per grammar.
    hl_sql: Option<gpui_component::highlighter::SyntaxHighlighter>,
    hl_json: Option<gpui_component::highlighter::SyntaxHighlighter>,
    /// The inspector's computed runs for its open cell.
    inspector_cache: Option<((usize, usize), Option<HighlightRuns>)>,
    /// Column widths haven't been fitted to content yet (no remembered
    /// layout); the first rows to arrive trigger a one-time auto-fit.
    needs_autofit: bool,
}

impl GridSpike {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.bind_keys([
            KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
            KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        ]);
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            requested_rows: None,
            result_complete: false,
            result_capped: false,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            selected: None,
            selecting: false,
            col_widths: Vec::new(),
            resizing_column: None,
            just_resized: false,
            sort: Vec::new(),
            pending: None,
            filters: Vec::new(),
            width_memory: HashMap::new(),
            filter_panel: None,
            inspected: None,
            highlight_cache: HashMap::new(),
            cache_dark: theme::is_dark(),
            hl_sql: None,
            hl_json: None,
            inspector_cache: None,
            needs_autofit: false,
        }
    }

    /// Fit each column to the wider of its header and its content (sampled
    /// from the first rows), clamped to a sensible min/max, so a result that
    /// doesn't fill the width sizes to its values instead of a flat default.
    fn autofit_column_widths(&mut self) {
        const CHAR_W: f32 = 7.0;
        const PAD: f32 = 22.0;
        const MIN_W: f32 = 48.0;
        const MAX_W: f32 = 360.0;
        let sample = self.rows.len().min(200);
        self.col_widths = self
            .columns
            .iter()
            .enumerate()
            .map(|(column, meta)| {
                let mut chars = meta.name.chars().count();
                for row in self.rows.iter().take(sample) {
                    if let Some(value) = row.get(column) {
                        chars = chars.max(value.to_string().chars().count().min(80));
                    }
                }
                (chars as f32 * CHAR_W + PAD).clamp(MIN_W, MAX_W)
            })
            .collect();
        self.needs_autofit = false;
    }

    pub fn begin_result(
        &mut self,
        columns: Vec<ColumnMeta>,
        requested_rows: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        // Keep the old result on screen until the replacement streams in.
        self.pending = Some((columns, requested_rows));
        cx.notify();
    }

    fn width_key(columns: &[ColumnMeta]) -> String {
        columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
            .join("\u{1}")
    }

    /// Swap the pending result in, discarding what was displayed.
    /// Hand every held row to a background drop (tab close).
    pub fn release_rows(&mut self) {
        crate::rt::drop_in_background(std::mem::take(&mut self.rows));
        if let Some(pending) = self.pending.take() {
            crate::rt::drop_in_background(pending);
        }
    }

    fn adopt_pending(&mut self) {
        if let Some((columns, requested_rows)) = self.pending.take() {
            match self
                .width_memory
                .get(&Self::width_key(&columns))
                .filter(|saved| saved.len() == columns.len())
            {
                // A remembered layout (the user resized this shape) wins.
                Some(saved) => {
                    self.col_widths = saved.clone();
                    self.needs_autofit = false;
                }
                // Otherwise fit columns to their content once rows arrive.
                None => {
                    self.col_widths = vec![COL_WIDTH; columns.len()];
                    self.needs_autofit = true;
                }
            }
            self.columns = columns;
            // The outgoing result may be enormous; free it off-thread.
            crate::rt::drop_in_background(std::mem::take(&mut self.rows));
            self.requested_rows = requested_rows;
            self.result_complete = false;
            self.result_capped = false;
            self.selected = None;
            self.selecting = false;
            self.inspected = None;
            self.highlight_cache.clear();
            self.inspector_cache = None;
            self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        }
    }

    pub fn append_rows(&mut self, batch: Vec<Vec<Value>>, cx: &mut Context<Self>) {
        self.adopt_pending();
        self.rows.extend(batch);
        if self.needs_autofit && !self.rows.is_empty() {
            self.autofit_column_widths();
        }
        cx.notify();
    }

    /// Prepend a live-tail batch newest-first: the newest row lands at the
    /// top and older rows push down, trimming the oldest off the bottom past
    /// `cap`. The batch arrives oldest-first (ORDER BY key ASC), so it is
    /// reversed. Follow the top only when the user is already there; if they
    /// have scrolled down to read, keep those rows in place by nudging the
    /// scroll offset down by exactly what was prepended.
    pub fn prepend_tail(&mut self, mut batch: Vec<Vec<Value>>, cap: usize, cx: &mut Context<Self>) {
        if batch.is_empty() {
            return;
        }
        self.adopt_pending();
        let added = batch.len();
        let at_top = {
            let state = self.scroll.0.borrow();
            state.base_handle.offset().y >= px(-1.0)
        };
        batch.reverse();
        let existing = std::mem::take(&mut self.rows);
        batch.extend(existing);
        self.rows = batch;
        if self.rows.len() > cap {
            crate::rt::drop_in_background(self.rows.split_off(cap));
        }
        if self.needs_autofit && !self.rows.is_empty() {
            self.autofit_column_widths();
        }
        if at_top {
            self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        } else {
            let state = self.scroll.0.borrow();
            let offset = state.base_handle.offset();
            state
                .base_handle
                .set_offset(point(offset.x, offset.y - px(added as f32 * ROW_HEIGHT)));
        }
        cx.notify();
    }

    /// Current retained row count (for the tail status line).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Drop all retained rows (keeping the header), for restarting a tail
    /// under an edited query. The old rows go to a background drop.
    pub fn clear_rows(&mut self, cx: &mut Context<Self>) {
        crate::rt::drop_in_background(std::mem::take(&mut self.rows));
        cx.notify();
    }

    /// Apply a header context-menu choice to the current sort.
    pub fn header_sort_action(&mut self, action: &HeaderSort, cx: &mut Context<Self>) {
        let mut sort = self.sort.clone();
        let column = action.column.clone();
        match action.direction {
            0 => sort.retain(|(name, _)| *name != column),
            direction => {
                let ascending = direction == 1;
                if action.multi {
                    match sort.iter_mut().find(|(name, _)| *name == column) {
                        Some(entry) => entry.1 = ascending,
                        None => sort.push((column, ascending)),
                    }
                } else {
                    sort = vec![(column, ascending)];
                }
            }
        }
        if sort != self.sort {
            // Optimistic: show the new sort immediately; completion
            // re-syncs the indicator from the SQL that actually ran.
            self.sort = sort.clone();
            cx.emit(GridEvent::SortRequested { sort });
            cx.notify();
        }
    }

    /// Close the filter popover if one is open; true when it was.
    pub fn close_filter_panel(&mut self, cx: &mut Context<Self>) -> bool {
        if self.filter_panel.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Set the filter indicators to what the executed SQL actually says.
    pub fn set_filters(&mut self, filters: Vec<(String, String)>, cx: &mut Context<Self>) {
        self.filters = filters;
        cx.notify();
    }

    /// Open the filter popover for a column. Dictionary columns
    /// (LowCardinality/Enum) with ten or fewer distinct values get
    /// checkboxes; everything else gets a text field.
    /// Open the filter popover. Enum columns resolve immediately from
    /// the type's variants; everything else opens in a loading state and
    /// returns true so the owner runs a distinct-values probe.
    pub fn begin_filter_panel(
        &mut self,
        column: String,
        prefill: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.columns.iter().position(|meta| meta.name == column) else {
            return false;
        };
        let type_name = self.columns[index].type_name.clone();
        let numeric = ["Int", "UInt", "Float", "Decimal"]
            .iter()
            .any(|kind| type_name.contains(kind))
            && !type_name.contains("String");
        let is_enum = type_name.contains("Enum8")
            || type_name.contains("Enum16")
            || type_name.trim_start().starts_with("Enum");
        if is_enum {
            let values = enum_variants(&type_name);
            if !values.is_empty() && values.len() <= 10 {
                let prechecked: Vec<String> =
                    prefill.as_deref().map(quoted_strings).unwrap_or_default();
                let null = type_name
                    .contains("Nullable")
                    .then(|| prefill_checks_null(prefill.as_deref()));
                self.filter_panel = Some(FilterPanel {
                    column,
                    numeric,
                    prefill,
                    mode: FilterMode::Checkboxes {
                        values: values
                            .into_iter()
                            .map(|value| {
                                let checked = prechecked.contains(&value);
                                (value, checked)
                            })
                            .collect(),
                        null,
                    },
                });
                cx.notify();
                return false;
            }
        }
        self.filter_panel = Some(FilterPanel {
            column,
            numeric,
            prefill,
            mode: FilterMode::Loading,
        });
        cx.notify();
        true
    }

    /// Resolve a loading popover with the probe's distinct values;
    /// None or more than ten means the text field.
    pub fn finish_filter_panel(
        &mut self,
        column: &str,
        values: Option<(Vec<String>, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.filter_panel.as_mut() else {
            return;
        };
        if panel.column != column || !matches!(panel.mode, FilterMode::Loading) {
            return;
        }
        let prefill = panel.prefill.clone();
        let mode = match values {
            Some((values, has_null)) if !values.is_empty() && values.len() <= 10 => {
                let prechecked: Vec<String> =
                    prefill.as_deref().map(quoted_strings).unwrap_or_default();
                let null = has_null.then(|| prefill_checks_null(prefill.as_deref()));
                FilterMode::Checkboxes {
                    values: values
                        .into_iter()
                        .map(|value| {
                            let checked = prechecked.contains(&value);
                            (value, checked)
                        })
                        .collect(),
                    null,
                }
            }
            _ => {
                let initial = prefill
                    .as_deref()
                    .map(|conjunct| {
                        let text =
                            quoted_strings(conjunct)
                                .into_iter()
                                .next()
                                .unwrap_or_else(|| {
                                    conjunct.rsplit(' ').next().unwrap_or_default().to_string()
                                });
                        text.trim_matches('%').to_string()
                    })
                    .unwrap_or_default();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("value, %pattern%, > 10, is null")
                        .default_value(initial)
                });
                cx.subscribe_in(
                    &input,
                    window,
                    |this, _, event: &gpui_component::input::InputEvent, _, cx| {
                        if matches!(event, gpui_component::input::InputEvent::PressEnter { .. }) {
                            this.apply_filter_panel(cx);
                        }
                    },
                )
                .detach();
                FilterMode::Text(input)
            }
        };
        panel.mode = mode;
        cx.notify();
    }

    fn apply_filter_panel(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.filter_panel.take() else {
            return;
        };
        let quote = |value: &str, numeric: bool| {
            if numeric {
                value.to_string()
            } else {
                format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
            }
        };
        let prefix = format!("`{}`", panel.column.replace('`', ""));
        let predicate = match &panel.mode {
            FilterMode::Loading => {
                self.filter_panel = Some(panel);
                return;
            }
            FilterMode::Checkboxes { values, null } => {
                let selected: Vec<&String> = values
                    .iter()
                    .filter(|(_, checked)| *checked)
                    .map(|(value, _)| value)
                    .collect();
                let null_checked = null.unwrap_or(false);
                let everything = selected.len() == values.len() && (null.is_none() || null_checked);
                let value_part = if selected.is_empty() || selected.len() == values.len() {
                    None
                } else if selected.len() == 1 {
                    Some(format!("{prefix} = {}", quote(selected[0], panel.numeric)))
                } else {
                    let list = selected
                        .iter()
                        .map(|value| quote(value, panel.numeric))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(format!("{prefix} IN ({list})"))
                };
                if everything || (selected.is_empty() && !null_checked) {
                    // No restriction, or nothing at all selected.
                    None
                } else {
                    match (value_part, null_checked) {
                        (Some(values_predicate), true) => {
                            Some(format!("({values_predicate} OR {prefix} IS NULL)"))
                        }
                        (Some(values_predicate), false) => Some(values_predicate),
                        (None, true) => Some(format!("{prefix} IS NULL")),
                        // All values, null unchecked: exclude the nulls.
                        (None, false) => Some(format!("{prefix} IS NOT NULL")),
                    }
                }
            }
            FilterMode::Text(input) => {
                let text = input.read(cx).value().trim().to_string();
                let lowered = text.to_lowercase();
                if text.is_empty() {
                    None
                } else if lowered == "is null" || lowered == "null" {
                    Some(format!("{prefix} IS NULL"))
                } else if lowered == "is not null" || lowered == "not null" {
                    Some(format!("{prefix} IS NOT NULL"))
                } else if ["<=", ">=", "!=", "<", ">", "="]
                    .iter()
                    .any(|operator| text.starts_with(operator))
                {
                    Some(format!("{prefix} {text}"))
                } else if panel.numeric && text.parse::<f64>().is_ok() {
                    Some(format!("{prefix} = {text}"))
                } else if text.contains('%') {
                    Some(format!("{prefix} LIKE {}", quote(&text, false)))
                } else {
                    Some(format!(
                        "{prefix} LIKE {}",
                        quote(&format!("%{text}%"), false)
                    ))
                }
            }
        };
        // Optimistic indicator; completion re-syncs from the SQL.
        self.filters.retain(|(name, _)| *name != panel.column);
        if let Some(predicate) = &predicate {
            self.filters.push((panel.column.clone(), predicate.clone()));
        }
        cx.emit(GridEvent::FilterRequested {
            column: panel.column,
            predicate,
        });
        cx.notify();
    }

    /// Set the sort indicator to what the executed SQL actually says.
    pub fn set_sort(&mut self, sort: Vec<(String, bool)>, cx: &mut Context<Self>) {
        self.sort = sort;
        cx.notify();
    }

    pub fn finish_result(&mut self, capped: bool, cx: &mut Context<Self>) {
        self.adopt_pending();
        self.result_complete = true;
        self.result_capped = capped;
        cx.notify();
    }

    /// One cell's clipboard text: composites as SQL-pasteable literals
    /// (quoted strings), everything else raw (not the collapsed cell
    /// display).
    fn cell_clipboard(&self, row: usize, col: usize) -> String {
        match self.rows.get(row).and_then(|row| row.get(col)) {
            Some(value @ (Value::Array(_) | Value::Tuple(_) | Value::Map(_))) => literal(value),
            Some(value) => value.to_string(),
            None => String::new(),
        }
    }

    /// Build the clipboard text for the current selection with the
    /// given field delimiter. A single cell copies its bare value; a
    /// region is led by the selected columns' header row so it pastes
    /// into a spreadsheet with labels intact.
    fn selection_delimited(&self, selection: Selection, delim: char) -> String {
        if selection.is_single() {
            let (row, col) = selection.focus;
            return self.cell_clipboard(row, col);
        }
        let mut lines: Vec<String> = Vec::new();
        lines.push(
            selection
                .cols()
                .map(|col| delimited_field(&self.header(col), delim))
                .collect::<Vec<_>>()
                .join(&delim.to_string()),
        );
        for row in selection.rows() {
            lines.push(
                selection
                    .cols()
                    .map(|col| delimited_field(&self.cell_clipboard(row, col), delim))
                    .collect::<Vec<_>>()
                    .join(&delim.to_string()),
            );
        }
        lines.join("\n")
    }

    /// Default copy (cmd-C): tab-separated, so it pastes straight into
    /// Excel / Sheets / Numbers columns (a plain paste splits on tab,
    /// not comma). Real CSV files come from Export; "Copy as CSV" on the
    /// right-click menu covers the explicit-CSV case.
    pub fn copy_selected(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selected else {
            return;
        };
        let text = self.selection_delimited(selection, '\t');
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub fn copy_selected_csv(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selected else {
            return;
        };
        let text = self.selection_delimited(selection, ',');
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn select_all(&mut self, _: &SelectAll, _window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows.len();
        let cols = self.columns.len();
        if rows == 0 || cols == 0 {
            return;
        }
        self.selected = Some(Selection {
            anchor: (0, 0),
            focus: (rows - 1, cols - 1),
        });
        self.inspected = None;
        cx.notify();
    }

    fn width(&self, column: usize) -> f32 {
        self.col_widths.get(column).copied().unwrap_or(COL_WIDTH)
    }

    fn total_width(&self) -> f32 {
        self.col_widths.iter().sum()
    }

    fn header(&self, column: usize) -> String {
        self.columns
            .get(column)
            .map(|column| column.name.clone())
            .unwrap_or_default()
    }

    /// How a temporal cell splits for tinting: (date, rest-of-value).
    fn cell_temporal_parts(&self, row: usize, column: usize) -> Option<(String, String)> {
        match self.rows.get(row).and_then(|row| row.get(column))? {
            Value::Date(date) => Some((date.to_string(), String::new())),
            Value::DateTime(time) => Some((
                time.format("%Y-%m-%d").to_string(),
                time.format(" %H:%M:%S%.f").to_string(),
            )),
            _ => None,
        }
    }

    fn cell_is_null(&self, row: usize, column: usize) -> bool {
        matches!(
            self.rows.get(row).and_then(|row| row.get(column)),
            Some(Value::Null)
        )
    }

    fn column_is_json(&self, column: usize) -> bool {
        self.columns
            .get(column)
            .map(|meta| {
                let name = meta.type_name.trim();
                name == "JSON" || name.starts_with("JSON(")
            })
            .unwrap_or(false)
    }

    /// Compact face for composite cells: short values inline as
    /// literals, long ones as a bracket glyph plus a dim count. The
    /// third field names the grammar that colors an inline face.
    fn cell_composite_parts(
        &self,
        row: usize,
        column: usize,
    ) -> Option<(String, String, Option<&'static str>)> {
        let value = self.rows.get(row)?.get(column)?;
        let (glyph, count, noun) = match value {
            Value::Array(items) => ("[\u{2026}]", items.len(), "items"),
            Value::Tuple(items) => ("(\u{2026})", items.len(), "fields"),
            Value::Map(pairs) => ("{\u{2026}}", pairs.len(), "entries"),
            Value::String(text) if self.column_is_json(column) => {
                return Some(if text.len() <= 60 {
                    (text.clone(), String::new(), Some("json"))
                } else {
                    ("{\u{2026}}".to_string(), " json".to_string(), None)
                });
            }
            // A "type" column whose values parse as ClickHouse types
            // (DESCRIBE, system.columns) colors like the editor.
            Value::String(text)
                if self
                    .columns
                    .get(column)
                    .map(|meta| meta.name == "type")
                    .unwrap_or(false)
                    && zedb_ch::parse_type(text).is_ok() =>
            {
                return Some((
                    crate::type_highlight::collapse(text),
                    String::new(),
                    Some("chtype"),
                ));
            }
            _ => return None,
        };
        let inline = literal(value);
        Some(if inline.len() <= 60 {
            (inline, String::new(), Some("sql"))
        } else {
            (glyph.to_string(), format!(" {count} {noun}"), None)
        })
    }

    /// Tree-sitter runs for `text` under `lang`, via the long-lived
    /// highlighters. Bare literals parse as nothing in the SQL
    /// grammar, so they parse as `SELECT <literal>` with ranges
    /// shifted back over the displayed text.
    fn highlight_runs(
        &mut self,
        lang: &'static str,
        text: &str,
        cx: &App,
    ) -> Option<HighlightRuns> {
        use gpui_component::highlighter::SyntaxHighlighter;
        if lang == "chtype" {
            let runs = crate::type_highlight::runs(text);
            return (!runs.is_empty()).then_some(runs);
        }
        let (parse_text, prefix) = if lang == "sql" {
            (format!("SELECT {text}"), "SELECT ".len())
        } else {
            (text.to_string(), 0)
        };
        let slot = if lang == "sql" {
            &mut self.hl_sql
        } else {
            &mut self.hl_json
        };
        let highlighter = slot.get_or_insert_with(|| SyntaxHighlighter::new(lang));
        highlighter.replace_all(&gpui_component::Rope::from(parse_text.as_str()));
        let highlight_theme = &gpui_component::Theme::global(cx).highlight_theme;
        let runs: HighlightRuns = highlighter
            .styles(&(0..parse_text.len()), highlight_theme)
            .into_iter()
            .filter(|(range, _)| range.end > prefix)
            .map(|(range, style)| {
                (
                    range.start.saturating_sub(prefix)..range.end - prefix,
                    style,
                )
            })
            .collect();
        (!runs.is_empty()).then_some(runs)
    }

    /// Cached per-cell runs; a theme flip invalidates everything.
    fn cell_highlights(
        &mut self,
        row: usize,
        column: usize,
        main: &str,
        lang: &'static str,
        cx: &App,
    ) -> Option<HighlightRuns> {
        let dark = theme::is_dark();
        if dark != self.cache_dark {
            self.highlight_cache.clear();
            self.inspector_cache = None;
            self.cache_dark = dark;
        }
        // A scroll through millions of rows must not hoard runs.
        if self.highlight_cache.len() > 50_000 {
            self.highlight_cache.clear();
        }
        if let Some(cached) = self.highlight_cache.get(&(row, column)) {
            return cached.clone();
        }
        let runs = self.highlight_runs(lang, main, cx);
        self.highlight_cache.insert((row, column), runs.clone());
        runs
    }

    /// Whether clicking this cell opens the inspector overlay.
    fn cell_expandable(&self, row: usize, column: usize) -> bool {
        match self.rows.get(row).and_then(|row| row.get(column)) {
            Some(Value::Array(_) | Value::Tuple(_) | Value::Map(_)) => true,
            Some(Value::String(text)) => {
                self.column_is_json(column) || text.len() > 100 || text.contains('\n')
            }
            _ => false,
        }
    }

    /// The inspector's expanded rendering: composites one element per
    /// line, JSON documents pretty-printed, strings verbatim. The
    /// second field names the tree-sitter grammar to color it with.
    fn inspector_text(&self, row: usize, column: usize) -> (String, Option<&'static str>) {
        let Some(value) = self.rows.get(row).and_then(|row| row.get(column)) else {
            return (String::new(), None);
        };
        match value {
            Value::String(text) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
                    if matches!(
                        parsed,
                        serde_json::Value::Object(_) | serde_json::Value::Array(_)
                    ) {
                        let pretty =
                            serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| text.clone());
                        return (pretty, Some("json"));
                    }
                }
                (text.clone(), None)
            }
            other => {
                let mut out = String::new();
                literal_pretty(other, 0, &mut out);
                // ClickHouse literals are SQL expressions; the SQL
                // grammar colors their strings and numbers.
                (out, Some("sql"))
            }
        }
    }

    fn cell(&self, row: usize, column: usize) -> String {
        let text = self
            .rows
            .get(row)
            .and_then(|row| row.get(column))
            .map(ToString::to_string)
            .unwrap_or_default();
        // Multi-line values (e.g. DESCRIBE's named-tuple types) must
        // not spill over the fixed row height.
        if text.contains('\n') {
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            text
        }
    }
}

mod render;
use render::*;
