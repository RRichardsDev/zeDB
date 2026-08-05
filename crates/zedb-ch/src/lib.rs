//! zedb-ch: the ClickHouse driver.
//!
//! HTTP client, typed RowBinary decoding, and (in Phase 1) the
//! clickhouse-local replay engine. See docs/SPEC.md.
//!
//! Results are decoded from `RowBinaryWithNamesAndTypes` into the
//! driver-agnostic [`zedb_core::Value`] model. We deliberately do not use
//! the `clickhouse` crate: it is built around compile-time serde row
//! structs, while an explorer discovers column types at runtime.

mod client;
mod error;
pub mod pin;
mod rowbinary;
mod schema;
mod types;

pub use client::{ChClient, ChConfig, QueryProgress, QueryStreamEvent, QueryStreamSummary};
pub use error::{ChError, Result};
pub use pin::{
    binary_cache_dir, cached_binary, discover_server_version, ensure_binary, smoke_replay, PinError,
};
pub use schema::{ColumnInfo, DatabaseMeta, ObjectDetails, SchemaObjectKind, SchemaObjectMeta};
pub use types::{parse_type, ChType};
