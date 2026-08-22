//! Query analytics: what happened over the window, and why it was
//! slow. Reads system.query_log through zedb-ch's analytics layer.
//! The fingerprint list is a real results grid: sorting and column
//! filters re-run the aggregation (grid events → SQL), so the table
//! is always an honest window onto a query that actually ran.

use gpui::{prelude::*, Context, Entity};
use zedb_ch::analytics::{AnalyticsWindow, FingerprintRun, QueryTestimony};

use crate::grid_spike::GridSpike;
use crate::Workspace;

pub(crate) struct AnalyticsState {
    pub window: AnalyticsWindow,
    /// The fingerprint grid; display columns only.
    pub grid: Entity<GridSpike>,
    /// (hash, shape) per grid row, same order, for drill-in.
    pub rows_meta: Vec<(String, String)>,
    /// Grid-requested sort, mapped into the SQL on fetch.
    pub sort: Vec<(String, bool)>,
    /// Grid-managed filter conjuncts per column, run in HAVING.
    pub filters: Vec<(String, String)>,
    pub loading: bool,
    pub error: Option<String>,
    pub fetched_at: Option<chrono::DateTime<chrono::Local>>,
    /// The drilled-in fingerprint hash, when any.
    pub selected: Option<String>,
    /// The drilled-in fingerprint's shape, for the detail pane.
    pub selected_shape: String,
    pub runs: Vec<FingerprintRun>,
    pub runs_loading: bool,
    /// The run whose testimony is shown: (query_id, testimony).
    pub testimony: Option<(String, QueryTestimony)>,
    pub testimony_loading: bool,
    /// Invalidates in-flight fetches on refetch or reset.
    pub generation: u64,
    pub detail_width: f32,
    pub resizing_detail: bool,
}

impl AnalyticsState {
    pub(crate) fn new(cx: &mut Context<Workspace>) -> Self {
        let grid = cx.new(GridSpike::new);
        // The query returns raw numbers; the grid humanizes them so
        // sort, filter, and copy keep seeing the real values.
        grid.update(cx, |grid, _| {
            use crate::grid_spike::ColumnDisplay::{Bytes, Millis};
            grid.set_column_displays(&[
                ("p50_ms", Millis),
                ("p95_ms", Millis),
                ("p99_ms", Millis),
                ("total_ms", Millis),
                ("peak_mem", Bytes),
                ("read", Bytes),
            ]);
            // Shapes are SQL; visible cells color lazily through the
            // grid's per-cell highlight cache.
            grid.set_column_grammars(&[("shape", "sqlstmt")]);
        });
        cx.subscribe(&grid, |this: &mut Workspace, _, event, cx| {
            this.analytics_grid_event(event, cx);
        })
        .detach();
        Self {
            window: AnalyticsWindow::LastDay,
            grid,
            rows_meta: Vec::new(),
            sort: Vec::new(),
            filters: Vec::new(),
            loading: false,
            error: None,
            fetched_at: None,
            selected: None,
            selected_shape: String::new(),
            runs: Vec::new(),
            runs_loading: false,
            testimony: None,
            testimony_loading: false,
            generation: 0,
            detail_width: 480.0,
            resizing_detail: false,
        }
    }
}
