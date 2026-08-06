//! In-app codegen and chain checks (docs/PHASE-3.md M2).
//!
//! Regen replays the chain through the pinned binary in the background
//! and shows the churn (which current-state files would change, appear,
//! or vanish) before anything is written; writing is a separate,
//! explicit step. The chain checks (sql, equivalence, lifecycle) run
//! concurrently with per-check progress, rendering the same reports the
//! CLI prints.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::{div, prelude::*, px, rgb, Context};
use zedb_core::repo::MigrationRepo;

use crate::rt;
use crate::theme::{BG_SIDEBAR, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};
use crate::Workspace;

/// One chain check's lifecycle in the modal.
pub enum CheckSlot {
    Running,
    /// Passed, with the CLI-style summary line.
    Pass(String),
    /// Failed, with the report's difference/error lines.
    Fail(Vec<String>),
}

pub struct ChecksState {
    pub generation: u64,
    pub slots: [CheckSlot; 3],
}

pub const CHECK_NAMES: [&str; 3] = ["sql", "equivalence", "lifecycle"];

pub struct RegenState {
    pub running: bool,
    pub generation: u64,
    pub error: Option<String>,
    /// The generated tree and its human-readable churn, awaiting the
    /// explicit write step.
    pub result: Option<(BTreeMap<String, String>, Vec<String>)>,
}

impl Workspace {
    pub(crate) fn codegen_start_regen(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let generation = self
            .regen
            .as_ref()
            .map(|state| state.generation + 1)
            .unwrap_or(0);
        self.regen = Some(RegenState {
            running: true,
            generation,
            error: None,
            result: None,
        });
        cx.notify();

        let handle = rt::tokio().spawn(async move {
            let version = repo.config.engine.version.clone();
            let binary = zedb_ch::ensure_binary(&version)
                .await
                .map_err(|error| error.to_string())?;
            tokio::task::spawn_blocking(move || {
                let files = zedb_ch::regen::Regenerator::new(&repo, binary)
                    .regenerate()
                    .map_err(|error| error.to_string())?;
                let diff =
                    zedb_ch::regen::diff_tree(&repo, &files).map_err(|error| error.to_string())?;
                Ok::<_, String>((files, diff))
            })
            .await
            .map_err(|error| error.to_string())?
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                let Some(regen) = &mut this.regen else {
                    return;
                };
                if regen.generation != generation {
                    return;
                }
                regen.running = false;
                match result.map_err(|error| error.to_string()) {
                    Ok(Ok(result)) => regen.result = Some(result),
                    Ok(Err(error)) | Err(error) => regen.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn codegen_write(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some((files, _)) = self.regen.as_ref().and_then(|state| state.result.clone()) else {
            return;
        };
        match zedb_ch::regen::write_tree(&repo, &files) {
            Ok((changed, removed)) => {
                if let Ok(reopened) = MigrationRepo::open_root(&repo.root) {
                    self.fleet.repo = Some(Arc::new(reopened));
                }
                self.fleet.git = zedb_core::git::read_git_status(&repo.root);
                self.notice = Some(format!(
                    "current-state written: {} file(s) changed, {} removed",
                    changed.len(),
                    removed.len()
                ));
                self.notice_warning = false;
                self.regen = None;
            }
            Err(error) => {
                if let Some(regen) = &mut self.regen {
                    regen.error = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn codegen_start_checks(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let generation = self
            .checks
            .as_ref()
            .map(|state| state.generation + 1)
            .unwrap_or(0);
        self.checks = Some(ChecksState {
            generation,
            slots: [CheckSlot::Running, CheckSlot::Running, CheckSlot::Running],
        });
        cx.notify();

        for index in 0..3 {
            let repo = repo.clone();
            let handle = rt::tokio().spawn(async move {
                let version = repo.config.engine.version.clone();
                let binary = zedb_ch::ensure_binary(&version)
                    .await
                    .map_err(|error| vec![error.to_string()])?;
                match index {
                    0 => tokio::task::spawn_blocking(move || {
                        let report = zedb_ch::checks::check_sql(&binary, &repo)
                            .map_err(|error| vec![error.to_string()])?;
                        if report.errors.is_empty() {
                            Ok(format!(
                                "{} SQL files parse as valid ClickHouse",
                                report.checked
                            ))
                        } else {
                            Err(report.errors)
                        }
                    })
                    .await
                    .map_err(|error| vec![error.to_string()])?,
                    1 => tokio::task::spawn_blocking(move || {
                        let report = zedb_ch::checks::check_equivalence(binary, &repo)
                            .map_err(|error| vec![error.to_string()])?;
                        if report.differences.is_empty() {
                            Ok(format!(
                                "current-state ({} objects) matches the chain replay ({})",
                                report.state_objects, report.chain_objects
                            ))
                        } else {
                            Err(report.differences)
                        }
                    })
                    .await
                    .map_err(|error| vec![error.to_string()])?,
                    _ => {
                        let report = zedb_ch::lifecycle::check_lifecycle(&repo, binary)
                            .await
                            .map_err(|error| vec![error.to_string()])?;
                        if report.differences.is_empty() {
                            Ok(format!(
                                "up, down, up again: {} live objects match {} expected",
                                report.live_objects, report.expected_objects
                            ))
                        } else {
                            Err(report.differences)
                        }
                    }
                }
            });
            cx.spawn(async move |this, cx| {
                let result = handle.await;
                this.update(cx, |this, cx| {
                    let Some(checks) = &mut this.checks else {
                        return;
                    };
                    if checks.generation != generation {
                        return;
                    }
                    checks.slots[index] = match result.map_err(|error| vec![error.to_string()]) {
                        Ok(Ok(summary)) => CheckSlot::Pass(summary),
                        Ok(Err(errors)) | Err(errors) => CheckSlot::Fail(errors),
                    };
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    pub(crate) fn codegen_panel(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.regen.is_none() && self.checks.is_none() {
            return None;
        }

        let mut body = div()
            .id("codegen-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2();
        let mut title = "";
        let mut writable = false;

        if let Some(regen) = &self.regen {
            title = "Regenerate current-state";
            if regen.running {
                body = body.child(
                    div()
                        .text_color(rgb(TEXT_DIM))
                        .child("Replaying the chain through the pinned server..."),
                );
            }
            if let Some(error) = &regen.error {
                body = body.child(div().text_color(rgb(DANGER)).child(error.clone()));
            }
            if let Some((files, diff)) = &regen.result {
                if diff.is_empty() {
                    body = body.child(div().text_color(rgb(SUCCESS)).child(format!(
                        "current-state is in sync ({} generated files); nothing to write",
                        files.len()
                    )));
                } else {
                    writable = true;
                    body = body.child(div().text_color(rgb(TEXT)).child(format!(
                        "Writing would change {} of {} generated file(s):",
                        diff.len(),
                        files.len()
                    )));
                    let mut list = div()
                        .p_2()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .text_xs()
                        .font_family("Menlo")
                        .flex()
                        .flex_col()
                        .gap_1();
                    for line in diff {
                        list = list.child(div().text_color(rgb(TEXT_DIM)).child(line.clone()));
                    }
                    body = body.child(list);
                }
            }
        } else if let Some(checks) = &self.checks {
            title = "Chain checks";
            for (name, slot) in CHECK_NAMES.iter().zip(&checks.slots) {
                let row = div().flex().flex_col().gap_1();
                let row = match slot {
                    CheckSlot::Running => row.child(
                        div()
                            .text_color(rgb(TEXT_DIM))
                            .child(format!("{name}: running...")),
                    ),
                    CheckSlot::Pass(summary) => row.child(
                        div()
                            .text_color(rgb(SUCCESS))
                            .child(format!("{name}: {summary}")),
                    ),
                    CheckSlot::Fail(errors) => {
                        let mut list = div()
                            .p_2()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(DANGER))
                            .text_color(rgb(DANGER))
                            .text_xs()
                            .font_family("Menlo")
                            .flex()
                            .flex_col()
                            .gap_1();
                        for error in errors {
                            list = list.child(error.clone());
                        }
                        row.child(
                            div()
                                .text_color(rgb(DANGER))
                                .child(format!("{name}: failed")),
                        )
                        .child(list)
                    }
                };
                body = body.child(row);
            }
        }

        let footer = div()
            .flex_none()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .items_center()
            .gap_2()
            .when(writable, |footer| {
                footer.child(
                    div()
                        .id("codegen-write")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(SUCCESS))
                        .text_color(rgb(SUCCESS))
                        .child("Write current-state")
                        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| this.codegen_write(cx))),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("codegen-close")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .text_color(rgb(TEXT_DIM))
                    .child("Close")
                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.regen = None;
                        this.checks = None;
                        cx.notify();
                    })),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .child(
                    div()
                        .w(px(680.))
                        .max_h(px(560.))
                        .flex()
                        .flex_col()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG_SIDEBAR))
                        .child(
                            div()
                                .flex_none()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(rgb(BORDER))
                                .child(title),
                        )
                        .child(body)
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}
