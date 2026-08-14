use super::*;

impl GridSpike {
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
                    .border_color(theme::border())
                    .when(is_filtered, |cell| {
                        cell.border_1().border_color(theme::filter_border())
                    })
                    .text_color(theme::text_dim())
                    .cursor_pointer()
                    .hover(|cell| cell.bg(theme::row_hover()))
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
                                    card = card
                                        .child(div().text_color(theme::text_dim()).child("Sort"));
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
                                                .text_color(theme::sort_indicator())
                                                .child(line),
                                        );
                                    }
                                }
                                if !filters.is_empty() {
                                    card = card.child(
                                        div().text_color(theme::text_dim()).child("Filters"),
                                    );
                                    for (_, conjunct) in &filters {
                                        card = card.child(
                                            // Wrap: the whole filter must
                                            // always be readable. Purple
                                            // matches the header border.
                                            div()
                                                .pl_2()
                                                .max_w(px(400.))
                                                .text_color(theme::filter_tint())
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
                                .text_color(theme::sort_indicator())
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
            .bg(theme::bg_sidebar())
            .border_b_1()
            .border_color(theme::border())
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
                        .bg(theme::bg_sidebar())
                        .text_color(theme::text_dim())
                        .child("running\u{2026}"),
                )
            })
    }
}

/// Whether an existing filter conjunct includes checked NULLs.
pub(super) fn prefill_checks_null(prefill: Option<&str>) -> bool {
    prefill.is_some_and(|conjunct| {
        let upper = conjunct.to_ascii_uppercase();
        upper.contains("IS NULL") && !upper.contains("IS NOT NULL")
    })
}

/// The quoted variant names of an Enum type string.
pub(super) fn enum_variants(type_name: &str) -> Vec<String> {
    quoted_strings(type_name)
}

/// A ClickHouse-literal rendering: strings quoted and escaped, so a
/// composite pastes straight into SQL.
pub(super) fn literal(value: &Value) -> String {
    match value {
        Value::String(text) | Value::Enum(text) => quote_literal(text),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(literal).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Tuple(items) => {
            let inner: Vec<String> = items.iter().map(literal).collect();
            format!("({})", inner.join(", "))
        }
        Value::Map(pairs) => {
            let inner: Vec<String> = pairs
                .iter()
                .map(|(key, value)| format!("{}: {}", literal(key), literal(value)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        other => other.to_string(),
    }
}

fn quote_literal(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Multi-line literal for the inspector: one element per line,
/// indented two spaces per depth; empty composites stay inline.
pub(super) fn literal_pretty(value: &Value, indent: usize, out: &mut String) {
    let open_close = match value {
        Value::Array(items) if !items.is_empty() => Some(("[", "]")),
        Value::Tuple(items) if !items.is_empty() => Some(("(", ")")),
        Value::Map(pairs) if !pairs.is_empty() => Some(("{", "}")),
        _ => None,
    };
    let Some((open, close)) = open_close else {
        out.push_str(&literal(value));
        return;
    };
    let pad = "  ".repeat(indent + 1);
    out.push_str(open);
    match value {
        Value::Map(pairs) => {
            for (index, (key, value)) in pairs.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&pad);
                out.push_str(&literal(key));
                out.push_str(": ");
                literal_pretty(value, indent + 1, out);
            }
        }
        Value::Array(items) | Value::Tuple(items) => {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&pad);
                literal_pretty(item, indent + 1, out);
            }
        }
        _ => unreachable!("only composites reach here"),
    }
    out.push('\n');
    out.push_str(&"  ".repeat(indent));
    out.push_str(close);
}

/// RFC-4180 CSV field: quote when it contains a comma, quote, CR, or
/// LF, doubling any interior quotes.
/// One field for a `delim`-separated row. Quote (RFC-4180 style, which
/// Excel honors on paste too) only when the value contains the
/// delimiter, a quote, or a newline. Used for both TSV (tab) and CSV
/// (comma).
pub(super) fn delimited_field(value: &str, delim: char) -> String {
    if value.contains([delim, '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Every '...'-quoted string in the text, unescaped.
pub(super) fn quoted_strings(text: &str) -> Vec<String> {
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
                                let is_selected = selected.is_some_and(|s| s.contains(row, col));
                                let is_null = this.cell_is_null(row, col);
                                let expandable = this.cell_expandable(row, col);
                                let temporal = this.cell_temporal_parts(row, col);
                                let face = if temporal.is_none() {
                                    this.cell_composite_parts(row, col)
                                } else {
                                    None
                                };
                                let highlights = match &face {
                                    Some((main, _, Some(lang))) => {
                                        this.cell_highlights(row, col, main, lang, cx)
                                    }
                                    _ => None,
                                };
                                div()
                                    .id(("cell", row * cols + col))
                                    .w(px(this.width(col)))
                                    .flex_none()
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .border_r_1()
                                    .border_color(theme::border())
                                    .when(is_null, |d| d.text_color(theme::text_dim()).italic())
                                    .when(is_selected, |d| d.bg(rgb(0x2f5f8f)))
                                    .when(expandable, |d| d.cursor_pointer())
                                    // Mouse-down starts (or, with shift,
                                    // extends) a rectangular selection;
                                    // hovering during a drag stretches it.
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this: &mut GridSpike,
                                                  event: &MouseDownEvent,
                                                  window,
                                                  cx| {
                                                if event.modifiers.shift {
                                                    if let Some(selection) = this.selected.as_mut()
                                                    {
                                                        selection.focus = (row, col);
                                                    } else {
                                                        this.selected =
                                                            Some(Selection::cell((row, col)));
                                                    }
                                                } else {
                                                    this.selected =
                                                        Some(Selection::cell((row, col)));
                                                }
                                                this.selecting = true;
                                                // Take keyboard focus so the
                                                // grid's (now focus-scoped)
                                                // cmd-c / cmd-a reach it.
                                                window.focus(&this.focus_handle);
                                                cx.notify();
                                            },
                                        ),
                                    )
                                    .on_mouse_move(cx.listener(
                                        move |this: &mut GridSpike, _, _, cx| {
                                            if this.selecting {
                                                if let Some(selection) = this.selected.as_mut() {
                                                    if selection.focus != (row, col) {
                                                        selection.focus = (row, col);
                                                        cx.notify();
                                                    }
                                                }
                                            }
                                        },
                                    ))
                                    .on_click(cx.listener(move |this: &mut GridSpike, _, _, cx| {
                                        // A plain click (no drag) opens the
                                        // inspector on an expandable cell.
                                        if this
                                            .selected
                                            .is_some_and(|s| s.is_single() && s.focus == (row, col))
                                        {
                                            this.inspected = expandable.then_some((row, col));
                                            cx.notify();
                                        }
                                    }))
                                    // Right-click selects the cell if it is
                                    // outside the current selection, then opens
                                    // the copy menu; an existing region is kept
                                    // so the menu copies the whole region.
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this: &mut GridSpike, _, window, cx| {
                                            let inside =
                                                this.selected.is_some_and(|s| s.contains(row, col));
                                            if !inside {
                                                this.selected = Some(Selection::cell((row, col)));
                                            }
                                            window.focus(&this.focus_handle);
                                            cx.notify();
                                        }),
                                    )
                                    .context_menu(move |menu, _, _| {
                                        menu.menu("Copy", Box::new(Copy))
                                            .menu("Copy as CSV", Box::new(CopyAsCsv))
                                    })
                                    .map(|d| match temporal {
                                        Some((date, time)) => d
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .text_color(theme::date_tint())
                                                    .child(date),
                                            )
                                            .when(!time.is_empty(), |d| {
                                                d.child(
                                                    div()
                                                        .flex_none()
                                                        .text_color(theme::text_dim())
                                                        .child(time),
                                                )
                                            }),
                                        None => match face {
                                            Some((main, dim, _)) => d
                                                .flex()
                                                .items_center()
                                                .child(
                                                    div().flex_none().child(match highlights {
                                                        Some(runs) => gpui::StyledText::new(main)
                                                            .with_highlights(runs)
                                                            .into_any_element(),
                                                        None => main.into_any_element(),
                                                    }),
                                                )
                                                .when(!dim.is_empty(), |d| {
                                                    d.child(
                                                        div()
                                                            .flex_none()
                                                            .text_xs()
                                                            .text_color(theme::text_dim())
                                                            .child(dim),
                                                    )
                                                }),
                                            None => d.child(this.cell(row, col)),
                                        },
                                    })
                            })
                            .collect();
                        div()
                            .flex()
                            .h(px(ROW_HEIGHT))
                            .items_center()
                            .w(px(this.total_width()))
                            .when(row % 2 == 1, |d| d.bg(theme::row_stripe()))
                            .when(row + 1 == rows, |d| {
                                d.border_b_1().border_color(theme::border())
                            })
                            .hover(|d| d.bg(theme::row_hover()))
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
            .bg(theme::bg_status())
            .border_t_1()
            .border_color(theme::border())
            .text_xs()
            .text_color(theme::text_dim())
            .child(fetch_status)
            .child(match self.selected {
                Some(selection) if selection.is_single() => {
                    let (r, c) = selection.focus;
                    format!("selected {r}:{c} (cmd-c copies)")
                }
                Some(selection) => {
                    let cells = (selection.rows().count()) * (selection.cols().count());
                    format!("{cells} cells selected (cmd-c copies)")
                }
                None => "click or drag to select \u{b7} cmd-a for all".to_string(),
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
                .bg(theme::bg_sidebar())
                .border_1()
                .border_color(theme::border())
                .text_xs()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_color(theme::text_dim())
                        .child(format!("Filter {}", panel.column))
                        .child(
                            div()
                                .id("filter-close")
                                .px_1()
                                .rounded(px(3.))
                                .hover(|close| close.bg(theme::hover()).cursor_pointer())
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
                            .text_color(theme::text_dim())
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
                                .hover(|row| row.bg(theme::row_hover()))
                                .child(div().flex_none().child(glyph))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(is_null_row, |label| {
                                            label.italic().text_color(theme::text_dim())
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
                            .text_color(theme::text_dim())
                            .hover(|button| {
                                button
                                    .bg(theme::hover())
                                    .text_color(theme::text())
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
                            .bg(theme::selected())
                            .text_color(theme::text())
                            .hover(|button| button.bg(rgb(0x37485f)).cursor_pointer())
                            .child("Apply")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.apply_filter_panel(cx);
                            })),
                    ),
            )
        });

        // Inspector content and runs, computed with &mut self before
        // the element closure borrows immutably; runs come from the
        // shared highlighters and cache per open cell.
        let mut inspector_parts: Option<(usize, usize, String, Option<HighlightRuns>)> = None;
        if let Some((row, col)) = self.inspected {
            if row < self.rows.len() && col < self.columns.len() {
                let (text, lang) = self.inspector_text(row, col);
                let runs = match lang.filter(|_| text.len() <= 256 * 1024) {
                    Some(lang) => match &self.inspector_cache {
                        Some((key, cached)) if *key == (row, col) => cached.clone(),
                        _ => {
                            let computed = self.highlight_runs(lang, &text, cx);
                            self.inspector_cache = Some(((row, col), computed.clone()));
                            computed
                        }
                    },
                    None => None,
                };
                inspector_parts = Some((row, col, text, runs));
            }
        }
        let inspector = inspector_parts.and_then(|(_row, col, text, runs)| {
            let column = self.columns.get(col)?;
            let title = column.name.clone();
            let type_name = column.type_name.clone();
            let copy_text = text.clone();
            let body_text: gpui::AnyElement = match runs {
                Some(runs) => gpui::StyledText::new(text.clone())
                    .with_highlights(runs)
                    .into_any_element(),
                None => div().child(text.clone()).into_any_element(),
            };
            Some(
                div()
                    .id("cell-inspector")
                    .occlude()
                    .absolute()
                    .top(px(ROW_HEIGHT + 2.0))
                    .bottom(px(28.))
                    .right_2()
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .rounded(px(4.))
                    .bg(theme::bg_sidebar())
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .py_1()
                            .border_b_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(theme::text()).child(title))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child(crate::type_highlight::styled(&type_name)),
                            )
                            .child(
                                div()
                                    .id("inspector-copy")
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .text_color(theme::text_dim())
                                    .child(
                                        gpui::svg()
                                            .path("icons/copy.svg")
                                            .size(px(12.))
                                            .text_color(theme::text_dim()),
                                    )
                                    .hover(|button| {
                                        button
                                            .bg(theme::hover())
                                            .text_color(theme::text())
                                            .cursor_pointer()
                                    })
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new("Copy value")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_text.clone(),
                                        ));
                                    })),
                            )
                            .child(
                                div()
                                    .id("inspector-close")
                                    .px_1()
                                    .rounded(px(3.))
                                    .text_color(theme::text_dim())
                                    .hover(|close| {
                                        close
                                            .bg(theme::hover())
                                            .text_color(theme::text())
                                            .cursor_pointer()
                                    })
                                    .child("\u{00d7}")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.inspected = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("cell-inspector-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_2()
                            .font_family("Menlo")
                            .text_xs()
                            .text_color(theme::text())
                            .child(body_text),
                    ),
            )
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(theme::bg())
            .text_color(theme::text())
            .text_sm()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.inspected.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy_selected(cx)))
            .on_action(cx.listener(|this, _: &CopyAsCsv, _, cx| this.copy_selected_csv(cx)))
            .on_action(cx.listener(Self::select_all))
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
                    this.selecting = false;
                }),
            )
            .child(self.header_row(scroll_x, cx))
            .child(div().flex_1().w_full().min_h_0().child(list))
            .child(status)
            .when_some(inspector, |root, panel| {
                // Deferred so it paints above the row list; no backdrop,
                // so the grid stays interactive and clicking another
                // expandable cell switches the inspected value.
                root.child(gpui::deferred(panel))
            })
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
