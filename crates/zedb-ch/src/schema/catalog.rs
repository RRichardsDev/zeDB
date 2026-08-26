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

    /// The server's settings catalog, folded with this connection's
    /// own driver settings so the editor can say which layer a query
    /// SETTINGS would override. The `default` column is newer than
    /// some servers; those fall back to the value-only form.
    pub async fn list_settings(&self) -> Result<Vec<crate::schema_cache::CachedSetting>> {
        let with_default = self
            .query(
                "SELECT name, value, changed, description, type, `default` \
                 FROM system.settings ORDER BY name",
            )
            .await;
        let result = match with_default {
            Ok(result) => result,
            Err(_) => {
                self.query(
                    "SELECT name, value, changed, description, type, value AS `default` \
                     FROM system.settings ORDER BY name",
                )
                .await?
            }
        };
        let text = |value: Option<&Value>| value.map(ToString::to_string).unwrap_or_default();
        let mut settings: Vec<crate::schema_cache::CachedSetting> = result
            .rows
            .iter()
            .map(|row| crate::schema_cache::CachedSetting {
                name: text(row.first()),
                value: text(row.get(1)),
                changed: matches!(row.get(2), Some(Value::UInt(1) | Value::Int(1))),
                description: text(row.get(3)),
                type_name: text(row.get(4)),
                default_value: text(row.get(5)),
                connection_value: None,
            })
            .filter(|setting| !setting.name.is_empty())
            .collect();
        for driver_setting in &self.cfg.driver.settings {
            let name = driver_setting.name.trim();
            if name.is_empty() {
                continue;
            }
            if let Some(setting) = settings
                .iter_mut()
                .find(|setting| setting.name.eq_ignore_ascii_case(name))
            {
                setting.connection_value = Some(driver_setting.value.trim().to_string());
            }
        }
        Ok(settings)
    }

    /// The server's function catalog. Doc columns (description, syntax,
    /// arguments, returned_value) are newer than some servers; those
    /// fall back to the flags-only form.
    pub async fn list_functions(&self) -> Result<Vec<crate::schema_cache::CachedFunction>> {
        let with_docs = self
            .query(
                "SELECT name, is_aggregate, case_insensitive, alias_to, \
                    description, syntax, arguments, returned_value \
                 FROM system.functions ORDER BY name",
            )
            .await;
        let result = match with_docs {
            Ok(result) => result,
            Err(_) => {
                self.query(
                    "SELECT name, is_aggregate, case_insensitive, alias_to, \
                        '' AS description, '' AS syntax, '' AS arguments, \
                        '' AS returned_value \
                     FROM system.functions ORDER BY name",
                )
                .await?
            }
        };
        let text = |value: Option<&Value>| value.map(ToString::to_string).unwrap_or_default();
        let flag = |value: Option<&Value>| matches!(value, Some(Value::UInt(1) | Value::Int(1)));
        Ok(result
            .rows
            .iter()
            .map(|row| crate::schema_cache::CachedFunction {
                name: text(row.first()),
                is_aggregate: flag(row.get(1)),
                case_insensitive: flag(row.get(2)),
                alias_to: text(row.get(3)),
                description: text(row.get(4)),
                syntax: text(row.get(5)),
                arguments: text(row.get(6)),
                returned_value: text(row.get(7)),
            })
            .filter(|function| !function.name.is_empty())
            .collect())
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
