//! Rule-based storage advisor. Pure logic, no AI:
//! given a column's type, current codec, size, and probed cardinality,
//! return a conservative, explainable verdict, and when it is actionable
//! the `ALTER` that applies it. Only high-confidence rules fire; when in
//! doubt the advisor says nothing rather than guess.

/// Everything the rules need about one column.
pub struct ColumnFacts<'a> {
    pub name: &'a str,
    pub type_name: &'a str,
    /// Current `compression_codec`, e.g. `CODEC(ZSTD(1))`; empty for the
    /// table/server default.
    pub codec: &'a str,
    /// Approximate distinct values from the cardinality probe.
    pub distinct: u64,
    pub total_rows: u64,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
}

/// The advisor's verdict for one column.
pub enum Advice {
    /// Already in good shape; `label` names why (e.g. "LowCardinality").
    Good(String),
    /// An actionable change. `label` is the short verdict, `reason` the
    /// one-line why. `apply` is the statement(s) to run in order to make
    /// the change take effect on existing data (a type change is one
    /// `ALTER` that mutates the column; a codec change is the `ALTER`
    /// plus an `OPTIMIZE ... FINAL`, since a codec only affects new and
    /// merged parts otherwise). `editor_sql` is the same, formatted with
    /// comments for the query editor. `base_def`/`cand_def` are the
    /// current and proposed column definitions used by the measured-
    /// savings trial.
    Suggest {
        label: String,
        reason: String,
        apply: Vec<String>,
        editor_sql: String,
        base_def: String,
        cand_def: String,
    },
    /// Nothing worth changing; `reason` explains (e.g. "high cardinality").
    Leave(String),
    /// Not enough information to assess (no rows / no stored data).
    Unknown,
}

/// LowCardinality stops paying off once the dictionary is large; above
/// this many distinct values it is usually a net loss.
const LOW_CARDINALITY_MAX_DISTINCT: u64 = 100_000;

fn escape_ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// Strip `Nullable(...)` and `LowCardinality(...)` wrappers down to the
/// core type name.
fn base_type(type_name: &str) -> &str {
    let mut t = type_name.trim();
    loop {
        if let Some(inner) = t
            .strip_prefix("LowCardinality(")
            .and_then(|s| s.strip_suffix(')'))
        {
            t = inner.trim();
        } else if let Some(inner) = t
            .strip_prefix("Nullable(")
            .and_then(|s| s.strip_suffix(')'))
        {
            t = inner.trim();
        } else {
            return t;
        }
    }
}

/// The optional `ON CLUSTER '<name>'` clause: present in cluster scope so
/// the change reaches every node, empty for a single node.
fn on_cluster(cluster: Option<&str>) -> String {
    match cluster {
        Some(name) => format!(
            " ON CLUSTER '{}'",
            name.replace('\\', "\\\\").replace('\'', "\\'")
        ),
        None => String::new(),
    }
}

fn modify_type(db: &str, table: &str, cluster: Option<&str>, col: &str, new_type: &str) -> String {
    format!(
        "ALTER TABLE {}.{}{} MODIFY COLUMN {} {};",
        escape_ident(db),
        escape_ident(table),
        on_cluster(cluster),
        escape_ident(col),
        new_type
    )
}

/// A column definition (`type [CODEC(...)]`) for the trial table.
fn col_def(type_str: &str, codec: &str) -> String {
    let type_str = type_str.trim();
    if codec.trim().is_empty() {
        type_str.to_string()
    } else {
        format!("{type_str} {}", codec.trim())
    }
}

fn modify_codec(db: &str, table: &str, cluster: Option<&str>, col: &str, codec: &str) -> String {
    format!(
        "ALTER TABLE {}.{}{} MODIFY COLUMN {} CODEC({});",
        escape_ident(db),
        escape_ident(table),
        on_cluster(cluster),
        escape_ident(col),
        codec
    )
}

fn optimize_final(db: &str, table: &str, cluster: Option<&str>) -> String {
    format!(
        "OPTIMIZE TABLE {}.{}{} FINAL;",
        escape_ident(db),
        escape_ident(table),
        on_cluster(cluster)
    )
}

/// Advise on one column. `db`/`table`/`cluster` build the statements;
/// `cluster` is `Some` in cluster scope so they run `ON CLUSTER`.
pub fn advise(f: &ColumnFacts, db: &str, table: &str, cluster: Option<&str>) -> Advice {
    if f.total_rows == 0 || f.compressed_bytes == 0 {
        return Advice::Unknown;
    }

    let base = base_type(f.type_name);
    let is_low_card = f.type_name.trim_start().starts_with("LowCardinality(");
    let distinct_ratio = f.distinct as f64 / f.total_rows as f64;
    let has_delta = f.codec.contains("Delta"); // Delta or DoubleDelta
    let is_temporal = base.starts_with("DateTime") || base.starts_with("Date");
    let is_stringy = base == "String" || base.starts_with("FixedString");

    // LowCardinality over a large distinct set churns the dictionary and
    // usually costs more than it saves: recommend the plain inner type.
    if is_low_card && f.distinct > LOW_CARDINALITY_MAX_DISTINCT {
        let alter = modify_type(db, table, cluster, f.name, base);
        return Advice::Suggest {
            label: "drop LowCardinality".into(),
            reason: format!("{} distinct is too many for a dictionary", f.distinct),
            apply: vec![alter.clone()],
            editor_sql: alter,
            base_def: col_def(f.type_name, f.codec),
            cand_def: col_def(base, f.codec),
        };
    }
    if is_low_card {
        return Advice::Good("already LowCardinality-encoded".into());
    }

    // A low-cardinality string benefits from dictionary encoding.
    if is_stringy && f.distinct <= LOW_CARDINALITY_MAX_DISTINCT && distinct_ratio < 0.5 {
        let wrapped = format!("LowCardinality({})", f.type_name.trim());
        let alter = modify_type(db, table, cluster, f.name, &wrapped);
        return Advice::Suggest {
            label: "LowCardinality".into(),
            reason: format!("only {} distinct values", f.distinct),
            apply: vec![alter.clone()],
            editor_sql: alter,
            base_def: col_def(f.type_name, f.codec),
            cand_def: col_def(&wrapped, f.codec),
        };
    }

    // Temporal columns compress far better with delta coding.
    if is_temporal {
        if has_delta {
            return Advice::Good("already uses delta coding".into());
        }
        // A codec change only affects new and merged parts; existing data
        // is recompressed by OPTIMIZE ... FINAL (a whole-table rewrite),
        // so both statements are needed to realise the saving.
        let alter = modify_codec(db, table, cluster, f.name, "DoubleDelta, ZSTD(1)");
        let optimize = optimize_final(db, table, cluster);
        let editor_sql = format!(
            "{alter}\n\
             -- A codec change only affects new and merged parts. Recompress\n\
             -- existing data with OPTIMIZE ... FINAL (rewrites the whole table;\n\
             -- run in a low-traffic window):\n\
             {optimize}"
        );
        return Advice::Suggest {
            label: "Delta + ZSTD".into(),
            reason: "timestamps compress well with delta coding".into(),
            apply: vec![alter, optimize],
            editor_sql,
            base_def: col_def(f.type_name, f.codec),
            cand_def: col_def(f.type_name, "CODEC(DoubleDelta, ZSTD(1))"),
        };
    }

    // High-entropy strings (hashes, ids, tokens): nothing compresses them.
    if is_stringy && distinct_ratio > 0.8 {
        return Advice::Leave("high cardinality, little to compress".into());
    }

    let ratio = f.uncompressed_bytes as f64 / f.compressed_bytes as f64;
    if ratio >= 10.0 {
        return Advice::Good("already compresses well".into());
    }

    Advice::Leave("no clear codec win".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(type_name: &'a str, codec: &'a str, distinct: u64) -> ColumnFacts<'a> {
        ColumnFacts {
            name: "col",
            type_name,
            codec,
            distinct,
            total_rows: 1_000_000,
            compressed_bytes: 1_000_000,
            uncompressed_bytes: 2_000_000,
        }
    }

    #[test]
    fn low_cardinality_string_is_suggested() {
        let advice = advise(&facts("String", "", 200), "db", "t", None);
        match advice {
            Advice::Suggest {
                label,
                editor_sql,
                apply,
                ..
            } => {
                assert_eq!(label, "LowCardinality");
                assert!(editor_sql.contains("MODIFY COLUMN `col` LowCardinality(String)"));
                assert!(editor_sql.contains("ALTER TABLE `db`.`t`"));
                // A type change is a single mutation, no OPTIMIZE needed.
                assert_eq!(apply.len(), 1);
            }
            _ => panic!("expected a LowCardinality suggestion"),
        }
    }

    #[test]
    fn nullable_string_wraps_correctly() {
        let advice = advise(&facts("Nullable(String)", "", 50), "db", "t", None);
        match advice {
            Advice::Suggest { editor_sql, .. } => {
                assert!(editor_sql.contains("LowCardinality(Nullable(String))"));
            }
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn high_cardinality_string_is_left_alone() {
        // 1M distinct of 1M rows: an id/hash column.
        assert!(matches!(
            advise(&facts("String", "", 1_000_000), "db", "t", None),
            Advice::Leave(_)
        ));
    }

    #[test]
    fn existing_low_cardinality_is_good() {
        assert!(matches!(
            advise(&facts("LowCardinality(String)", "", 5), "db", "t", None),
            Advice::Good(_)
        ));
    }

    #[test]
    fn low_cardinality_with_too_many_distinct_is_dropped() {
        let advice = advise(
            &facts("LowCardinality(String)", "", 500_000),
            "db",
            "t",
            None,
        );
        match advice {
            Advice::Suggest {
                label, editor_sql, ..
            } => {
                assert_eq!(label, "drop LowCardinality");
                assert!(editor_sql.contains("MODIFY COLUMN `col` String"));
            }
            _ => panic!("expected a drop-LowCardinality suggestion"),
        }
    }

    #[test]
    fn temporal_without_delta_is_suggested_delta() {
        let advice = advise(&facts("DateTime", "", 900_000), "db", "t", None);
        match advice {
            Advice::Suggest {
                label,
                editor_sql,
                apply,
                ..
            } => {
                assert_eq!(label, "Delta + ZSTD");
                assert!(editor_sql.contains("CODEC(DoubleDelta, ZSTD(1))"));
                // A codec change must also recompress existing data.
                assert!(editor_sql.contains("OPTIMIZE TABLE `db`.`t` FINAL"));
                assert_eq!(apply.len(), 2);
                assert!(apply[1].contains("OPTIMIZE TABLE `db`.`t` FINAL"));
            }
            _ => panic!("expected a delta suggestion"),
        }
    }

    #[test]
    fn temporal_with_delta_is_good() {
        assert!(matches!(
            advise(
                &facts("DateTime", "CODEC(DoubleDelta, ZSTD(1))", 900_000),
                "db",
                "t",
                None
            ),
            Advice::Good(_)
        ));
    }

    #[test]
    fn cluster_scope_adds_on_cluster() {
        // Type change: ON CLUSTER on the single ALTER.
        match advise(&facts("String", "", 200), "db", "t", Some("my_cluster")) {
            Advice::Suggest {
                apply, editor_sql, ..
            } => {
                assert!(apply[0].contains("ON CLUSTER 'my_cluster'"));
                assert!(editor_sql.contains("ON CLUSTER 'my_cluster'"));
            }
            _ => panic!("expected a suggestion"),
        }
        // Codec change: ON CLUSTER on both the ALTER and the OPTIMIZE.
        match advise(&facts("DateTime", "", 900_000), "db", "t", Some("c")) {
            Advice::Suggest { apply, .. } => {
                assert!(apply[0].contains("ON CLUSTER 'c'"));
                assert!(apply[1].contains("OPTIMIZE TABLE `db`.`t` ON CLUSTER 'c' FINAL"));
            }
            _ => panic!("expected a delta suggestion"),
        }
    }

    #[test]
    fn empty_table_is_unknown() {
        let mut f = facts("String", "", 0);
        f.total_rows = 0;
        assert!(matches!(advise(&f, "db", "t", None), Advice::Unknown));
    }
}
