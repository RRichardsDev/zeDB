//! Query analytics: what happened over the window, and why it was
//! slow. Reads system.query_log through zedb-ch's analytics layer.
//! Fetches on open, on scope/window change, and on demand; never on a
//! poll, because history does not need watching.

use gpui::Action;
use zedb_ch::analytics::{AnalyticsWindow, FingerprintRun, QueryFingerprint, QueryTestimony};

pub(crate) use crate::ops::OpsScope as AnalyticsScope;

pub(crate) struct AnalyticsState {
    pub window: AnalyticsWindow,
    pub scope: AnalyticsScope,
    pub fingerprints: Vec<QueryFingerprint>,
    pub loading: bool,
    pub error: Option<String>,
    pub fetched_at: Option<chrono::DateTime<chrono::Local>>,
    /// The drilled-in fingerprint hash, when any.
    pub selected: Option<String>,
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

impl Default for AnalyticsState {
    fn default() -> Self {
        Self {
            window: AnalyticsWindow::LastDay,
            scope: AnalyticsScope::Node,
            fingerprints: Vec::new(),
            loading: false,
            error: None,
            fetched_at: None,
            selected: None,
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

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct SetAnalyticsWindow {
    pub hours: u32,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct SetAnalyticsScope {
    /// None selects the connected node; Some(name) a known cluster.
    pub cluster: Option<String>,
}
