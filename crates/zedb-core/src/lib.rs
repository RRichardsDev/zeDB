//! zedb-core: domain model for zeDB.
//!
//! Everything the CLI and the GPUI app share lives here. See docs/SPEC.md.

mod connection;
pub mod git;
mod preferences;
pub mod repo;
#[cfg(target_vendor = "apple")]
pub mod secrets;
mod session;
mod store;
mod value;

pub use connection::{ConnectionConfig, ConnectionNode, EnvTier};
pub use preferences::{load_preferences, save_preferences, CustomAgent, Preferences};
pub use session::{save_session, take_session, SavedQueryTab, SavedSession};
pub use store::{load_connections, save_connections, StoreError};
pub use value::{ColumnMeta, QueryResult, Value};
