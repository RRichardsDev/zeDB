use gpui::{div, prelude::*, px, Context, MouseButton, MouseDownEvent};
use gpui_component::{
    button::Button,
    menu::{DropdownMenu, PopupMenu},
};
use zedb_ch::analytics::AnalyticsWindow;

use super::model::*;
use crate::{theme, Workspace};

fn fmt_ms(ms: f64) -> String {
    if ms >= 10_000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else if ms >= 1_000.0 {
        format!("{:.2} s", ms / 1000.0)
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
        let scope_options = self.ops_cluster_options();
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
            .when(!scope_options.is_empty(), |header| {
                let label = match self.analytics.scope.cluster() {
                    Some(name) => format!("Cluster: {name}"),
                    None => "This node".to_string(),
                };
                header.child(
                    Button::new("analytics-scope")
                        .label(label)
                        .dropdown_caret(true)
                        .compact()
                        .outline()
                        .dropdown_menu(move |menu: PopupMenu, _, _| {
                            let menu = menu
                                .min_w(px(160.))
                                .menu("This node", Box::new(SetAnalyticsScope { cluster: None }));
                            scope_options.iter().fold(menu, |menu, name| {
                                menu.menu(
                                    format!("Cluster: {name}"),
                                    Box::new(SetAnalyticsScope {
                                        cluster: Some(name.clone()),
                                    }),
                                )
                            })
                        }),
                )
            })
            .child(
                Button::new("analytics-refresh")
                    .label("Refresh")
                    .compact()
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| this.analytics_fetch(cx))),
            )
            .child(div().text_sm().text_color(theme::text_dim()).child(
                if self.analytics.loading {
                    "loading...".to_string()
                } else {
                    as_of
                },
            ));

        let column = |width: f32, label: &'static str| {
            div().w(px(width)).flex_none().text_right().child(label)
        };
        let list_header = div()
            .flex_none()
            .px_3()
            .py_1()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .text_color(theme::text_dim())
            .border_b_1()
            .border_color(theme::border())
            .child(div().flex_1().min_w_0().child("query shape"))
            .child(column(52., "runs"))
            .child(column(44., "err"))
            .child(column(64., "p50"))
            .child(column(64., "p95"))
            .child(column(64., "p99"))
            .child(column(72., "total"))
            .child(column(72., "peak mem"))
            .child(column(72., "read"));

        let mut list = div()
            .id("analytics-fingerprints")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if let Some(error) = &self.analytics.error {
            list = list.child(div().p_3().text_color(theme::danger()).child(error.clone()));
        }
        if self.analytics.fingerprints.is_empty()
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
        for fingerprint in &self.analytics.fingerprints {
            let hash = fingerprint.hash.clone();
            let selected = self.analytics.selected.as_deref() == Some(fingerprint.hash.as_str());
            let value = |text: String, width: f32| {
                div()
                    .w(px(width))
                    .flex_none()
                    .text_right()
                    .text_color(theme::text_dim())
                    .child(text)
            };
            list = list.child(
                div()
                    .id(gpui::SharedString::from(format!("fp-{}", fingerprint.hash)))
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .when(selected, |row| row.bg(theme::selected()))
                    .hover(|row| row.bg(theme::hover()).cursor_pointer())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(theme::text())
                            .child(fingerprint.sample.clone()),
                    )
                    .child(value(fmt_count(fingerprint.runs), 52.))
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .text_right()
                            .text_color(if fingerprint.errors > 0 {
                                theme::danger()
                            } else {
                                theme::text_dim()
                            })
                            .child(fmt_count(fingerprint.errors)),
                    )
                    .child(value(fmt_ms(fingerprint.p50_ms), 64.))
                    .child(value(fmt_ms(fingerprint.p95_ms), 64.))
                    .child(value(fmt_ms(fingerprint.p99_ms), 64.))
                    .child(value(fmt_ms(fingerprint.total_ms as f64), 72.))
                    .child(value(Self::format_bytes(fingerprint.max_memory), 72.))
                    .child(value(Self::format_bytes(fingerprint.read_bytes), 72.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.analytics_select(hash.clone(), cx);
                    })),
            );
        }

        let mut body = div().flex_1().min_h_0().flex();
        body = body.child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(list_header)
                .child(list),
        );
        if self.analytics.selected.is_some() {
            body = body.child(self.analytics_detail(cx));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, action: &SetAnalyticsScope, _, cx| {
                this.analytics_set_scope(action.cluster.clone(), cx);
            }))
            .on_action(cx.listener(|this, action: &SetAnalyticsWindow, _, cx| {
                this.analytics_set_window(action.hours, cx);
            }))
            .child(header)
            .child(body)
    }

    fn analytics_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sample = self
            .analytics
            .selected
            .as_deref()
            .and_then(|hash| {
                self.analytics
                    .fingerprints
                    .iter()
                    .find(|fingerprint| fingerprint.hash == hash)
            })
            .map(|fingerprint| fingerprint.sample.clone())
            .unwrap_or_default();

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
        for run in &self.analytics.runs {
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
