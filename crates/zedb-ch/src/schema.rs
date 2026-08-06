use zedb_core::{QueryResult, Value};

use crate::{
    schema_cache::{CachedColumn, CachedObjectKind, ColumnRecord, TableRecord},
    ChClient, ChError, Result,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDetails {
    pub engine_full: String,
    pub partition_key: String,
    pub sorting_key: String,
    pub primary_key: String,
    pub create_table_query: String,
}

impl ChClient {
    /// Fetch the cheap, fleet-wide portion of the schema cache in one sweep.
    /// Column metadata is intentionally fetched separately per database.
    pub async fn list_schema_cache_tables(&self) -> Result<Vec<TableRecord>> {
        let result = self
            .query(
                "SELECT database, name, engine, \
                    multiIf(engine = 'View', 'view', \
                            engine = 'MaterializedView', 'materialized_view', \
                            engine = 'Dictionary', 'dictionary', 'table') AS kind, \
                    total_rows, comment \
                 FROM system.tables \
                 WHERE database NOT IN ('INFORMATION_SCHEMA', 'information_schema', 'system') \
                 ORDER BY database, name",
            )
            .await?;
        parse_cache_tables(result)
    }

    /// Fetch every column for one database. Keeping the database predicate
    /// mandatory prevents a large fleet scan during connection warm-up.
    pub async fn list_schema_cache_columns(&self, database: &str) -> Result<Vec<ColumnRecord>> {
        let database = escape_string(database);
        let result = self
            .query(&format!(
                "SELECT table, name, type, default_kind, default_expression, \
                        compression_codec, comment \
                 FROM system.columns \
                 WHERE database = '{database}' \
                 ORDER BY table, position"
            ))
            .await?;
        parse_cache_columns(result)
    }

    pub async fn list_databases(&self) -> Result<Vec<DatabaseMeta>> {
        let result = self
            .query(
                "SELECT name FROM system.databases \
                 WHERE name NOT IN ('INFORMATION_SCHEMA', 'information_schema', 'system') \
                 ORDER BY name",
            )
            .await?;
        parse_databases(result)
    }

    pub async fn list_schema_objects(&self, database: &str) -> Result<Vec<SchemaObjectMeta>> {
        let database = escape_string(database);
        let result = self
            .query(&format!(
                "SELECT name, engine, \
                    multiIf(engine = 'View', 'view', \
                            engine = 'MaterializedView', 'materialized_view', \
                            engine = 'Dictionary', 'dictionary', 'table') AS kind, \
                    total_rows, total_bytes \
                 FROM system.tables \
                 WHERE database = '{database}' \
                 ORDER BY name"
            ))
            .await?;
        parse_schema_objects(result)
    }

    pub async fn list_columns(&self, database: &str, object: &str) -> Result<Vec<ColumnInfo>> {
        let database = escape_string(database);
        let object = escape_string(object);
        let result = self
            .query(&format!(
                "SELECT name, type \
                 FROM system.columns \
                 WHERE database = '{database}' AND table = '{object}' \
                 ORDER BY position"
            ))
            .await?;
        parse_columns(result)
    }

    pub async fn object_details(&self, database: &str, object: &str) -> Result<ObjectDetails> {
        let database = escape_string(database);
        let object = escape_string(object);
        let result = self
            .query(&format!(
                "SELECT engine_full, partition_key, sorting_key, primary_key, \
                        formatQuery(create_table_query) \
                 FROM system.tables \
                 WHERE database = '{database}' AND name = '{object}'"
            ))
            .await?;
        parse_object_details(result)
    }
}

fn parse_cache_tables(result: QueryResult) -> Result<Vec<TableRecord>> {
    result
        .rows
        .into_iter()
        .map(|row| {
            let kind = match string_at(&row, 3, "schema object kind")?.as_str() {
                "table" => CachedObjectKind::Table,
                "view" => CachedObjectKind::View,
                "materialized_view" => CachedObjectKind::MaterializedView,
                "dictionary" => CachedObjectKind::Dictionary,
                value => {
                    return Err(ChError::Decode(format!(
                        "unknown schema object kind {value:?}"
                    )))
                }
            };
            Ok(TableRecord {
                database: string_at(&row, 0, "database name")?,
                name: string_at(&row, 1, "schema object name")?,
                engine: string_at(&row, 2, "schema object engine")?,
                kind,
                total_rows: optional_u64_at(&row, 4, "schema object row count")?,
                comment: string_at(&row, 5, "schema object comment")?,
            })
        })
        .collect()
}

fn parse_cache_columns(result: QueryResult) -> Result<Vec<ColumnRecord>> {
    result
        .rows
        .into_iter()
        .map(|row| {
            let default_kind = string_at(&row, 3, "column default kind")?;
            let default_expression = string_at(&row, 4, "column default expression")?;
            let codec = string_at(&row, 5, "column codec")?;
            let codec_expression = match (default_kind.is_empty(), default_expression.is_empty()) {
                (true, _) => codec,
                (false, true) if codec.is_empty() => default_kind,
                (false, true) => format!("{default_kind}; {codec}"),
                (false, false) if codec.is_empty() => {
                    format!("{default_kind} {default_expression}")
                }
                (false, false) => format!("{default_kind} {default_expression}; {codec}"),
            };
            Ok(ColumnRecord {
                object: string_at(&row, 0, "schema object name")?,
                column: CachedColumn {
                    name: string_at(&row, 1, "column name")?,
                    type_name: string_at(&row, 2, "column type")?,
                    codec_expression,
                    comment: string_at(&row, 6, "column comment")?,
                },
            })
        })
        .collect()
}

fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn parse_databases(result: QueryResult) -> Result<Vec<DatabaseMeta>> {
    result
        .rows
        .into_iter()
        .map(|row| {
            Ok(DatabaseMeta {
                name: string_at(&row, 0, "database name")?,
            })
        })
        .collect()
}

fn parse_schema_objects(result: QueryResult) -> Result<Vec<SchemaObjectMeta>> {
    result
        .rows
        .into_iter()
        .map(|row| {
            let kind = match string_at(&row, 2, "schema object kind")?.as_str() {
                "table" => SchemaObjectKind::Table,
                "view" => SchemaObjectKind::View,
                "materialized_view" => SchemaObjectKind::MaterializedView,
                "dictionary" => SchemaObjectKind::Dictionary,
                value => {
                    return Err(ChError::Decode(format!(
                        "unknown schema object kind {value:?}"
                    )))
                }
            };
            Ok(SchemaObjectMeta {
                name: string_at(&row, 0, "schema object name")?,
                engine: string_at(&row, 1, "schema object engine")?,
                kind,
                total_rows: optional_u64_at(&row, 3, "schema object row count")?,
                total_bytes: optional_u64_at(&row, 4, "schema object byte count")?,
            })
        })
        .collect()
}

fn parse_columns(result: QueryResult) -> Result<Vec<ColumnInfo>> {
    result
        .rows
        .into_iter()
        .map(|row| {
            Ok(ColumnInfo {
                name: string_at(&row, 0, "column name")?,
                type_name: string_at(&row, 1, "column type")?,
            })
        })
        .collect()
}

fn parse_object_details(result: QueryResult) -> Result<ObjectDetails> {
    let mut rows = result.rows.into_iter();
    let row = rows
        .next()
        .ok_or_else(|| ChError::Decode("schema object no longer exists".into()))?;
    if rows.next().is_some() {
        return Err(ChError::Decode(
            "expected one schema object details row".into(),
        ));
    }
    Ok(ObjectDetails {
        engine_full: string_at(&row, 0, "engine definition")?,
        partition_key: string_at(&row, 1, "partition key")?,
        sorting_key: string_at(&row, 2, "sorting key")?,
        primary_key: string_at(&row, 3, "primary key")?,
        create_table_query: string_at(&row, 4, "create table query")?,
    })
}

fn string_at(row: &[Value], index: usize, label: &str) -> Result<String> {
    match row.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        value => Err(ChError::Decode(format!(
            "expected String for {label}, got {value:?}"
        ))),
    }
}

fn optional_u64_at(row: &[Value], index: usize, label: &str) -> Result<Option<u64>> {
    match row.get(index) {
        Some(Value::UInt(value)) => Ok(Some(*value)),
        Some(Value::Null) => Ok(None),
        value => Err(ChError::Decode(format!(
            "expected Nullable(UInt64) for {label}, got {value:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use zedb_core::{ColumnMeta, QueryResult};

    use super::*;

    fn result(rows: Vec<Vec<Value>>) -> QueryResult {
        QueryResult {
            columns: Vec::<ColumnMeta>::new(),
            rows,
        }
    }

    #[test]
    fn parses_schema_objects_and_nullable_metrics() {
        let objects = parse_schema_objects(result(vec![
            vec![
                Value::String("events".into()),
                Value::String("MergeTree".into()),
                Value::String("table".into()),
                Value::UInt(12),
                Value::UInt(512),
            ],
            vec![
                Value::String("latest".into()),
                Value::String("View".into()),
                Value::String("view".into()),
                Value::Null,
                Value::Null,
            ],
        ]))
        .unwrap();

        assert_eq!(objects[0].kind, SchemaObjectKind::Table);
        assert_eq!(objects[0].total_rows, Some(12));
        assert_eq!(objects[1].kind, SchemaObjectKind::View);
        assert_eq!(objects[1].total_rows, None);
    }

    #[test]
    fn escapes_clickhouse_string_literals() {
        assert_eq!(escape_string("a'b\\c"), "a\\'b\\\\c");
    }

    #[test]
    fn parses_object_details() {
        let details = parse_object_details(result(vec![vec![
            Value::String("MergeTree ORDER BY id".into()),
            Value::String("toYYYYMM(created_at)".into()),
            Value::String("id".into()),
            Value::String("id".into()),
            Value::String("CREATE TABLE events (id UInt64) ENGINE = MergeTree ORDER BY id".into()),
        ]]))
        .unwrap();

        assert_eq!(details.sorting_key, "id");
        assert!(details
            .create_table_query
            .starts_with("CREATE TABLE events"));
    }
}
