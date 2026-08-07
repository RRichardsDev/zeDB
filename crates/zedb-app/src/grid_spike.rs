//! Virtualized results grid: only visible cells are rendered, so
//! multi-million-row results scroll smoothly with flat memory. Started as
//! the M2 spike; findings are in docs/devlog.md.

use gpui::{
    actions, div, prelude::*, px, rgb, uniform_list, Action, App, ClipboardItem, Context,
    EventEmitter, FocusHandle, Focusable, KeyBinding, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, UniformListScrollHandle, Window,
};
use gpui_component::menu::ContextMenuExt as _;
use zedb_core::{ColumnMeta, Value};

use crate::theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, TEXT, TEXT_DIM};

actions!(grid_spike, [Copy]);

/// A header click asking the owning tab to re-sort the query.
pub enum GridEvent {
    /// The complete desired sort, in priority order; empty clears it.
    SortRequested { sort: Vec<(String, bool)> },
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

const ROW_HEIGHT: f32 = 24.0;
/// Dull orange for the sort arrows and priorities in the header.
const SORT_INDICATOR: u32 = 0xc08a52;
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
    /// The sort the displayed result actually ran with, by column name.
    sort: Vec<(String, bool)>,
    /// A result whose header arrived but whose rows have not: the old
    /// rows stay visible until the replacement starts streaming.
    pending: Option<(Vec<ColumnMeta>, Option<usize>)>,
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
            sort: Vec::new(),
            pending: None,
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

    /// Swap the pending result in, discarding what was displayed.
    fn adopt_pending(&mut self) {
        if let Some((columns, requested_rows)) = self.pending.take() {
            self.col_widths = vec![COL_WIDTH; columns.len()];
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
            cx.emit(GridEvent::SortRequested { sort });
        }
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
                let indicator = position.map(|index| {
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
                });
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
                    .text_color(rgb(TEXT_DIM))
                    .cursor_pointer()
                    .hover(|cell| cell.bg(rgb(0x2a2f37)))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let column = this.header(col);
                        let mut sort = this.sort.clone();
                        if event.modifiers().shift {
                            // Shift-click: add or cycle within the list.
                            match sort.iter().position(|(name, _)| *name == column) {
                                Some(index) if sort[index].1 => sort[index].1 = false,
                                Some(index) => {
                                    sort.remove(index);
                                }
                                None => sort.push((column, true)),
                            }
                        } else {
                            // Plain click: this column only, cycling
                            // ascending, descending, none.
                            sort = match sort.as_slice() {
                                [(name, true)] if *name == column => {
                                    vec![(column, false)]
                                }
                                [(name, false)] if *name == column => Vec::new(),
                                _ => vec![(column, true)],
                            };
                        }
                        cx.emit(GridEvent::SortRequested { sort });
                    }))
                    .context_menu({
                        let column = name.clone();
                        move |menu, window, cx| {
                            let multi = window.modifiers().shift;
                            let label = if multi { "Add to order by" } else { "Order by" };
                            let column = column.clone();
                            menu.submenu(label, window, cx, move |menu, _, _| {
                                menu.menu(
                                    "Ascending",
                                    Box::new(HeaderSort {
                                        column: column.clone(),
                                        direction: 1,
                                        multi,
                                    }),
                                )
                                .menu(
                                    "Descending",
                                    Box::new(HeaderSort {
                                        column: column.clone(),
                                        direction: 2,
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
                            .w(px(7.))
                            .cursor_col_resize()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.resizing_column =
                                        Some((col, this.width(col), f32::from(event.position.x)));
                                    cx.notify();
                                }),
                            ),
                    )
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
                                    .child(this.cell(row, col))
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

        div()
            .size_full()
            .flex()
            .flex_col()
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
                        cx.notify();
                    }
                }),
            )
            .child(self.header_row(scroll_x, cx))
            .child(div().flex_1().w_full().min_h_0().child(list))
            .child(status)
    }
}
