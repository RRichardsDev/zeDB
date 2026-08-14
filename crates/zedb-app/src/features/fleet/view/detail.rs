use super::*;

impl Workspace {
    pub(super) fn fleet_detail_panel(
        &mut self,
        row: &FleetRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let database = row.database.clone();
        let repo = self.fleet.repo.clone();
        let drift = self.fleet.drift.get(&database);
        let drift_loading = self.fleet.drift_loading.contains(&database);

        let mut summary: Vec<(String, gpui::Hsla)> = Vec::new();
        if let Some(group) = &row.excluded {
            summary.push((format!("excluded by group {group}"), theme::text_dim()));
        }
        summary.push((
            format!(
                "head {}",
                row.head
                    .map(|head| format!("{head:05}"))
                    .unwrap_or_else(|| "none".into())
            ),
            theme::text_dim(),
        ));
        if !row.failed.is_empty() {
            let failed: Vec<String> = row.failed.iter().map(|n| format!("{n:05}")).collect();
            summary.push((format!("failed: {}", failed.join(", ")), theme::danger()));
        }
        if !row.customised.is_empty() {
            let customised: Vec<String> =
                row.customised.iter().map(|n| format!("{n:05}")).collect();
            summary.push((
                format!("customised: {}", customised.join(", ")),
                accent_custom(),
            ));
        }

        // Dry-run: every pending migration rendered with this
        // database's parameters; unresolved placeholders stay visible.
        let mut pending_sql: Vec<(u32, String)> = Vec::new();
        if let Some(repo) = &repo {
            let mut params: BTreeMap<String, String> = BTreeMap::new();
            params.insert("db".into(), database.clone());
            for (name, config) in &repo.config.params {
                if let Some(default) = &config.default {
                    params.insert(name.clone(), default.clone());
                }
            }
            for migration in &repo.migrations {
                if row.pending.contains(&migration.number) && migration.targeted.is_none() {
                    if let Ok(sql) = migration.upgrade_sql() {
                        pending_sql
                            .push((migration.number, render_lenient(sql.trim_end(), &params)));
                    }
                }
            }
        }

        let drift_section: gpui::Div = match (drift_loading, drift) {
            (true, _) => div()
                .text_color(theme::text_dim())
                .child("Verifying against replayed chain state..."),
            (false, Some(info)) if info.findings.is_empty() => {
                div().text_color(theme::success()).child(format!(
                    "verified clean {}s ago",
                    info.checked_at.elapsed().as_secs()
                ))
            }
            (false, Some(info)) => {
                let mut section = div().flex().flex_col().gap_1();
                section = section.child(
                    div()
                        .text_color(theme::danger())
                        .child(format!("{} drift finding(s)", info.findings.len())),
                );
                for finding in &info.findings {
                    section = section.child(
                        div()
                            .p_2()
                            .rounded(px(3.))
                            .bg(rgb(0x2a2126))
                            .text_xs()
                            .font_family("Menlo")
                            .child(finding.clone()),
                    );
                }
                section
            }
            (false, None) => div()
                .text_color(theme::text_dim())
                .child("Not verified yet"),
        };

        let mut panel = div()
            .w(px(self.fleet.detail_width))
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
                    .id("fleet-detail-resize")
                    .absolute()
                    .left(px(-6.))
                    .top_0()
                    .bottom_0()
                    .w(px(13.))
                    .cursor_col_resize()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                            this.fleet.resizing_detail = true;
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
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().text_color(theme::text()).child(database.clone()))
                    .child(
                        div()
                            .id("fleet-detail-close")
                            .px_2()
                            .rounded(px(3.))
                            .text_color(theme::text_dim())
                            .child("✕")
                            .hover(|button| button.text_color(theme::text()).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.fleet.selected = None;
                                cx.notify();
                            })),
                    ),
            );

        let mut body = div()
            .id("fleet-detail-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2();
        for (text, color) in summary {
            body = body.child(div().text_color(color).child(text));
        }
        body = body
            .child(
                div()
                    .id("fleet-verify")
                    .mt_1()
                    .px_3()
                    .py_1()
                    .w(px(120.))
                    .text_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .text_color(theme::text())
                    .child(if drift_loading {
                        "Verifying..."
                    } else {
                        "Verify"
                    })
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .on_click({
                        let database = database.clone();
                        cx.listener(move |this, _, _, cx| this.fleet_verify(database.clone(), cx))
                    }),
            )
            .child(drift_section);
        if let Some(error) = &self.fleet.drift_error {
            body = body.child(div().text_color(theme::danger()).child(error.clone()));
        }

        if self.fleet.write_unlocked {
            let mut actions = div().mt_2().flex().flex_col().gap_1();
            let mut any = false;
            if !row.pending.is_empty() {
                any = true;
                actions = actions.child(action_button(
                    "fleet-act-upgrade",
                    format!("Upgrade {database}"),
                    theme::warning(),
                    {
                        let database = database.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.fleet_request_action(
                                FleetAction::UpgradeDatabase(database.clone()),
                                cx,
                            )
                        })
                    },
                ));
            }
            if let (Some(head), Some(repo)) = (row.head, repo.as_ref()) {
                if head != 0
                    && repo.migration(head).is_some_and(|migration| {
                        migration.targeted.is_none() && migration.rollback_class.is_some()
                    })
                {
                    any = true;
                    actions = actions.child(action_button(
                        "fleet-act-rollback",
                        format!("Roll back {head:05}"),
                        if self.agent_highlight("rollback") {
                            theme::filter_tint()
                        } else {
                            theme::danger()
                        },
                        {
                            let database = database.clone();
                            cx.listener(move |this, _, _, cx| {
                                this.fleet_request_action(
                                    FleetAction::Rollback {
                                        database: database.clone(),
                                        number: head,
                                    },
                                    cx,
                                )
                            })
                        },
                    ));
                }
            }
            if let Some(repo) = repo.as_ref() {
                for migration in &repo.migrations {
                    let Some(allow_list) = &migration.targeted else {
                        continue;
                    };
                    let allowed = allow_list.is_empty() || allow_list.contains(&database);
                    if !allowed {
                        continue;
                    }
                    let number = migration.number;
                    if row.customised.contains(&number) {
                        any = true;
                        actions = actions.child(action_button(
                            "fleet-act-remove-targeted",
                            format!("Remove customisation {number:05}"),
                            theme::danger(),
                            {
                                let database = database.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.fleet_request_action(
                                        FleetAction::RemoveTargeted {
                                            database: database.clone(),
                                            number,
                                        },
                                        cx,
                                    )
                                })
                            },
                        ));
                    } else {
                        any = true;
                        actions = actions.child(action_button(
                            "fleet-act-apply-targeted",
                            format!("Apply customisation {number:05}"),
                            accent_custom(),
                            {
                                let database = database.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.fleet_request_action(
                                        FleetAction::ApplyTargeted {
                                            database: database.clone(),
                                            number,
                                        },
                                        cx,
                                    )
                                })
                            },
                        ));
                    }
                }
            }
            if any {
                body = body.child(actions);
            }
        }

        if pending_sql.is_empty() && row.excluded.is_none() && row.failed.is_empty() {
            body = body.child(
                div()
                    .mt_2()
                    .text_color(theme::text_dim())
                    .child("Nothing pending: an upgrade would do no work."),
            );
        }
        for (number, sql) in pending_sql {
            body = body
                .child(
                    div()
                        .mt_2()
                        .text_color(theme::warning())
                        .child(format!("pending {number:05}: an upgrade would run")),
                )
                .child(
                    div()
                        .id(("fleet-sql", number as usize))
                        .p_2()
                        .rounded(px(3.))
                        .bg(theme::bg_inset())
                        .border_1()
                        .border_color(theme::border())
                        .text_xs()
                        .font_family("Menlo")
                        .whitespace_nowrap()
                        .overflow_x_scroll()
                        .child(sql),
                );
        }

        panel = panel.child(body);
        panel
    }
}
