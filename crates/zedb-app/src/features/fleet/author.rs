//! In-app migration authoring (docs/PHASE-3.md M1).
//!
//! A draft lives entirely in memory: the SQL is checked against the
//! repo's pinned ClickHouse binary while it is still editor text, and
//! nothing touches the chain until the checks pass and the user saves.
//! Saving scaffolds the next migration directory and writes the draft,
//! then reopens the repo so the fleet view sees the new tip.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{prelude::*, Context, Entity, Window};
use gpui_component::input::InputState;
use zedb_core::repo::{scaffold_migration, MigrationRepo, ScaffoldOptions};

use crate::rt;
use crate::Workspace;

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
    pub(crate) const ALL: [Self; 4] = [
        Self::Clean,
        Self::Structural,
        Self::Irreversible,
        Self::NoFile,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Structural => "structural",
            Self::Irreversible => "irreversible",
            Self::NoFile => "no rollback",
        }
    }

    pub(crate) fn marker(self) -> Option<&'static str> {
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
    /// Some when this draft edits an existing migration in place; None
    /// when it will scaffold a new chain tip on save.
    pub existing: Option<u32>,
    /// How many databases the fleet status reports this migration
    /// applied (or customised) on; more than zero makes it read-only.
    pub applied_on: usize,
    /// Whether any fleet status has been fetched; without it, editing
    /// an existing migration is allowed but carries a warning.
    pub status_known: bool,
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

    pub(crate) fn saveable(&self, cx: &Context<Workspace>) -> bool {
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
            existing: None,
            applied_on: 0,
            status_known: true,
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

    /// Open an existing migration for viewing, or editing when the
    /// fleet status says it has never been applied on any database.
    /// The chain is append-only precisely because history ran
    /// somewhere; until it has run anywhere, it is still a draft.
    pub(crate) fn author_open_migration(
        &mut self,
        number: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = &self.fleet.repo else {
            return;
        };
        let Some(migration) = repo.migration(number) else {
            return;
        };
        let upgrade_text = migration.upgrade_sql().unwrap_or_default();
        let rollback_text = migration.rollback_sql().ok().flatten();
        let targeted = migration.targeted.is_some();
        let rollback_choice = match &rollback_text {
            None => RollbackChoice::NoFile,
            Some(text) => {
                let marker = text.lines().next().unwrap_or("").trim();
                match marker.strip_prefix("-- rollback-class:").map(str::trim) {
                    Some("structural") => RollbackChoice::Structural,
                    Some("irreversible") => RollbackChoice::Irreversible,
                    _ => RollbackChoice::Clean,
                }
            }
        };
        let status_known = self.fleet.fetched_at.is_some();
        let applied_on = self
            .fleet
            .rows
            .iter()
            .filter(|row| {
                if targeted {
                    row.customised.contains(&number)
                } else {
                    row.excluded.is_none()
                        && row.head.is_some_and(|head| head >= number)
                        && !row.pending.contains(&number)
                }
            })
            .count();
        let rollback_template = rollback_text.unwrap_or_else(|| {
            format!(
                "{}\n\n-- Revert every object upgrade.sql creates or changes.\n",
                RollbackChoice::Clean.marker().unwrap()
            )
        });
        self.author = Some(AuthorState {
            number,
            existing: Some(number),
            applied_on,
            status_known,
            targeted,
            rollback_choice,
            upgrade: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sql")
                    .default_value(upgrade_text)
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

    pub(crate) fn author_set_rollback_choice(
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
        if author.applied_on > 0 {
            return;
        }
        let editing = author.existing.is_some();
        let (upgrade, rollback) = author.texts(cx);
        let with_rollback = author.rollback_choice != RollbackChoice::NoFile;
        let outcome = match author.existing {
            Some(number) => Self::author_rewrite(
                &repo,
                number,
                author.targeted,
                &upgrade,
                with_rollback.then_some(&rollback),
            ),
            None => {
                let options = ScaffoldOptions {
                    description: String::new(),
                    targeted: author.targeted,
                    with_rollback,
                };
                Self::author_write(
                    &repo,
                    &options,
                    &upgrade,
                    with_rollback.then_some(&rollback),
                )
            }
        };
        match outcome {
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
                    "{} migration {number:05} at {}",
                    if editing { "Updated" } else { "Created" },
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

    /// Rewrite an existing, never-applied migration in place: the SQL
    /// files, the rollback file's presence, and the targeted marker all
    /// follow the draft.
    fn author_rewrite(
        repo: &MigrationRepo,
        number: u32,
        targeted: bool,
        upgrade: &str,
        rollback: Option<&str>,
    ) -> Result<(u32, PathBuf), String> {
        let migration = repo
            .migration(number)
            .ok_or_else(|| format!("migration {number:05} vanished from the chain"))?;
        let directory = migration.directory.clone();
        std::fs::write(directory.join("upgrade.sql"), upgrade)
            .map_err(|error| error.to_string())?;
        let rollback_path = directory.join("rollback.sql");
        match rollback {
            Some(rollback) => {
                std::fs::write(&rollback_path, rollback).map_err(|error| error.to_string())?
            }
            None => {
                if rollback_path.exists() {
                    std::fs::remove_file(&rollback_path).map_err(|error| error.to_string())?;
                }
            }
        }
        let targeted_path = directory.join("targeted.toml");
        if targeted && !targeted_path.exists() {
            std::fs::write(
                &targeted_path,
                "# Opt-in migration: applied per database with `zedb apply`.\n# allow_list = [\"db_name\"]\n",
            )
            .map_err(|error| error.to_string())?;
        } else if !targeted && targeted_path.exists() {
            std::fs::remove_file(&targeted_path).map_err(|error| error.to_string())?;
        }
        Ok((number, directory))
    }
}
