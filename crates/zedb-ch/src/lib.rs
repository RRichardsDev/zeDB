//! zedb-ch: the ClickHouse driver.
//!
//! HTTP client, typed RowBinary decoding, and the clickhouse-local
//! replay engine. See docs/contracts/SPEC.md.
//!
//! Results are decoded from `RowBinaryWithNamesAndTypes` into the
//! driver-agnostic [`zedb_core::Value`] model. We deliberately do not use
//! the `clickhouse` crate: it is built around compile-time serde row
//! structs, while an explorer discovers column types at runtime.

pub mod checks;
mod client;
pub mod ephemeral;
mod error;
pub mod explain;
pub mod lifecycle;
pub mod mcp;
pub mod native;
pub mod pin;
mod process;
pub mod regen;
pub mod replay;
mod rowbinary;
pub mod runner;
mod schema;
pub mod schema_cache;
pub mod schema_intelligence;
mod types;
pub mod verify;
pub mod workload;

pub use client::{
    ChClient, ChConfig, ClusterMembership, QueryProgress, QueryStreamEvent, QueryStreamSummary,
};
pub use error::{ChError, Result};
pub use pin::{
    binary_cache_dir, cached_binary, discover_server_version, ensure_binary, smoke_replay, PinError,
};
pub use schema::{
    distributed_sharding_key, ColumnInfo, DatabaseMeta, MergeInfo, MvDependency,
    ObjectDependencies, ObjectDetails, PartitionStats, ProjectionInfo, SchemaObjectKind,
    SchemaObjectMeta, TableStorage,
};
pub use types::{parse_type, ChType};
