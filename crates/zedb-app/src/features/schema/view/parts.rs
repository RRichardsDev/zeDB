use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// The Parts tab: active parts grouped by partition, with a "too many
    /// parts" warning. Reads the connected node's `system.parts`.
    pub(crate) fn parts_panel(
        &self,
        selected: &SelectedSchemaObject,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // A single partition with this many active parts is worth flagging
        // (ClickHouse delays inserts around 150, throws around 300).
        const TOO_MANY_PARTS: u64 = 100;

        let loading = selected.partitions_loading;
        let error = selected.partitions_error.clone();
        let partitions = selected.partitions.clone().unwrap_or_default();
        let total_parts: u64 = partitions.iter().map(|partition| partition.parts).sum();
        let total_rows: u64 = partitions.iter().map(|partition| partition.rows).sum();
        let total_compressed: u64 = partitions
            .iter()
            .map(|partition| partition.compressed_bytes)
            .sum();
        let busiest = partitions.iter().map(|p| p.parts).max().unwrap_or(0);

        let num_cell = |width: f32, text: String, dim: bool| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .when(dim, |cell| cell.text_color(theme::text_dim()))
                .child(text)
        };
        let header_cell = |width: f32, text: &'static str| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .text_color(theme::text_dim())
                .child(text)
        };

        let rows: Vec<_> = partitions
            .iter()
            .map(|partition| {
                let ratio = if partition.compressed_bytes > 0 {
                    format!(
                        "{:.1}x",
                        partition.uncompressed_bytes as f64 / partition.compressed_bytes as f64
                    )
                } else {
                    "-".to_string()
                };
                let label = if partition.partition == "tuple()" {
                    "(unpartitioned)".to_string()
                } else {
                    partition.partition.clone()
                };
                let hot = partition.parts >= TOO_MANY_PARTS;
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text())
                            .child(label),
                    )
                    .child(
                        num_cell(70.0, Self::format_count(partition.parts), false)
                            .when(hot, |cell| cell.text_color(theme::warning())),
                    )
                    .child(num_cell(110.0, Self::format_count(partition.rows), true))
                    .child(num_cell(
                        110.0,
                        Self::format_bytes(partition.compressed_bytes),
                        true,
                    ))
                    .child(num_cell(
                        110.0,
                        Self::format_bytes(partition.uncompressed_bytes),
                        true,
                    ))
                    .child(num_cell(60.0, ratio, true))
                    .child(num_cell(
                        60.0,
                        Self::format_count(partition.max_level),
                        true,
                    ))
            })
            .collect();

        // Compact count: 42_401_792 -> "42.4M", so a live merge line never
        // wraps.
        let compact = |n: u64| -> String {
            let (value, suffix) = if n >= 1_000_000_000 {
                (n as f64 / 1e9, "B")
            } else if n >= 1_000_000 {
                (n as f64 / 1e6, "M")
            } else if n >= 1_000 {
                (n as f64 / 1e3, "K")
            } else {
                return n.to_string();
            };
            let text = format!("{value:.1}");
            format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
        };

        // Live merges (auto-refreshed by the poll): a thin progress bar and
        // a compact, dot-separated status line. Mutations are tagged.
        let merge_rows: Vec<_> = selected
            .merges
            .iter()
            .map(|merge| {
                let pct = merge.progress_pct.min(100);
                let partition = if merge.partition_id.is_empty() || merge.partition_id == "all" {
                    "(unpartitioned)".to_string()
                } else {
                    merge.partition_id.clone()
                };
                // The stats that ride the right edge; the progress (bar + %)
                // stays grouped with the label on the left.
                let stats = format!(
                    "{}\u{2192}1 \u{b7} {} rows \u{b7} {} \u{b7} {}s",
                    merge.num_parts,
                    compact(merge.rows_written),
                    Self::format_bytes(merge.memory_usage),
                    merge.elapsed_secs,
                );
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .py_1p5()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().text_color(theme::text()).child(partition))
                            .when(merge.is_mutation, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("mutation"),
                                )
                            })
                            .child(
                                div()
                                    .w(px(120.))
                                    .h(px(6.))
                                    .rounded(px(3.))
                                    .bg(theme::border())
                                    .child(
                                        div()
                                            .h_full()
                                            .w(px(120. * pct as f32 / 100.))
                                            .rounded(px(3.))
                                            .bg(theme::accent()),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(40.))
                                    .text_color(theme::text_dim())
                                    .child(format!("{pct}%")),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(stats),
                    )
            })
            .collect();
        let has_merges = !merge_rows.is_empty();

        let body = div()
            .id("object-parts")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2()
            .map(|panel| {
                if loading && partitions.is_empty() {
                    panel.child(
                        div()
                            .py_3()
                            .text_color(theme::text_dim())
                            .child("Loading parts\u{2026}"),
                    )
                } else if let Some(error) = error {
                    panel.child(div().py_3().text_color(theme::danger()).child(error))
                } else if partitions.is_empty() {
                    panel.child(div().py_3().text_color(theme::text_dim()).child(
                        "No active parts. This object stores nothing on disk (a view or \
                             dictionary), or the table is empty.",
                    ))
                } else {
                    panel
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .py_1()
                                .border_b_1()
                                .border_color(theme::border())
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(theme::text_dim())
                                        .child("Partition"),
                                )
                                .child(header_cell(70.0, "Parts"))
                                .child(header_cell(110.0, "Rows"))
                                .child(header_cell(110.0, "Compressed"))
                                .child(header_cell(110.0, "Uncompressed"))
                                .child(header_cell(60.0, "Ratio"))
                                .child(header_cell(60.0, "Level")),
                        )
                        .children(rows)
                }
            });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().text_xs().text_color(theme::text_dim()).child(
                        if partitions.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "{} partition(s) \u{b7} {} active parts \u{b7} {} rows \u{b7} {}",
                                Self::format_count(partitions.len() as u64),
                                Self::format_count(total_parts),
                                Self::format_count(total_rows),
                                Self::format_bytes(total_compressed),
                            )
                        },
                    ))
                    .child(
                        div()
                            .id("refresh-parts")
                            .size(px(22.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .child(
                                svg()
                                    .path("icons/refresh.svg")
                                    .size(px(13.))
                                    .text_color(theme::text_dim()),
                            )
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Refresh").build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(selected) = &mut this.schema.selected_object {
                                    selected.partitions = None;
                                }
                                this.load_partitions(cx);
                            })),
                    ),
            )
            // Live merges (shown only while something is merging).
            .when(has_merges, |panel| {
                panel.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sunken())
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_dim())
                                .pb_1()
                                .child("Merges in progress"),
                        )
                        .children(merge_rows),
                )
            })
            // A single partition with too many parts slows reads and inserts.
            .when(busiest >= TOO_MANY_PARTS, |panel| {
                panel.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(theme::bg_status())
                        .text_xs()
                        .text_color(theme::warning())
                        .child(format!(
                            "A partition has {} active parts. Many small parts slow reads \
                             and inserts; consider OPTIMIZE, fewer partitions, or larger \
                             inserts.",
                            Self::format_count(busiest)
                        )),
                )
            })
            .child(body)
    }
}
