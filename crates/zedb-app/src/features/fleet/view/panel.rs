use super::*;

impl Workspace {
    pub(crate) fn fleet_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = self.fleet.repo.clone();
        let rows = self.fleet_filtered_rows();
        let migrations: Vec<(u32, bool)> = repo
            .as_ref()
            .map(|repo| {
                repo.migrations
                    .iter()
                    .map(|migration| (migration.number, migration.targeted.is_some()))
                    .collect()
            })
            .unwrap_or_default();
        let latest_fleet = migrations
            .iter()
            .rev()
            .find(|(_, targeted)| !targeted)
            .map(|(number, _)| *number);
        let selected = self.fleet.selected.clone();
        let loading = self.fleet.loading;
        let selected_row = selected
            .as_deref()
            .and_then(|name| rows.iter().find(|row| row.database == name).cloned());
        let detail = selected_row
            .as_ref()
            .map(|row| self.fleet_detail_panel(row, cx).into_any_element());

        let toolbar =
            div()
                .flex_none()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(theme::border())
                .child(div().flex_1().min_w_0().child(self.fleet.repo_path.clone()))
                .child(
                    div()
                        .id("fleet-open")
                        .px_2()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .text_color(theme::text_dim())
                        .child(
                            svg()
                                .path("icons/folder-open.svg")
                                .size(px(14.))
                                .text_color(theme::text_dim()),
                        )
                        .hover(|button| {
                            button
                                .bg(theme::bg_sidebar())
                                .text_color(theme::text())
                                .cursor_pointer()
                        })
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Open the repo: a local checkout path or a git URL to clone",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.fleet_repo_button(cx))),
                )
                .child(
                    div()
                        .id("fleet-refresh")
                        .px_2()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .text_color(if loading {
                            theme::success()
                        } else {
                            theme::text_dim()
                        })
                        .child(svg().path("icons/refresh.svg").size(px(14.)).text_color(
                            if loading {
                                theme::success()
                            } else {
                                theme::text_dim()
                            },
                        ))
                        .hover(|button| {
                            button
                                .bg(theme::bg_sidebar())
                                .text_color(theme::text())
                                .cursor_pointer()
                        })
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Refresh fleet status from the connected cluster",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.fleet_refresh(cx))),
                );

        // Second row: matrix controls, kept apart from the repo source.
        let hidden_count = self.fleet.hidden_databases.len();
        let unlocked = self.fleet.write_unlocked;
        let controls = div()
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .id("fleet-db-filter")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .text_color(if hidden_count > 0 {
                        theme::text()
                    } else {
                        theme::text_dim()
                    })
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(if hidden_count > 0 {
                        format!("Databases ({hidden_count} hidden)")
                    } else {
                        "Databases".into()
                    })
                    .child(
                        svg()
                            .path("icons/chevron-down.svg")
                            .size(px(12.))
                            .text_color(if hidden_count > 0 {
                                theme::text()
                            } else {
                                theme::text_dim()
                            }),
                    )
                    .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.fleet.filter_open = !this.fleet.filter_open;
                        cx.notify();
                    })),
            )
            .child(
                fleet_icon_button(
                    "fleet-new-migration",
                    "icons/migration-plus.svg",
                    "New migration: author a draft against the pinned server",
                    cx.listener(|this, _, window, cx| this.author_open(window, cx)),
                )
                .when(self.agent_highlight("new_migration"), |button| {
                    button.border_color(theme::filter_tint())
                }),
            )
            .child({
                let regen_status = self.regen_status;
                // A failed chain check is the regen's cue: stale
                // current-state is its most common cause.
                let checks_failed = self.checks.as_ref().is_some_and(|checks| {
                    checks
                        .slots
                        .iter()
                        .any(|slot| matches!(slot, crate::codegen::CheckSlot::Fail(_)))
                }) && !self.checks.as_ref().is_some_and(|checks| {
                    checks
                        .slots
                        .iter()
                        .any(|slot| matches!(slot, crate::codegen::CheckSlot::Running))
                });
                div()
                    .id("fleet-regen")
                    .group("fleet-regen")
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if self.agent_highlight("regen") {
                        theme::filter_tint()
                    } else {
                        theme::border()
                    })
                    .child(
                        svg()
                            .path("icons/regen.svg")
                            .size(px(14.))
                            .text_color(match regen_status {
                                Some(true) => theme::success(),
                                Some(false) => theme::warning(),
                                None if checks_failed => theme::warning(),
                                None => theme::text_dim(),
                            })
                            .when(regen_status.is_none() && !checks_failed, |icon| {
                                icon.group_hover("fleet-regen", |icon| {
                                    icon.text_color(theme::text())
                                })
                            }),
                    )
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(match regen_status {
                            Some(true) => "current-state matches the chain \u{b7} click to re-check",
                            Some(false) => {
                                "current-state has drifted from the chain; review the churn and write"
                            }
                            None if checks_failed => "Chain check failed. Please regen.",
                            None => {
                                "Regen: replay the chain and preview current-state churn before writing"
                            }
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.codegen_start_regen(cx)))
            })
            .child({
                let checks_clean = self.checks_clean;
                let checks_running = self.checks.as_ref().is_some_and(|checks| {
                    checks
                        .slots
                        .iter()
                        .any(|slot| matches!(slot, crate::codegen::CheckSlot::Running))
                });
                let checks_failed = !checks_running
                    && self.checks.as_ref().is_some_and(|checks| {
                        checks
                            .slots
                            .iter()
                            .any(|slot| matches!(slot, crate::codegen::CheckSlot::Fail(_)))
                    });
                div()
                    .id("fleet-checks")
                    .group("fleet-checks")
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if self.agent_highlight("check_chain") {
                        theme::filter_tint()
                    } else {
                        theme::border()
                    })
                    .child(if checks_running {
                        use gpui::{percentage, Animation, AnimationExt as _, Transformation};
                        use gpui_component::Sizable as _;
                        gpui_component::Icon::empty()
                            .path("icons/hourglass.svg")
                            .with_size(gpui_component::Size::Small)
                            .text_color(theme::text_dim())
                            .with_animation(
                                "fleet-checks-spin",
                                Animation::new(std::time::Duration::from_secs(1)).repeat(),
                                |icon, delta| {
                                    icon.transform(Transformation::rotate(percentage(delta)))
                                },
                            )
                            .into_any_element()
                    } else {
                        svg()
                            .path("icons/check-chain.svg")
                            .size(px(14.))
                            .text_color(if checks_clean {
                                theme::success()
                            } else if checks_failed {
                                theme::danger()
                            } else {
                                theme::text_dim()
                            })
                            .when(!checks_clean && !checks_failed, |icon| {
                                icon.group_hover("fleet-checks", |icon| {
                                    icon.text_color(theme::text())
                                })
                            })
                            .into_any_element()
                    })
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if checks_running {
                            "Chain checks running \u{2026} click for details"
                        } else if checks_clean {
                            "Chain checks passed (sql, equivalence, lifecycle) \u{b7} click to re-run"
                        } else if checks_failed {
                            "Chain checks FAILED \u{b7} click for details"
                        } else {
                            "Check chain: sql, equivalence, and lifecycle against the pinned server"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.codegen_start_checks(cx)))
            })
            .child(self.fleet_verify_all_button(cx))
            .when(
                self.fleet.git.as_ref().is_some_and(|git| git.dirty > 0),
                |controls| {
                    controls.child(fleet_icon_button(
                        "fleet-commit",
                        "icons/commit.svg",
                        "Commit the repo's own changes and push to your remote",
                        cx.listener(|this, _, window, cx| this.commit_open(window, cx)),
                    ))
                },
            )
            .when(
                self.fleet
                    .git
                    .as_ref()
                    .and_then(|git| git.ahead_behind)
                    .is_some_and(|(_, behind)| behind > 0),
                |controls| {
                    controls.child(fleet_icon_button(
                        "fleet-pull",
                        "icons/pull.svg",
                        "Pull: the checkout is behind its upstream (fast-forward only)",
                        cx.listener(|this, _, _, cx| this.fleet_pull(cx)),
                    ))
                },
            )
            .child(
                div()
                    .id("fleet-write-unlock")
                    .group("fleet-write-unlock")
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if self.agent_highlight("lock") {
                        theme::filter_tint()
                    } else if unlocked {
                        theme::danger()
                    } else {
                        theme::border()
                    })
                    .child(
                        svg()
                            .path(if unlocked {
                                "icons/lock-open.svg"
                            } else {
                                "icons/lock.svg"
                            })
                            .size(px(14.))
                            .text_color(if unlocked {
                                theme::danger()
                            } else {
                                theme::text_dim()
                            })
                            .when(!unlocked, |icon| {
                                icon.group_hover("fleet-write-unlock", |icon| {
                                    icon.text_color(theme::success())
                                })
                            }),
                    )
                    .hover(|button| {
                        let button = button.bg(theme::bg_sidebar()).cursor_pointer();
                        if unlocked {
                            button
                        } else {
                            button.border_color(theme::success())
                        }
                    })
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if unlocked {
                            "Writes unlocked; click to lock"
                        } else {
                            "Writes locked; click to unlock"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.fleet.write_unlocked = !this.fleet.write_unlocked;
                        cx.notify();
                    })),
            )
            .map(|controls| {
                // "Upgrade all" only while there is anything to
                // upgrade; a fully applied fleet says so instead.
                let status_known = !self.fleet.rows.is_empty();
                let any_pending = self
                    .fleet
                    .rows
                    .iter()
                    .any(|row| row.excluded.is_none() && !row.pending.is_empty());
                if status_known && !any_pending {
                    controls.child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::success())
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child("\u{2713}")
                            .child("Up to date"),
                    )
                } else if unlocked && any_pending {
                    controls.child(
                        div()
                            .id("fleet-upgrade-all")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(if self.agent_highlight("upgrade_all") {
                                theme::filter_tint()
                            } else {
                                theme::warning()
                            })
                            .text_color(theme::warning())
                            .child("Upgrade all")
                            .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.fleet_request_action(FleetAction::UpgradeAll, cx)
                            })),
                    )
                } else {
                    controls
                }
            });

        let mut header = div()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .bg(theme::bg_sidebar())
            .border_b_1()
            .border_color(theme::border())
            .text_color(theme::text_dim())
            .child(div().w(px(200.)).flex_none().px_2().child("database"))
            .child(div().w(px(70.)).flex_none().px_1().child("head"));
        for (index, (number, targeted)) in migrations.iter().enumerate() {
            let label = if *targeted {
                format!("{number:05}*")
            } else {
                format!("{number:05}")
            };
            let number = *number;
            let targeted = *targeted;
            // Applied anywhere means read-only viewing; never applied
            // means clicking opens an editable draft.
            let applied_anywhere = rows.iter().any(|row| {
                if targeted {
                    row.customised.contains(&number)
                } else {
                    row.excluded.is_none()
                        && row.head.is_some_and(|head| head >= number)
                        && !row.pending.contains(&number)
                }
            });
            header = header.child(
                div()
                    .id(("fleet-migration-header", index))
                    .w(px(64.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    // Editable (never applied anywhere) reads green at
                    // all times; hover-time text recolors do not render
                    // in this gpui version, and a standing cue is more
                    // scannable anyway.
                    .child(
                        div()
                            .text_color(if applied_anywhere {
                                theme::text_dim()
                            } else {
                                theme::success()
                            })
                            .child(label),
                    )
                    .hover(|cell| cell.bg(theme::hover()).cursor_pointer())
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if applied_anywhere {
                            "View migration (applied; read-only)"
                        } else {
                            "Edit migration (never applied anywhere)"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.author_open_migration(number, window, cx);
                    })),
            );
        }
        header = header.child(div().flex_1().px_2().child("state"));

        let migrations_for_rows = migrations.clone();
        let rows_for_list = rows.clone();
        // Verified drift state per database, for the matrix badges:
        // Some(count) = drifted, Some(0) = verified clean, None = not
        // verified this session.
        let drift_for_list: std::collections::HashMap<String, usize> = self
            .fleet
            .drift
            .iter()
            .map(|(database, info)| (database.clone(), info.findings.len()))
            .collect();
        let list = uniform_list(
            "fleet-matrix",
            rows.len(),
            cx.processor(
                move |_this: &mut Workspace, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .map(|index| {
                            let row = &rows_for_list[index];
                            let is_selected = selected.as_deref() == Some(row.database.as_str());
                            let mut line = div()
                                .id(("fleet-row", index))
                                .flex()
                                .items_center()
                                .h(px(ROW_HEIGHT))
                                .when(index % 2 == 1, |line| line.bg(theme::row_stripe()))
                                .when(is_selected, |line| line.bg(theme::selected()))
                                .hover(|line| line.bg(theme::row_hover()))
                                .on_click({
                                    let database = row.database.clone();
                                    cx.listener(move |this: &mut Workspace, _, _, cx| {
                                        this.fleet.selected = Some(database.clone());
                                        cx.notify();
                                    })
                                })
                                .child(
                                    div()
                                        .w(px(200.))
                                        .flex_none()
                                        .px_2()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(row.excluded.is_some(), |name| {
                                            name.text_color(theme::text_dim())
                                        })
                                        .child(row.database.clone()),
                                )
                                .child(
                                    div()
                                        .w(px(70.))
                                        .flex_none()
                                        .px_1()
                                        .text_color(theme::text_dim())
                                        .child(
                                            row.head
                                                .map(|head| format!("{head:05}"))
                                                .unwrap_or_else(|| "none".into()),
                                        ),
                                );
                            for (number, targeted) in &migrations_for_rows {
                                let (glyph, color) = match cell_for(row, *number, *targeted) {
                                    Cell::Applied => ("●", theme::success()),
                                    Cell::Pending => ("○", theme::warning()),
                                    Cell::Failed => ("✕", theme::danger()),
                                    Cell::Customised => ("◆", accent_custom()),
                                    Cell::NotApplicable => ("·", gpui::rgb(0x3a3f4b).into()),
                                };
                                line = line.child(
                                    div()
                                        .w(px(64.))
                                        .flex_none()
                                        .text_center()
                                        .text_color(color)
                                        .child(glyph),
                                );
                            }
                            let drift = drift_for_list.get(&row.database).copied();
                            let state = if let Some(group) = &row.excluded {
                                format!("excluded ({group})")
                            } else if !row.failed.is_empty() {
                                let failed: Vec<String> =
                                    row.failed.iter().map(|n| format!("{n:05}")).collect();
                                format!("FAILED: {}", failed.join(", "))
                            } else if matches!(drift, Some(count) if count > 0) {
                                format!("DRIFTED: {} finding(s)", drift.unwrap_or(0))
                            } else if !row.pending.is_empty() {
                                format!("{} pending", row.pending.len())
                            } else if row.head == latest_fleet {
                                if drift == Some(0) {
                                    "up to date, verified".into()
                                } else {
                                    "up to date".into()
                                }
                            } else {
                                String::new()
                            };
                            let state_color = if row.excluded.is_some() {
                                theme::text_dim()
                            } else if !row.failed.is_empty() {
                                theme::danger()
                            } else if matches!(drift, Some(count) if count > 0) {
                                accent_custom()
                            } else if !row.pending.is_empty() {
                                theme::warning()
                            } else {
                                theme::success()
                            };
                            line.child(
                                div()
                                    .flex_1()
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(state_color)
                                    .child(SharedString::from(state)),
                            )
                        })
                        .collect()
                },
            ),
        )
        .h_full()
        .flex_grow();

        let status_line = if let Some(error) = &self.fleet.fetch_error {
            div().text_color(theme::danger()).child(error.clone())
        } else if let Some(error) = &self.fleet.repo_error {
            div().text_color(theme::danger()).child(error.clone())
        } else if loading {
            div()
                .text_color(theme::text_dim())
                .child("Loading fleet status...")
        } else if let Some(fetched_at) = self.fleet.fetched_at {
            let up_to_date = rows
                .iter()
                .filter(|row| {
                    row.excluded.is_none() && row.pending.is_empty() && row.failed.is_empty()
                })
                .count();
            div().text_color(theme::text_dim()).child(format!(
                "{} database(s), {} up to date, refreshed {}s ago",
                rows.len(),
                up_to_date,
                fetched_at.elapsed().as_secs()
            ))
        } else if self.fleet.repo.is_some() {
            div()
                .text_color(theme::text_dim())
                .child("Repo open; connect and refresh to load fleet status")
        } else {
            div()
                .text_color(theme::text_dim())
                .child("Open a migration repo to see the fleet")
        };

        let modal = self
            .fleet
            .pending_action
            .clone()
            .map(|action| self.fleet_action_modal(action, cx));

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .text_color(theme::text())
            .text_sm()
            .child(toolbar)
            .child(controls)
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_h_0().child(list))
                    .when_some(detail, |content, detail| content.child(detail)),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(theme::bg_status())
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .child(status_line)
                    .when_some(repo.as_ref(), |strip, repo| {
                        let git = self.fleet.git.as_ref();
                        let stale = git.map(|git| git.stale()).unwrap_or(false);
                        strip.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .when_some(git, |chip, git| {
                                    chip.child(
                                        div()
                                            .text_color(if stale {
                                                theme::warning()
                                            } else {
                                                theme::text_dim()
                                            })
                                            .child(git.summary()),
                                    )
                                })
                                .child(div().text_color(theme::text_dim()).child(format!(
                                    "{}  |  {} migration(s)  |  ClickHouse {}",
                                    repo.root
                                        .file_name()
                                        .map(|name| name.to_string_lossy().to_string())
                                        .unwrap_or_else(|| repo.root.display().to_string()),
                                    repo.migrations.len(),
                                    repo.config.engine.version
                                ))),
                        )
                    }),
            )
            .when(self.fleet.filter_open, |root| {
                let mut card = div()
                    .id("fleet-db-filter-list")
                    .absolute()
                    .top(px(84.))
                    .left(px(12.))
                    .w(px(280.))
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .p_1()
                    .flex()
                    .flex_col();
                card = card.child(
                    div()
                        .id("fleet-db-filter-all")
                        .px_2()
                        .py_1()
                        .rounded(px(3.))
                        .text_color(theme::text_dim())
                        .child(if hidden_count > 0 {
                            "Show all"
                        } else {
                            "All shown"
                        })
                        .hover(|item| item.bg(theme::hover()).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.fleet.hidden_databases.clear();
                            cx.notify();
                        })),
                );
                for (index, row) in self.fleet.rows.iter().enumerate() {
                    let database = row.database.clone();
                    let checked = !self.fleet.hidden_databases.contains(&database);
                    card = card.child(
                        div()
                            .id(("fleet-db-filter-item", index))
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(12.))
                                    .rounded(px(2.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .when(checked, |tick| tick.bg(theme::success())),
                            )
                            .child(
                                div()
                                    .text_color(if checked {
                                        theme::text()
                                    } else {
                                        theme::text_dim()
                                    })
                                    .child(database.clone()),
                            )
                            .hover(|item| item.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.fleet.hidden_databases.remove(&database) {
                                    this.fleet.hidden_databases.insert(database.clone());
                                }
                                cx.notify();
                            })),
                    );
                }
                root.child(card)
            })
            .when_some(self.repo_picker_panel(cx), |root, panel| root.child(panel))
            .when_some(modal, |root, modal| root.child(modal))
            .when_some(self.author_panel(cx), |root, panel| root.child(panel))
            .when_some(self.codegen_panel(cx), |root, panel| root.child(panel))
            .when_some(self.commit_panel(cx), |root, panel| root.child(panel))
    }
}

/// A lock that blinks shut and open while macOS assesses the fresh
/// harness binary: path-swapped per animation frame, since SVGs
/// themselves cannot animate here.
pub(crate) fn verifying_lock(id: &'static str, size: f32) -> impl IntoElement {
    use gpui::{Animation, AnimationExt as _};
    svg()
        .path("icons/lock.svg")
        .size(px(size))
        .text_color(theme::warning())
        .with_animation(
            id,
            Animation::new(std::time::Duration::from_millis(1600)).repeat(),
            |lock, delta| {
                lock.path(if delta < 0.7 {
                    "icons/lock.svg"
                } else {
                    "icons/lock-open.svg"
                })
            },
        )
}

impl Workspace {
    /// The Verify-all icon button with its live state: percentage over
    /// a green fill while the harness downloads, a lock while macOS
    /// verifies it, a spinning hourglass while databases are diffed.
    fn fleet_verify_all_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let phase = self.fleet.harness_phase;
        let in_flight = self.fleet.drift_loading.len();
        let total = self.fleet.verify_total;
        // Every database verified and none drifted: the icon earns
        // its green (the state decays as soon as a row is unverified).
        let all_clean = in_flight == 0
            && !self.fleet.rows.is_empty()
            && self
                .fleet
                .rows
                .iter()
                .filter(|row| row.excluded.is_none())
                .all(|row| {
                    self.fleet
                        .drift
                        .get(&row.database)
                        .is_some_and(|info| info.findings.is_empty())
                });
        let button = div()
            .id("fleet-verify-all")
            .group("fleet-verify-all")
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .border_1()
            .border_color(if self.agent_highlight("verify_all") {
                theme::filter_tint()
            } else {
                theme::border()
            })
            .hover(|button| button.bg(theme::hover()).cursor_pointer())
            .on_click(cx.listener(|this, _, _, cx| this.fleet_verify_all(cx)));
        match phase {
            Some(zedb_ch::pin::PinPhase::Downloading { received, total }) => {
                let fraction = total
                    .filter(|total| *total > 0)
                    .map(|total| received as f32 / total as f32)
                    .unwrap_or(0.0);
                button
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui::relative(fraction.clamp(0.02, 1.0)))
                            .rounded(px(3.))
                            .bg(theme::success().opacity(0.35)),
                    )
                    .child(
                        div()
                            .relative()
                            .text_xs()
                            .text_color(theme::text())
                            .child(format!("{:.0}%", fraction * 100.0)),
                    )
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(match total {
                            Some(total) => format!(
                                "Getting a ClickHouse harness to verify against \u{b7} {} of {}",
                                Workspace::format_bytes(received),
                                Workspace::format_bytes(total),
                            ),
                            None => "Getting a ClickHouse harness to verify against".into(),
                        })
                        .build(window, cx)
                    })
            }
            Some(zedb_ch::pin::PinPhase::Verifying) => button
                .child(verifying_lock("fleet-verify-lock", 14.))
                .tooltip(|window, cx| {
                    gpui_component::tooltip::Tooltip::new(
                        "macOS is verifying the ClickHouse harness (first run only)",
                    )
                    .build(window, cx)
                }),
            None if in_flight > 0 => {
                use gpui::{percentage, Animation, AnimationExt as _, Transformation};
                use gpui_component::Sizable as _;
                let done = total.saturating_sub(in_flight);
                button
                    .child(
                        gpui_component::Icon::empty()
                            .path("icons/hourglass.svg")
                            .with_size(gpui_component::Size::Small)
                            .text_color(theme::text_dim())
                            .with_animation(
                                "fleet-verify-spin",
                                Animation::new(std::time::Duration::from_secs(1)).repeat(),
                                |icon, delta| {
                                    icon.transform(Transformation::rotate(percentage(delta)))
                                },
                            ),
                    )
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if total > 0 {
                            format!("Verifying \u{b7} {done}/{total} databases done")
                        } else {
                            "Verifying\u{2026}".to_string()
                        })
                        .build(window, cx)
                    })
            }
            None => button
                .child(
                    svg()
                        .path("icons/verify.svg")
                        .size(px(14.))
                        .text_color(if all_clean {
                            theme::success()
                        } else {
                            theme::text_dim()
                        })
                        .when(!all_clean, |icon| {
                            icon.group_hover("fleet-verify-all", |icon| {
                                icon.text_color(theme::text())
                            })
                        }),
                )
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(if all_clean {
                        "Verified: every database matches its chain position \u{b7} click to re-verify"
                    } else {
                        "Verify all: diff every database's live schema against its chain position"
                    })
                    .build(window, cx)
                }),
        }
    }
}

impl Workspace {
    /// The repo picker overlay: source menu, device-code wait, repo
    /// list with create, or the empty-directory confirmation.
    fn repo_picker_panel(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use crate::fleet::view::RepoPicker;
        let picker = self.fleet.repo_picker.as_ref()?;
        let row = |id: gpui::SharedString, label: String| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded(px(3.))
                .text_color(theme::text())
                .child(label)
                .hover(|item| item.bg(theme::hover()).cursor_pointer())
        };
        let mut card = div()
            .w(px(420.))
            .max_h(px(420.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child("OPEN A MIGRATION REPO"),
                    )
                    .child(
                        div()
                            .id("repo-picker-close")
                            .px_1()
                            .rounded(px(3.))
                            .text_color(theme::text_dim())
                            .child("\u{00d7}")
                            .hover(|close| {
                                close
                                    .bg(theme::hover())
                                    .text_color(theme::text())
                                    .cursor_pointer()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.repo_picker_close(cx))),
                    ),
            );
        match picker {
            RepoPicker::Menu => {
                card = card.child(
                    row("repo-picker-local".into(), "Local folder\u{2026}".into())
                        .on_click(cx.listener(|this, _, _, cx| this.repo_picker_local(cx))),
                );
                if let crate::GithubAuth::SignedIn(profile) = &self.github {
                    card = card.child(
                        row(
                            "repo-picker-git".into(),
                            format!("From {} \u{b7} {}", profile.provider.name(), profile.login),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.repo_picker_git(cx))),
                    );
                } else {
                    card = card.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(
                            "Sign in to a git host in Preferences to pick from your repositories",
                        ),
                    );
                }
            }
            RepoPicker::Authorizing { user_code } => {
                card = card.child(div().px_2().py_1().text_color(theme::text()).child(format!(
                    "Approve the elevated access in your browser \u{b7} code {user_code} (copied)"
                )));
            }
            RepoPicker::Loading(message) => {
                card = card.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_color(theme::text_dim())
                        .child(message.clone()),
                );
            }
            RepoPicker::Repos {
                repos, create_name, ..
            } => {
                let mut list = div()
                    .id("repo-picker-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_0p5();
                for (index, repo) in repos.iter().enumerate() {
                    let ssh_url = repo.ssh_url.clone();
                    list = list.child(
                        div()
                            .id(("repo-picker-repo", index))
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .text_color(theme::text())
                            .child(repo.full_name.clone())
                            .hover(|item| item.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.repo_picker_choose(ssh_url.clone(), cx)
                            })),
                    );
                }
                if repos.is_empty() {
                    list = list.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(theme::text_dim())
                            .child("No repositories"),
                    );
                }
                card = card.child(list).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .border_t_1()
                        .border_color(theme::border())
                        .child(div().flex_1().child(create_name.clone()))
                        .child(
                            div()
                                .id("repo-picker-create")
                                .px_3()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::success())
                                .text_color(theme::success())
                                .child("Create private repo")
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.repo_picker_create(cx)),
                                ),
                        ),
                );
            }
            RepoPicker::ConfirmInit { path, .. } => {
                card = card
                    .child(div().px_2().py_1().text_color(theme::text()).child(format!(
                        "{} is an empty directory. Create a new migration repo here?",
                        path.display()
                    )))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("repo-picker-init")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::success())
                                    .text_color(theme::success())
                                    .child("Create migration repo")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.repo_picker_confirm_init(cx)
                                    })),
                            )
                            .child(
                                div()
                                    .id("repo-picker-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .text_color(theme::text_dim())
                                    .child("Cancel")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.repo_picker_close(cx)),
                                    ),
                            ),
                    );
            }
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .occlude()
                .child(card)
                .into_any_element(),
        )
    }
}
