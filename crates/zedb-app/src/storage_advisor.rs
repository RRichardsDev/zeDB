//! Rule-based storage advisor (Phase 8, Tier 2). Pure logic, no AI:
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
    /// An actionable change: `label` is the short verdict, `reason` the
    /// one-line why, `alter` the ready-to-run statement.
    Suggest {
        label: String,
        reason: String,
        alter: String,
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

fn modify_type(db: &str, table: &str, col: &str, new_type: &str) -> String {
    format!(
        "ALTER TABLE {}.{} MODIFY COLUMN {} {};",
        escape_ident(db),
        escape_ident(table),
        escape_ident(col),
        new_type
    )
}

fn modify_codec(db: &str, table: &str, col: &str, codec: &str) -> String {
    format!(
        "ALTER TABLE {}.{} MODIFY COLUMN {} CODEC({});",
        escape_ident(db),
        escape_ident(table),
        escape_ident(col),
        codec
    )
}

/// Advise on one column. `db`/`table` are used to build the `ALTER`.
pub fn advise(f: &ColumnFacts, db: &str, table: &str) -> Advice {
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
        return Advice::Suggest {
            label: "drop LowCardinality".into(),
            reason: format!("{} distinct is too many for a dictionary", f.distinct),
            alter: modify_type(db, table, f.name, base),
        };
    }
    if is_low_card {
        return Advice::Good("already LowCardinality-encoded".into());
    }

    // A low-cardinality string benefits from dictionary encoding.
    if is_stringy && f.distinct <= LOW_CARDINALITY_MAX_DISTINCT && distinct_ratio < 0.5 {
        return Advice::Suggest {
            label: "LowCardinality".into(),
            reason: format!("only {} distinct values", f.distinct),
            alter: modify_type(
                db,
                table,
                f.name,
                &format!("LowCardinality({})", f.type_name.trim()),
            ),
        };
    }

    // Temporal columns compress far better with delta coding.
    if is_temporal {
        if has_delta {
            return Advice::Good("already uses delta coding".into());
        }
        return Advice::Suggest {
            label: "Delta + ZSTD".into(),
            reason: "timestamps compress well with delta coding".into(),
            alter: modify_codec(db, table, f.name, "DoubleDelta, ZSTD(1)"),
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
        let advice = advise(&facts("String", "", 200), "db", "t");
        match advice {
            Advice::Suggest { label, alter, .. } => {
                assert_eq!(label, "LowCardinality");
                assert!(alter.contains("MODIFY COLUMN `col` LowCardinality(String)"));
                assert!(alter.contains("ALTER TABLE `db`.`t`"));
            }
            _ => panic!("expected a LowCardinality suggestion"),
        }
    }

    #[test]
    fn nullable_string_wraps_correctly() {
        let advice = advise(&facts("Nullable(String)", "", 50), "db", "t");
        match advice {
            Advice::Suggest { alter, .. } => {
                assert!(alter.contains("LowCardinality(Nullable(String))"));
            }
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn high_cardinality_string_is_left_alone() {
        // 1M distinct of 1M rows: an id/hash column.
        assert!(matches!(
            advise(&facts("String", "", 1_000_000), "db", "t"),
            Advice::Leave(_)
        ));
    }

    #[test]
    fn existing_low_cardinality_is_good() {
        assert!(matches!(
            advise(&facts("LowCardinality(String)", "", 5), "db", "t"),
            Advice::Good(_)
        ));
    }

    #[test]
    fn low_cardinality_with_too_many_distinct_is_dropped() {
        let advice = advise(&facts("LowCardinality(String)", "", 500_000), "db", "t");
        match advice {
            Advice::Suggest { label, alter, .. } => {
                assert_eq!(label, "drop LowCardinality");
                assert!(alter.contains("MODIFY COLUMN `col` String"));
            }
            _ => panic!("expected a drop-LowCardinality suggestion"),
        }
    }

    #[test]
    fn temporal_without_delta_is_suggested_delta() {
        let advice = advise(&facts("DateTime", "", 900_000), "db", "t");
        match advice {
            Advice::Suggest { label, alter, .. } => {
                assert_eq!(label, "Delta + ZSTD");
                assert!(alter.contains("CODEC(DoubleDelta, ZSTD(1))"));
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
                "t"
            ),
            Advice::Good(_)
        ));
    }

    #[test]
    fn empty_table_is_unknown() {
        let mut f = facts("String", "", 0);
        f.total_rows = 0;
        assert!(matches!(advise(&f, "db", "t"), Advice::Unknown));
    }
}
