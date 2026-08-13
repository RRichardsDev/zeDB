use super::*;

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
}
