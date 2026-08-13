//! Live tail (Phase 10): the pure query-building and key-tracking core.
//!
//! A tail is a periodic `SELECT ... WHERE key > :last ORDER BY key LIMIT n`
//! over the HTTP interface (mechanism decided in `docs/PHASE-10.md`): each
//! poll prunes by the monotonic key instead of rescanning, so cost stays
//! flat however long the tail runs. This module is the deterministic part,
//! the SQL strings and the last-seen-key literal, unit-tested; the poll
//! loop and rendering live in `main.rs`. ClickHouse 26.6 instant tails use a
//! direct `STREAM CURSOR` query for simple single-table bodies; richer edited
//! queries keep using the polling path.

use zedb_core::Value;

/// Rows fetched per poll. A burst larger than this is carried across polls
/// by the advancing key, never lost.
pub const TAIL_BATCH: usize = 500;
/// Rows the initial (priming) load shows, always small: a tail should open
/// light and only grow if the user chose a larger retention cap.
pub const TAIL_SEED: usize = 20;
/// Poll cadence over HTTP.
pub const TAIL_INTERVAL_MS: u64 = 1_500;
/// Poll cadence in "instant updates" fast mode, where each poll rides the
/// persistent native (TCP) connection instead of a fresh HTTP request.
pub const TAIL_INTERVAL_FAST_MS: u64 = 400;

/// Private result-column aliases carried by a direct streaming query. The app
/// removes both columns before sending rows to the result grid.
pub const STREAM_BLOCK_COLUMN: &str = "__zedb_stream_block_number";
pub const STREAM_OFFSET_COLUMN: &str = "__zedb_stream_block_offset";

/// A resumable ClickHouse streaming position. The 26.6 MergeTree stream uses
/// one `all` cursor containing the last persisted insert block and row offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamCursor {
    pub block_number: u64,
    pub block_offset: u64,
}

impl StreamCursor {
    fn sql(self) -> String {
        format!(
            "{{'all': {{'block_number': {}, 'block_offset': {}}}}}",
            self.block_number, self.block_offset
        )
    }
}

/// A tail's editable definition. `body` is the user's query with its
/// top-level `ORDER BY` / `LIMIT` stripped, kept verbatim (columns, WHERE,
/// JOINs, GROUP BY, functions, ... all preserved). The tail wraps `body` as
/// a subquery to advance a cursor on `key`, so whatever the user writes is
/// what gets tailed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailQuery {
    pub body: String,
    /// The monotonic key (the first top-level ORDER BY column) the cursor
    /// advances on. Must be an output column of `body`.
    pub key: String,
    /// The top-level LIMIT, used as the per-poll row cap.
    pub limit: usize,
}

/// The initial body for tailing a whole table: `SELECT * FROM db.table`.
pub fn table_body(database: &str, table: &str) -> String {
    format!(
        "SELECT *\nFROM {db}.{tbl}",
        db = quote_ident(database),
        tbl = quote_ident(table),
    )
}

/// The runnable query shown in the tab editor: the user body plus the tail's
/// own `ORDER BY key ASC` and `LIMIT`, so it reads and runs as a whole.
pub fn base_sql(query: &TailQuery) -> String {
    format!(
        "{body}\nORDER BY {key} ASC\nLIMIT {limit}",
        body = query.body,
        key = quote_ident(&query.key),
        limit = query.limit,
    )
}

/// The newest `limit` rows of the body, returned oldest-first so the grid
/// reads top-to-bottom. Wrapping the body preserves its exact projection.
pub fn seed_sql(query: &TailQuery, limit: usize) -> String {
    format!(
        "SELECT * FROM (SELECT * FROM ({body}) ORDER BY {key} DESC LIMIT {limit}) \
         ORDER BY {key} ASC",
        body = query.body,
        key = quote_ident(&query.key),
    )
}

/// Rows of the body strictly newer than the last seen key, oldest-first,
/// capped. The cursor predicate is applied outside the user's body.
pub fn poll_sql(query: &TailQuery, last: &str, limit: usize) -> String {
    format!(
        "SELECT * FROM ({body}) WHERE {key} > {last} ORDER BY {key} ASC LIMIT {limit}",
        body = query.body,
        key = quote_ident(&query.key),
    )
}

/// A direct ClickHouse 26.6 streaming query for the deliberately narrow shape
/// we can rewrite without changing meaning: one simple table source and no
/// existing top-level filter or aggregation. Unsupported edited queries return
/// `None` and use native fast polling instead.
///
/// Before ClickHouse has emitted a cursor, `last` preserves the existing
/// monotonic-key boundary. Once a cursor exists it is the primary resume point;
/// the app still deduplicates by key when applying rows.
pub fn stream_sql(
    query: &TailQuery,
    cursor: Option<StreamCursor>,
    last: Option<&str>,
) -> Option<String> {
    let body = query.body.trim();
    let clauses = top_clauses(body);
    let select = clauses.get("SELECT")?;
    let from = clauses.get("FROM")?;
    if clauses.contains_key("WHERE") || clauses.contains_key("GROUP BY") {
        return None;
    }

    let projection = body[select.value_span.0..from.keyword_start].trim();
    let projection_upper = projection.to_ascii_uppercase();
    if projection.is_empty()
        || projection_upper.starts_with("DISTINCT ")
        || projection_upper.starts_with("WITH ")
    {
        return None;
    }
    let source = body[from.value_span.0..from.value_span.1].trim();
    if !is_simple_table_source(source) {
        return None;
    }

    let cursor_sql = cursor.map(StreamCursor::sql).unwrap_or_else(|| "{}".into());
    let boundary = match (cursor, last) {
        (None, Some(last)) => format!("\nWHERE {} > {last}", quote_ident(&query.key)),
        _ => String::new(),
    };
    Some(format!(
        "SELECT _block_number AS {block}, _block_offset AS {offset}, {projection}\n\
         FROM {source} STREAM CURSOR {cursor_sql}{boundary}\n\
         SETTINGS enable_streaming_queries = 1",
        block = STREAM_BLOCK_COLUMN,
        offset = STREAM_OFFSET_COLUMN,
    ))
}

/// Read the two private cursor columns at the front of a streamed row.
pub fn stream_cursor(row: &[Value]) -> Option<StreamCursor> {
    let number = row.first().and_then(unsigned_value)?;
    let offset = row.get(1).and_then(unsigned_value)?;
    Some(StreamCursor {
        block_number: number,
        block_offset: offset,
    })
}

/// Version gate for ClickHouse's real streaming-query implementation. Older
/// parsers can accept trailing `STREAM` as a table alias, so syntax probing is
/// not a capability check.
pub fn supports_streaming_version(version: &str) -> bool {
    let mut parts = version
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    matches!((parts.next(), parts.next()), (Some(major), Some(minor)) if (major, minor) >= (26, 6))
}

fn unsigned_value(value: &Value) -> Option<u64> {
    match value {
        Value::UInt(value) => Some(*value),
        Value::Int(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

/// A backtick-aware `db.table` check. Anything involving aliases, joins,
/// functions, or subqueries stays on the polling path until it is separately
/// characterized against ClickHouse streaming queries.
fn is_simple_table_source(source: &str) -> bool {
    if source.is_empty() {
        return false;
    }
    let mut quoted = false;
    let mut has_name = false;
    for ch in source.chars() {
        if ch == '`' {
            quoted = !quoted;
        } else if quoted || ch.is_ascii_alphanumeric() || ch == '_' {
            has_name = true;
        } else if ch != '.' {
            return false;
        }
    }
    has_name && !quoted && !source.starts_with('.') && !source.ends_with('.')
}

/// Parse an edited query into a [`TailQuery`]: the body is everything before
/// the top-level `ORDER BY` (whose first column is the key), with the
/// top-level `LIMIT` read off. Returns `None` when it isn't a
/// `SELECT ... FROM ... ORDER BY key [...]`, so the caller keeps the tail.
pub fn parse_tail_query(sql: &str, default_limit: usize) -> Option<TailQuery> {
    let stripped = strip_line_comments(sql);
    let clauses = top_clauses(&stripped);

    // A top-level ORDER BY gives the monotonic key and marks the body end.
    let order = clauses.get("ORDER BY")?;
    let order_text = stripped[order.value_span.0..order.value_span.1].trim();
    let key = order_text
        .split(',')
        .next()
        .map(|entry| entry.trim())
        .and_then(|entry| entry.split_whitespace().next())
        .map(|name| name.trim_matches('`').to_string())
        .filter(|name| !name.is_empty())?;

    // The body is everything before the ORDER BY, kept verbatim. It must at
    // least be a SELECT with a FROM.
    let body = stripped[..order.keyword_start].trim().to_string();
    let has_select = clauses
        .get("SELECT")
        .is_some_and(|clause| clause.keyword_start < order.keyword_start);
    let has_from = clauses
        .get("FROM")
        .is_some_and(|clause| clause.keyword_start < order.keyword_start);
    if body.is_empty() || !has_select || !has_from {
        return None;
    }

    let limit = clauses
        .get("LIMIT")
        .and_then(|clause| {
            stripped[clause.value_span.0..clause.value_span.1]
                .trim()
                .split(|c: char| c == ',' || c.is_whitespace())
                .find(|token| !token.is_empty())
                .and_then(|token| token.parse::<usize>().ok())
        })
        .unwrap_or(default_limit);

    Some(TailQuery { body, key, limit })
}

/// Drop `-- ...` line comments so keyword scanning ignores them.
fn strip_line_comments(sql: &str) -> String {
    sql.lines()
        .map(|line| match line.find("--") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Clause {
    /// Byte offset of the clause keyword itself.
    keyword_start: usize,
    /// Byte span of the clause's value (after the keyword, to the next
    /// top-level clause or end).
    value_span: (usize, usize),
}

/// Locate the value span of each recognised top-level clause keyword,
/// ignoring anything inside parens, quotes, or backticks.
fn top_clauses(sql: &str) -> std::collections::HashMap<&'static str, Clause> {
    const KEYWORDS: [&str; 7] = [
        "SELECT", "FROM", "WHERE", "ORDER BY", "GROUP BY", "LIMIT", "SETTINGS",
    ];
    let bytes = sql.as_bytes();
    // Boundary positions of top-level keywords: (keyword, start, value_start).
    let mut hits: Vec<(&'static str, usize, usize)> = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let upper = sql.to_ascii_uppercase();
    let uppb = upper.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' | b'`' => {
                quote = Some(c);
                i += 1;
                continue;
            }
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && (i == 0 || !uppb[i - 1].is_ascii_alphanumeric() && uppb[i - 1] != b'_') {
            for keyword in KEYWORDS {
                let kb = keyword.as_bytes();
                if uppb[i..].starts_with(kb) {
                    let after = i + kb.len();
                    let boundary = after >= uppb.len()
                        || (!uppb[after].is_ascii_alphanumeric() && uppb[after] != b'_');
                    if boundary {
                        hits.push((keyword, i, after));
                        i = after;
                        break;
                    }
                }
            }
        }
        i += 1;
    }

    let mut map = std::collections::HashMap::new();
    for (index, &(keyword, start, value_start)) in hits.iter().enumerate() {
        let value_end = hits
            .get(index + 1)
            .map(|&(_, next_start, _)| next_start)
            .unwrap_or(sql.len());
        map.entry(keyword).or_insert(Clause {
            keyword_start: start,
            value_span: (value_start, value_end),
        });
    }
    map
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

    fn query() -> TailQuery {
        TailQuery {
            body: "SELECT *\nFROM `logs`.`events`".into(),
            key: "at".into(),
            limit: 500,
        }
    }

    #[test]
    fn wraps_the_body_for_seed_and_poll() {
        assert_eq!(
            seed_sql(&query(), 20),
            "SELECT * FROM (SELECT * FROM (SELECT *\nFROM `logs`.`events`) \
             ORDER BY `at` DESC LIMIT 20) ORDER BY `at` ASC"
        );
        assert_eq!(
            poll_sql(&query(), "'2026-01-01 00:00:00'", 500),
            "SELECT * FROM (SELECT *\nFROM `logs`.`events`) \
             WHERE `at` > '2026-01-01 00:00:00' ORDER BY `at` ASC LIMIT 500"
        );
    }

    #[test]
    fn builds_stream_query_with_key_fallback_then_cursor_resume() {
        assert_eq!(
            stream_sql(&query(), None, Some("'2026-01-01 00:00:00'")),
            Some(
                "SELECT _block_number AS __zedb_stream_block_number, _block_offset AS \
                 __zedb_stream_block_offset, *\nFROM `logs`.`events` STREAM CURSOR {}\n\
                 WHERE `at` > '2026-01-01 00:00:00'\nSETTINGS \
                 enable_streaming_queries = 1"
                    .into()
            )
        );
        assert_eq!(
            stream_sql(
                &query(),
                Some(StreamCursor {
                    block_number: 7,
                    block_offset: 19,
                }),
                Some("'ignored'"),
            ),
            Some(
                "SELECT _block_number AS __zedb_stream_block_number, _block_offset AS \
                 __zedb_stream_block_offset, *\nFROM `logs`.`events` STREAM CURSOR \
                 {'all': {'block_number': 7, 'block_offset': 19}}\nSETTINGS \
                 enable_streaming_queries = 1"
                    .into()
            )
        );
    }

    #[test]
    fn stream_query_rejects_shapes_not_characterized_for_streaming() {
        for body in [
            "SELECT * FROM db.t WHERE kind = 'x'",
            "SELECT kind, count() FROM db.t GROUP BY kind",
            "SELECT * FROM db.t AS t",
            "SELECT * FROM (SELECT * FROM db.t)",
            "SELECT * FROM db.a JOIN db.b USING id",
        ] {
            let query = TailQuery {
                body: body.into(),
                key: "id".into(),
                limit: 500,
            };
            assert_eq!(stream_sql(&query, None, Some("1")), None, "{body}");
        }
    }

    #[test]
    fn reads_stream_cursor_columns() {
        assert_eq!(
            stream_cursor(&[Value::UInt(3), Value::UInt(9), Value::String("x".into())]),
            Some(StreamCursor {
                block_number: 3,
                block_offset: 9,
            })
        );
        assert_eq!(stream_cursor(&[Value::String("bad".into())]), None);
    }

    #[test]
    fn version_gate_does_not_mistake_the_26_3_alias_for_streaming() {
        assert!(!supports_streaming_version("25.8.25.37"));
        assert!(!supports_streaming_version("26.3.12.3"));
        assert!(supports_streaming_version("26.6.2.160"));
        assert!(supports_streaming_version("27.1.1.1"));
        assert!(!supports_streaming_version("not-a-version"));
    }

    #[test]
    fn base_sql_appends_order_and_limit() {
        assert_eq!(
            base_sql(&query()),
            "SELECT *\nFROM `logs`.`events`\nORDER BY `at` ASC\nLIMIT 500"
        );
    }

    #[test]
    fn parses_whatever_the_user_wrote_as_the_body() {
        // Columns, WHERE, and GROUP BY are all kept verbatim in the body;
        // only the top-level ORDER BY / LIMIT are peeled off.
        let edited = "-- live tail\nSELECT id, at, count() c\nFROM db.t\nWHERE a > 1 AND b = 'x'\nGROUP BY id, at\nORDER BY at DESC\nLIMIT 100";
        let parsed = parse_tail_query(edited, 500).unwrap();
        assert_eq!(
            parsed.body,
            "SELECT id, at, count() c\nFROM db.t\nWHERE a > 1 AND b = 'x'\nGROUP BY id, at"
        );
        assert_eq!(parsed.key, "at");
        assert_eq!(parsed.limit, 100);
        // Round-trip: base_sql of the parse re-appends ORDER BY / LIMIT.
        assert_eq!(parse_tail_query(&base_sql(&parsed), 500).unwrap(), parsed);
        // Missing FROM or ORDER BY -> not a tailable query.
        assert!(parse_tail_query("SELECT 1 ORDER BY 1", 500).is_none());
        assert!(parse_tail_query("SELECT * FROM db.t", 500).is_none());
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
    fn table_body_backtick_escapes_identifiers() {
        assert_eq!(
            table_body("d", "weird`name"),
            "SELECT *\nFROM `d`.`weird``name`"
        );
        let q = TailQuery {
            body: table_body("d", "weird`name"),
            key: "k".into(),
            limit: 10,
        };
        // The key is backtick-quoted in the wrapper's ORDER BY.
        assert!(poll_sql(&q, "1", 10).contains("ORDER BY `k` ASC"));
    }
}
