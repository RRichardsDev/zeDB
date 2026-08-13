use super::*;

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
}
