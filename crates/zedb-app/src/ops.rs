//! The ops view (docs/PHASE-6.md M1): what is this cluster doing
//! right now. Small capped SELECTs against system tables, polled only
//! while the view is visible; read-only by construction except KILL
//! QUERY, which follows the connection's write posture.

use std::time::Duration;

use gpui::{div, prelude::*, px, Context, Timer};
use zedb_core::Value;

use crate::theme;
use crate::{rt, Workspace};

const POLL_SECS: u64 = 2;

#[derive(Clone, Debug)]
pub struct OpsProcess {
    pub query_id: String,
    pub user: String,
    pub elapsed_secs: f64,
    pub read_rows: u64,
    pub read_bytes: u64,
    pub total_rows: u64,
    pub memory_bytes: u64,
    pub query: String,
}

#[derive(Default)]
pub struct OpsState {
    pub processes: Vec<OpsProcess>,
    /// Wall-clock stamp of the last successful fetch.
    pub as_of: Option<chrono::DateTime<chrono::Local>>,
    pub error: Option<String>,
    pub poll_generation: u64,
    pub fetch_in_flight: bool,
    /// query_id currently being killed (disables its button).
    pub killing: Option<String>,
}

fn number(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::UInt(number)) => *number,
        Some(Value::Int(number)) => (*number).max(0) as u64,
        Some(Value::Float(number)) => number.max(0.0) as u64,
        _ => 0,
    }
}

fn float(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Float(number)) => *number,
        Some(Value::UInt(number)) => *number as f64,
        Some(Value::Int(number)) => *number as f64,
        _ => 0.0,
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

fn format_elapsed(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else if seconds < 3600.0 {
        format!(
            "{}m {:02}s",
            (seconds / 60.0) as u64,
            (seconds % 60.0) as u64
        )
    } else {
        format!(
            "{}h {:02}m",
            (seconds / 3600.0) as u64,
            ((seconds % 3600.0) / 60.0) as u64
        )
    }
}

impl Workspace {
    pub(crate) fn ops_toggle(&mut self, cx: &mut Context<Self>) {
        if self.connected.is_none() {
            self.flash_warning("Connect to a cluster to see its ops view", cx);
            return;
        }
        self.show_ops = !self.show_ops;
        if self.show_ops {
            self.show_query_editor = false;
            self.show_fleet = false;
            self.ops_start_poll(cx);
        }
        cx.notify();
    }

    /// Fetch immediately, then every POLL_SECS while the view stays
    /// visible. Generation-guarded like the health poll; hiding the
    /// view or reconnecting ends the loop.
    fn ops_start_poll(&mut self, cx: &mut Context<Self>) {
        self.ops.poll_generation += 1;
        let generation = self.ops.poll_generation;
        self.ops_fetch(cx);
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_secs(POLL_SECS)).await;
            let live = this
                .update(cx, |this, cx| {
                    let live = this.ops.poll_generation == generation
                        && this.show_ops
                        && this.connected.is_some();
                    if live {
                        this.ops_fetch(cx);
                    }
                    live
                })
                .unwrap_or(false);
            if !live {
                break;
            }
        })
        .detach();
    }

    fn ops_fetch(&mut self, cx: &mut Context<Self>) {
        if self.ops.fetch_in_flight {
            return;
        }
        let Some(connected) = &self.connected else {
            return;
        };
        self.ops.fetch_in_flight = true;
        let config = connected.client_config.clone();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            client
                .query(
                    "SELECT query_id, user, elapsed, read_rows, read_bytes, \
                        total_rows_approx, memory_usage, query \
                     FROM system.processes \
                     WHERE query NOT LIKE '%system.processes%' \
                     ORDER BY elapsed DESC \
                     LIMIT 50",
                )
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.fetch_in_flight = false;
                match result {
                    Ok(Ok(result)) => {
                        this.ops.processes = result
                            .rows
                            .iter()
                            .map(|row| OpsProcess {
                                query_id: text(row.first()),
                                user: text(row.get(1)),
                                elapsed_secs: float(row.get(2)),
                                read_rows: number(row.get(3)),
                                read_bytes: number(row.get(4)),
                                total_rows: number(row.get(5)),
                                memory_bytes: number(row.get(6)),
                                query: text(row.get(7)),
                            })
                            .collect();
                        this.ops.as_of = Some(chrono::Local::now());
                        this.ops.error = None;
                    }
                    Ok(Err(error)) => this.ops.error = Some(error.to_string()),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn ops_kill(&mut self, query_id: String, cx: &mut Context<Self>) {
        let Some(connected) = &self.connected else {
            return;
        };
        if connected.client_config.read_only {
            self.flash_warning("This connection is read-only; KILL QUERY needs write", cx);
            return;
        }
        self.ops.killing = Some(query_id.clone());
        let config = connected.client_config.clone();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let escaped = query_id.replace('\'', "''");
            client
                .query(&format!("KILL QUERY WHERE query_id = '{escaped}'"))
                .await
                .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.killing = None;
                if let Ok(Err(error)) = result {
                    this.flash_warning(format!("KILL QUERY failed: {error}"), cx);
                }
                this.ops_fetch(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn ops_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let read_only = self
            .connected
            .as_ref()
            .map(|connected| connected.client_config.read_only)
            .unwrap_or(true);
        let as_of = self
            .ops
            .as_of
            .map(|stamp| format!("as of {}", stamp.format("%H:%M:%S")))
            .unwrap_or_else(|| "loading...".into());

        let header = div()
            .flex_none()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_lg().text_color(theme::text()).child("Ops"))
            .child(div().text_sm().text_color(theme::text_dim()).child(format!(
                "queries now \u{b7} {as_of} \u{b7} refreshes every {POLL_SECS}s"
            )))
            .when_some(self.ops.error.clone(), |header, error| {
                header.child(div().text_sm().text_color(theme::danger()).child(error))
            });

        let column_header = div()
            .flex_none()
            .px_4()
            .py_1()
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .gap_3()
            .text_xs()
            .text_color(theme::text_dim())
            .child(div().w(px(80.)).flex_none().child("ELAPSED"))
            .child(div().w(px(110.)).flex_none().child("USER"))
            .child(div().w(px(90.)).flex_none().child("MEMORY"))
            .child(div().w(px(150.)).flex_none().child("READ"))
            .child(div().flex_1().min_w_0().child("QUERY"))
            .child(div().w(px(60.)).flex_none());

        let rows: Vec<_> =
            self.ops
                .processes
                .iter()
                .enumerate()
                .map(|(index, process)| {
                    let progress = if process.total_rows > 0 {
                        format!(
                            "{} / {} rows",
                            Self::format_count(process.read_rows),
                            Self::format_count(process.total_rows)
                        )
                    } else {
                        format!(
                            "{} rows \u{b7} {}",
                            Self::format_count(process.read_rows),
                            Self::format_bytes(process.read_bytes)
                        )
                    };
                    let killing = self.ops.killing.as_deref() == Some(process.query_id.as_str());
                    let kill_id = process.query_id.clone();
                    let mut query_snippet = process.query.replace(['\n', '\t'], " ");
                    if query_snippet.len() > 200 {
                        let mut cut = 200;
                        while !query_snippet.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        query_snippet.truncate(cut);
                        query_snippet.push('\u{2026}');
                    }
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .flex()
                        .gap_3()
                        .items_center()
                        .text_sm()
                        .when(index % 2 == 1, |row| row.bg(theme::row_stripe()))
                        .child(
                            div()
                                .w(px(80.))
                                .flex_none()
                                .text_color(theme::warning())
                                .child(format_elapsed(process.elapsed_secs)),
                        )
                        .child(
                            div()
                                .w(px(110.))
                                .flex_none()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(theme::text())
                                .child(process.user.clone()),
                        )
                        .child(
                            div()
                                .w(px(90.))
                                .flex_none()
                                .text_color(theme::text_dim())
                                .child(Self::format_bytes(process.memory_bytes)),
                        )
                        .child(
                            div()
                                .w(px(150.))
                                .flex_none()
                                .text_color(theme::text_dim())
                                .child(progress),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family("Menlo")
                                .text_color(theme::text())
                                .child(query_snippet),
                        )
                        .child(div().w(px(60.)).flex_none().child(if read_only {
                            div()
                                .id(("ops-kill", index))
                                .px_2()
                                .py_0p5()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::disabled_border())
                                .text_xs()
                                .text_color(theme::disabled())
                                .child("Kill")
                                .tooltip(|window, cx| {
                                    gpui_component::tooltip::Tooltip::new(
                                        "Read-only connection: KILL QUERY needs write",
                                    )
                                    .build(window, cx)
                                })
                                .into_any_element()
                        } else {
                            div()
                                .id(("ops-kill", index))
                                .px_2()
                                .py_0p5()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::danger())
                                .text_xs()
                                .text_color(theme::danger())
                                .when(killing, |button| button.opacity(0.5))
                                .child(if killing { "..." } else { "Kill" })
                                .hover(|button| button.bg(theme::danger_hover()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ops_kill(kill_id.clone(), cx)
                                }))
                                .into_any_element()
                        }))
                })
                .collect();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .child(header)
            .child(column_header)
            .child(
                div()
                    .id("ops-process-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(self.ops.processes.is_empty(), |list| {
                        list.child(
                            div()
                                .px_4()
                                .py_6()
                                .text_color(theme::text_dim())
                                .child("No queries running right now."),
                        )
                    }),
            )
    }
}
