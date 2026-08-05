//! zedb-core: domain model for zeDB.
//!
//! Everything the CLI and the GPUI app share lives here. See docs/SPEC.md.

mod connection;
pub mod secrets;
mod store;
mod value;

pub use connection::{ConnectionConfig, EnvTier};
pub use store::{load_connections, save_connections, StoreError};
pub use value::{ColumnMeta, QueryResult, Value};
