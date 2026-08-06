//! In-app migration authoring (docs/PHASE-3.md M1).
//!
//! A draft lives entirely in memory: the SQL is checked against the
//! repo's pinned ClickHouse binary while it is still editor text, and
//! nothing touches the chain until the checks pass and the user saves.
//! Saving scaffolds the next migration directory and writes the draft,
//! then reopens the repo so the fleet view sees the new tip.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{div, prelude::*, px, rgb, Context, Entity, Window};
use gpui_component::input::{Input, InputState};
use zedb_core::repo::{scaffold_migration, MigrationRepo, ScaffoldOptions};

use crate::rt;
use crate::theme::{BG, BG_SIDEBAR, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};
use crate::Workspace;

const WARN: u32 = 0xd7a65f;

/// The rollback story a draft declares; NoFile means irreversible by
/// omission (no rollback.sql at all).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RollbackChoice {
    Clean,
    Structural,
    Irreversible,
    NoFile,
}

impl RollbackChoice {
    const ALL: [Self; 4] = [
        Self::Clean,
        Self::Structural,
        Self::Irreversible,
        Self::NoFile,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Structural => "structural",
            Self::Irreversible => "irreversible",
            Self::NoFile => "no rollback",
        }
    }

    fn marker(self) -> Option<&'static str> {
        match self {
            Self::Clean => Some("-- rollback-class: clean"),
            Self::Structural => Some("-- rollback-class: structural"),
            Self::Irreversible => Some("-- rollback-class: irreversible"),
            Self::NoFile => None,
        }
    }
}

/// An in-memory migration draft being authored.
pub struct AuthorState {
    pub number: u32,
    pub targeted: bool,
    pub rollback_choice: RollbackChoice,
    pub upgrade: Entity<InputState>,
    pub rollback: Entity<InputState>,
    pub checking: bool,
    pub check_generation: u64,
    /// Errors from the last completed check (empty means it passed).
    pub check_errors: Option<Vec<String>>,
    /// The exact texts the last passing check covered; saving is only
    /// allowed while the editors still match them.
    pub clean_for: Option<(String, String)>,
}

impl AuthorState {
    fn texts(&self, cx: &Context<Workspace>) -> (String, String) {
        (
            self.upgrade.read(cx).value().to_string(),
            self.rollback.read(cx).value().to_string(),
        )
    }

    fn saveable(&self, cx: &Context<Workspace>) -> bool {
        let (upgrade, rollback) = self.texts(cx);
        self.check_errors.as_ref().is_some_and(|e| e.is_empty())
            && self.clean_for.as_ref() == Some(&(upgrade, rollback))
    }
}

impl Workspace {
    pub(crate) fn author_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = &self.fleet.repo else {
            return;
        };
        let number = repo.next_number();
        let upgrade_template = format!("-- migration {number:05}: describe the change here\n\n");
        let rollback_template = format!(
            "{}\n\n-- Revert every object upgrade.sql creates or changes.\n",
            RollbackChoice::Clean.marker().unwrap()
        );
        self.author = Some(AuthorState {
            number,
            targeted: false,
            rollback_choice: RollbackChoice::Clean,
            upgrade: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sql")
                    .default_value(upgrade_template)
            }),
            rollback: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sql")
                    .default_value(rollback_template)
            }),
            checking: false,
            check_generation: 0,
            check_errors: None,
            clean_for: None,
        });
        cx.notify();
    }

    fn author_set_rollback_choice(
        &mut self,
        choice: RollbackChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(author) = &mut self.author else {
            return;
        };
        author.rollback_choice = choice;
        if let Some(marker) = choice.marker() {
            let text = author.rollback.read(cx).value().to_string();
            let rest = text
                .split_once('\n')
                .filter(|(first, _)| first.trim_start().starts_with("-- rollback-class:"))
                .map(|(_, rest)| rest.to_string())
                .unwrap_or(text);
            author.rollback.update(cx, |editor, cx| {
                editor.set_value(format!("{marker}\n{rest}"), window, cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn author_check(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(author) = &mut self.author else {
            return;
        };
        let (upgrade, rollback) = (
            author.upgrade.read(cx).value().to_string(),
            author.rollback.read(cx).value().to_string(),
        );
        let with_rollback = author.rollback_choice != RollbackChoice::NoFile;
        let number = author.number;
        author.checking = true;
        author.check_generation += 1;
        let generation = author.check_generation;
        cx.notify();

        let upgrade_for_check = upgrade.clone();
        let rollback_for_check = rollback.clone();
        let handle = rt::tokio().spawn(async move {
            let version = repo.config.engine.version.clone();
            let binary = zedb_ch::ensure_binary(&version)
                .await
                .map_err(|error| error.to_string())?;
            tokio::task::spawn_blocking(move || {
                let params = zedb_ch::checks::dummy_params(&repo);
                let mut errors = Vec::new();
                let mut check = |text: &str, label: String| match zedb_ch::checks::check_sql_text(
                    &binary, text, &params, &label,
                ) {
                    Ok(Some(error)) => errors.push(error),
                    Ok(None) => {}
                    Err(error) => errors.push(format!("{label}: {error}")),
                };
                check(&upgrade_for_check, format!("{number:05}/upgrade.sql"));
                if with_rollback {
                    check(&rollback_for_check, format!("{number:05}/rollback.sql"));
                }
                errors
            })
            .await
            .map_err(|error| error.to_string())
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                let Some(author) = &mut this.author else {
                    return;
                };
                if author.check_generation != generation {
                    return;
                }
                author.checking = false;
                match result.map_err(|error| error.to_string()) {
                    Ok(Ok(errors)) => {
                        author.clean_for = errors.is_empty().then_some((upgrade, rollback));
                        author.check_errors = Some(errors);
                    }
                    Ok(Err(error)) | Err(error) => {
                        author.check_errors = Some(vec![error]);
                        author.clean_for = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn author_save(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(author) = &self.author else {
            return;
        };
        if !author.saveable(cx) {
            return;
        }
        let (upgrade, rollback) = author.texts(cx);
        let with_rollback = author.rollback_choice != RollbackChoice::NoFile;
        let options = ScaffoldOptions {
            description: String::new(),
            targeted: author.targeted,
            with_rollback,
        };
        match Self::author_write(
            &repo,
            &options,
            &upgrade,
            with_rollback.then_some(&rollback),
        ) {
            Ok((number, directory)) => {
                match MigrationRepo::open_root(&repo.root) {
                    Ok(reopened) => {
                        self.fleet.repo = Some(Arc::new(reopened));
                        self.fleet.git = zedb_core::git::read_git_status(&repo.root);
                    }
                    Err(error) => self.fleet.repo_error = Some(error.to_string()),
                }
                self.author = None;
                self.notice = Some(format!(
                    "Created migration {number:05} at {}",
                    directory.display()
                ));
                self.notice_warning = false;
                self.fleet_refresh(cx);
            }
            Err(error) => {
                self.notice = Some(format!("Could not save migration: {error}"));
                self.notice_warning = true;
                self.notice_flash_id += 1;
            }
        }
        cx.notify();
    }

    /// Scaffold the next migration and replace its templates with the
    /// draft. Everything the draft did not opt into stays unwritten.
    fn author_write(
        repo: &MigrationRepo,
        options: &ScaffoldOptions,
        upgrade: &str,
        rollback: Option<&str>,
    ) -> Result<(u32, PathBuf), String> {
        let number = repo.next_number();
        let directory = scaffold_migration(repo, options).map_err(|error| error.to_string())?;
        std::fs::write(directory.join("upgrade.sql"), upgrade)
            .map_err(|error| error.to_string())?;
        if let Some(rollback) = rollback {
            std::fs::write(directory.join("rollback.sql"), rollback)
                .map_err(|error| error.to_string())?;
        }
        Ok((number, directory))
    }

    pub(crate) fn author_panel(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let author = self.author.as_ref()?;
        let number = author.number;
        let checking = author.checking;
        let saveable = author.saveable(cx);
        let targeted = author.targeted;
        let choice = author.rollback_choice;
        let with_rollback = choice != RollbackChoice::NoFile;
        let check_errors = author.check_errors.clone();
        let stale_check = author.check_errors.is_some() && !saveable && !checking;

        let mut classes = div().flex().items_center().gap_1();
        for option in RollbackChoice::ALL {
            let selected = option == choice;
            classes = classes.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "author-class-{}",
                        option.label()
                    )))
                    .px_2()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(if selected { TEXT } else { BORDER }))
                    .text_color(rgb(if selected { TEXT } else { TEXT_DIM }))
                    .child(option.label())
                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.author_set_rollback_choice(option, window, cx);
                    })),
            );
        }

        let editor = |label: &'static str, state: &Entity<InputState>, height: f32| {
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(rgb(TEXT_DIM)).text_xs().child(label))
                .child(
                    div()
                        .h(px(height))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .child(
                            Input::new(state)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .pl(px(4.))
                                .h_full(),
                        ),
                )
        };

        let mut body = div()
            .id("author-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_color(rgb(TEXT_DIM)).child("rollback:"))
                    .child(classes)
                    .child(
                        div()
                            .id("author-targeted")
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(if targeted { TEXT } else { BORDER }))
                            .text_color(rgb(if targeted { TEXT } else { TEXT_DIM }))
                            .child(if targeted {
                                "targeted (opt-in)"
                            } else {
                                "chain migration"
                            })
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(author) = &mut this.author {
                                    author.targeted = !author.targeted;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(editor("upgrade.sql", &author.upgrade, 200.));
        if with_rollback {
            body = body.child(editor("rollback.sql", &author.rollback, 140.));
        }
        if let Some(errors) = &check_errors {
            if errors.is_empty() {
                body = body.child(div().text_color(rgb(SUCCESS)).child(if stale_check {
                    "Checks passed, but the draft changed since; check again."
                } else {
                    "Checks passed against the pinned server."
                }));
            } else {
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
                body = body.child(list);
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
            .child(
                div()
                    .id("author-check")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(if checking { TEXT_DIM } else { TEXT }))
                    .child(if checking { "Checking..." } else { "Check" })
                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| this.author_check(cx))),
            )
            .child(
                div()
                    .id("author-save")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(if saveable { SUCCESS } else { BORDER }))
                    .text_color(rgb(if saveable { SUCCESS } else { TEXT_DIM }))
                    .child("Save to repo")
                    .when(saveable, |button| {
                        button
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.author_save(cx)))
                    })
                    .when(!saveable, |button| {
                        button.tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Run Check on the current draft first; saving requires a clean check",
                            )
                            .build(window, cx)
                        })
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("author-cancel")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .text_color(rgb(TEXT_DIM))
                    .child("Discard draft")
                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.author = None;
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
                        .w(px(760.))
                        .max_h(px(620.))
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
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(format!("New migration {number:05}"))
                                .child(div().text_color(rgb(WARN)).text_xs().child(
                                    "Draft only: nothing is written until checks pass and you save",
                                )),
                        )
                        .child(body)
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}
