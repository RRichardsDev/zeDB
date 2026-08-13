use super::*;

impl Workspace {
    /// The safety ladder: tier identity, rendered dry-run, class
    /// acknowledgements, and typed confirmation where the tier or the
    /// action demands it. Nothing here is skippable.
    pub(super) fn fleet_action_modal(
        &mut self,
        action: FleetAction,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tier = self.fleet_tier();
        let (tier_bg, tier_fg) = Self::tier_colors(tier);
        let tier_label = match tier {
            zedb_core::EnvTier::Dev => "DEV",
            zedb_core::EnvTier::Staging => "STAGING",
            zedb_core::EnvTier::Production => "PRODUCTION",
        };
        let class = self.action_rollback_class(&action);
        let structural = matches!(
            class,
            Some(Some(zedb_core::repo::RollbackClass::Structural))
        );
        let irreversible = matches!(
            class,
            Some(None) | Some(Some(zedb_core::repo::RollbackClass::Irreversible))
        );
        let phrase = action.required_phrase(tier, irreversible);
        let typed = self.fleet.confirm_input.read(cx).text().trim().to_string();
        let phrase_ok = phrase
            .as_ref()
            .map(|phrase| typed == *phrase)
            .unwrap_or(true);
        let ack_ok = !structural || self.fleet.ack_structural;
        let running = self.fleet.action_running;
        let confirmable = phrase_ok && ack_ok && !running && self.fleet.action_result.is_none();

        // The dry-run: exactly what would execute, rendered.
        let mut dry_run: Vec<(String, String)> = Vec::new();
        if let Some(repo) = &self.fleet.repo {
            let mut params: BTreeMap<String, String> = BTreeMap::new();
            for (name, config) in &repo.config.params {
                if let Some(default) = &config.default {
                    params.insert(name.clone(), default.clone());
                }
            }
            if let Some(cluster) = &self.fleet.selected_cluster {
                params.insert("cluster".into(), cluster.clone());
            }
            let with_db = |database: &str| {
                let mut params = params.clone();
                params.insert("db".into(), database.to_string());
                params
            };
            match &action {
                FleetAction::UpgradeAll => {
                    for row in &self.fleet.rows {
                        if row.excluded.is_some() || row.pending.is_empty() {
                            continue;
                        }
                        let pending: Vec<String> =
                            row.pending.iter().map(|n| format!("{n:05}")).collect();
                        dry_run.push((
                            format!("{}: would run {}", row.database, pending.join(", ")),
                            String::new(),
                        ));
                    }
                    for migration in &repo.migrations {
                        if migration.targeted.is_some() {
                            continue;
                        }
                        if self.fleet.rows.iter().any(|row| {
                            row.excluded.is_none() && row.pending.contains(&migration.number)
                        }) {
                            if let Ok(sql) = migration.upgrade_sql() {
                                dry_run.push((
                                    format!("migration {:05}", migration.number),
                                    render_lenient(sql.trim_end(), &params),
                                ));
                            }
                        }
                    }
                }
                FleetAction::UpgradeDatabase(database) => {
                    let params = with_db(database);
                    if let Some(row) = self.fleet.rows.iter().find(|row| row.database == *database)
                    {
                        for migration in &repo.migrations {
                            if migration.targeted.is_none()
                                && row.pending.contains(&migration.number)
                            {
                                if let Ok(sql) = migration.upgrade_sql() {
                                    dry_run.push((
                                        format!("migration {:05}", migration.number),
                                        render_lenient(sql.trim_end(), &params),
                                    ));
                                }
                            }
                        }
                    }
                }
                FleetAction::Rollback { database, number }
                | FleetAction::RemoveTargeted { database, number } => {
                    let params = with_db(database);
                    if let Some(Ok(Some(sql))) = repo
                        .migration(*number)
                        .map(|migration| migration.rollback_sql())
                    {
                        dry_run.push((
                            format!("rollback {number:05}"),
                            render_lenient(sql.trim_end(), &params),
                        ));
                    }
                }
                FleetAction::ApplyTargeted { database, number } => {
                    let params = with_db(database);
                    if let Some(Ok(sql)) = repo
                        .migration(*number)
                        .map(|migration| migration.upgrade_sql())
                    {
                        dry_run.push((
                            format!("apply {number:05}"),
                            render_lenient(sql.trim_end(), &params),
                        ));
                    }
                }
            }
        }

        let mut card = div()
            .w(px(640.))
            .max_h(px(560.))
            .flex()
            .flex_col()
            .rounded(px(6.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::bg())
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(tier_bg))
                    .text_color(rgb(tier_fg))
                    .child(action.title())
                    .child(format!("{tier_label} tier")),
            );

        let mut body = div()
            .id("fleet-modal-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(git) = self.fleet.git.as_ref().filter(|git| git.stale()) {
            let mut warning = div()
                .p_2()
                .rounded(px(3.))
                .border_1()
                .border_color(theme::warning())
                .text_color(theme::warning())
                .flex()
                .flex_col()
                .gap_1()
                .child("Repo checkout may not match what was reviewed:");
            for line in git.deploy_warnings() {
                warning = warning.child(format!("  {line}"));
            }
            body = body.child(warning);
        }
        for (label, sql) in dry_run {
            let key = label.split(':').next().unwrap_or(&label);
            let applied = self.fleet.action_progress.get(key).copied();
            body = body.child(
                div()
                    .text_color(if applied.is_some() {
                        theme::success()
                    } else {
                        theme::text_dim()
                    })
                    .child(match applied {
                        Some(seconds) if seconds < 1.0 => {
                            format!("{label}  [applied {:.0}ms]", seconds * 1000.0)
                        }
                        Some(seconds) => format!("{label}  [applied {seconds:.1}s]"),
                        None => label.clone(),
                    }),
            );
            if !sql.is_empty() {
                body = body.child(
                    div()
                        .id(gpui::SharedString::from(format!("modal-sql-{label}")))
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
        }
        if structural {
            body = body.child(
                div()
                    .id("fleet-ack-structural")
                    .p_2()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if self.fleet.ack_structural {
                        theme::success()
                    } else {
                        theme::danger()
                    })
                    .text_color(if self.fleet.ack_structural {
                        theme::success()
                    } else {
                        theme::danger()
                    })
                    .child(if self.fleet.ack_structural {
                        "Acknowledged: schema is restored but newer data may be lost"
                    } else {
                        "Structural rollback: click to acknowledge that newer data may be lost"
                    })
                    .hover(|ack| ack.cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.fleet.ack_structural = !this.fleet.ack_structural;
                        cx.notify();
                    })),
            );
        }
        if irreversible {
            body =
                body.child(div().text_color(theme::danger()).child(
                    "This rollback is IRREVERSIBLE: it does not restore the previous state.",
                ));
        }
        if let Some(phrase) = &phrase {
            body = body
                .child(
                    div()
                        .text_color(theme::text_dim())
                        .child(format!("Type \"{phrase}\" to confirm:")),
                )
                .child(div().w(px(260.)).child(self.fleet.confirm_input.clone()));
        }
        match &self.fleet.action_result {
            Some(Ok(message)) => {
                body = body.child(div().text_color(theme::success()).child(message.clone()));
            }
            Some(Err(error)) => {
                body = body.child(div().text_color(theme::danger()).child(error.clone()));
            }
            None => {}
        }
        if running {
            body = body.child(div().text_color(theme::text_dim()).child("Applying..."));
        }
        card = card.child(body);

        card = card.child(
            div()
                .flex_none()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .justify_end()
                .gap_2()
                .border_t_1()
                .border_color(theme::border())
                .child(
                    div()
                        .id("fleet-modal-cancel")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .child(if self.fleet.action_result.is_some() {
                            "Close"
                        } else {
                            "Cancel"
                        })
                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.fleet.pending_action = None;
                            cx.notify();
                        })),
                )
                .when(self.fleet.action_result.is_none(), |footer| {
                    footer.child(
                        div()
                            .id("fleet-modal-confirm")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(if confirmable {
                                theme::danger()
                            } else {
                                theme::border()
                            })
                            .text_color(if confirmable {
                                theme::danger()
                            } else {
                                theme::text_dim()
                            })
                            .child(if running { "Applying..." } else { "Confirm" })
                            .when(confirmable, |button| {
                                button
                                    .hover(|button| button.bg(rgb(0x3a2a2a)).cursor_pointer())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.fleet_execute_action(cx)),
                                    )
                            }),
                    )
                }),
        );

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x000000aa))
            .occlude()
            .child(card)
            .into_any_element()
    }
}
