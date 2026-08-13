use super::*;

impl ChClient {
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

    /// Measure the actual size difference between a column's current
    /// definition and a proposed one (Phase 8, Tier 3). Builds a throwaway
    /// table with two columns, `base` and `cand`, holding the same sample
    /// of the column's data under each definition, reads back their
    /// compressed sizes, and drops the table. Returns how many times
    /// smaller `cand` is than `base` (e.g. 4.8 for "4.8x smaller"), or None
    /// if it cannot be measured. WRITES to the server, so callers must
    /// gate this to writable connections. The table is forced to Wide parts
    /// so per-column bytes are populated, and dropped even on failure.
    pub async fn measure_codec_savings(
        &self,
        database: &str,
        object: &str,
        column: &str,
        base_def: &str,
        cand_def: &str,
        trial_name: &str,
    ) -> Result<Option<f64>> {
        // A sample large enough to be representative but cheap to write.
        const SAMPLE_ROWS: u64 = 200_000;
        let db = escape_identifier(database);
        let table = escape_identifier(object);
        let col = escape_identifier(column);
        let trial = escape_identifier(trial_name);
        let drop_sql = format!("DROP TABLE IF EXISTS {db}.{trial}");

        // DDL/DML must use `execute` (no result), not `query`: `query`
        // decodes the response as RowBinary and would error on the empty
        // body a CREATE/INSERT/DROP returns, aborting before cleanup and
        // orphaning the table. Only the measurement SELECT uses `query`.
        //
        // Clean up any leftover from an interrupted run, then build.
        let _ = self.execute(&drop_sql).await;
        self.execute(&format!(
            "CREATE TABLE {db}.{trial} (base {base_def}, cand {cand_def}) \
             ENGINE = MergeTree ORDER BY tuple() \
             SETTINGS min_bytes_for_wide_part = 0"
        ))
        .await?;

        let measured = async {
            self.execute(&format!(
                "INSERT INTO {db}.{trial} \
                 SELECT {col}, {col} FROM {db}.{table} LIMIT {SAMPLE_ROWS}"
            ))
            .await?;
            self.query(&format!(
                "SELECT column, sum(column_data_compressed_bytes) \
                 FROM system.parts_columns \
                 WHERE database = '{}' AND table = '{}' AND active \
                 GROUP BY column",
                escape_string(database),
                escape_string(trial_name)
            ))
            .await
        }
        .await;

        // Always drop the trial table, even if the measurement failed.
        let _ = self.execute(&drop_sql).await;

        let result = measured?;
        let mut base_bytes = 0u64;
        let mut cand_bytes = 0u64;
        for row in &result.rows {
            let name = string_at(row, 0, "trial column name")?;
            let bytes = optional_u64_at(row, 1, "trial column bytes")?.unwrap_or(0);
            match name.as_str() {
                "base" => base_bytes = bytes,
                "cand" => cand_bytes = bytes,
                _ => {}
            }
        }
        if cand_bytes == 0 || base_bytes == 0 {
            return Ok(None);
        }
        Ok(Some(base_bytes as f64 / cand_bytes as f64))
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
