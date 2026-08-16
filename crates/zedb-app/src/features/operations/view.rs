use gpui::{div, prelude::*, px, Context};
use gpui_component::{
    button::Button,
    menu::{DropdownMenu, PopupMenu},
};

use super::model::*;
use crate::{theme, Workspace};

impl Workspace {
    pub(crate) fn ops_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let read_only = self
            .connection
            .connected
            .as_ref()
            .map(|connected| connected.client_config.read_only)
            .unwrap_or(true);
        let as_of = self
            .ops
            .as_of
            .map(|stamp| format!("as of {}", stamp.format("%H:%M:%S")))
            .unwrap_or_else(|| "loading...".into());
        let cluster_scope = self.ops.scope.cluster().is_some();
        let scope_options = self.ops_cluster_options();

        let header_row = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_lg().text_color(theme::text()).child("Ops"))
            .when(!scope_options.is_empty(), |header| {
                let label = match self.ops.scope.cluster() {
                    Some(name) => format!("Cluster: {name}"),
                    None => "This node".to_string(),
                };
                header.child(
                    Button::new("ops-scope")
                        .label(label)
                        .dropdown_caret(true)
                        .compact()
                        .outline()
                        .dropdown_menu(move |menu: PopupMenu, _, _| {
                            let menu = menu
                                .min_w(px(160.))
                                .menu("This node", Box::new(SetOpsScope { cluster: None }));
                            scope_options.iter().fold(menu, |menu, name| {
                                menu.menu(
                                    format!("Cluster: {name}"),
                                    Box::new(SetOpsScope {
                                        cluster: Some(name.clone()),
                                    }),
                                )
                            })
                        }),
                )
            })
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
            });

        // Cluster health at a glance, on every tab: green until a
        // replica is readonly, lagging, stuck, or off its Keeper.
        let health_strip = (self.ops.replica_total > 0).then(|| {
            let readonly = self
                .ops
                .replica_problems
                .iter()
                .filter(|problem| problem.is_readonly)
                .count();
            let expired = self
                .ops
                .replica_problems
                .iter()
                .filter(|problem| problem.session_expired)
                .count();
            let max_delay = self
                .ops
                .replica_problems
                .iter()
                .map(|problem| problem.delay_secs)
                .max()
                .unwrap_or(0);
            let stuck = self
                .ops
                .queue_issues
                .iter()
                .filter(|issue| !issue.exception.is_empty())
                .count();
            let keeper_expired = self
                .ops
                .keeper
                .iter()
                .filter(|keeper| keeper.expired)
                .count();
            let critical = readonly > 0 || expired > 0 || keeper_expired > 0;
            let degraded = stuck > 0 || max_delay > 10;
            let (color, summary) = if critical {
                let mut parts = Vec::new();
                if readonly > 0 {
                    parts.push(format!("{readonly} readonly replica(s)"));
                }
                if expired > 0 {
                    parts.push(format!("{expired} expired session(s)"));
                }
                if keeper_expired > 0 {
                    parts.push(format!("{keeper_expired} Keeper session(s) expired"));
                }
                (theme::danger(), parts.join(" \u{b7} "))
            } else if degraded {
                let mut parts = Vec::new();
                if max_delay > 10 {
                    parts.push(format!("max replica delay {max_delay}s"));
                }
                if stuck > 0 {
                    parts.push(format!("{stuck} stuck queue entr(ies)"));
                }
                (theme::warning(), parts.join(" \u{b7} "))
            } else if self.ops.smt {
                // SharedMergeTree: no ZooKeeper-era signals to judge
                // health by, and pretending otherwise painted a
                // fabricated green. State what is actually known.
                (
                    theme::text_dim(),
                    "SharedMergeTree \u{b7} coordination is Cloud-managed".to_string(),
                )
            } else {
                (
                    theme::success(),
                    format!(
                        "replication healthy \u{b7} {} table(s){}",
                        self.ops.replica_total,
                        if self.ops.keeper.is_empty() {
                            ""
                        } else {
                            " \u{b7} Keeper connected"
                        }
                    ),
                )
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().size(px(7.)).rounded_full().bg(color))
                .child(div().text_sm().text_color(theme::text_dim()).child(summary))
        });

        let header = div()
            .flex_none()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap_1()
            .child(header_row)
            .when_some(health_strip, |header, strip| header.child(strip))
            .when_some(self.ops.error.clone(), |header, error| {
                // Fan-out to a replica whose cluster config carries no
                // credentials fails remote auth; explain that instead
                // of relaying ClickHouse's password-reset essay.
                let summary = if cluster_scope && error.contains("AUTHENTICATION_FAILED") {
                    "Remote nodes refused the fanned-out query: this cluster's config \
                     has no credentials for distributed queries. Switch back to This node."
                        .to_string()
                } else {
                    let mut line = error.lines().next().unwrap_or_default().to_string();
                    if line.len() > 180 {
                        let mut cut = 180;
                        while !line.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        line.truncate(cut);
                        line.push('\u{2026}');
                    }
                    line
                };
                header.child(
                    div()
                        .id("ops-error")
                        .w_full()
                        .text_sm()
                        .text_color(theme::danger())
                        .child(summary)
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(error.clone()).build(window, cx)
                        }),
                )
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
            .when(cluster_scope, |header| {
                header.child(div().w(px(110.)).flex_none().child("NODE"))
            })
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
                        .when(cluster_scope, |row| {
                            row.child(
                                div()
                                    .w(px(110.))
                                    .flex_none()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text_dim())
                                    .child(process.node.clone()),
                            )
                        })
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
                    .when(cluster_scope, |row| {
                        row.child(
                            div()
                                .w(px(110.))
                                .flex_none()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(theme::text_dim())
                                .child(merge.node.clone()),
                        )
                    })
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
                            .when(cluster_scope, |row| {
                                row.child(
                                    div()
                                        .w(px(110.))
                                        .flex_none()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(theme::text_dim())
                                        .child(mutation.node.clone()),
                                )
                            })
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
                .when(self.ops.smt, |list| {
                    list.child(
                        div()
                            .px_4()
                            .py_2()
                            .text_sm()
                            .text_color(theme::text_dim())
                            .child(
                                "This service runs SharedMergeTree: parts live in shared object \
                             storage and coordination is Cloud-managed, so replication queues \
                             and Keeper sessions do not apply. Watch parts, merges, and \
                             mutations on the Background tab instead.",
                            ),
                    )
                })
                .map(|list| {
                    if self.ops.smt {
                        list
                    } else if self.ops.replica_total == 0 {
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
                                    .when(cluster_scope, |row| {
                                        row.child(
                                            div()
                                                .w(px(110.))
                                                .flex_none()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_color(theme::text_dim())
                                                .child(problem.node.clone()),
                                        )
                                    })
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
                                    "{}{}.{} \u{b7} queue {} \u{b7} oldest {}",
                                    if issue.node.is_empty() {
                                        String::new()
                                    } else {
                                        format!("{} \u{b7} ", issue.node)
                                    },
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
                )
                .when(!self.ops.keeper.is_empty(), |list| {
                    list.child(section_title("KEEPER SESSIONS")).children(
                        self.ops.keeper.iter().enumerate().map(|(index, keeper)| {
                            div()
                                .px_4()
                                .py_2()
                                .flex()
                                .gap_3()
                                .items_center()
                                .text_sm()
                                .when(index % 2 == 1, |row| row.bg(theme::row_stripe()))
                                .when(cluster_scope, |row| {
                                    row.child(
                                        div()
                                            .w(px(110.))
                                            .flex_none()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_color(theme::text_dim())
                                            .child(keeper.node.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .w(px(300.))
                                        .flex_none()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(theme::text())
                                        .child(format!("{} \u{b7} {}", keeper.name, keeper.host)),
                                )
                                .child(if keeper.expired {
                                    div()
                                        .flex_1()
                                        .text_color(theme::danger())
                                        .child("SESSION EXPIRED")
                                } else {
                                    div().flex_1().text_color(theme::text_dim()).child(format!(
                                        "connected \u{b7} up {}",
                                        format_elapsed(keeper.uptime_secs as f64)
                                    ))
                                })
                        }),
                    )
                }),
            OpsTab::Ingestion => content
                .child(section_title("KAFKA CONSUMERS"))
                .children(
                    self.ops
                        .kafka_consumers
                        .iter()
                        .enumerate()
                        .map(|(index, consumer)| {
                            let stale = consumer.stale_secs > 60;
                            let failing = !consumer.exception.is_empty();
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
                                        .when(cluster_scope, |row| {
                                            row.child(
                                                div()
                                                    .w(px(110.))
                                                    .flex_none()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_color(theme::text_dim())
                                                    .child(consumer.node.clone()),
                                            )
                                        })
                                        .child(
                                            div()
                                                .w(px(300.))
                                                .flex_none()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_color(theme::text())
                                                .child(format!(
                                                    "{}.{}",
                                                    consumer.database, consumer.table
                                                )),
                                        )
                                        .child(
                                            div()
                                                .w(px(170.))
                                                .flex_none()
                                                .text_color(if stale {
                                                    theme::warning()
                                                } else {
                                                    theme::text_dim()
                                                })
                                                .child(format!(
                                                    "last poll {} ago",
                                                    format_elapsed(consumer.stale_secs as f64)
                                                )),
                                        )
                                        .child(div().flex_1().text_color(theme::text_dim()).child(
                                            format!(
                                                "{} messages read",
                                                Self::format_count(consumer.messages)
                                            ),
                                        )),
                                )
                                .when(failing, |row| {
                                    row.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::danger())
                                            .child(consumer.exception.clone()),
                                    )
                                })
                        }),
                )
                .when(self.ops.kafka_consumers.is_empty(), |list| {
                    list.child(empty_line(if self.ops.smt {
                        "No SQL-side Kafka consumers. ClickPipes run in the Cloud control \
                         plane and do not appear here; see the Cloud console's Data sources."
                    } else {
                        "No Kafka consumers."
                    }))
                })
                .child(section_title(
                    "MATERIALIZED VIEW INSERT FAILURES \u{b7} 24H",
                ))
                .children(self.ops.view_failures.iter().map(|failure| {
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
                            "{}{} \u{2192} {} \u{b7} {} failure(s)",
                            if failure.node.is_empty() {
                                String::new()
                            } else {
                                format!("{} \u{b7} ", failure.node)
                            },
                            failure.view,
                            failure.target,
                            failure.failures
                        )))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::danger())
                                .child(failure.exception.clone()),
                        )
                }))
                .when(self.ops.view_failures.is_empty(), |list| {
                    list.child(empty_line(
                        "No materialized view insert failures in the last 24 hours \
                         (needs query_views_log enabled).",
                    ))
                })
                .child(section_title("ASYNC INSERT QUEUE"))
                .children(
                    self.ops
                        .async_inserts
                        .iter()
                        .enumerate()
                        .map(|(index, pending)| {
                            div()
                                .px_4()
                                .py_2()
                                .flex()
                                .gap_3()
                                .items_center()
                                .text_sm()
                                .when(index % 2 == 1, |row| row.bg(theme::row_stripe()))
                                .when(cluster_scope, |row| {
                                    row.child(
                                        div()
                                            .w(px(110.))
                                            .flex_none()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_color(theme::text_dim())
                                            .child(pending.node.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .w(px(300.))
                                        .flex_none()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(theme::text())
                                        .child(format!("{}.{}", pending.database, pending.table)),
                                )
                                .child(div().flex_1().text_color(theme::text_dim()).child(format!(
                                    "{} pending \u{b7} {} \u{b7} oldest {}",
                                    pending.entries,
                                    Self::format_bytes(pending.bytes),
                                    format_elapsed(pending.oldest_secs as f64)
                                )))
                        }),
                )
                .when(self.ops.async_inserts.is_empty(), |list| {
                    list.child(empty_line("No pending async-insert batches."))
                }),
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
                        .when(cluster_scope, |row| {
                            row.child(
                                div()
                                    .w(px(110.))
                                    .flex_none()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text_dim())
                                    .child(disk.node.clone()),
                            )
                        })
                        .child(
                            div()
                                .w(px(120.))
                                .flex_none()
                                .text_color(theme::text())
                                .child(disk.name.clone()),
                        )
                        .map(|row| {
                            // Object storage has no meaningful
                            // capacity: a percent-full bar would be
                            // fabricated. Size is the honest number.
                            if disk.is_object_storage() {
                                row.child(div().text_color(theme::text_dim()).child(format!(
                                    "object storage ({}) \u{b7} {} stored \u{b7} no fixed capacity",
                                    disk.kind,
                                    Self::format_bytes(used),
                                )))
                            } else {
                                row.child(
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
                                .child(
                                    div().text_color(theme::text_dim()).child(format!(
                                        "{} of {} used ({:.0}%)",
                                        Self::format_bytes(used),
                                        Self::format_bytes(disk.total),
                                        fraction * 100.0
                                    )),
                                )
                            }
                        })
                }))
                .when(self.ops.disks.is_empty(), |list| {
                    list.child(empty_line("No disk information."))
                })
                .child(
                    div()
                        .px_4()
                        .pt_4()
                        .pb_1()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_xs().text_color(theme::text_dim()).child(
                            if cluster_scope {
                                "LARGEST TABLES (summed across shards)"
                            } else {
                                "LARGEST TABLES"
                            },
                        ))
                        .child(
                            Button::new("ops-top-limit")
                                .label(format!("Top: {}", self.ops.top_limit.label()))
                                .dropdown_caret(true)
                                .compact()
                                .outline()
                                .dropdown_menu(|menu: PopupMenu, _, _| {
                                    let entry = |menu: PopupMenu, limit: OpsTopLimit| {
                                        menu.menu(limit.label(), Box::new(SetOpsTopLimit { limit }))
                                    };
                                    let menu = menu.min_w(px(96.));
                                    let menu = entry(menu, OpsTopLimit::Ten);
                                    let menu = entry(menu, OpsTopLimit::TwentyFive);
                                    let menu = entry(menu, OpsTopLimit::Fifty);
                                    let menu = entry(menu, OpsTopLimit::Hundred);
                                    entry(menu, OpsTopLimit::All)
                                }),
                        ),
                )
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
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .flex()
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_color(theme::text())
                                                .child(format!("{}.", table.database)),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_color(theme::table_tint())
                                                .child(table.table.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(px(90.))
                                        .flex_none()
                                        .whitespace_nowrap()
                                        .text_right()
                                        .text_color(theme::text_dim())
                                        .child(Self::format_bytes(table.bytes)),
                                )
                                .child(
                                    div()
                                        .w(px(160.))
                                        .flex_none()
                                        .whitespace_nowrap()
                                        .text_right()
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
