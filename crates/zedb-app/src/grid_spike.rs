//! Virtualized results grid: only visible cells are rendered, so
//! multi-million-row results scroll smoothly with flat memory. Started as
//! the M2 spike; findings are in docs/devlog.md.

use gpui::{
    actions, div, prelude::*, px, rgb, uniform_list, App, ClipboardItem, Context, EventEmitter,
    FocusHandle, Focusable, KeyBinding, ListHorizontalSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, UniformListScrollHandle, Window,
};
use zedb_core::{ColumnMeta, Value};

use crate::theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, TEXT, TEXT_DIM};

actions!(grid_spike, [Copy]);

/// A header click asking the owning tab to re-sort the query.
pub enum GridEvent {
    /// The complete desired sort, in priority order; empty clears it.
    SortRequested { sort: Vec<(String, bool)> },
}

const ROW_HEIGHT: f32 = 24.0;
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
                let label = match position.map(|index| (index, self.sort[index].1)) {
                    Some((index, ascending)) => {
                        let arrow = if ascending { '\u{25b4}' } else { '\u{25be}' };
                        if self.sort.len() > 1 {
                            format!("{name} {arrow}{}", index + 1)
                        } else {
                            format!("{name} {arrow}")
                        }
                    }
                    None => name.clone(),
                };
                div()
                    .id(("col-head", col))
                    .w(px(self.width(col)))
                    .flex_none()
                    .relative()
                    .px_2()
                    .py_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
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
                    .child(label)
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
