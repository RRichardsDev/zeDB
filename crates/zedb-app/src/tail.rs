//! Live tail (Phase 10): the pure query-building and key-tracking core.
//!
//! A tail is a periodic `SELECT ... WHERE key > :last ORDER BY key LIMIT n`
//! over the HTTP interface (mechanism decided in `docs/PHASE-10.md`): each
//! poll prunes by the monotonic key instead of rescanning, so cost stays
//! flat however long the tail runs. This module is the deterministic part,
//! the SQL strings and the last-seen-key literal, unit-tested; the poll
//! loop and rendering live in `main.rs`.

use zedb_core::Value;

/// Rows fetched per poll. A burst larger than this is carried across polls
/// by the advancing key, never lost.
pub const TAIL_BATCH: usize = 500;
/// Rows the initial (priming) load shows, always small: a tail should open
/// light and only grow if the user chose a larger retention cap.
pub const TAIL_SEED: usize = 20;
/// Poll cadence.
pub const TAIL_INTERVAL_MS: u64 = 1_500;

/// The newest `limit` rows, returned oldest-first so the grid reads
/// top-to-bottom like `tail -f`. Used once to prime the view.
pub fn seed_sql(database: &str, table: &str, key: &str, limit: usize) -> String {
    format!(
        "SELECT * FROM (SELECT * FROM {db}.{tbl} ORDER BY {key} DESC LIMIT {limit}) \
         ORDER BY {key} ASC",
        db = quote_ident(database),
        tbl = quote_ident(table),
        key = quote_ident(key),
    )
}

/// Rows strictly newer than the last seen key, oldest-first, capped.
pub fn poll_sql(database: &str, table: &str, key: &str, last: &str, limit: usize) -> String {
    format!(
        "SELECT * FROM {db}.{tbl} WHERE {key} > {last} ORDER BY {key} ASC LIMIT {limit}",
        db = quote_ident(database),
        tbl = quote_ident(table),
        key = quote_ident(key),
    )
}

/// A column value rendered as a SQL literal for the `key > :last`
/// predicate. `None` for types that don't make sense as a monotonic tail
/// key (null, bool, arrays, network/bytes), which the caller reports.
pub fn key_literal(value: &Value) -> Option<String> {
    match value {
        Value::Int(_)
        | Value::UInt(_)
        | Value::Int128(_)
        | Value::UInt128(_)
        | Value::Decimal { .. } => Some(value.to_string()),
        Value::Float(v) => v.is_finite().then(|| value.to_string()),
        Value::Date(date) => Some(format!("'{date}'")),
        Value::DateTime(dt) => Some(format!("'{}'", dt.format("%Y-%m-%d %H:%M:%S%.f"))),
        Value::String(text) | Value::Enum(text) => Some(quote_string(text)),
        Value::Uuid(_) => Some(format!("toUUID('{value}')")),
        Value::Null
        | Value::Bool(_)
        | Value::Bytes(_)
        | Value::Ipv4(_)
        | Value::Ipv6(_)
        | Value::Array(_)
        | Value::Tuple(_)
        | Value::Map(_) => None,
    }
}

/// The last-seen key literal from a batch: the key column's value in the
/// final (newest) row. `None` when the batch is empty or the value isn't a
/// usable key.
pub fn last_key(rows: &[Vec<Value>], key_index: usize) -> Option<String> {
    rows.last()
        .and_then(|row| row.get(key_index))
        .and_then(key_literal)
}

/// Backtick-quote an identifier, escaping any embedded backtick.
fn quote_ident(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}

/// Single-quote a string literal, escaping backslashes and quotes.
fn quote_string(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn builds_seed_and_poll_sql() {
        assert_eq!(
            seed_sql("logs", "events", "at", 500),
            "SELECT * FROM (SELECT * FROM `logs`.`events` ORDER BY `at` DESC LIMIT 500) \
             ORDER BY `at` ASC"
        );
        assert_eq!(
            poll_sql("logs", "events", "at", "'2026-01-01 00:00:00'", 500),
            "SELECT * FROM `logs`.`events` WHERE `at` > '2026-01-01 00:00:00' \
             ORDER BY `at` ASC LIMIT 500"
        );
    }

    #[test]
    fn key_literals_quote_by_type() {
        assert_eq!(key_literal(&Value::UInt(42)).unwrap(), "42");
        assert_eq!(key_literal(&Value::Int(-7)).unwrap(), "-7");
        assert_eq!(
            key_literal(&Value::String("a'b".into())).unwrap(),
            "'a\\'b'"
        );
        assert_eq!(
            key_literal(&Value::DateTime(
                chrono::Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap()
            ))
            .unwrap(),
            "'2026-01-02 03:04:05'"
        );
        // Not usable as a monotonic key.
        assert!(key_literal(&Value::Null).is_none());
        assert!(key_literal(&Value::Bool(true)).is_none());
        assert!(key_literal(&Value::Array(vec![Value::UInt(1)])).is_none());
    }

    #[test]
    fn last_key_reads_the_final_row() {
        let rows = vec![
            vec![Value::UInt(1), Value::String("a".into())],
            vec![Value::UInt(9), Value::String("b".into())],
        ];
        assert_eq!(last_key(&rows, 0).unwrap(), "9");
        assert!(last_key(&[], 0).is_none());
    }

    #[test]
    fn identifiers_are_backtick_escaped() {
        assert_eq!(
            seed_sql("d", "weird`name", "k", 10),
            "SELECT * FROM (SELECT * FROM `d`.`weird``name` ORDER BY `k` DESC LIMIT 10) \
             ORDER BY `k` ASC"
        );
    }
}
