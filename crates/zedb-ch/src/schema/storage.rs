use super::*;

impl ChClient {
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

    /// Active parts grouped by partition, so the schema inspector can
    /// show the MergeTree lifecycle: how many parts each
    /// partition has (a "too many parts" signal), its rows and compressed
    /// size. Reads the connected node's `system.parts`.
    pub async fn table_partitions(
        &self,
        database: &str,
        object: &str,
    ) -> Result<Vec<PartitionStats>> {
        let database = escape_string(database);
        let object = escape_string(object);
        let result = self
            .query(&format!(
                "SELECT partition, count(), sum(rows), \
                        sum(data_compressed_bytes), sum(data_uncompressed_bytes), max(level) \
                 FROM system.parts \
                 WHERE database = '{database}' AND table = '{object}' AND active \
                 GROUP BY partition \
                 ORDER BY min(min_time), partition"
            ))
            .await?;
        result
            .rows
            .iter()
            .map(|row| {
                Ok(PartitionStats {
                    partition: string_at(row, 0, "partition")?,
                    parts: optional_u64_at(row, 1, "part count")?.unwrap_or(0),
                    rows: optional_u64_at(row, 2, "rows")?.unwrap_or(0),
                    compressed_bytes: optional_u64_at(row, 3, "compressed bytes")?.unwrap_or(0),
                    uncompressed_bytes: optional_u64_at(row, 4, "uncompressed bytes")?.unwrap_or(0),
                    max_level: optional_u64_at(row, 5, "max level")?.unwrap_or(0),
                })
            })
            .collect()
    }

    /// Merges (and mutation-merges) running right now for this table.
    /// Polled while the Parts tab is open so progress updates live.
    /// `elapsed` and `progress` are cast to integers in SQL to reuse the
    /// integer row accessors.
    pub async fn active_merges(&self, database: &str, object: &str) -> Result<Vec<MergeInfo>> {
        let database = escape_string(database);
        let object = escape_string(object);
        let result = self
            .query(&format!(
                "SELECT partition_id, result_part_name, num_parts, \
                        toUInt64(round(elapsed)), toUInt64(round(progress * 100)), \
                        rows_read, rows_written, memory_usage, is_mutation \
                 FROM system.merges \
                 WHERE database = '{database}' AND table = '{object}' \
                 ORDER BY progress DESC"
            ))
            .await?;
        result
            .rows
            .iter()
            .map(|row| {
                Ok(MergeInfo {
                    partition_id: string_at(row, 0, "partition_id")?,
                    result_part: string_at(row, 1, "result_part_name")?,
                    num_parts: optional_u64_at(row, 2, "num_parts")?.unwrap_or(0),
                    elapsed_secs: optional_u64_at(row, 3, "elapsed")?.unwrap_or(0),
                    progress_pct: optional_u64_at(row, 4, "progress")?.unwrap_or(0),
                    rows_read: optional_u64_at(row, 5, "rows_read")?.unwrap_or(0),
                    rows_written: optional_u64_at(row, 6, "rows_written")?.unwrap_or(0),
                    memory_usage: optional_u64_at(row, 7, "memory_usage")?.unwrap_or(0),
                    is_mutation: optional_u64_at(row, 8, "is_mutation")?.unwrap_or(0) != 0,
                })
            })
            .collect()
    }
}
