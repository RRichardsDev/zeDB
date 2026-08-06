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

/// A mutating action awaiting the safety ladder's consent.
#[derive(Clone, PartialEq)]
pub enum FleetAction {
    UpgradeAll,
    UpgradeDatabase(String),
    Rollback { database: String, number: u32 },
    ApplyTargeted { database: String, number: u32 },
    RemoveTargeted { database: String, number: u32 },
}

impl FleetAction {
    fn title(&self) -> String {
        match self {
            Self::UpgradeAll => "Upgrade every non-excluded database".into(),
            Self::UpgradeDatabase(database) => format!("Upgrade {database}"),
            Self::Rollback { database, number } => {
                format!("Roll back {number:05} on {database}")
            }
            Self::ApplyTargeted { database, number } => {
                format!("Apply customisation {number:05} to {database}")
            }
            Self::RemoveTargeted { database, number } => {
                format!("Remove customisation {number:05} from {database}")
            }
        }
    }

    /// What the operator must type to confirm on a production tier (or
    /// for irreversible work on any tier).
    fn required_phrase(&self, tier: zedb_core::EnvTier, irreversible: bool) -> Option<String> {
        if irreversible {
            return Some("irreversible".into());
        }
        if tier != zedb_core::EnvTier::Production {
            return None;
        }
        Some(match self {
            Self::UpgradeAll => "all".into(),
            Self::UpgradeDatabase(database)
            | Self::Rollback { database, .. }
            | Self::ApplyTargeted { database, .. }
            | Self::RemoveTargeted { database, .. } => database.clone(),
        })
    }
}

pub struct FleetState {
    pub repo_path: Entity<TextInput>,
    pub repo: Option<Arc<MigrationRepo>>,
    /// Git state of the open checkout; None when it is not a git repo.
    pub git: Option<zedb_core::git::GitStatus>,
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
    /// Explicit per-session consent to mutate through this connection;
    /// reset whenever the connection changes.
    pub write_unlocked: bool,
    /// Cluster names discovered from system.clusters on refresh.
    pub clusters: Vec<String>,
    /// The ${cluster} value for fleet operations; None means declustered.
    pub selected_cluster: Option<String>,
    /// The cluster dropdown is open.
    pub cluster_open: bool,
    pub pending_action: Option<FleetAction>,
    pub ack_structural: bool,
    pub confirm_input: Entity<TextInput>,
    pub action_running: bool,
    pub action_result: Option<Result<String, String>>,
    /// Show the repo path editor instead of the compact repo chip.
    pub editing_repo_path: bool,
    /// Databases unchecked in the filter list (hidden from the matrix).
    pub hidden_databases: HashSet<String>,
    /// The database checkbox list is open.
    pub filter_open: bool,
}

impl FleetState {
    pub fn new(
        initial_path: &str,
        initial_cluster: &str,
        window: &mut gpui::Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let _ = window;
        let initial_path = initial_path.to_string();
        let repo_path =
            cx.new(move |cx| TextInput::new(&initial_path, "path to a migration repo", false, cx));
        let confirm_input = cx.new(|cx| TextInput::new("", "type to confirm", false, cx));
        cx.observe(&confirm_input, |_, _, cx| cx.notify()).detach();
        Self {
            repo_path,
            repo: None,
            git: None,
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
            write_unlocked: false,
            clusters: Vec::new(),
            selected_cluster: (!initial_cluster.is_empty()).then(|| initial_cluster.to_string()),
            cluster_open: false,
            pending_action: None,
            ack_structural: false,
            confirm_input,
            action_running: false,
            action_result: None,
            editing_repo_path: false,
            hidden_databases: HashSet::new(),
            filter_open: false,
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

fn action_button(
    id: &'static str,
    label: String,
    color: u32,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded(px(3.))
        .border_1()
        .border_color(rgb(color))
        .text_color(rgb(color))
        .text_center()
        .child(label)
        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
        .on_click(on_click)
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
                self.fleet.git = zedb_core::git::read_git_status(&repo.root);
                self.fleet.repo = Some(Arc::new(repo));
                self.fleet.repo_error = None;
                self.fleet.editing_repo_path = false;
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
                self.fleet.git = None;
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
            let git = zedb_core::git::read_git_status(&repo.root);
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
            let clusters: Vec<String> = runner
                .client()
                .query("SELECT DISTINCT cluster FROM system.clusters ORDER BY cluster")
                .await
                .map(|result| {
                    result
                        .rows
                        .iter()
                        .filter_map(|row| row.first().map(|value| value.to_string()))
                        .collect()
                })
                .unwrap_or_default();
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
            Ok::<_, String>((rows, clusters, git))
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                if this.fleet.fetch_generation != generation {
                    return;
                }
                this.fleet.loading = false;
                match result {
                    Ok(Ok((rows, clusters, git))) => {
                        this.fleet.rows = rows;
                        this.fleet.clusters = clusters;
                        this.fleet.git = git;
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

    fn fleet_tier(&self) -> zedb_core::EnvTier {
        self.selected
            .and_then(|index| self.connections.get(index))
            .map(|connection| connection.tier)
            .unwrap_or(zedb_core::EnvTier::Dev)
    }

    /// The rollback class the ladder must gate on for an action, when any.
    fn action_rollback_class(
        &self,
        action: &FleetAction,
    ) -> Option<Option<zedb_core::repo::RollbackClass>> {
        let repo = self.fleet.repo.as_ref()?;
        match action {
            FleetAction::Rollback { number, .. } | FleetAction::RemoveTargeted { number, .. } => {
                repo.migration(*number)
                    .map(|migration| migration.rollback_class)
            }
            _ => None,
        }
    }

    pub(crate) fn fleet_request_action(&mut self, action: FleetAction, cx: &mut Context<Self>) {
        if !self.fleet.write_unlocked {
            self.notice = Some("Unlock writes first: mutations need explicit consent".into());
            cx.notify();
            return;
        }
        self.fleet.pending_action = Some(action);
        self.fleet.ack_structural = false;
        self.fleet.action_running = false;
        self.fleet.action_result = None;
        // TextInput has no setter; a fresh entity is an empty input.
        self.fleet.confirm_input = cx.new(|cx| TextInput::new("", "type to confirm", false, cx));
        cx.observe(&self.fleet.confirm_input, |_, _, cx| cx.notify())
            .detach();
        cx.notify();
    }

    pub(crate) fn fleet_execute_action(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.fleet.pending_action.clone() else {
            return;
        };
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connected else {
            return;
        };
        let cluster = self.fleet.selected_cluster.clone();
        let config = connected.client_config.clone();
        self.fleet.action_running = true;
        self.fleet.action_result = None;
        cx.notify();

        let handle = rt::tokio().spawn(async move {
            let no_cluster = cluster.is_none();
            let runner = Runner::new(
                &repo,
                RunnerOptions {
                    server: config,
                    admin: None,
                    cluster,
                    no_cluster,
                    write: true,
                    dry_run: false,
                    overrides: BTreeMap::new(),
                },
            );
            let result = match &action {
                FleetAction::UpgradeAll => runner.upgrade(&Targets::All, None).await,
                FleetAction::UpgradeDatabase(database) => {
                    runner
                        .upgrade(&Targets::Databases(vec![database.clone()]), None)
                        .await
                }
                FleetAction::Rollback { database, number } => {
                    runner
                        .rollback_one(
                            &Targets::Databases(vec![database.clone()]),
                            *number,
                            true,
                            false,
                        )
                        .await
                }
                FleetAction::ApplyTargeted { database, number } => {
                    runner
                        .apply_targeted(&Targets::Databases(vec![database.clone()]), *number)
                        .await
                }
                FleetAction::RemoveTargeted { database, number } => {
                    runner
                        .rollback_one(
                            &Targets::Databases(vec![database.clone()]),
                            *number,
                            true,
                            true,
                        )
                        .await
                }
            };
            result.map_err(|error| error.to_string())
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.fleet.action_running = false;
                this.fleet.action_result = Some(match result {
                    Ok(Ok(())) => Ok("Completed; tracking table and audit log updated.".into()),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(error.to_string()),
                });
                this.fleet_refresh(cx);
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

    fn fleet_filtered_rows(&self) -> Vec<FleetRow> {
        self.fleet
            .rows
            .iter()
            .filter(|row| !self.fleet.hidden_databases.contains(&row.database))
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

        if self.fleet.write_unlocked {
            let mut actions = div().mt_2().flex().flex_col().gap_1();
            let mut any = false;
            if !row.pending.is_empty() {
                any = true;
                actions = actions.child(action_button(
                    "fleet-act-upgrade",
                    format!("Upgrade {database}"),
                    ACCENT_PENDING,
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
                        DANGER,
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
                            DANGER,
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
                            ACCENT_CUSTOM,
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

    /// The safety ladder: tier identity, rendered dry-run, class
    /// acknowledgements, and typed confirmation where the tier or the
    /// action demands it. Nothing here is skippable.
    fn fleet_action_modal(
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
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
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
                .border_color(rgb(ACCENT_PENDING))
                .text_color(rgb(ACCENT_PENDING))
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
            body = body.child(div().text_color(rgb(TEXT_DIM)).child(label.clone()));
            if !sql.is_empty() {
                body = body.child(
                    div()
                        .id(gpui::SharedString::from(format!("modal-sql-{label}")))
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
        }
        if structural {
            body = body.child(
                div()
                    .id("fleet-ack-structural")
                    .p_2()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(if self.fleet.ack_structural {
                        SUCCESS
                    } else {
                        DANGER
                    }))
                    .text_color(rgb(if self.fleet.ack_structural {
                        SUCCESS
                    } else {
                        DANGER
                    }))
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
                body.child(div().text_color(rgb(DANGER)).child(
                    "This rollback is IRREVERSIBLE: it does not restore the previous state.",
                ));
        }
        if let Some(phrase) = &phrase {
            body = body
                .child(
                    div()
                        .text_color(rgb(TEXT_DIM))
                        .child(format!("Type \"{phrase}\" to confirm:")),
                )
                .child(div().w(px(260.)).child(self.fleet.confirm_input.clone()));
        }
        match &self.fleet.action_result {
            Some(Ok(message)) => {
                body = body.child(div().text_color(rgb(SUCCESS)).child(message.clone()));
            }
            Some(Err(error)) => {
                body = body.child(div().text_color(rgb(DANGER)).child(error.clone()));
            }
            None => {}
        }
        if running {
            body = body.child(div().text_color(rgb(TEXT_DIM)).child("Applying..."));
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
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .id("fleet-modal-cancel")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .child(if self.fleet.action_result.is_some() {
                            "Close"
                        } else {
                            "Cancel"
                        })
                        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
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
                            .border_color(rgb(if confirmable { DANGER } else { BORDER }))
                            .text_color(rgb(if confirmable { DANGER } else { TEXT_DIM }))
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
            .child(card)
            .into_any_element()
    }

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
            .border_color(rgb(BORDER))
            .child(
                div()
                    .id("fleet-db-filter")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(if hidden_count > 0 { TEXT } else { TEXT_DIM }))
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
                            .text_color(rgb(if hidden_count > 0 { TEXT } else { TEXT_DIM })),
                    )
                    .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.fleet.filter_open = !this.fleet.filter_open;
                        this.fleet.cluster_open = false;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("fleet-new-migration")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(TEXT_DIM))
                    .child("New migration")
                    .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                    .on_click(cx.listener(|this, _, window, cx| this.author_open(window, cx))),
            )
            .when(unlocked, |controls| {
                controls.child(
                    div()
                        .id("fleet-cluster")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .text_color(rgb(TEXT_DIM))
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(format!(
                            "cluster: {}",
                            self.fleet.selected_cluster.as_deref().unwrap_or("none")
                        ))
                        .child(
                            svg()
                                .path("icons/chevron-down.svg")
                                .size(px(12.))
                                .text_color(rgb(TEXT_DIM)),
                        )
                        .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.fleet.cluster_open = !this.fleet.cluster_open;
                            this.fleet.filter_open = false;
                            cx.notify();
                        })),
                )
            })
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
                    .border_color(rgb(if unlocked { DANGER } else { BORDER }))
                    .child(
                        svg()
                            .path(if unlocked {
                                "icons/lock-open.svg"
                            } else {
                                "icons/lock.svg"
                            })
                            .size(px(14.))
                            .text_color(rgb(if unlocked { DANGER } else { TEXT_DIM }))
                            .when(!unlocked, |icon| {
                                icon.group_hover("fleet-write-unlock", |icon| {
                                    icon.text_color(rgb(SUCCESS))
                                })
                            }),
                    )
                    .hover(|button| {
                        let button = button.bg(rgb(BG_SIDEBAR)).cursor_pointer();
                        if unlocked {
                            button
                        } else {
                            button.border_color(rgb(SUCCESS))
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
                        this.fleet.cluster_open = false;
                        cx.notify();
                    })),
            )
            .when(unlocked, |controls| {
                controls.child(
                    div()
                        .id("fleet-upgrade-all")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(ACCENT_PENDING))
                        .text_color(rgb(ACCENT_PENDING))
                        .child("Upgrade all")
                        .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.fleet_request_action(FleetAction::UpgradeAll, cx)
                        })),
                )
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
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
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
                    .bg(rgb(BG_STATUS))
                    .border_t_1()
                    .border_color(rgb(BORDER))
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
                                            .text_color(rgb(if stale {
                                                ACCENT_PENDING
                                            } else {
                                                TEXT_DIM
                                            }))
                                            .child(git.summary()),
                                    )
                                })
                                .child(div().text_color(rgb(TEXT_DIM)).child(format!(
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
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG_SIDEBAR))
                    .p_1()
                    .flex()
                    .flex_col();
                card = card.child(
                    div()
                        .id("fleet-db-filter-all")
                        .px_2()
                        .py_1()
                        .rounded(px(3.))
                        .text_color(rgb(TEXT_DIM))
                        .child(if hidden_count > 0 {
                            "Show all"
                        } else {
                            "All shown"
                        })
                        .hover(|item| item.bg(rgb(0x303640)).cursor_pointer())
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
                                    .border_color(rgb(BORDER))
                                    .when(checked, |tick| tick.bg(rgb(SUCCESS))),
                            )
                            .child(
                                div()
                                    .text_color(rgb(if checked { TEXT } else { TEXT_DIM }))
                                    .child(database.clone()),
                            )
                            .hover(|item| item.bg(rgb(0x303640)).cursor_pointer())
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
            .when(self.fleet.cluster_open, |root| {
                let mut card = div()
                    .id("fleet-cluster-list")
                    .absolute()
                    .top(px(84.))
                    .left(px(150.))
                    .w(px(240.))
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG_SIDEBAR))
                    .p_1()
                    .flex()
                    .flex_col();
                let mut options: Vec<Option<String>> = vec![None];
                options.extend(self.fleet.clusters.iter().cloned().map(Some));
                for (index, option) in options.into_iter().enumerate() {
                    let selected = self.fleet.selected_cluster == option;
                    let label = option
                        .clone()
                        .unwrap_or_else(|| "none (declustered)".into());
                    card = card.child(
                        div()
                            .id(("fleet-cluster-item", index))
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .text_color(rgb(if selected { TEXT } else { TEXT_DIM }))
                            .when(selected, |item| item.bg(rgb(0x2c3a4d)))
                            .child(label)
                            .hover(|item| item.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.fleet.selected_cluster = option.clone();
                                this.fleet.cluster_open = false;
                                this.preferences.fleet_cluster =
                                    this.fleet.selected_cluster.clone();
                                let _ = zedb_core::save_preferences(&this.preferences);
                                cx.notify();
                            })),
                    );
                }
                root.child(card)
            })
            .when_some(modal, |root, modal| root.child(modal))
            .when_some(self.author_panel(cx), |root, panel| root.child(panel))
    }
}
