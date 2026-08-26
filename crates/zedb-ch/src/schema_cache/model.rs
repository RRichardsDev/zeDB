use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::SNAPSHOT_FORMAT;
use crate::SchemaObjectKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachedObjectKind {
    Table,
    View,
    MaterializedView,
    Dictionary,
}

impl From<SchemaObjectKind> for CachedObjectKind {
    fn from(value: SchemaObjectKind) -> Self {
        match value {
            SchemaObjectKind::Table => Self::Table,
            SchemaObjectKind::View => Self::View,
            SchemaObjectKind::MaterializedView => Self::MaterializedView,
            SchemaObjectKind::Dictionary => Self::Dictionary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedColumn {
    pub name: String,
    pub type_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub codec_expression: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedObject {
    pub name: String,
    pub engine: String,
    pub kind: CachedObjectKind,
    pub total_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// `None` means columns have not been fetched, not that the object has no
    /// columns. This distinction prevents stale or partial data being marked
    /// invalid in the editor.
    pub columns: Option<HashMap<String, CachedColumn>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedDatabase {
    pub name: String,
    pub objects: HashMap<String, CachedObject>,
    #[serde(default)]
    pub touched: u64,
}

/// One server setting from system.settings, with the layers that can
/// override it: the ClickHouse default, the server/profile value
/// (`changed` when it differs from the default), and the connection's
/// own driver setting when the user configured one in zeDB.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedSetting {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_value: String,
    #[serde(default)]
    pub changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_value: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub type_name: String,
}

/// One function from system.functions. The doc fields (description,
/// syntax, arguments, returned_value) are empty on servers that
/// predate them; the flags exist everywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedFunction {
    pub name: String,
    #[serde(default)]
    pub is_aggregate: bool,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alias_to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub syntax: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub returned_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    pub(super) format: u32,
    pub refreshed_at_ms: u64,
    pub databases: HashMap<String, CachedDatabase>,
    /// The server's settings catalog, refreshed with the table sweep;
    /// empty until a connection has answered once.
    #[serde(default)]
    pub settings: Vec<CachedSetting>,
    /// The server's function catalog, same lifecycle as `settings`.
    #[serde(default)]
    pub functions: Vec<CachedFunction>,
}

impl Default for SchemaSnapshot {
    fn default() -> Self {
        Self {
            format: SNAPSHOT_FORMAT,
            refreshed_at_ms: 0,
            databases: HashMap::new(),
            settings: Vec::new(),
            functions: Vec::new(),
        }
    }
}

impl SchemaSnapshot {
    pub fn database(&self, name: &str) -> Option<&CachedDatabase> {
        self.databases.get(name)
    }

    pub fn object(&self, database: &str, object: &str) -> Option<&CachedObject> {
        self.database(database)?.objects.get(object)
    }

    pub fn column(&self, database: &str, object: &str, column: &str) -> Option<&CachedColumn> {
        self.object(database, object)?.columns.as_ref()?.get(column)
    }

    pub fn setting(&self, name: &str) -> Option<&CachedSetting> {
        self.settings
            .iter()
            .find(|setting| setting.name.eq_ignore_ascii_case(name))
    }

    /// Exact-name match first (ClickHouse function names are mostly
    /// case-sensitive); a case-insensitive fallback only for entries
    /// the server itself flags as case-insensitive.
    pub fn function(&self, name: &str) -> Option<&CachedFunction> {
        self.functions
            .iter()
            .find(|function| function.name == name)
            .or_else(|| {
                self.functions.iter().find(|function| {
                    function.case_insensitive && function.name.eq_ignore_ascii_case(name)
                })
            })
    }

    pub fn warmed_databases(&self) -> usize {
        self.databases
            .values()
            .filter(|database| {
                database
                    .objects
                    .values()
                    .all(|object| object.columns.is_some())
            })
            .count()
    }
}

#[derive(Debug, Clone)]
pub struct TableRecord {
    pub database: String,
    pub name: String,
    pub engine: String,
    pub kind: CachedObjectKind,
    pub total_rows: Option<u64>,
    pub total_bytes: Option<u64>,
    pub comment: String,
}

#[derive(Debug, Clone)]
pub struct ColumnRecord {
    pub object: String,
    pub column: CachedColumn,
}
