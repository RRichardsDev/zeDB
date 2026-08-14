use regex::Regex;
use zedb_core::{QueryResult, Value};

use crate::{
    schema_cache::{CachedColumn, CachedObjectKind, ColumnRecord, TableRecord},
    ChClient, ChError, Result,
};

mod cache;
mod catalog;
mod columns;
mod parsers;
mod relationships;
mod storage;

pub use parsers::{distributed_sharding_key, distributed_target};
pub(crate) use parsers::{
    escape_identifier, escape_string, optional_u64_at, parse_cache_columns, parse_cache_tables,
    parse_columns, parse_databases, parse_mv_from, parse_mv_to, parse_object_details,
    parse_schema_objects, string_at,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseMeta {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaObjectKind {
    Table,
    View,
    MaterializedView,
    Dictionary,
}

impl SchemaObjectKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::View => "view",
            Self::MaterializedView => "materialized view",
            Self::Dictionary => "dictionary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaObjectMeta {
    pub name: String,
    pub engine: String,
    pub kind: SchemaObjectKind,
    pub total_rows: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    /// Compressed bytes for this column across active parts
    /// (`system.columns.data_compressed_bytes`). 0 for objects that
    /// hold no data (views, empty tables).
    pub compressed_bytes: u64,
    /// Uncompressed bytes across active parts
    /// (`system.columns.data_uncompressed_bytes`).
    pub uncompressed_bytes: u64,
    /// The column's compression codec expression
    /// (`system.columns.compression_codec`), e.g. `CODEC(ZSTD(1))`;
    /// empty when the column uses the table/server default.
    pub codec: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDetails {
    pub engine_full: String,
    pub partition_key: String,
    pub sorting_key: String,
    pub primary_key: String,
    pub create_table_query: String,
}

/// Table-wide storage totals from the local node's active parts.
/// Part-level compressed/uncompressed bytes are populated for every
/// part type, so the compression ratio is always available, unlike the
/// per-column bytes in `ColumnInfo`, which ClickHouse only tracks for
/// Wide parts. The part-type counts let the UI explain when per-column
/// sizes are missing because the parts are all Compact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStorage {
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub wide_parts: u64,
    pub compact_parts: u64,
}

/// One partition's active-part rollup, for the schema inspector's Parts
/// tab.
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionStats {
    /// The partition expression value (`tuple()` for unpartitioned tables).
    pub partition: String,
    /// Active parts in this partition; a large count is the "too many
    /// parts" signal.
    pub parts: u64,
    pub rows: u64,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    /// Highest merge level among the partition's parts (0 = never merged).
    pub max_level: u64,
}

/// One in-progress merge (or mutation-merge) for a table, from
/// `system.merges`.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeInfo {
    pub partition_id: String,
    pub result_part: String,
    /// Source parts being merged into one.
    pub num_parts: u64,
    pub elapsed_secs: u64,
    /// Completion percent (0..100).
    pub progress_pct: u64,
    pub rows_read: u64,
    pub rows_written: u64,
    pub memory_usage: u64,
    /// True when this is a mutation applying an ALTER, not a plain merge.
    pub is_mutation: bool,
}

/// A materialized view edge: the MV, and the tables it reads from / writes
/// to (parsed from its `create_table_query`).
#[derive(Clone, Debug, PartialEq)]
pub struct MvDependency {
    /// `db.view`.
    pub view: String,
    /// `db.table` the MV reads from (its SELECT's FROM).
    pub source: Option<String>,
    /// `db.table` the MV writes to (its `TO` target).
    pub target: Option<String>,
}

/// A projection attached to a table.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionInfo {
    pub name: String,
    /// `Aggregate` or `Normal`.
    pub kind: String,
    pub sorting_key: String,
    pub query: String,
    pub rows: u64,
    pub compressed_bytes: u64,
    pub parts: u64,
}

/// The materialized-view dependency neighborhood of one object: what
/// feeds it, what it feeds.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ObjectDependencies {
    pub is_materialized_view: bool,
    /// If this object is an MV: the table it reads from / writes to.
    pub reads_from: Option<String>,
    pub writes_to: Option<String>,
    /// MVs that read from this object (this -> view -> target).
    pub feeds: Vec<MvDependency>,
    /// MVs whose target is this object (source -> view -> this).
    pub written_by: Vec<MvDependency>,
    /// Referenced tables in the lineage that no longer exist (a broken MV
    /// silently drops inserts), as `db.table`.
    pub missing_tables: Vec<String>,
}
