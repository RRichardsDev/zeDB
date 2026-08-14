//! The fleet view: open a migration repo and render the databases x
//! migrations matrix for the active connection.
//!
//! BYO git: "opening a repo" means pointing at a local checkout
//! directory; committing and pushing stay in the user's git workflow.
//! Everything shown here comes from the same zedb-core/zedb-ch calls the
//! CLI makes; this file only fetches and renders.

#[path = "view/action_modal.rs"]
mod action_modal;
#[path = "view/author.rs"]
mod author;
#[path = "view/detail.rs"]
mod detail;
#[path = "view/panel.rs"]
mod panel;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui::{div, prelude::*, px, rgb, svg, uniform_list, Context, Entity, SharedString};
use zedb_core::repo::MigrationRepo;

use crate::components::text_input::TextInput;
use crate::theme;
use crate::Workspace;

fn accent_custom() -> gpui::Hsla {
    gpui::rgb(0x9d7cd8).into()
}
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
    /// While a verify pass fetches its ClickHouse harness: download
    /// progress, then macOS verifying the fresh binary.
    pub harness_phase: Option<zedb_ch::pin::PinPhase>,
    /// How many databases the running verify pass covers, for the
    /// button's progress tooltip.
    pub verify_total: usize,
    /// Explicit per-session consent to mutate through this connection;
    /// reset whenever the connection changes.
    pub write_unlocked: bool,
    /// Cluster names discovered from system.clusters on refresh.
    pub clusters: Vec<String>,
    pub pending_action: Option<FleetAction>,
    pub ack_structural: bool,
    pub confirm_input: Entity<TextInput>,
    pub action_running: bool,
    pub action_result: Option<Result<String, String>>,
    /// Dry-run entries already applied in the running action, keyed by
    /// the text before the first ':' of their label, with how long each
    /// step took.
    pub action_progress: HashMap<String, f32>,
    /// Show the repo path editor instead of the compact repo chip.
    pub editing_repo_path: bool,
    /// Databases unchecked in the filter list (hidden from the matrix).
    pub hidden_databases: HashSet<String>,
    /// The database checkbox list is open.
    pub filter_open: bool,
    /// Width of the detail panel; user-draggable.
    pub detail_width: f32,
    pub resizing_detail: bool,
    /// A pasted git URL is being cloned in the background.
    pub cloning: bool,
}

impl FleetState {
    pub fn new(
        initial_path: &str,
        _initial_cluster: &str,
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
            harness_phase: None,
            verify_total: 0,
            write_unlocked: false,
            clusters: Vec::new(),
            pending_action: None,
            ack_structural: false,
            confirm_input,
            action_running: false,
            action_result: None,
            action_progress: HashMap::new(),
            editing_repo_path: false,
            hidden_databases: HashSet::new(),
            filter_open: false,
            detail_width: 420.0,
            resizing_detail: false,
            cloning: false,
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

/// A 28px square icon button matching the app toolbar style: dim icon
/// that brightens on hover with a background highlight, plus a tooltip.
fn fleet_icon_button(
    id: &'static str,
    icon: &'static str,
    tooltip: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .group(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(theme::border())
        .child(
            svg()
                .path(icon)
                .size(px(14.))
                .text_color(theme::text_dim())
                .group_hover(id, |icon| icon.text_color(theme::text())),
        )
        .hover(|button| button.bg(theme::hover()).cursor_pointer())
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx))
        .on_click(on_click)
}

impl Workspace {
    /// Whether the agent asked to point at this control right now.
    pub(crate) fn agent_highlight(&self, control: &str) -> bool {
        self.control_highlight.as_deref() == Some(control)
    }
}

fn action_button(
    id: &'static str,
    label: String,
    color: gpui::Hsla,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded(px(3.))
        .border_1()
        .border_color(color)
        .text_color(color)
        .text_center()
        .child(label)
        .hover(|button| button.bg(theme::hover()).cursor_pointer())
        .on_click(on_click)
}

impl Workspace {
    fn fleet_filtered_rows(&self) -> Vec<FleetRow> {
        self.fleet
            .rows
            .iter()
            .filter(|row| !self.fleet.hidden_databases.contains(&row.database))
            .cloned()
            .collect()
    }
}
