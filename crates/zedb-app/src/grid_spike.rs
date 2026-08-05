//! M2 spike: virtualized data grid over synthetic data.
//!
//! Goal: 1M+ rows x 50 columns with only visible cells rendered, smooth
//! scroll, flat memory. Cell content is generated on demand from
//! (row, col); nothing is materialized. Findings go to docs/devlog.md.

use gpui::{
    actions, div, prelude::*, px, rgb, uniform_list, App, ClipboardItem, Context, FocusHandle,
    Focusable, KeyBinding, ListHorizontalSizingBehavior, UniformListScrollHandle, Window,
};

use crate::theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, TEXT, TEXT_DIM};

actions!(grid_spike, [Copy]);

const ROW_HEIGHT: f32 = 24.0;
const COL_WIDTH: f32 = 120.0;

/// Deterministic synthetic table; cells are computed, never stored.
pub struct SyntheticTable {
    pub rows: usize,
    pub cols: usize,
}

impl SyntheticTable {
    fn header(&self, col: usize) -> String {
        format!("col_{col:02}")
    }

    fn cell(&self, row: usize, col: usize) -> String {
        // Cheap deterministic mix resembling real result data.
        let h = (row as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (col as u64).wrapping_mul(0xff51afd7ed558ccd);
        match col % 5 {
            0 => format!("{row}"),
            1 => format!("{}", h % 1_000_000),
            2 => format!("{:.4}", (h % 100_000) as f64 / 1000.0),
            3 => {
                if h.is_multiple_of(7) {
                    "NULL".to_string()
                } else {
                    format!("user_{}", h % 10_000)
                }
            }
            _ => format!("{:016x}", h),
        }
    }
}

pub struct GridSpike {
    table: SyntheticTable,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    selected: Option<(usize, usize)>,
}

impl GridSpike {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new("cmd-c", Copy, None)]);
        Self {
            table: SyntheticTable {
                rows: 1_000_000,
                cols: 50,
            },
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            selected: None,
        }
    }

    fn copy_selected(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some((row, col)) = self.selected {
            let text = self.table.cell(row, col);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Header outside the list, following the list's horizontal offset.
    fn header_row(&self, scroll_x: gpui::Pixels) -> impl IntoElement {
        let cells: Vec<_> = (0..self.table.cols)
            .map(|col| {
                div()
                    .w(px(COL_WIDTH))
                    .flex_none()
                    .px_2()
                    .py_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .border_r_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(TEXT_DIM))
                    .child(self.table.header(col))
            })
            .collect();
        div()
            .flex_none()
            .w_full()
            .h(px(ROW_HEIGHT))
            .overflow_hidden()
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .h_full()
                    .ml(scroll_x)
                    .w(px(COL_WIDTH * self.table.cols as f32))
                    .children(cells),
            )
    }
}

impl Focusable for GridSpike {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GridSpike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cols = self.table.cols;
        let selected = self.selected;

        let list = uniform_list(
            "grid-rows",
            self.table.rows,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|row| {
                        let cells: Vec<_> = (0..cols)
                            .map(|col| {
                                let is_selected = selected == Some((row, col));
                                div()
                                    .id(("cell", row * cols + col))
                                    .w(px(COL_WIDTH))
                                    .flex_none()
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .when(is_selected, |d| d.bg(rgb(0x2f5f8f)))
                                    .on_click(cx.listener(move |this: &mut GridSpike, _, _, cx| {
                                        this.selected = Some((row, col));
                                        cx.notify();
                                    }))
                                    .child(this.table.cell(row, col))
                            })
                            .collect();
                        div()
                            .flex()
                            .h(px(ROW_HEIGHT))
                            .items_center()
                            .w(px(COL_WIDTH * cols as f32))
                            .when(row % 2 == 1, |d| d.bg(rgb(0x21252b)))
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
            .child(format!(
                "{} rows x {} cols (synthetic)",
                self.table.rows, self.table.cols
            ))
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
            .child(self.header_row(scroll_x))
            .child(div().flex_1().w_full().min_h_0().child(list))
            .child(status)
    }
}
