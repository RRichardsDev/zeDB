use super::*;

pub(crate) fn parse_cache_tables(result: QueryResult) -> Result<Vec<TableRecord>> {
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

pub(crate) fn parse_cache_columns(result: QueryResult) -> Result<Vec<ColumnRecord>> {
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

pub(crate) fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

/// The `TO db.target` of a materialized-view create query (Phase 9, Part C).
pub(crate) fn parse_mv_to(create_query: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bTO\s+([A-Za-z0-9_.`]+)").ok()?;
    re.captures(create_query)
        .map(|caps| caps[1].replace('`', ""))
}

/// The first `FROM db.source` of a materialized view's SELECT.
pub(crate) fn parse_mv_from(create_query: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bFROM\s+([A-Za-z0-9_.`]+)").ok()?;
    re.captures(create_query)
        .map(|caps| caps[1].replace('`', ""))
}

/// Backtick-quote an identifier (database, table, column) for use in a
/// query, doubling any embedded backtick. ClickHouse allows arbitrary
/// characters in quoted identifiers, so this must be used wherever a
/// name is interpolated as an identifier rather than a string literal.
pub(crate) fn escape_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

pub(crate) fn parse_databases(result: QueryResult) -> Result<Vec<DatabaseMeta>> {
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

pub(crate) fn parse_schema_objects(result: QueryResult) -> Result<Vec<SchemaObjectMeta>> {
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

pub(crate) fn parse_columns(result: QueryResult) -> Result<Vec<ColumnInfo>> {
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

pub(crate) fn parse_object_details(result: QueryResult) -> Result<ObjectDetails> {
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

pub(crate) fn string_at(row: &[Value], index: usize, label: &str) -> Result<String> {
    match row.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        value => Err(ChError::Decode(format!(
            "expected String for {label}, got {value:?}"
        ))),
    }
}

pub(crate) fn optional_u64_at(row: &[Value], index: usize, label: &str) -> Result<Option<u64>> {
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
