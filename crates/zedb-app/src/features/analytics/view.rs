use gpui::{div, prelude::*, px, Context, MouseButton, MouseDownEvent};
use gpui_component::button::Button;
use zedb_ch::analytics::AnalyticsWindow;

use crate::{theme, Workspace};

fn fmt_ms(ms: f64) -> String {
    if ms.is_nan() {
        // No finished runs: there is no percentile to report.
        "\u{2013}".into()
    } else if ms >= 10_000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else if ms >= 1_000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms > 0.0 && ms < 1.0 {
        "<1 ms".into()
    } else {
        format!("{ms:.0} ms")
    }
}

fn fmt_count(count: u64) -> String {
    if count >= 10_000_000 {
        format!("{:.0}M", count as f64 / 1_000_000.0)
    } else if count >= 10_000 {
        format!("{:.0}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

impl Workspace {
    pub(crate) fn analytics_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let as_of = self
            .analytics
            .fetched_at
            .map(|stamp| format!("as of {}", stamp.format("%H:%M:%S")))
            .unwrap_or_else(|| "loading...".into());
        let window = self.analytics.window;

        let window_button = |id: &'static str, choice: AnalyticsWindow, hours: u32| {
            let active = window == choice;
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(px(3.))
                .border_1()
                .border_color(if active {
                    theme::accent()
                } else {
                    theme::border()
                })
                .text_color(if active {
                    theme::text()
                } else {
                    theme::text_dim()
                })
                .child(choice.label())
                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.analytics_set_window(hours, cx);
                }))
        };

        let header = div()
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_lg()
                    .text_color(theme::text())
                    .child("Query analytics"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(window_button("analytics-1h", AnalyticsWindow::LastHour, 1))
                    .child(window_button("analytics-24h", AnalyticsWindow::LastDay, 24))
                    .child(window_button(
                        "analytics-7d",
                        AnalyticsWindow::LastWeek,
                        168,
                    )),
            )
            .child(div().text_sm().text_color(theme::text_dim()).child(
                match self.view_scope_cluster() {
                    Some(name) => format!("cluster {name}"),
                    None => "this node".to_string(),
                },
            ))
            .child(
                div()
                    .id("analytics-refresh")
                    .debug_selector(|| "analytics-refresh".into())
                    .size(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .child(
                        gpui::svg()
                            .path("icons/refresh.svg")
                            .size(px(13.))
                            .text_color(theme::text_dim()),
                    )
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Refresh").build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.analytics_fetch(cx))),
            )
            .child(div().text_sm().text_color(theme::text_dim()).child(
                if self.analytics.loading {
                    "loading...".to_string()
                } else {
                    as_of
                },
            ));

        let mut list = div()
            .id("analytics-fingerprints")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col();
        if let Some(error) = &self.analytics.error {
            list = list.child(div().p_3().text_color(theme::danger()).child(error.clone()));
        }
        if self.analytics.rows_meta.is_empty()
            && !self.analytics.loading
            && self.analytics.error.is_none()
        {
            list = list.child(
                div()
                    .p_3()
                    .text_color(theme::text_dim())
                    .child("Nothing in the window. Queries land here once query_log has them."),
            );
        }
        // The fingerprint grid: the same results surface as query tabs,
        // so sorting, column filters, selection, and copy all behave
        // identically. Double-click a row to drill in.
        list = list.child(div().flex_1().min_h_0().child(self.analytics.grid.clone()));

        let mut body = div().flex_1().min_h_0().flex();
        body = body.child(div().flex_1().min_w_0().flex().flex_col().child(list));
        if self.analytics.selected.is_some() {
            body = body.child(self.analytics_detail(cx));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
    }

    fn analytics_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sample = self.analytics.selected_shape.clone();

        let mut runs_list = div()
            .id("analytics-runs")
            .flex_none()
            .max_h(px(220.))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if self.analytics.runs_loading {
            runs_list = runs_list.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(theme::text_dim())
                    .child("loading runs..."),
            );
        }
        for (index, run) in self.analytics.runs.iter().enumerate() {
            let query_id = run.query_id.clone();
            let active = self
                .analytics
                .testimony
                .as_ref()
                .is_some_and(|(current, _)| *current == run.query_id);
            let failed = !run.exception.is_empty();
            runs_list = runs_list.child(
                div()
                    .id(gpui::SharedString::from(format!("run-{}", run.query_id)))
                    .debug_selector(move || format!("analytics-run-{index}"))
                    .px_3()
                    .py_0p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .when(active, |row| row.bg(theme::selected()))
                    .hover(|row| row.bg(theme::hover()).cursor_pointer())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme::text_dim())
                            .child(run.at.clone()),
                    )
                    .when(!run.host.is_empty(), |row| {
                        row.child(div().text_color(theme::text_dim()).child(run.host.clone()))
                    })
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .text_right()
                            .child(fmt_ms(run.duration_ms as f64)),
                    )
                    .child(
                        div()
                            .w(px(72.))
                            .flex_none()
                            .text_right()
                            .text_color(theme::text_dim())
                            .child(Self::format_bytes(run.memory)),
                    )
                    .child(
                        div()
                            .w(px(20.))
                            .flex_none()
                            .text_right()
                            .text_color(theme::danger())
                            .child(if failed { "✗" } else { "" }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.analytics_select_run(query_id.clone(), cx);
                    })),
            );
        }

        let mut testimony_block = div()
            .id("analytics-testimony")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px_3()
            .py_2()
            .gap_1();
        if self.analytics.testimony_loading {
            testimony_block = testimony_block.child(
                div()
                    .text_color(theme::text_dim())
                    .child("loading testimony..."),
            );
        }
        if let Some((_, testimony)) = &self.analytics.testimony {
            let headline = format!(
                "{} · {} peak · read {} rows / {} · {} result rows",
                fmt_ms(testimony.duration_ms as f64),
                Self::format_bytes(testimony.memory),
                fmt_count(testimony.read_rows),
                Self::format_bytes(testimony.read_bytes),
                fmt_count(testimony.result_rows),
            );
            testimony_block =
                testimony_block.child(div().text_color(theme::text()).child(headline));
            if !testimony.exception.is_empty() {
                testimony_block = testimony_block.child(
                    div()
                        .text_color(theme::danger())
                        .child(testimony.exception.clone()),
                );
            }
            for (name, value) in testimony.events.iter().take(48) {
                testimony_block = testimony_block.child(
                    div()
                        .flex()
                        .items_center()
                        .text_sm()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(theme::text_dim())
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(theme::text())
                                .child(fmt_count(*value)),
                        ),
                );
            }
        } else if !self.analytics.testimony_loading {
            testimony_block = testimony_block.child(
                div()
                    .text_color(theme::text_dim())
                    .child("Pick a run to see its ProfileEvents testimony."),
            );
        }

        let open_sample = sample.clone();
        div()
            .w(px(self.analytics.detail_width))
            .flex_none()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            .child(gpui::deferred(
                div()
                    .id("analytics-detail-resize")
                    .absolute()
                    .left(px(-6.))
                    .top_0()
                    .bottom_0()
                    .w(px(13.))
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.analytics.resizing_detail = true;
                            cx.notify();
                        }),
                    ),
            ))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        Button::new("analytics-open-sample")
                            .label("Open in editor")
                            .compact()
                            .outline()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.analytics_open_sample(open_sample.clone(), window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("analytics-detail-close")
                            .px_2()
                            .rounded(px(3.))
                            .text_color(theme::text_dim())
                            .child("✕")
                            .hover(|button| button.text_color(theme::text()).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.analytics.selected = None;
                                this.analytics.runs.clear();
                                this.analytics.testimony = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("analytics-sample")
                    .flex_none()
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(theme::text())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(sample),
            )
            .child(runs_list)
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_sm()
                    .text_color(theme::text_dim())
                    .border_t_1()
                    .border_b_1()
                    .border_color(theme::border())
                    .child("testimony"),
            )
            .child(testimony_block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds_render_honestly() {
        assert_eq!(fmt_ms(f64::NAN), "\u{2013}", "no runs, no percentile");
        assert_eq!(fmt_ms(0.0), "0 ms");
        assert_eq!(fmt_ms(0.4), "<1 ms");
        assert_eq!(fmt_ms(42.0), "42 ms");
        assert_eq!(fmt_ms(1_500.0), "1.50 s");
        assert_eq!(fmt_ms(12_500.0), "12.5 s");
    }
}
