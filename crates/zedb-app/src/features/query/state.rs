use std::time::{Duration, Instant};

use gpui::Entity;
use gpui_component::input::InputState;
use zedb_ch::QueryStreamEvent;

use crate::grid_spike::GridSpike;
use crate::query_advisor;
use crate::tail;
use crate::vim::{CommandLineSnapshot, VimController};

pub(crate) struct QueryState {
    pub(crate) tabs: Vec<QueryTab>,
    pub(crate) active_tab: usize,
    pub(crate) next_tab_id: usize,
    pub(crate) next_tail_generation: u64,
    pub(crate) next_tail_number: usize,
    pub(crate) next_stream_epoch: u64,
    pub(crate) abort: Option<tokio::task::AbortHandle>,
    pub(crate) rerun_pending: Option<String>,
    pub(crate) rerun_generation: u64,
    pub(crate) error_decision: Option<tokio::sync::oneshot::Sender<bool>>,
    pub(crate) run_id: u64,
    pub(crate) resize: Option<(QueryResizeTarget, f32)>,
}

impl QueryState {
    pub(crate) fn new(tabs: Vec<QueryTab>, active_tab: usize, next_tab_id: usize) -> Self {
        Self {
            tabs,
            active_tab,
            next_tab_id,
            next_tail_generation: 0,
            next_tail_number: 0,
            next_stream_epoch: 0,
            abort: None,
            rerun_pending: None,
            rerun_generation: 0,
            error_decision: None,
            run_id: 0,
            resize: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaxRows {
    Rows1k,
    Rows10k,
    Rows50k,
    Rows100k,
    Rows1m,
    Unlimited,
}

impl MaxRows {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Rows1k => "1k",
            Self::Rows10k => "10k",
            Self::Rows50k => "50k",
            Self::Rows100k => "100k",
            Self::Rows1m => "1m",
            Self::Unlimited => "Unlimited",
        }
    }

    pub(crate) fn limit(self) -> Option<usize> {
        match self {
            Self::Rows1k => Some(1_000),
            Self::Rows10k => Some(10_000),
            Self::Rows50k => Some(50_000),
            Self::Rows100k => Some(100_000),
            Self::Rows1m => Some(1_000_000),
            Self::Unlimited => None,
        }
    }
}

pub(crate) struct QueryTab {
    pub(crate) persistent_id: String,
    pub(crate) saved_tab_id: Option<String>,
    pub(crate) name: String,
    pub(crate) id: usize,
    pub(crate) editor: Entity<InputState>,
    pub(crate) result_grid: Entity<GridSpike>,
    pub(crate) result_columns: usize,
    pub(crate) result_rows: usize,
    pub(crate) has_result: bool,
    pub(crate) max_rows: MaxRows,
    pub(crate) result_capped: bool,
    pub(crate) read_rows: Option<u64>,
    pub(crate) read_bytes: Option<u64>,
    pub(crate) total_rows: Option<u64>,
    pub(crate) received_bytes: u64,
    pub(crate) editor_height: f32,
    pub(crate) status_height: f32,
    pub(crate) outcome: QueryOutcome,
    pub(crate) started_at: Option<Instant>,
    pub(crate) elapsed: Option<Duration>,
    pub(crate) vim: VimController,
    pub(crate) vim_command_line: Option<CommandLineSnapshot>,
    pub(crate) vim_recording: Option<char>,
    pub(crate) schema_analysis_generation: u64,
    pub(crate) explain: Option<zedb_ch::explain::ExplainNode>,
    pub(crate) estimate: Option<QueryEstimate>,
    pub(crate) advisor: Option<Vec<query_advisor::QueryFinding>>,
    pub(crate) advise_pending: bool,
    pub(crate) advisor_generation: u64,
    pub(crate) failed_sql: Option<String>,
    pub(crate) displayed_statement: Option<String>,
    pub(crate) displayed_statement_offset: Option<usize>,
    pub(crate) running_query_id: Option<String>,
    pub(crate) tail: Option<TailState>,
}

/// A pre-flight cost estimate: `EXPLAIN ESTIMATE` totals plus the
/// primary-key granule pruning the plan reports for the same
/// statement.
#[derive(Clone)]
pub(crate) struct QueryEstimate {
    pub(crate) tables: Vec<zedb_ch::explain::EstimateRow>,
    pub(crate) parts: u64,
    pub(crate) rows: u64,
    pub(crate) marks: u64,
    /// (selected, initial) granules summed over the plan's reads;
    /// None when the plan carries no index stats.
    pub(crate) pruning: Option<(u64, u64)>,
}

impl QueryEstimate {
    /// Rows past this read as "you are about to scan a lot".
    pub(crate) const WARN_ROWS: u64 = 100_000_000;

    pub(crate) fn heavy(&self) -> bool {
        self.rows >= Self::WARN_ROWS
    }

    /// The primary key barely prunes: most granules survive.
    pub(crate) fn unpruned(&self) -> bool {
        match self.pruning {
            Some((selected, initial)) if initial > 0 => selected as f64 / initial as f64 > 0.7,
            _ => false,
        }
    }
}

pub(crate) struct TailState {
    pub(crate) number: usize,
    pub(crate) query: tail::TailQuery,
    pub(crate) baseline: String,
    pub(crate) last: Option<String>,
    pub(crate) key_index: usize,
    pub(crate) cap: Option<usize>,
    pub(crate) native_available: Option<bool>,
    pub(crate) push: TailPush,
    pub(crate) stream: Option<TailStream>,
    pub(crate) watch: Option<TailWatch>,
    pub(crate) stream_rejected: bool,
    pub(crate) generation: u64,
    pub(crate) paused: bool,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TailPush {
    Poll,
    Fast,
    Stream,
    Watch,
}

#[derive(Clone, Debug)]
pub(crate) struct TailStream {
    pub(crate) epoch: u64,
    pub(crate) abort: tokio::task::AbortHandle,
}

impl Drop for TailStream {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TailWatch {
    pub(crate) view: String,
    pub(crate) epoch: u64,
    pub(crate) abort: tokio::task::AbortHandle,
}

impl Drop for TailWatch {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

pub(crate) type TailStreamBatch = (Vec<zedb_core::ColumnMeta>, Vec<Vec<zedb_core::Value>>);
pub(crate) type TailBatch = (
    Option<Vec<zedb_core::ColumnMeta>>,
    Vec<Vec<zedb_core::Value>>,
);

pub(crate) struct TailStripInfo {
    pub(crate) tab_id: usize,
    pub(crate) key: String,
    pub(crate) paused: bool,
    pub(crate) error: Option<String>,
    pub(crate) rows: usize,
    pub(crate) native_available: bool,
    pub(crate) push: TailPush,
    pub(crate) experimental_streaming_enabled: bool,
    pub(crate) dirty: bool,
}

pub(crate) enum QueryOutcome {
    Idle,
    Running,
    Complete {
        columns: usize,
        rows: usize,
        skipped: usize,
    },
    Error(String),
    StatementError {
        index: usize,
        total: usize,
        message: String,
    },
    Cancelled,
}

pub(crate) enum RunEvent {
    Stream(QueryStreamEvent),
    StatementFailed {
        index: usize,
        total: usize,
        message: String,
        decision: tokio::sync::oneshot::Sender<bool>,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum QueryResizeTarget {
    Editor,
    Status,
}

pub(crate) fn max_rows_from_limit(limit: Option<usize>) -> MaxRows {
    match limit {
        Some(1_000) => MaxRows::Rows1k,
        Some(10_000) => MaxRows::Rows10k,
        Some(50_000) => MaxRows::Rows50k,
        Some(1_000_000) => MaxRows::Rows1m,
        None => MaxRows::Unlimited,
        _ => MaxRows::Rows100k,
    }
}

pub(crate) fn tab_display_name(tab: &QueryTab) -> String {
    tab.tail
        .as_ref()
        .map(|tail| format!("Tail {}", tail.number))
        .unwrap_or_else(|| tab.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_row_limits_restore_to_matching_choices() {
        assert!(matches!(max_rows_from_limit(Some(1_000)), MaxRows::Rows1k));
        assert!(matches!(
            max_rows_from_limit(Some(100_000)),
            MaxRows::Rows100k
        ));
        assert!(matches!(max_rows_from_limit(None), MaxRows::Unlimited));
        assert!(matches!(max_rows_from_limit(Some(123)), MaxRows::Rows100k));
    }
}
