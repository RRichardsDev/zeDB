//! The fleet view (docs/PHASE-2.md M0-M1): open a migration repo and
//! render the databases x migrations matrix for the active connection.
//!
//! BYO git: "opening a repo" means pointing at a local checkout
//! directory; committing and pushing stay in the user's git workflow.
//! Everything shown here comes from the same zedb-core/zedb-ch calls the
//! CLI makes; this file only fetches and renders.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use gpui::{div, prelude::*, px, rgb, svg, uniform_list, Context, Entity, SharedString};
use zedb_ch::runner::{Runner, RunnerOptions, Targets};
use zedb_ch::verify::Verifier;
use zedb_core::repo::MigrationRepo;
use zedb_core::save_preferences;

use crate::components::text_input::TextInput;
use crate::rt;
use crate::theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};
use crate::Workspace;

const ACCENT_PENDING: u32 = 0xd7a65f;
const ACCENT_CUSTOM: u32 = 0x9d7cd8;
const ROW_HEIGHT: f32 = 26.0;

/// One matrix row, precomputed from the runner's status data.
#[derive(Clone)]
pub struct FleetRow {
    pub database: String,
    pub head: Option<u32>,
    pub pending: Vec<u32>,
    pub customised: Vec<u32>,
    pub failed: Vec<u32>,
    /// The exclusion group parking this database, when any.
    pub excluded: Option<String>,
}

/// Drift findings for one database, from `zedb verify` semantics.
pub struct DriftInfo {
    pub findings: Vec<String>,
    pub checked_at: Instant,
}

pub struct FleetState {
    pub repo_path: Entity<TextInput>,
    pub filter: Entity<TextInput>,
    pub repo: Option<Arc<MigrationRepo>>,
    pub repo_error: Option<String>,
    pub rows: Vec<FleetRow>,
    pub fetch_error: Option<String>,
    pub loading: bool,
    pub fetched_at: Option<Instant>,
    pub selected: Option<String>,
    pub fetch_generation: u64,
    pub drift: HashMap<String, DriftInfo>,
    pub drift_loading: HashSet<String>,
    pub drift_error: Option<String>,
}

impl FleetState {
    pub fn new(initial_path: &str, window: &mut gpui::Window, cx: &mut Context<Workspace>) -> Self {
        let _ = window;
        let initial_path = initial_path.to_string();
        let repo_path =
            cx.new(move |cx| TextInput::new(&initial_path, "path to a migration repo", false, cx));
        let filter = cx.new(|cx| TextInput::new("", "Filter databases", false, cx));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        Self {
            repo_path,
            filter,
            repo: None,
            repo_error: None,
            rows: Vec::new(),
            fetch_error: None,
            loading: false,
            fetched_at: None,
            selected: None,
            fetch_generation: 0,
            drift: HashMap::new(),
            drift_loading: HashSet::new(),
            drift_error: None,
        }
    }
}

/// Substitute the parameters we know (the database and declared
/// defaults), leaving anything unresolved as a visible `${name}`
/// placeholder: the dry-run shows exactly what is and is not decided yet.
fn render_lenient(sql: &str, params: &BTreeMap<String, String>) -> String {
    let mut rendered = sql.to_string();
    for (name, value) in params {
        rendered = rendered.replace(&format!("${{{name}}}"), value);
    }
    rendered
}

/// Cell state for one (database, migration) pair.
#[derive(Clone, Copy, PartialEq)]
enum Cell {
    Applied,
    Pending,
    Failed,
    Customised,
    /// A targeted migration this database has not opted into.
    NotApplicable,
}

fn cell_for(row: &FleetRow, number: u32, targeted: bool) -> Cell {
    if row.failed.contains(&number)
        && !row.customised.contains(&number)
        && row.pending.contains(&number)
    {
        return Cell::Failed;
    }
    if targeted {
        if row.customised.contains(&number) {
            return Cell::Customised;
        }
        return Cell::NotApplicable;
    }
    if row.pending.contains(&number) {
        Cell::Pending
    } else {
        Cell::Applied
    }
}

impl Workspace {
    pub(crate) fn fleet_open_repo(&mut self, cx: &mut Context<Self>) {
        let path_text = self.fleet.repo_path.read(cx).text().trim().to_string();
        if path_text.is_empty() {
            self.fleet.repo_error = Some("Enter the path of a migration repo checkout".into());
            cx.notify();
            return;
        }
        let expanded = match (path_text.strip_prefix("~/"), std::env::var_os("HOME")) {
            (Some(rest), Some(home)) => Path::new(&home).join(rest),
            _ => Path::new(&path_text).to_path_buf(),
        };
        match MigrationRepo::open(&expanded) {
            Ok(repo) => {
                self.fleet.repo = Some(Arc::new(repo));
                self.fleet.repo_error = None;
                self.fleet.rows.clear();
                self.fleet.fetched_at = None;
                self.preferences.fleet_repo = Some(path_text);
                if let Err(error) = save_preferences(&self.preferences) {
                    self.notice = Some(format!("Could not save preferences: {error}"));
                }
                self.fleet_refresh(cx);
            }
            Err(error) => {
                self.fleet.repo = None;
                self.fleet.rows.clear();
                self.fleet.repo_error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    pub(crate) fn fleet_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connected else {
            self.fleet.fetch_error = Some("Connect to a cluster to load fleet status".into());
            cx.notify();
            return;
        };
        let config = connected.client_config.clone();
        self.fleet.loading = true;
        self.fleet.fetch_error = None;
        self.fleet.fetch_generation += 1;
        let generation = self.fleet.fetch_generation;
        cx.notify();

        let handle = rt::tokio().spawn(async move {
            let runner = Runner::new(
                &repo,
                RunnerOptions {
                    server: config,
                    admin: None,
                    cluster: None,
                    no_cluster: true,
                    write: false,
                    dry_run: false,
                    overrides: BTreeMap::new(),
                },
            );
            let resolved = runner
                .resolve_targets(&Targets::All)
                .await
                .map_err(|error| error.to_string())?;
            let mut databases = resolved.databases.clone();
            let excluded: BTreeMap<String, String> = resolved.skipped.into_iter().collect();
            databases.extend(excluded.keys().cloned());
            databases.sort();
            databases.dedup();
            let statuses = runner
                .status(&Targets::Databases(databases))
                .await
                .map_err(|error| error.to_string())?;
            let rows: Vec<FleetRow> = statuses
                .into_iter()
                .map(|status| FleetRow {
                    excluded: excluded.get(&status.database).cloned(),
                    database: status.database,
                    head: status.head,
                    pending: status.pending,
                    customised: status.customised,
                    failed: status
                        .failed
                        .into_iter()
                        .map(|(number, _)| number)
                        .collect(),
                })
                .collect();
            Ok::<_, String>(rows)
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                if this.fleet.fetch_generation != generation {
                    return;
                }
                this.fleet.loading = false;
                match result {
                    Ok(Ok(rows)) => {
                        this.fleet.rows = rows;
                        this.fleet.fetched_at = Some(Instant::now());
                    }
                    Ok(Err(error)) => this.fleet.fetch_error = Some(error),
                    Err(error) => this.fleet.fetch_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn fleet_verify(&mut self, database: String, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connected else {
            return;
        };
        let Some(binary) = zedb_ch::cached_binary(&repo.config.engine.version) else {
            self.fleet.drift_error = Some(format!(
                "pinned ClickHouse {} is not cached; run `zedb pin` first",
                repo.config.engine.version
            ));
            cx.notify();
            return;
        };
        if self.fleet.drift_loading.contains(&database) {
            return;
        }
        let config = connected.client_config.clone();
        self.fleet.drift_loading.insert(database.clone());
        self.fleet.drift_error = None;
        cx.notify();

        let task_database = database.clone();
        let handle = rt::tokio().spawn(async move {
            let runner = Runner::new(
                &repo,
                RunnerOptions {
                    server: config,
                    admin: None,
                    cluster: None,
                    no_cluster: true,
                    write: false,
                    dry_run: false,
                    overrides: BTreeMap::new(),
                },
            );
            let verifier = Verifier::new(&repo, &runner, binary);
            let drifts = verifier
                .verify(&Targets::Databases(vec![task_database.clone()]))
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>(
                drifts
                    .into_iter()
                    .next()
                    .map(|drift| drift.findings)
                    .unwrap_or_default(),
            )
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.fleet.drift_loading.remove(&database);
                match result {
                    Ok(Ok(findings)) => {
                        this.fleet.drift.insert(
                            database,
                            DriftInfo {
                                findings,
                                checked_at: Instant::now(),
                            },
                        );
                    }
                    Ok(Err(error)) => this.fleet.drift_error = Some(error),
                    Err(error) => this.fleet.drift_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fleet_filtered_rows(&self, cx: &Context<Self>) -> Vec<FleetRow> {
        let needle = self.fleet.filter.read(cx).text().trim().to_lowercase();
        self.fleet
            .rows
            .iter()
            .filter(|row| needle.is_empty() || row.database.to_lowercase().contains(&needle))
            .cloned()
            .collect()
    }

    fn fleet_detail_panel(&mut self, row: &FleetRow, cx: &mut Context<Self>) -> impl IntoElement {
        let database = row.database.clone();
        let repo = self.fleet.repo.clone();
        let drift = self.fleet.drift.get(&database);
        let drift_loading = self.fleet.drift_loading.contains(&database);

        let mut summary: Vec<(String, u32)> = Vec::new();
        if let Some(group) = &row.excluded {
            summary.push((format!("excluded by group {group}"), TEXT_DIM));
        }
        summary.push((
            format!(
                "head {}",
                row.head
                    .map(|head| format!("{head:05}"))
                    .unwrap_or_else(|| "none".into())
            ),
            TEXT_DIM,
        ));
        if !row.failed.is_empty() {
            let failed: Vec<String> = row.failed.iter().map(|n| format!("{n:05}")).collect();
            summary.push((format!("failed: {}", failed.join(", ")), DANGER));
        }
        if !row.customised.is_empty() {
            let customised: Vec<String> =
                row.customised.iter().map(|n| format!("{n:05}")).collect();
            summary.push((
                format!("customised: {}", customised.join(", ")),
                ACCENT_CUSTOM,
            ));
        }

        // Dry-run (M3): every pending migration rendered with this
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
                .text_color(rgb(TEXT_DIM))
                .child("Verifying against replayed chain state..."),
            (false, Some(info)) if info.findings.is_empty() => {
                div().text_color(rgb(SUCCESS)).child(format!(
                    "verified clean {}s ago",
                    info.checked_at.elapsed().as_secs()
                ))
            }
            (false, Some(info)) => {
                let mut section = div().flex().flex_col().gap_1();
                section = section.child(
                    div()
                        .text_color(rgb(DANGER))
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
            (false, None) => div().text_color(rgb(TEXT_DIM)).child("Not verified yet"),
        };

        let mut panel = div()
            .w(px(420.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG_SIDEBAR))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_color(rgb(TEXT)).child(database.clone()))
                    .child(
                        div()
                            .id("fleet-detail-close")
                            .px_2()
                            .rounded(px(3.))
                            .text_color(rgb(TEXT_DIM))
                            .child("✕")
                            .hover(|button| button.text_color(rgb(TEXT)).cursor_pointer())
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
            body = body.child(div().text_color(rgb(color)).child(text));
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
                    .border_color(rgb(BORDER))
                    .text_color(rgb(TEXT))
                    .child(if drift_loading {
                        "Verifying..."
                    } else {
                        "Verify"
                    })
                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                    .on_click({
                        let database = database.clone();
                        cx.listener(move |this, _, _, cx| this.fleet_verify(database.clone(), cx))
                    }),
            )
            .child(drift_section);
        if let Some(error) = &self.fleet.drift_error {
            body = body.child(div().text_color(rgb(DANGER)).child(error.clone()));
        }

        if pending_sql.is_empty() && row.excluded.is_none() && row.failed.is_empty() {
            body = body.child(
                div()
                    .mt_2()
                    .text_color(rgb(TEXT_DIM))
                    .child("Nothing pending: an upgrade would do no work."),
            );
        }
        for (number, sql) in pending_sql {
            body = body
                .child(
                    div()
                        .mt_2()
                        .text_color(rgb(ACCENT_PENDING))
                        .child(format!("pending {number:05}: an upgrade would run")),
                )
                .child(
                    div()
                        .id(("fleet-sql", number as usize))
                        .p_2()
                        .rounded(px(3.))
                        .bg(rgb(0x1b1e23))
                        .border_1()
                        .border_color(rgb(BORDER))
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

    pub(crate) fn fleet_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = self.fleet.repo.clone();
        let rows = self.fleet_filtered_rows(cx);
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

        let toolbar = div()
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().w(px(340.)).child(self.fleet.repo_path.clone()))
            .child(
                div()
                    .id("fleet-open")
                    .px_2()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(TEXT_DIM))
                    .child(
                        svg()
                            .path("icons/folder-open.svg")
                            .size(px(14.))
                            .text_color(rgb(TEXT_DIM)),
                    )
                    .hover(|button| {
                        button
                            .bg(rgb(BG_SIDEBAR))
                            .text_color(rgb(TEXT))
                            .cursor_pointer()
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.fleet_open_repo(cx))),
            )
            .child(
                div()
                    .id("fleet-refresh")
                    .px_2()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(if loading { SUCCESS } else { TEXT_DIM }))
                    .child(
                        svg()
                            .path("icons/refresh.svg")
                            .size(px(14.))
                            .text_color(rgb(if loading { SUCCESS } else { TEXT_DIM })),
                    )
                    .hover(|button| {
                        button
                            .bg(rgb(BG_SIDEBAR))
                            .text_color(rgb(TEXT))
                            .cursor_pointer()
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.fleet_refresh(cx))),
            )
            .child(div().w(px(220.)).child(self.fleet.filter.clone()))
            .when_some(repo.as_ref(), |toolbar, repo| {
                toolbar.child(div().text_color(rgb(TEXT_DIM)).child(format!(
                            "{}  |  {} migration(s)  |  ClickHouse {}",
                            repo.root
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| repo.root.display().to_string()),
                            repo.migrations.len(),
                            repo.config.engine.version
                        )))
            });

        let mut header = div()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT_DIM))
            .child(div().w(px(200.)).flex_none().px_2().child("database"))
            .child(div().w(px(70.)).flex_none().px_1().child("head"));
        for (number, targeted) in &migrations {
            let label = if *targeted {
                format!("{number:05}*")
            } else {
                format!("{number:05}")
            };
            header = header.child(div().w(px(64.)).flex_none().text_center().child(label));
        }
        header = header.child(div().flex_1().px_2().child("state"));

        let migrations_for_rows = migrations.clone();
        let rows_for_list = rows.clone();
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
                                .when(index % 2 == 1, |line| line.bg(rgb(0x21252b)))
                                .when(is_selected, |line| line.bg(rgb(0x2c3a4d)))
                                .hover(|line| line.bg(rgb(0x2a2f37)))
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
                                            name.text_color(rgb(TEXT_DIM))
                                        })
                                        .child(row.database.clone()),
                                )
                                .child(
                                    div()
                                        .w(px(70.))
                                        .flex_none()
                                        .px_1()
                                        .text_color(rgb(TEXT_DIM))
                                        .child(
                                            row.head
                                                .map(|head| format!("{head:05}"))
                                                .unwrap_or_else(|| "none".into()),
                                        ),
                                );
                            for (number, targeted) in &migrations_for_rows {
                                let (glyph, color) = match cell_for(row, *number, *targeted) {
                                    Cell::Applied => ("●", SUCCESS),
                                    Cell::Pending => ("○", ACCENT_PENDING),
                                    Cell::Failed => ("✕", DANGER),
                                    Cell::Customised => ("◆", ACCENT_CUSTOM),
                                    Cell::NotApplicable => ("·", 0x3a3f4b),
                                };
                                line = line.child(
                                    div()
                                        .w(px(64.))
                                        .flex_none()
                                        .text_center()
                                        .text_color(rgb(color))
                                        .child(glyph),
                                );
                            }
                            let state = if let Some(group) = &row.excluded {
                                format!("excluded ({group})")
                            } else if !row.failed.is_empty() {
                                let failed: Vec<String> =
                                    row.failed.iter().map(|n| format!("{n:05}")).collect();
                                format!("FAILED: {}", failed.join(", "))
                            } else if !row.pending.is_empty() {
                                format!("{} pending", row.pending.len())
                            } else if row.head == latest_fleet {
                                "up to date".into()
                            } else {
                                String::new()
                            };
                            let state_color = if row.excluded.is_some() {
                                TEXT_DIM
                            } else if !row.failed.is_empty() {
                                DANGER
                            } else if !row.pending.is_empty() {
                                ACCENT_PENDING
                            } else {
                                SUCCESS
                            };
                            line.child(
                                div()
                                    .flex_1()
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(rgb(state_color))
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
            div().text_color(rgb(DANGER)).child(error.clone())
        } else if let Some(error) = &self.fleet.repo_error {
            div().text_color(rgb(DANGER)).child(error.clone())
        } else if loading {
            div()
                .text_color(rgb(TEXT_DIM))
                .child("Loading fleet status...")
        } else if let Some(fetched_at) = self.fleet.fetched_at {
            let up_to_date = rows
                .iter()
                .filter(|row| {
                    row.excluded.is_none() && row.pending.is_empty() && row.failed.is_empty()
                })
                .count();
            div().text_color(rgb(TEXT_DIM)).child(format!(
                "{} database(s), {} up to date, refreshed {}s ago",
                rows.len(),
                up_to_date,
                fetched_at.elapsed().as_secs()
            ))
        } else if self.fleet.repo.is_some() {
            div()
                .text_color(rgb(TEXT_DIM))
                .child("Repo open; connect and refresh to load fleet status")
        } else {
            div()
                .text_color(rgb(TEXT_DIM))
                .child("Open a migration repo to see the fleet")
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            .child(toolbar)
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
                    .bg(rgb(BG_STATUS))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .child(status_line),
            )
    }
}
