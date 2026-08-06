//! In-app commit and push (docs/PHASE-3.md M3).
//!
//! The commit stages exactly the repo's own files (migrations,
//! current-state, config) and commits them with an editable templated
//! message; anything else dirty in the checkout is shown but left
//! strictly alone. Push uses the user's own git auth and hands git's
//! words back verbatim on failure. zeDB never resolves conflicts or
//! rewrites history; the user's normal git workflow is the escape
//! hatch.

use gpui::{div, prelude::*, px, rgb, Context, Entity, Window};
use gpui_component::input::{Input, InputState};

use crate::rt;
use crate::theme::{BG, BG_SIDEBAR, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};
use crate::Workspace;

/// Checkout paths a migration repo owns; only these are ever staged.
fn repo_owned(path: &str) -> bool {
    path.starts_with("migrations")
        || path.starts_with("current-state")
        || path == "zedb.toml"
        || path == "exclusions.toml"
}

pub struct CommitState {
    pub message: Entity<InputState>,
    /// Repo-owned dirty paths; exactly what the commit will name.
    pub include: Vec<String>,
    /// Other dirty paths in the checkout, shown and left alone.
    pub others: Vec<String>,
    pub committed: Option<String>,
    pub pushing: bool,
    pub push_result: Option<Result<String, String>>,
    pub error: Option<String>,
    pub generation: u64,
}

impl Workspace {
    pub(crate) fn commit_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = &self.fleet.repo else {
            return;
        };
        let Some(changed) = zedb_core::git::changed_paths(&repo.root) else {
            self.notice = Some("This repo is not a git checkout".into());
            self.notice_warning = true;
            self.notice_flash_id += 1;
            cx.notify();
            return;
        };
        let (include, others): (Vec<String>, Vec<String>) =
            changed.into_iter().partition(|path| repo_owned(path));
        let template = repo
            .migrations
            .last()
            .map(|tip| {
                format!(
                    "migration {:05}: {}",
                    tip.number,
                    tip.headline().unwrap_or_else(|| "describe it".into())
                )
            })
            .unwrap_or_else(|| "update migration repo".into());
        self.commit = Some(CommitState {
            message: cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .auto_grow(2, 6)
                    .default_value(template)
            }),
            include,
            others,
            committed: None,
            pushing: false,
            push_result: None,
            error: None,
            generation: self.commit.as_ref().map(|c| c.generation + 1).unwrap_or(0),
        });
        cx.notify();
    }

    pub(crate) fn commit_run(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(commit) = &mut self.commit else {
            return;
        };
        if commit.committed.is_some() || commit.include.is_empty() {
            return;
        }
        let message = commit.message.read(cx).value().trim().to_string();
        if message.is_empty() {
            commit.error = Some("write a commit message first".into());
            cx.notify();
            return;
        }
        match zedb_core::git::commit_paths(&repo.root, &commit.include, &message) {
            Ok(hash) => {
                commit.committed = Some(hash);
                commit.error = None;
                self.fleet.git = zedb_core::git::read_git_status(&repo.root);
            }
            Err(error) => commit.error = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn commit_push(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(commit) = &mut self.commit else {
            return;
        };
        if commit.pushing {
            return;
        }
        commit.pushing = true;
        commit.push_result = None;
        let generation = commit.generation;
        cx.notify();

        let root = repo.root.clone();
        let handle = rt::tokio().spawn(async move {
            tokio::task::spawn_blocking(move || zedb_core::git::push(&root)).await
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                let root = this.fleet.repo.as_ref().map(|repo| repo.root.clone());
                let Some(commit) = &mut this.commit else {
                    return;
                };
                if commit.generation != generation {
                    return;
                }
                commit.pushing = false;
                commit.push_result = Some(
                    match result {
                        Ok(Ok(outcome)) => outcome,
                        Ok(Err(error)) | Err(error) => Err(error.to_string()),
                    }
                    .map(|output| {
                        if output.is_empty() {
                            "pushed".into()
                        } else {
                            output
                        }
                    }),
                );
                if let Some(root) = root {
                    this.fleet.git = zedb_core::git::read_git_status(&root);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn commit_panel(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let commit = self.commit.as_ref()?;
        let committed = commit.committed.clone();
        let pushing = commit.pushing;
        let can_commit = committed.is_none() && !commit.include.is_empty();

        let mut body = div()
            .id("commit-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2();

        if commit.include.is_empty() && committed.is_none() {
            body = body.child(
                div()
                    .text_color(rgb(TEXT_DIM))
                    .child("No repo-owned changes to commit."),
            );
        } else if committed.is_none() {
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
            for path in &commit.include {
                list = list.child(div().text_color(rgb(TEXT)).child(path.clone()));
            }
            body = body
                .child(div().text_color(rgb(TEXT_DIM)).child("Will commit:"))
                .child(list)
                .child(
                    div()
                        .flex_none()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .child(
                            Input::new(&commit.message)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .pl(px(4.)),
                        ),
                );
        }
        if !commit.others.is_empty() {
            body = body.child(div().text_color(rgb(TEXT_DIM)).text_xs().child(format!(
                "{} other changed path(s) in the checkout stay untouched: {}",
                commit.others.len(),
                commit.others.join(", ")
            )));
        }
        if let Some(hash) = &committed {
            body = body.child(
                div()
                    .text_color(rgb(SUCCESS))
                    .child(format!("Committed {hash}. Push when ready.")),
            );
        }
        if let Some(error) = &commit.error {
            body = body.child(div().text_color(rgb(DANGER)).child(error.clone()));
        }
        match &commit.push_result {
            Some(Ok(output)) => {
                body = body.child(div().text_color(rgb(SUCCESS)).child(output.clone()));
            }
            Some(Err(error)) => {
                body = body.child(
                    div()
                        .p_2()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(DANGER))
                        .text_color(rgb(DANGER))
                        .text_xs()
                        .font_family("Menlo")
                        .child(error.clone()),
                );
            }
            None => {}
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
            .when(can_commit, |footer| {
                footer.child(
                    div()
                        .id("commit-run")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(SUCCESS))
                        .text_color(rgb(SUCCESS))
                        .child("Commit")
                        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| this.commit_run(cx))),
                )
            })
            .when(committed.is_some(), |footer| {
                footer.child(
                    div()
                        .id("commit-push")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(if pushing { BORDER } else { SUCCESS }))
                        .text_color(rgb(if pushing { TEXT_DIM } else { SUCCESS }))
                        .child(if pushing { "Pushing..." } else { "Push" })
                        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| this.commit_push(cx))),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("commit-close")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .text_color(rgb(TEXT_DIM))
                    .child("Close")
                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.commit = None;
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
                        .w(px(600.))
                        .max_h(px(520.))
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
                                .child("Commit and push"),
                        )
                        .child(body)
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}
