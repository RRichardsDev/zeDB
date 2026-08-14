//! zedb-core: domain model for zeDB.
//!
//! Everything the CLI and the GPUI app share lives here. See docs/SPEC.md.

mod connection;
pub mod git;
mod history;
mod preferences;
pub mod repo;
mod saved_tabs;
#[cfg(target_vendor = "apple")]
pub mod secrets;
mod session;
mod store;
pub mod sync;
mod value;

pub use connection::{
    CloudProvenance, ConnectionConfig, ConnectionNode, DriverConfig, DriverSetting, EnvTier,
};
pub use history::{load_history, push_entry, save_history, HistoryEntry, HISTORY_CAP};
pub use preferences::{
    load_preferences, save_preferences, settings_file_path, CloudOrgRef, CustomAgent, Preferences,
    SavedQuery,
};
pub use saved_tabs::{load_saved_tabs, new_local_id, save_saved_tabs, SavedTab, SAVED_TAB_CAP};
pub use session::{save_session, take_session, SavedQueryTab, SavedSession};
pub use store::{load_connections, save_connections, StoreError};
pub use value::{ColumnMeta, QueryResult, Value};
