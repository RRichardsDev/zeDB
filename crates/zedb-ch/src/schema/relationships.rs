use super::*;

impl ChClient {
    /// The materialized-view dependency neighborhood of an object (Phase 9,
    /// Part C): its own source/target if it is an MV, the MVs it feeds
    /// (from `dependencies_table`), and the MVs that target it. Two reads:
    /// the object row, and every MV's create query (MVs are few).
    pub async fn object_dependencies(
        &self,
        database: &str,
        object: &str,
    ) -> Result<ObjectDependencies> {
        let db_esc = escape_string(database);
        let obj_esc = escape_string(object);
        let full = format!("{database}.{object}");

        // Every MV's read/write edges, parsed from its create query.
        let mv_rows = self
            .query(
                "SELECT concat(database, '.', name), create_table_query \
                 FROM system.tables WHERE engine LIKE '%MaterializedView%'",
            )
            .await?;
        let mut mvs: Vec<MvDependency> = Vec::new();
        for row in &mv_rows.rows {
            let view = string_at(row, 0, "mv name")?;
            let create = string_at(row, 1, "mv create")?;
            mvs.push(MvDependency {
                view,
                source: parse_mv_from(&create),
                target: parse_mv_to(&create),
            });
        }

        let this = self
            .query(&format!(
                "SELECT engine, create_table_query, \
                        arrayStringConcat(arrayMap((d, t) -> concat(d, '.', t), \
                            dependencies_database, dependencies_table), '\\n') \
                 FROM system.tables WHERE database = '{db_esc}' AND name = '{obj_esc}'"
            ))
            .await?;

        let mut deps = ObjectDependencies::default();
        if let Some(row) = this.rows.first() {
            let engine = string_at(row, 0, "engine")?;
            let create = string_at(row, 1, "create query")?;
            deps.is_materialized_view = engine.contains("MaterializedView");
            if deps.is_materialized_view {
                deps.reads_from = parse_mv_from(&create);
                deps.writes_to = parse_mv_to(&create);
            }
            for view in string_at(row, 2, "dependencies")?
                .lines()
                .filter(|line| !line.is_empty())
            {
                let target = mvs
                    .iter()
                    .find(|mv| mv.view == view)
                    .and_then(|mv| mv.target.clone());
                deps.feeds.push(MvDependency {
                    view: view.to_string(),
                    source: Some(full.clone()),
                    target,
                });
            }
        }

        deps.written_by = mvs
            .into_iter()
            .filter(|mv| mv.target.as_deref() == Some(full.as_str()))
            .collect();

        // Flag any referenced table in the lineage that no longer exists
        // (a broken MV silently drops inserts).
        let mut referenced: Vec<String> = Vec::new();
        referenced.extend(deps.reads_from.clone());
        referenced.extend(deps.writes_to.clone());
        for mv in deps.feeds.iter().chain(deps.written_by.iter()) {
            referenced.push(mv.view.clone());
            referenced.extend(mv.source.clone());
            referenced.extend(mv.target.clone());
        }
        referenced.retain(|name| name != &full);
        referenced.sort();
        referenced.dedup();
        if !referenced.is_empty() {
            let list = referenced
                .iter()
                .map(|name| format!("'{}'", escape_string(name)))
                .collect::<Vec<_>>()
                .join(", ");
            let existing: Vec<String> = self
                .query(&format!(
                    "SELECT concat(database, '.', name) FROM system.tables \
                     WHERE concat(database, '.', name) IN ({list})"
                ))
                .await
                .map(|result| {
                    result
                        .rows
                        .iter()
                        .filter_map(|row| string_at(row, 0, "table name").ok())
                        .collect()
                })
                .unwrap_or_default();
            deps.missing_tables = referenced
                .into_iter()
                .filter(|name| !existing.contains(name))
                .collect();
        }

        Ok(deps)
    }

    /// Projections attached to a table with their size (Phase 9, Part C).
    /// `system.projections` is a newer table; callers tolerate its absence.
    pub async fn table_projections(
        &self,
        database: &str,
        object: &str,
    ) -> Result<Vec<ProjectionInfo>> {
        let db_esc = escape_string(database);
        let obj_esc = escape_string(object);
        let defs = self
            .query(&format!(
                "SELECT name, toString(type), arrayStringConcat(sorting_key, ', '), query \
                 FROM system.projections WHERE database = '{db_esc}' AND table = '{obj_esc}' \
                 ORDER BY name"
            ))
            .await?;
        // Sizes are best-effort (tolerate system.projection_parts absence).
        let size_rows = self
            .query(&format!(
                "SELECT name, sum(rows), sum(data_compressed_bytes), count() \
                 FROM system.projection_parts \
                 WHERE database = '{db_esc}' AND table = '{obj_esc}' AND active \
                 GROUP BY name"
            ))
            .await
            .map(|result| result.rows)
            .unwrap_or_default();

        defs.rows
            .iter()
            .map(|row| {
                let name = string_at(row, 0, "projection name")?;
                let mut rows = 0;
                let mut compressed_bytes = 0;
                let mut parts = 0;
                for size in &size_rows {
                    if string_at(size, 0, "size name").ok().as_deref() == Some(name.as_str()) {
                        rows = optional_u64_at(size, 1, "rows")?.unwrap_or(0);
                        compressed_bytes = optional_u64_at(size, 2, "bytes")?.unwrap_or(0);
                        parts = optional_u64_at(size, 3, "parts")?.unwrap_or(0);
                        break;
                    }
                }
                Ok(ProjectionInfo {
                    name,
                    kind: string_at(row, 1, "projection type")?,
                    sorting_key: string_at(row, 2, "sorting key")?,
                    query: string_at(row, 3, "projection query")?,
                    rows,
                    compressed_bytes,
                    parts,
                })
            })
            .collect()
    }
}
