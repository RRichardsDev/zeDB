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
    /// Where the client connects from, port stripped.
    pub address: String,
    /// client_name (native) or http_user_agent (HTTP), whichever is set.
    pub client: String,
    pub os_user: String,
    pub initial_user: String,
}

#[derive(Clone, Debug)]
pub struct OpsMerge {
    pub database: String,
    pub table: String,
    pub elapsed_secs: f64,
    /// 0.0..=1.0
    pub progress: f64,
    pub num_parts: u64,
    pub total_size_bytes: u64,
    pub is_mutation: bool,
}

#[derive(Clone, Debug)]
pub struct OpsMutation {
    pub database: String,
    pub table: String,
    pub command: String,
    pub parts_to_do: u64,
    pub latest_fail_reason: String,
}

#[derive(Clone, Debug)]
pub struct OpsReplicaProblem {
    pub database: String,
    pub table: String,
    pub is_readonly: bool,
    pub session_expired: bool,
    pub delay_secs: u64,
    pub queue_size: u64,
}

#[derive(Clone, Debug)]
pub struct OpsQueueIssue {
    pub database: String,
    pub table: String,
    pub depth: u64,
    pub oldest_secs: u64,
    pub exception: String,
}

#[derive(Clone, Debug)]
pub struct OpsDisk {
    pub name: String,
    pub free: u64,
    pub total: u64,
}

#[derive(Clone, Debug)]
pub struct OpsTopTable {
    pub database: String,
    pub table: String,
    pub bytes: u64,
    pub rows: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum OpsTab {
    #[default]
    Queries,
    Background,
    Replication,
    Storage,
}

impl OpsTab {
    const ALL: [OpsTab; 4] = [
        OpsTab::Queries,
        OpsTab::Background,
        OpsTab::Replication,
        OpsTab::Storage,
    ];

    fn label(self) -> &'static str {
        match self {
            OpsTab::Queries => "Queries",
            OpsTab::Background => "Background",
            OpsTab::Replication => "Replication",
            OpsTab::Storage => "Storage",
        }
    }
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
    /// Open-connection counters from system.metrics, label + count.
    pub connections: Vec<(String, u64)>,
    pub merges: Vec<OpsMerge>,
    pub mutations: Vec<OpsMutation>,
    /// Ticks since the view opened; the slow queries run every fifth.
    pub tick: u64,
    pub replica_total: u64,
    pub replica_problems: Vec<OpsReplicaProblem>,
    pub queue_issues: Vec<OpsQueueIssue>,
    pub disks: Vec<OpsDisk>,
    pub top_tables: Vec<OpsTopTable>,
    pub slow_fetch_in_flight: bool,
    pub tab: OpsTab,
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

/// A human address from system.processes: port stripped, the
/// IPv6-mapped-IPv4 prefix unwrapped, brackets removed.
fn display_address(address: &str) -> String {
    let mut host = address.trim();
    // [host]:port or host:port with a numeric tail.
    if let Some(stripped) = host.strip_prefix('[') {
        host = stripped.split(']').next().unwrap_or(stripped);
    } else if let Some((head, tail)) = host.rsplit_once(':') {
        if !tail.is_empty() && tail.chars().all(|ch| ch.is_ascii_digit()) {
            host = head;
        }
    }
    // ::ffff:172.18.0.1 is an IPv4 in IPv6 clothing.
    host.strip_prefix("::ffff:").unwrap_or(host).to_string()
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
        self.ops.tick = 0;
        self.ops_fetch(cx);
        self.ops_fetch_slow(cx);
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_secs(POLL_SECS)).await;
            let live = this
                .update(cx, |this, cx| {
                    let live = this.ops.poll_generation == generation
                        && this.show_ops
                        && this.connected.is_some();
                    if live {
                        this.ops.tick += 1;
                        this.ops_fetch(cx);
                        if this.ops.tick % 5 == 0 {
                            this.ops_fetch_slow(cx);
                        }
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
            let processes = client
                .query(
                    "SELECT query_id, user, elapsed, read_rows, read_bytes, \
                        total_rows_approx, memory_usage, query, \
                        toString(address), client_name, http_user_agent, \
                        os_user, initial_user \
                     FROM system.processes \
                     WHERE query NOT LIKE '%system.processes%' \
                     ORDER BY elapsed DESC \
                     LIMIT 50",
                )
                .await;
            // Aggregate open-connection counters; ClickHouse has no
            // session table (HTTP is stateless), so counts are the
            // whole truth here.
            let connections = client
                .query(
                    "SELECT metric, value FROM system.metrics \
                     WHERE metric IN ('HTTPConnection', 'TCPConnection', \
                        'MySQLConnection', 'PostgreSQLConnection', \
                        'InterserverConnection') \
                     ORDER BY metric",
                )
                .await;
            let merges = client
                .query(
                    "SELECT database, table, elapsed, progress, num_parts, \
                        total_size_bytes_compressed, is_mutation \
                     FROM system.merges \
                     ORDER BY elapsed DESC LIMIT 20",
                )
                .await;
            // Unfinished mutations only; failing ones first.
            let mutations = client
                .query(
                    "SELECT database, table, command, parts_to_do, \
                        latest_fail_reason \
                     FROM system.mutations \
                     WHERE NOT is_done \
                     ORDER BY latest_fail_reason != '' DESC, create_time ASC \
                     LIMIT 20",
                )
                .await;
            (processes, connections, merges, mutations)
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.fetch_in_flight = false;
                match result {
                    Ok((Ok(result), connections, merges, mutations)) => {
                        this.ops.processes = result
                            .rows
                            .iter()
                            .map(|row| {
                                let client_name = text(row.get(9));
                                let user_agent = text(row.get(10));
                                let address = text(row.get(8));
                                OpsProcess {
                                    query_id: text(row.first()),
                                    user: text(row.get(1)),
                                    elapsed_secs: float(row.get(2)),
                                    read_rows: number(row.get(3)),
                                    read_bytes: number(row.get(4)),
                                    total_rows: number(row.get(5)),
                                    memory_bytes: number(row.get(6)),
                                    query: text(row.get(7)),
                                    address: display_address(&address),
                                    client: if client_name.is_empty() {
                                        user_agent
                                    } else {
                                        client_name
                                    },
                                    os_user: text(row.get(11)),
                                    initial_user: text(row.get(12)),
                                }
                            })
                            .collect();
                        if let Ok(connections) = connections {
                            this.ops.connections = connections
                                .rows
                                .iter()
                                .map(|row| {
                                    let label = match text(row.first()).as_str() {
                                        "HTTPConnection" => "http".to_string(),
                                        "TCPConnection" => "tcp".to_string(),
                                        "MySQLConnection" => "mysql".to_string(),
                                        "PostgreSQLConnection" => "postgres".to_string(),
                                        "InterserverConnection" => "interserver".to_string(),
                                        other => other.to_string(),
                                    };
                                    (label, number(row.get(1)))
                                })
                                .filter(|(_, count)| *count > 0)
                                .collect();
                        }
                        if let Ok(merges) = merges {
                            this.ops.merges = merges
                                .rows
                                .iter()
                                .map(|row| OpsMerge {
                                    database: text(row.first()),
                                    table: text(row.get(1)),
                                    elapsed_secs: float(row.get(2)),
                                    progress: float(row.get(3)).clamp(0.0, 1.0),
                                    num_parts: number(row.get(4)),
                                    total_size_bytes: number(row.get(5)),
                                    is_mutation: number(row.get(6)) > 0,
                                })
                                .collect();
                        }
                        if let Ok(mutations) = mutations {
                            this.ops.mutations = mutations
                                .rows
                                .iter()
                                .map(|row| OpsMutation {
                                    database: text(row.first()),
                                    table: text(row.get(1)),
                                    command: text(row.get(2)),
                                    parts_to_do: number(row.get(3)),
                                    latest_fail_reason: text(row.get(4)),
                                })
                                .collect();
                        }
                        this.ops.as_of = Some(chrono::Local::now());
                        this.ops.error = None;
                    }
                    Ok((Err(error), _, _, _)) => this.ops.error = Some(error.to_string()),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Replication health, disks, and largest tables: slow-moving,
    /// fetched every fifth tick.
    fn ops_fetch_slow(&mut self, cx: &mut Context<Self>) {
        if self.ops.slow_fetch_in_flight {
            return;
        }
        let Some(connected) = &self.connected else {
            return;
        };
        self.ops.slow_fetch_in_flight = true;
        let config = connected.client_config.clone();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let replica_total = client.query("SELECT count() FROM system.replicas").await;
            let replica_problems = client
                .query(
                    "SELECT database, table, is_readonly, is_session_expired, \
                        absolute_delay, queue_size \
                     FROM system.replicas \
                     WHERE is_readonly OR is_session_expired \
                        OR absolute_delay > 0 OR queue_size > 0 \
                     ORDER BY absolute_delay DESC LIMIT 20",
                )
                .await;
            let queue_issues = client
                .query(
                    "SELECT database, table, count() AS depth, \
                        max(dateDiff('second', create_time, now())), \
                        anyIf(last_exception, last_exception != '') \
                     FROM system.replication_queue \
                     GROUP BY database, table \
                     ORDER BY depth DESC LIMIT 10",
                )
                .await;
            let disks = client
                .query(
                    "SELECT name, free_space, total_space \
                     FROM system.disks ORDER BY name",
                )
                .await;
            let top_tables = client
                .query(
                    "SELECT database, table, sum(bytes_on_disk), sum(rows) \
                     FROM system.parts WHERE active \
                     GROUP BY database, table \
                     ORDER BY sum(bytes_on_disk) DESC LIMIT 10",
                )
                .await;
            (
                replica_total,
                replica_problems,
                queue_issues,
                disks,
                top_tables,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.slow_fetch_in_flight = false;
                let Ok((replica_total, replica_problems, queue_issues, disks, top_tables)) = result
                else {
                    return;
                };
                if let Ok(total) = replica_total {
                    this.ops.replica_total = total
                        .rows
                        .first()
                        .map(|row| number(row.first()))
                        .unwrap_or(0);
                }
                if let Ok(problems) = replica_problems {
                    this.ops.replica_problems = problems
                        .rows
                        .iter()
                        .map(|row| OpsReplicaProblem {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            is_readonly: number(row.get(2)) > 0,
                            session_expired: number(row.get(3)) > 0,
                            delay_secs: number(row.get(4)),
                            queue_size: number(row.get(5)),
                        })
                        .collect();
                }
                if let Ok(issues) = queue_issues {
                    this.ops.queue_issues = issues
                        .rows
                        .iter()
                        .map(|row| OpsQueueIssue {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            depth: number(row.get(2)),
                            oldest_secs: number(row.get(3)),
                            exception: text(row.get(4)),
                        })
                        .collect();
                }
                if let Ok(disks) = disks {
                    this.ops.disks = disks
                        .rows
                        .iter()
                        .map(|row| OpsDisk {
                            name: text(row.first()),
                            free: number(row.get(1)),
                            total: number(row.get(2)),
                        })
                        .collect();
                }
                if let Ok(top) = top_tables {
                    this.ops.top_tables = top
                        .rows
                        .iter()
                        .map(|row| OpsTopTable {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            bytes: number(row.get(2)),
                            rows: number(row.get(3)),
                        })
                        .collect();
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
        self.ops_killed.insert(query_id.clone());
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
            .when(!self.ops.connections.is_empty(), |header| {
                let summary = self
                    .ops
                    .connections
                    .iter()
                    .map(|(label, count)| format!("{count} {label}"))
                    .collect::<Vec<_>>()
                    .join(" \u{b7} ");
                header.child(
                    div()
                        .text_sm()
                        .text_color(theme::text_dim())
                        .child(format!("connections: {summary}")),
                )
            })
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
            .child(div().w(px(150.)).flex_none().child("USER / CLIENT"))
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
                        .child({
                            let mut identity = process.client.clone();
                            if !process.address.is_empty() {
                                if !identity.is_empty() {
                                    identity.push_str(" \u{b7} ");
                                }
                                identity.push_str(&process.address);
                            }
                            if !process.os_user.is_empty() {
                                identity.push_str(" \u{b7} ");
                                identity.push_str(&process.os_user);
                            }
                            let mut user = process.user.clone();
                            if !process.initial_user.is_empty()
                                && process.initial_user != process.user
                            {
                                user.push_str(" (as ");
                                user.push_str(&process.initial_user);
                                user.push(')');
                            }
                            div()
                                .w(px(150.))
                                .flex_none()
                                .flex()
                                .flex_col()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_color(theme::text())
                                        .child(user),
                                )
                                .child(
                                    div()
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_size(px(9.))
                                        .text_color(theme::text_dim())
                                        .child(identity),
                                )
                        })
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

        let section_title = |title: &'static str| {
            div()
                .px_4()
                .pt_4()
                .pb_1()
                .text_xs()
                .text_color(theme::text_dim())
                .child(title)
        };

        let merge_rows: Vec<_> = self
            .ops
            .merges
            .iter()
            .enumerate()
            .map(|(index, merge)| {
                let bar_width = 120.0_f32;
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
                            .child(format_elapsed(merge.elapsed_secs)),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text())
                            .child(format!("{}.{}", merge.database, merge.table)),
                    )
                    .child(
                        div()
                            .w(px(bar_width))
                            .flex_none()
                            .h(px(6.))
                            .rounded(px(3.))
                            .bg(theme::row_stripe())
                            .child(
                                div()
                                    .w(px(bar_width * merge.progress as f32))
                                    .h(px(6.))
                                    .rounded(px(3.))
                                    .bg(theme::warning()),
                            ),
                    )
                    .child(
                        div()
                            .w(px(60.))
                            .flex_none()
                            .text_color(theme::text_dim())
                            .child(format!("{:.0}%", merge.progress * 100.0)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme::text_dim())
                            .child(format!(
                                "{} parts \u{b7} {}{}",
                                merge.num_parts,
                                Self::format_bytes(merge.total_size_bytes),
                                if merge.is_mutation {
                                    " \u{b7} mutation"
                                } else {
                                    ""
                                }
                            )),
                    )
            })
            .collect();

        let mutation_rows: Vec<_> = self
            .ops
            .mutations
            .iter()
            .enumerate()
            .map(|(index, mutation)| {
                let failing = !mutation.latest_fail_reason.is_empty();
                div()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .when(index % 2 == 1, |row| row.bg(theme::row_stripe()))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .w(px(300.))
                                    .flex_none()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text())
                                    .child(format!("{}.{}", mutation.database, mutation.table)),
                            )
                            .child(
                                div()
                                    .w(px(110.))
                                    .flex_none()
                                    .text_color(if failing {
                                        theme::danger()
                                    } else {
                                        theme::text_dim()
                                    })
                                    .child(format!("{} parts left", mutation.parts_to_do)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family("Menlo")
                                    .text_color(theme::text_dim())
                                    .child(mutation.command.clone()),
                            ),
                    )
                    .when(failing, |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(theme::danger())
                                .child(mutation.latest_fail_reason.clone()),
                        )
                    })
            })
            .collect();

        let empty_line = |message: &'static str| {
            div()
                .px_4()
                .py_2()
                .text_sm()
                .text_color(theme::text_dim())
                .child(message)
        };

        let active_tab = self.ops.tab;
        let tab_bar = div()
            .flex_none()
            .px_4()
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .gap_4()
            .children(OpsTab::ALL.into_iter().map(|tab| {
                let active = tab == active_tab;
                div()
                    .id(tab.label())
                    .py_2()
                    .border_b_2()
                    .border_color(if active {
                        theme::accent()
                    } else {
                        gpui::transparent_black()
                    })
                    .text_sm()
                    .text_color(if active {
                        theme::text()
                    } else {
                        theme::text_dim()
                    })
                    .hover(|label| label.text_color(theme::text()).cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ops.tab = tab;
                        cx.notify();
                    }))
                    .child(tab.label())
            }));

        let content = div()
            .id("ops-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        let content = match active_tab {
            OpsTab::Queries => content
                .child(column_header)
                .children(rows)
                .when(self.ops.processes.is_empty(), |list| {
                    list.child(empty_line("No queries running right now."))
                }),
            OpsTab::Background => content
                .child(section_title("MERGES"))
                .children(merge_rows)
                .when(self.ops.merges.is_empty(), |list| {
                    list.child(empty_line("No merges in flight."))
                })
                .child(section_title("MUTATIONS"))
                .children(mutation_rows)
                .when(self.ops.mutations.is_empty(), |list| {
                    list.child(empty_line("No unfinished mutations."))
                }),
            OpsTab::Replication => content
                .map(|list| {
                    if self.ops.replica_total == 0 {
                        list.child(empty_line("No replicated tables."))
                    } else if self.ops.replica_problems.is_empty() {
                        list.child(
                            div()
                                .px_4()
                                .py_2()
                                .text_sm()
                                .text_color(theme::success())
                                .child(format!(
                                    "{} replicated table(s), all healthy",
                                    self.ops.replica_total
                                )),
                        )
                    } else {
                        list.children(self.ops.replica_problems.iter().enumerate().map(
                            |(index, problem)| {
                                let mut flags: Vec<String> = Vec::new();
                                if problem.is_readonly {
                                    flags.push("READONLY".into());
                                }
                                if problem.session_expired {
                                    flags.push("SESSION EXPIRED".into());
                                }
                                if problem.delay_secs > 0 {
                                    flags.push(format!("{}s behind", problem.delay_secs));
                                }
                                if problem.queue_size > 0 {
                                    flags.push(format!("queue {}", problem.queue_size));
                                }
                                div()
                                    .px_4()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .gap_3()
                                    .text_sm()
                                    .when(index % 2 == 1, |row| row.bg(theme::row_stripe()))
                                    .child(
                                        div()
                                            .w(px(300.))
                                            .flex_none()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_color(theme::text())
                                            .child(format!(
                                                "{}.{}",
                                                problem.database, problem.table
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_color(theme::danger())
                                            .child(flags.join(" \u{b7} ")),
                                    )
                            },
                        ))
                    }
                })
                .children(
                    self.ops
                        .queue_issues
                        .iter()
                        .filter(|issue| !issue.exception.is_empty())
                        .map(|issue| {
                            div()
                                .px_4()
                                .py_2()
                                .border_b_1()
                                .border_color(theme::border())
                                .flex()
                                .flex_col()
                                .gap_1()
                                .text_sm()
                                .child(div().text_color(theme::text()).child(format!(
                                    "{}.{} \u{b7} queue {} \u{b7} oldest {}",
                                    issue.database,
                                    issue.table,
                                    issue.depth,
                                    format_elapsed(issue.oldest_secs as f64)
                                )))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::danger())
                                        .child(issue.exception.clone()),
                                )
                        }),
                ),
            OpsTab::Storage => content
                .child(section_title("DISKS"))
                .children(self.ops.disks.iter().map(|disk| {
                    let used = disk.total.saturating_sub(disk.free);
                    let fraction = if disk.total > 0 {
                        used as f64 / disk.total as f64
                    } else {
                        0.0
                    };
                    let bar_width = 200.0_f32;
                    div()
                        .px_4()
                        .py_2()
                        .flex()
                        .gap_3()
                        .items_center()
                        .text_sm()
                        .child(
                            div()
                                .w(px(120.))
                                .flex_none()
                                .text_color(theme::text())
                                .child(disk.name.clone()),
                        )
                        .child(
                            div()
                                .w(px(bar_width))
                                .flex_none()
                                .h(px(6.))
                                .rounded(px(3.))
                                .bg(theme::row_stripe())
                                .child(
                                    div()
                                        .w(px(bar_width * fraction as f32))
                                        .h(px(6.))
                                        .rounded(px(3.))
                                        .bg(if fraction > 0.9 {
                                            theme::danger()
                                        } else if fraction > 0.75 {
                                            theme::warning()
                                        } else {
                                            theme::success()
                                        }),
                                ),
                        )
                        .child(div().text_color(theme::text_dim()).child(format!(
                            "{} of {} used ({:.0}%)",
                            Self::format_bytes(used),
                            Self::format_bytes(disk.total),
                            fraction * 100.0
                        )))
                }))
                .when(self.ops.disks.is_empty(), |list| {
                    list.child(empty_line("No disk information."))
                })
                .child(section_title("LARGEST TABLES"))
                .children(
                    self.ops
                        .top_tables
                        .iter()
                        .enumerate()
                        .map(|(index, table)| {
                            div()
                                .px_4()
                                .py_1p5()
                                .border_b_1()
                                .border_color(theme::border())
                                .flex()
                                .gap_3()
                                .text_sm()
                                .when(index % 2 == 1, |row| row.bg(theme::row_stripe()))
                                .child(
                                    div()
                                        .w(px(300.))
                                        .flex_none()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(theme::text())
                                        .child(format!("{}.{}", table.database, table.table)),
                                )
                                .child(
                                    div()
                                        .w(px(100.))
                                        .flex_none()
                                        .text_color(theme::text_dim())
                                        .child(Self::format_bytes(table.bytes)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_color(theme::text_dim())
                                        .child(format!("{} rows", Self::format_count(table.rows))),
                                )
                        }),
                )
                .when(self.ops.top_tables.is_empty(), |list| {
                    list.child(empty_line("No table parts on this node."))
                }),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .child(header)
            .child(tab_bar)
            .child(content)
    }
}
