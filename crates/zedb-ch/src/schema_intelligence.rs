//! Conservative SQL name intelligence over an immutable schema snapshot.
//!
//! The analyzer only reports an unknown name when its containing database or
//! object's metadata is complete. Ambiguous, unqualified, CTE, and uncached
//! cases stay neutral.
//!
//! This module holds the vocabulary the submodules share; each of them owns
//! one job over it: `tokens` scans, `bindings` resolves what is in scope, and
//! `analysis`, `completions`, `hover`, and `search` are the entry points.
//! `filters`, `limit`, and `order_by` rewrite clauses for the grid controls.

use std::ops::Range;

mod analysis;
mod bindings;
mod completions;
mod filters;
mod hover;
mod limit;
mod order_by;
mod search;
mod tokens;
mod vocabulary;

pub use analysis::{analyze_sql, recognized_identifiers, referenced_databases, touched_databases};
pub use completions::{completions, completions_with_placeholders};
pub use filters::{column_filter, column_filters, filtered_columns, set_column_filter};
pub use hover::{hover, object_at};
pub use limit::strip_top_level_limit;
pub use order_by::{aggregate_projection, has_group_by, set_order_by, top_level_order_by};
pub use search::{schema_search, SchemaSearchHit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierIssue {
    pub range: Range<usize>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionKind {
    Database,
    Object,
    Column,
    Function,
    Keyword,
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSuggestion {
    pub label: String,
    pub detail: String,
    pub kind: SuggestionKind,
    pub replace: Range<usize>,
}

/// A name the snapshot can vouch for, for positive highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognizedKind {
    Database,
    Object,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognizedIdentifier {
    pub range: Range<usize>,
    pub kind: RecognizedKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: Range<usize>,
    pub markdown: String,
}

/// Shared by the submodule tests so they all resolve names against the
/// same one-database, one-table, one-column snapshot.
#[cfg(test)]
mod fixtures {
    use std::collections::HashMap;

    use crate::schema_cache::{
        CachedColumn, CachedDatabase, CachedObject, CachedObjectKind, SchemaSnapshot,
    };

    pub(super) fn snapshot(columns: Option<HashMap<String, CachedColumn>>) -> SchemaSnapshot {
        let mut snapshot = SchemaSnapshot::default();
        snapshot.databases.insert(
            "analytics".into(),
            CachedDatabase {
                name: "analytics".into(),
                touched: 1,
                objects: HashMap::from([(
                    "events".into(),
                    CachedObject {
                        total_bytes: None,
                        name: "events".into(),
                        engine: "MergeTree".into(),
                        kind: CachedObjectKind::Table,
                        total_rows: Some(42),
                        comment: "Event stream".into(),
                        columns,
                    },
                )]),
            },
        );
        snapshot
    }

    pub(super) fn columns() -> HashMap<String, CachedColumn> {
        HashMap::from([(
            "event_id".into(),
            CachedColumn {
                name: "event_id".into(),
                type_name: "UInt64".into(),
                codec_expression: "CODEC(Delta)".into(),
                comment: "Primary event id".into(),
            },
        )])
    }
}
