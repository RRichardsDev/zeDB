mod buffer;
mod editor;
#[path = "editor/diagnostics.rs"]
mod editor_diagnostics;
mod execution;
#[path = "execution/advisor.rs"]
mod execution_advisor;
#[path = "execution/controls.rs"]
mod execution_controls;
mod input;
mod state;
mod tabs;
#[path = "tail_controller/consumers.rs"]
mod tail_consumers;
mod tail_controller;
#[path = "tail_controller/native.rs"]
mod tail_native;
#[path = "tail_controller/polling.rs"]
mod tail_polling;
#[path = "tail_controller/strip.rs"]
mod tail_strip;
#[path = "view/apply.rs"]
mod view_apply;
#[path = "view/export.rs"]
mod view_export;

pub(crate) use buffer::{
    nearest_occurrence, resolve_query_variables, split_statements, statement_at_cursor,
};
pub(crate) use state::{
    max_rows_from_limit, tab_display_name, MaxRows, QueryOutcome, QueryResizeTarget, QueryState,
    QueryTab, RunEvent, TailBatch, TailPush, TailState, TailStream, TailStreamBatch, TailStripInfo,
    TailWatch,
};
