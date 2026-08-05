use zedb_core::{QueryResult, Value};

use crate::{ChClient, ChError, Result};

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

impl ChClient {
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
}
