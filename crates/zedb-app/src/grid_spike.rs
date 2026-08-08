//! Virtualized results grid: only visible cells are rendered, so
//! multi-million-row results scroll smoothly with flat memory. Started as
//! the M2 spike; findings are in docs/devlog.md.

use gpui::Entity;
use gpui::{
    actions, div, prelude::*, px, rgb, uniform_list, Action, App, ClipboardItem, Context,
    EventEmitter, FocusHandle, Focusable, KeyBinding, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, UniformListScrollHandle, Window,
};
use gpui_component::input::{Input, InputState};
use gpui_component::menu::ContextMenuExt as _;
use gpui_component::Sizable as _;
use std::collections::HashMap;
use zedb_core::{ColumnMeta, Value};

use crate::theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, TEXT, TEXT_DIM};

actions!(grid_spike, [Copy]);

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
/// Dull orange for the sort arrows and priorities in the header.
const SORT_INDICATOR: u32 = 0xc08a52;
/// Muted purple ring around a filtered column's header.
const FILTER_BORDER: u32 = 0x6f5b99;
/// Brighter cousin of the border for filter text in hover cards.
const FILTER_TINT: u32 = 0x9d84cc;
/// Muted red for the date part of temporal values.
const DATE_TINT: u32 = 0xb56b6b;
const COL_WIDTH: f32 = 120.0;

pub struct GridSpike {
    columns: Vec<ColumnMeta>,
    rows: Vec<Vec<Value>>,
    requested_rows: Option<usize>,
    result_complete: bool,
    result_capped: bool,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    selected: Option<(usize, usize)>,
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
}

impl GridSpike {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new("cmd-c", Copy, None)]);
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            requested_rows: None,
            result_complete: false,
            result_capped: false,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            selected: None,
            col_widths: Vec::new(),
            resizing_column: None,
            just_resized: false,
            sort: Vec::new(),
            pending: None,
            filters: Vec::new(),
            width_memory: HashMap::new(),
            filter_panel: None,
        }
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
    fn adopt_pending(&mut self) {
        if let Some((columns, requested_rows)) = self.pending.take() {
            self.col_widths = self
                .width_memory
                .get(&Self::width_key(&columns))
                .filter(|saved| saved.len() == columns.len())
                .cloned()
                .unwrap_or_else(|| vec![COL_WIDTH; columns.len()]);
            self.columns = columns;
            self.rows = Vec::new();
            self.requested_rows = requested_rows;
            self.result_complete = false;
            self.result_capped = false;
            self.selected = None;
            self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        }
    }

    pub fn append_rows(&mut self, batch: Vec<Vec<Value>>, cx: &mut Context<Self>) {
        self.adopt_pending();
        self.rows.extend(batch);
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

    fn copy_selected(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some((row, col)) = self.selected {
            let text = self.cell(row, col);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
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

    fn cell(&self, row: usize, column: usize) -> String {
        self.rows
            .get(row)
            .and_then(|row| row.get(column))
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    /// Header outside the list, following the list's horizontal offset.
    fn header_row(&self, scroll_x: gpui::Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let column_count = self.columns.len();
        let cells: Vec<_> = (0..column_count)
            .map(|col| {
                let name = self.header(col);
                let position = self.sort.iter().position(|(column, _)| *column == name);
                let indicator = position
                    .map(|index| {
                        let arrow = if self.sort[index].1 {
                            '\u{25b4}'
                        } else {
                            '\u{25be}'
                        };
                        if self.sort.len() > 1 {
                            format!("{arrow}{}", index + 1)
                        } else {
                            arrow.to_string()
                        }
                    })
                    .unwrap_or_default();
                // The purple header border is the filter signal.
                let is_filtered = self.filters.iter().any(|(column, _)| *column == name);
                let indicator = (!indicator.is_empty()).then_some(indicator);
                div()
                    .id(("col-head", col))
                    .w(px(self.width(col)))
                    .flex_none()
                    .relative()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_r_1()
                    .border_color(rgb(BORDER))
                    .when(is_filtered, |cell| {
                        cell.border_1().border_color(rgb(FILTER_BORDER))
                    })
                    .text_color(rgb(TEXT_DIM))
                    .cursor_pointer()
                    .hover(|cell| cell.bg(rgb(0x2a2f37)))
                    .tooltip({
                        // Only this column's modifiers, with their overall
                        // sort priority preserved.
                        let sort: Vec<(usize, String, bool)> = self
                            .sort
                            .iter()
                            .enumerate()
                            .filter(|(_, (column, _))| *column == name)
                            .map(|(index, (column, ascending))| (index, column.clone(), *ascending))
                            .collect();
                        let multi = self.sort.len() > 1;
                        let filters: Vec<(String, String)> = self
                            .filters
                            .iter()
                            .filter(|(column, _)| *column == name)
                            .cloned()
                            .collect();
                        move |window, cx| {
                            if sort.is_empty() && filters.is_empty() {
                                return gpui_component::tooltip::Tooltip::new(
                                    "Click to sort, right-click to filter",
                                )
                                .build(window, cx);
                            }
                            let sort = sort.clone();
                            let filters = filters.clone();
                            gpui_component::tooltip::Tooltip::element(move |_, _| {
                                let mut card =
                                    div().flex().flex_col().gap_0p5().max_w(px(420.)).text_xs();
                                if !sort.is_empty() {
                                    card =
                                        card.child(div().text_color(rgb(TEXT_DIM)).child("Sort"));
                                    for (index, column, ascending) in sort.iter() {
                                        let arrow =
                                            if *ascending { '\u{25b4}' } else { '\u{25be}' };
                                        let line = if multi {
                                            format!("{}. {column} {arrow}", index + 1)
                                        } else {
                                            format!("{column} {arrow}")
                                        };
                                        card = card.child(
                                            div()
                                                .pl_2()
                                                .text_color(rgb(SORT_INDICATOR))
                                                .child(line),
                                        );
                                    }
                                }
                                if !filters.is_empty() {
                                    card = card
                                        .child(div().text_color(rgb(TEXT_DIM)).child("Filters"));
                                    for (_, conjunct) in &filters {
                                        card = card.child(
                                            // Wrap: the whole filter must
                                            // always be readable. Purple
                                            // matches the header border.
                                            div()
                                                .pl_2()
                                                .max_w(px(400.))
                                                .text_color(rgb(FILTER_TINT))
                                                .child(conjunct.clone()),
                                        );
                                    }
                                }
                                card
                            })
                            .build(window, cx)
                        }
                    })
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        if std::mem::take(&mut this.just_resized) {
                            return;
                        }
                        let column = this.header(col);
                        let mut sort = this.sort.clone();
                        if event.modifiers().shift {
                            // Shift-click: add or cycle within the list,
                            // descending first.
                            match sort.iter().position(|(name, _)| *name == column) {
                                Some(index) if !sort[index].1 => sort[index].1 = true,
                                Some(index) => {
                                    sort.remove(index);
                                }
                                None => sort.push((column, false)),
                            }
                        } else {
                            // Plain click: this column only, cycling
                            // descending, ascending, none.
                            sort = match sort.as_slice() {
                                [(name, false)] if *name == column => {
                                    vec![(column, true)]
                                }
                                [(name, true)] if *name == column => Vec::new(),
                                _ => vec![(column, false)],
                            };
                        }
                        this.sort = sort.clone();
                        cx.emit(GridEvent::SortRequested { sort });
                        cx.notify();
                    }))
                    .context_menu({
                        let column = name.clone();
                        move |menu, window, cx| {
                            let multi = window.modifiers().shift;
                            let label = if multi { "Add to order by" } else { "Order by" };
                            let column = column.clone();
                            let filter_column = column.clone();
                            menu.submenu(label, window, cx, move |menu, _, _| {
                                menu.menu(
                                    "Descending",
                                    Box::new(HeaderSort {
                                        column: column.clone(),
                                        direction: 2,
                                        multi,
                                    }),
                                )
                                .menu(
                                    "Ascending",
                                    Box::new(HeaderSort {
                                        column: column.clone(),
                                        direction: 1,
                                        multi,
                                    }),
                                )
                                .separator()
                                .menu(
                                    "Clear",
                                    Box::new(HeaderSort {
                                        column: column.clone(),
                                        direction: 0,
                                        multi,
                                    }),
                                )
                            })
                            .menu(
                                "Filter\u{2026}",
                                Box::new(HeaderFilter {
                                    column: filter_column,
                                }),
                            )
                        }
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(name.clone()),
                    )
                    .when_some(indicator, |cell, indicator| {
                        cell.child(
                            div()
                                .flex_none()
                                // Clear the resize handle on the right edge.
                                .pr(px(6.))
                                .text_color(rgb(SORT_INDICATOR))
                                .child(indicator),
                        )
                    })
                    .child(
                        div()
                            .id(("col-resize", col))
                            .absolute()
                            .right_0()
                            .top_0()
                            .h_full()
                            .w(px(10.))
                            .cursor_col_resize()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.resizing_column =
                                        Some((col, this.width(col), f32::from(event.position.x)));
                                    cx.notify();
                                }),
                            ),
                    )
                    .when(col > 0, |cell| {
                        // The previous divider is grabbable from this side
                        // too, doubling the effective target.
                        cell.child(
                            div()
                                .id(("col-resize-left", col))
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(px(10.))
                                .cursor_col_resize()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.resizing_column = Some((
                                            col - 1,
                                            this.width(col - 1),
                                            f32::from(event.position.x),
                                        ));
                                        cx.notify();
                                    }),
                                ),
                        )
                    })
            })
            .collect();
        div()
            .flex_none()
            .w_full()
            .h(px(ROW_HEIGHT))
            .relative()
            .overflow_hidden()
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .h_full()
                    .ml(scroll_x)
                    .w(px(self.total_width()))
                    .children(cells),
            )
            .when(self.pending.is_some(), |header| {
                header.child(
                    div()
                        .absolute()
                        .right_2()
                        .top_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .px_2()
                        .bg(rgb(BG_SIDEBAR))
                        .text_color(rgb(TEXT_DIM))
                        .child("running\u{2026}"),
                )
            })
    }
}

/// Whether an existing filter conjunct includes checked NULLs.
fn prefill_checks_null(prefill: Option<&str>) -> bool {
    prefill.is_some_and(|conjunct| {
        let upper = conjunct.to_ascii_uppercase();
        upper.contains("IS NULL") && !upper.contains("IS NOT NULL")
    })
}

/// The quoted variant names of an Enum type string.
fn enum_variants(type_name: &str) -> Vec<String> {
    quoted_strings(type_name)
}

/// Every '...'-quoted string in the text, unescaped.
fn quoted_strings(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\'' {
            continue;
        }
        let mut value = String::new();
        while let Some(character) = chars.next() {
            match character {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        value.push(escaped);
                    }
                }
                '\'' => break,
                other => value.push(other),
            }
        }
        values.push(value);
    }
    values
}

impl EventEmitter<GridEvent> for GridSpike {}

impl Focusable for GridSpike {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GridSpike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cols = self.columns.len();
        let rows = self.rows.len();
        let selected = self.selected;

        let list = uniform_list(
            "grid-rows",
            rows,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|row| {
                        let cells: Vec<_> = (0..cols)
                            .map(|col| {
                                let is_selected = selected == Some((row, col));
                                let is_null = this.cell_is_null(row, col);
                                div()
                                    .id(("cell", row * cols + col))
                                    .w(px(this.width(col)))
                                    .flex_none()
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .when(is_null, |d| d.text_color(rgb(TEXT_DIM)).italic())
                                    .when(is_selected, |d| d.bg(rgb(0x2f5f8f)))
                                    .on_click(cx.listener(move |this: &mut GridSpike, _, _, cx| {
                                        this.selected = Some((row, col));
                                        cx.notify();
                                    }))
                                    .map(|d| match this.cell_temporal_parts(row, col) {
                                        Some((date, time)) => d
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .text_color(rgb(DATE_TINT))
                                                    .child(date),
                                            )
                                            .when(!time.is_empty(), |d| {
                                                d.child(
                                                    div()
                                                        .flex_none()
                                                        .text_color(rgb(TEXT_DIM))
                                                        .child(time),
                                                )
                                            }),
                                        None => d.child(this.cell(row, col)),
                                    })
                            })
                            .collect();
                        div()
                            .flex()
                            .h(px(ROW_HEIGHT))
                            .items_center()
                            .w(px(this.total_width()))
                            .when(row % 2 == 1, |d| d.bg(rgb(0x21252b)))
                            .when(row + 1 == rows, |d| {
                                d.border_b_1().border_color(rgb(BORDER))
                            })
                            .hover(|d| d.bg(rgb(0x2a2f37)))
                            .children(cells)
                    })
                    .collect()
            }),
        )
        .track_scroll(self.scroll.clone())
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .h_full()
        .flex_grow();

        let fetched = self.rows.len();
        let fetch_status = if self.result_capped {
            format!(
                "Fetched {fetched} of {} requested rows, more available",
                self.requested_rows.unwrap_or(fetched)
            )
        } else if self.result_complete {
            format!("Fetched {fetched} of {fetched} rows")
        } else {
            format!("Fetched {fetched} rows")
        };
        let status = div()
            .h(px(24.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .bg(rgb(BG_STATUS))
            .border_t_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .text_color(rgb(TEXT_DIM))
            .child(fetch_status)
            .child(match self.selected {
                Some((r, c)) => format!("selected {r}:{c} (cmd-c copies)"),
                None => "click a cell to select".to_string(),
            });

        // Mirror the list's horizontal offset, clamped to its scrollable
        // range: the raw handle offset overshoots during overscroll.
        let scroll_x = {
            let state = self.scroll.0.borrow();
            let x = state.base_handle.offset().x;
            match state.last_item_size {
                Some(size) => {
                    let min_x = -(size.contents.width - size.item.width).max(px(0.));
                    x.max(min_x).min(px(0.))
                }
                None => x,
            }
        };

        let filter_panel = self.filter_panel.as_ref().map(|panel| {
            let column_index = self
                .columns
                .iter()
                .position(|meta| meta.name == panel.column)
                .unwrap_or(0);
            let offset: f32 = (0..column_index).map(|col| self.width(col)).sum();
            let left = (offset + f32::from(scroll_x)).max(0.0);
            let mut body = div()
                .id("filter-panel-body")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.filter_panel = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .absolute()
                .left(px(left))
                .top(px(ROW_HEIGHT + 2.0))
                .w(px(230.))
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .rounded(px(4.))
                .bg(rgb(BG_SIDEBAR))
                .border_1()
                .border_color(rgb(BORDER))
                .text_xs()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_color(rgb(TEXT_DIM))
                        .child(format!("Filter {}", panel.column))
                        .child(
                            div()
                                .id("filter-close")
                                .px_1()
                                .rounded(px(3.))
                                .hover(|close| close.bg(rgb(0x303640)).cursor_pointer())
                                .child("\u{00d7}")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.filter_panel = None;
                                    cx.notify();
                                })),
                        ),
                );
            match &panel.mode {
                FilterMode::Loading => {
                    body = body.child(
                        div()
                            .py_1()
                            .text_color(rgb(TEXT_DIM))
                            .child("checking distinct values\u{2026}"),
                    );
                }
                FilterMode::Checkboxes { values, null } => {
                    let mut rows: Vec<(usize, String, bool, bool)> = values
                        .iter()
                        .enumerate()
                        .map(|(index, (value, checked))| (index, value.clone(), *checked, false))
                        .collect();
                    if let Some(null_checked) = null {
                        rows.push((usize::MAX, "(null)".into(), *null_checked, true));
                    }
                    for (index, value, checked, is_null_row) in rows {
                        let glyph = if checked { "\u{2611}" } else { "\u{2610}" };
                        body = body.child(
                            div()
                                .id(("filter-check", index.min(100_000)))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_1()
                                .rounded(px(3.))
                                .cursor_pointer()
                                .hover(|row| row.bg(rgb(0x2a2f37)))
                                .child(div().flex_none().child(glyph))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(is_null_row, |label| {
                                            label.italic().text_color(rgb(TEXT_DIM))
                                        })
                                        .child(value),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(panel) = this.filter_panel.as_mut() {
                                        if let FilterMode::Checkboxes { values, null } =
                                            &mut panel.mode
                                        {
                                            if is_null_row {
                                                if let Some(null_checked) = null {
                                                    *null_checked = !*null_checked;
                                                }
                                            } else if let Some(entry) = values.get_mut(index) {
                                                entry.1 = !entry.1;
                                            }
                                        }
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                }
                FilterMode::Text(input) => {
                    body = body.child(Input::new(input).small());
                }
            }
            body.child(
                div()
                    .flex()
                    .justify_end()
                    .gap_1()
                    .pt_1()
                    .child(
                        div()
                            .id("filter-clear")
                            .px_2()
                            .py_0p5()
                            .rounded(px(3.))
                            .text_color(rgb(TEXT_DIM))
                            .hover(|button| {
                                button
                                    .bg(rgb(0x303640))
                                    .text_color(rgb(TEXT))
                                    .cursor_pointer()
                            })
                            .child("Clear")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(panel) = this.filter_panel.take() {
                                    this.filters.retain(|(name, _)| *name != panel.column);
                                    cx.emit(GridEvent::FilterRequested {
                                        column: panel.column,
                                        predicate: None,
                                    });
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("filter-apply")
                            .px_2()
                            .py_0p5()
                            .rounded(px(3.))
                            .bg(rgb(0x2c3a4d))
                            .text_color(rgb(TEXT))
                            .hover(|button| button.bg(rgb(0x37485f)).cursor_pointer())
                            .child("Apply")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.apply_filter_panel(cx);
                            })),
                    ),
            )
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::copy_selected))
            // The list consumes wheel events; repaint so the header can
            // mirror its horizontal offset.
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some((col, start_width, start_x)) = this.resizing_column {
                    let width =
                        (start_width + f32::from(event.position.x) - start_x).clamp(48.0, 1200.0);
                    if let Some(slot) = this.col_widths.get_mut(col) {
                        *slot = width;
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.resizing_column.take().is_some() {
                        // Same column set, same widths next time; and the
                        // release must not read as a sort click.
                        this.just_resized = true;
                        this.width_memory
                            .insert(Self::width_key(&this.columns), this.col_widths.clone());
                        cx.notify();
                    }
                }),
            )
            .child(self.header_row(scroll_x, cx))
            .child(div().flex_1().w_full().min_h_0().child(list))
            .child(status)
            .when_some(filter_panel, |root, panel| {
                // Deferred so the popover paints above the row list; the
                // backdrop dismisses on click-away within the grid.
                root.child(gpui::deferred(
                    div()
                        .id("filter-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.filter_panel = None;
                                cx.notify();
                            }),
                        )
                        .child(panel),
                ))
            })
    }
}
