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
mod rowbinary;
mod schema;
mod types;

pub use client::{ChClient, ChConfig};
pub use error::{ChError, Result};
pub use schema::{ColumnInfo, DatabaseMeta, SchemaObjectKind, SchemaObjectMeta};
pub use types::{parse_type, ChType};
