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
                    total_rows, total_bytes, comment \
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

    /// Summed size and rows of a sharded table: one replica per shard
    /// via the cluster() table function. Distributed tables report no
    /// storage of their own; this is the honest fleet-wide number.
    pub async fn distributed_totals(
        &self,
        cluster: &str,
        database: &str,
        table: &str,
    ) -> Result<(Option<u64>, Option<u64>)> {
        let cluster = escape_string(cluster);
        let database = escape_string(database);
        let table = escape_string(table);
        let result = self
            .query(&format!(
                "SELECT sum(total_bytes), sum(total_rows)                  FROM cluster('{cluster}', system.tables)                  WHERE database = '{database}' AND name = '{table}'"
            ))
            .await?;
        let number = |value: Option<&Value>| match value {
            Some(Value::UInt(number)) => Some(*number),
            Some(Value::Int(number)) => Some(*number as u64),
            _ => None,
        };
        let row = result.rows.first();
        Ok((
            number(row.and_then(|row| row.first())),
            number(row.and_then(|row| row.get(1))),
        ))
    }

    /// Table-wide compression from the local node's active parts. Part
    /// bytes are tracked for every part type (Compact included), so this
    /// is the always-available compression figure even when per-column
    /// sizes are not. Returns None for objects with no parts (views,
    /// dictionaries, empty tables).
    pub async fn table_storage(
        &self,
        database: &str,
        object: &str,
    ) -> Result<Option<TableStorage>> {
        let database = escape_string(database);
        let object = escape_string(object);
        let result = self
            .query(&format!(
                "SELECT sum(data_compressed_bytes), sum(data_uncompressed_bytes), \
                        countIf(part_type = 'Wide'), countIf(part_type = 'Compact') \
                 FROM system.parts \
                 WHERE database = '{database}' AND table = '{object}' AND active"
            ))
            .await?;
        let Some(row) = result.rows.first() else {
            return Ok(None);
        };
        let compressed = optional_u64_at(row, 0, "compressed bytes")?.unwrap_or(0);
        if compressed == 0 {
            return Ok(None);
        }
        Ok(Some(TableStorage {
            compressed_bytes: compressed,
            uncompressed_bytes: optional_u64_at(row, 1, "uncompressed bytes")?.unwrap_or(0),
            wide_parts: optional_u64_at(row, 2, "wide part count")?.unwrap_or(0),
            compact_parts: optional_u64_at(row, 3, "compact part count")?.unwrap_or(0),
        }))
    }

    pub async fn list_columns(&self, database: &str, object: &str) -> Result<Vec<ColumnInfo>> {
        let database = escape_string(database);
        let object = escape_string(object);
        let result = self
            .query(&format!(
                "SELECT name, type, \
                        data_compressed_bytes, data_uncompressed_bytes, \
                        compression_codec \
                 FROM system.columns \
                 WHERE database = '{database}' AND table = '{object}' \
                 ORDER BY position"
            ))
            .await?;
        parse_columns(result)
    }

    /// Approximate distinct count per column in one table scan
    /// (`uniqCombined`), aligned to the given column order. Opt-in from
    /// the UI because it reads the whole table; callers run it off the
    /// main thread. Feeds the codec advisor (a low distinct-ratio column
    /// is a `LowCardinality` candidate).
    pub async fn column_cardinalities(
        &self,
        database: &str,
        object: &str,
        columns: &[String],
    ) -> Result<Vec<u64>> {
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let db = escape_identifier(database);
        let table = escape_identifier(object);
        let exprs = columns
            .iter()
            .map(|column| format!("uniqCombined({})", escape_identifier(column)))
            .collect::<Vec<_>>()
            .join(", ");
        let result = self
            .query(&format!("SELECT {exprs} FROM {db}.{table}"))
            .await?;
        let row = result
            .rows
            .first()
            .ok_or_else(|| ChError::Decode("cardinality probe returned no rows".into()))?;
        (0..columns.len())
            .map(|index| Ok(optional_u64_at(row, index, "column cardinality")?.unwrap_or(0)))
            .collect()
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
                total_bytes: optional_u64_at(&row, 5, "schema object byte count")?,
                comment: string_at(&row, 6, "schema object comment")?,
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

/// Backtick-quote an identifier (database, table, column) for use in a
/// query, doubling any embedded backtick. ClickHouse allows arbitrary
/// characters in quoted identifiers, so this must be used wherever a
/// name is interpolated as an identifier rather than a string literal.
fn escape_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
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
                compressed_bytes: optional_u64_at(&row, 2, "compressed bytes")?.unwrap_or(0),
                uncompressed_bytes: optional_u64_at(&row, 3, "uncompressed bytes")?.unwrap_or(0),
                codec: string_at(&row, 4, "compression codec")?,
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

/// The comma-split top-level arguments of a Distributed engine
/// definition: `Distributed(cluster, database, table[, key[, policy]])`.
fn distributed_args(engine_full: &str) -> Option<Vec<String>> {
    let start = engine_full.find("Distributed(")? + "Distributed(".len();
    let rest = &engine_full[start..];
    let mut depth = 0usize;
    let mut in_string = false;
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in rest.chars() {
        match ch {
            '\'' if depth == 0 => {
                in_string = !in_string;
                current.push(ch);
            }
            '(' if !in_string => {
                depth += 1;
                current.push(ch);
            }
            ')' if !in_string => {
                if depth == 0 {
                    args.push(current.trim().to_string());
                    return Some(args);
                }
                depth -= 1;
                current.push(ch);
            }
            ',' if !in_string && depth == 0 => {
                args.push(current.trim().to_string());
                current = String::new();
            }
            other => current.push(other),
        }
    }
    None
}

fn unquote(argument: &str) -> String {
    argument.trim().trim_matches('\'').to_string()
}

/// The sharding expression of a Distributed engine definition.
/// Returns `None` for non-Distributed engines or the 3-argument form.
pub fn distributed_sharding_key(engine_full: &str) -> Option<String> {
    let args = distributed_args(engine_full)?;
    let key = args.get(3)?.clone();
    // A quoted fourth argument is a storage policy, not a sharding
    // expression.
    (!key.is_empty() && !key.starts_with('\'')).then_some(key)
}

/// The (cluster, database, local table) a Distributed engine scatters
/// over.
pub fn distributed_target(engine_full: &str) -> Option<(String, String, String)> {
    let args = distributed_args(engine_full)?;
    if args.len() < 3 {
        return None;
    }
    Some((unquote(&args[0]), unquote(&args[1]), unquote(&args[2])))
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

#[cfg(test)]
mod sharding_key_tests {
    use super::distributed_sharding_key;

    #[test]
    fn extracts_the_sharding_expression() {
        assert_eq!(
            distributed_sharding_key(
                "Distributed('zedb_sharded', 'shard_demo', 'events_local', user_id)"
            )
            .as_deref(),
            Some("user_id")
        );
        assert_eq!(
            distributed_sharding_key("Distributed('c', 'db', 't', cityHash64(user_id, kind))")
                .as_deref(),
            Some("cityHash64(user_id, kind)")
        );
        // Three-argument form: no key.
        assert_eq!(
            distributed_sharding_key("Distributed('c', 'db', 't')"),
            None
        );
        // Quoted fourth argument is a policy, not a key.
        assert_eq!(
            distributed_sharding_key("Distributed('c', 'db', 't', 'policy')"),
            None
        );
        assert_eq!(distributed_sharding_key("MergeTree ORDER BY id"), None);
    }
}
