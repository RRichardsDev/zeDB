//! CREATE TABLE clause intelligence: the statement's own declared
//! columns feed ORDER BY / PARTITION BY / PRIMARY KEY / SAMPLE BY
//! completion (the table doesn't exist yet, so no snapshot knows it),
//! and PARTITION BY over a raw high-cardinality column gets a nudge
//! at type time instead of after the parts explosion.

use std::ops::Range;

use super::tokens::{statement_bounds, tokenize, Token};

/// A column declared in the CREATE's own column list.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct DeclaredColumn {
    pub(super) name: String,
    /// The head of the declared type (`DateTime64` of
    /// `DateTime64(3, 'UTC')`), enough for cardinality reasoning.
    pub(super) type_head: String,
}

/// Words that start a non-column entry in a column list.
const NON_COLUMN_HEADS: [&str; 5] = ["INDEX", "CONSTRAINT", "PRIMARY", "PROJECTION", "COLUMNS"];

fn is_create_table(tokens: &[Token<'_>]) -> bool {
    let mut words = tokens.iter().filter(|token| token.identifier);
    words
        .next()
        .is_some_and(|token| token.text.eq_ignore_ascii_case("CREATE"))
        && words.take(3).any(|token| {
            token.text.eq_ignore_ascii_case("TABLE")
                || token.text.eq_ignore_ascii_case("VIEW")
                || token.text.eq_ignore_ascii_case("DICTIONARY")
        })
}

/// The columns a CREATE TABLE statement declares, parsed from the
/// depth-1 segments of its first parenthesized list: each segment's
/// first identifier is the name, the second the type head.
pub(super) fn declared_columns(statement: &str) -> Vec<DeclaredColumn> {
    let tokens = tokenize(statement);
    if !is_create_table(&tokens) {
        return Vec::new();
    }
    let mut columns = Vec::new();
    let mut depth = 0usize;
    let mut segment: Vec<&Token<'_>> = Vec::new();
    let mut in_list = false;
    for token in &tokens {
        match token.text {
            "(" => {
                depth += 1;
                if depth == 1 {
                    in_list = true;
                    segment.clear();
                    continue;
                }
            }
            ")" => {
                depth = depth.saturating_sub(1);
                if depth == 0 && in_list {
                    push_column(&segment, &mut columns);
                    break;
                }
            }
            "," if depth == 1 => {
                push_column(&segment, &mut columns);
                segment.clear();
                continue;
            }
            _ => {}
        }
        if in_list && depth >= 1 {
            segment.push(token);
        }
    }
    columns
}

fn push_column(segment: &[&Token<'_>], columns: &mut Vec<DeclaredColumn>) {
    let mut words = segment.iter().filter(|token| token.identifier);
    let Some(name) = words.next() else {
        return;
    };
    if NON_COLUMN_HEADS
        .iter()
        .any(|head| name.text.eq_ignore_ascii_case(head))
    {
        return;
    }
    let type_head = words
        .next()
        .map(|token| token.text.to_string())
        .unwrap_or_default();
    columns.push(DeclaredColumn {
        name: name.text.to_string(),
        type_head,
    });
}

/// Clauses after the column list that take column expressions.
const ENGINE_CLAUSES: [(&str, &str); 4] = [
    ("ORDER", "BY"),
    ("PARTITION", "BY"),
    ("PRIMARY", "KEY"),
    ("SAMPLE", "BY"),
];

/// Whether the cursor sits in an engine clause (ORDER BY, PARTITION
/// BY, PRIMARY KEY, SAMPLE BY) of a CREATE statement, where the
/// statement's own declared columns are the vocabulary.
pub(super) fn in_engine_clause(sql: &str, cursor: usize) -> bool {
    let bounds = statement_bounds(sql, cursor);
    let Some(statement) = sql.get(bounds.clone()) else {
        return false;
    };
    let tokens = tokenize(statement);
    if !is_create_table(&tokens) {
        return false;
    }
    let relative = cursor - bounds.start;
    // The last clause opener before the cursor, at paren depth 0
    // (inside the column list PRIMARY KEY is a column-list entry, not
    // the engine clause).
    let mut depth = 0i32;
    let mut in_clause = false;
    for (index, token) in tokens.iter().enumerate() {
        if token.range.start >= relative {
            break;
        }
        match token.text {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth > 0 || !token.identifier {
            continue;
        }
        let opens = ENGINE_CLAUSES.iter().any(|(first, second)| {
            token.text.eq_ignore_ascii_case(second)
                && tokens[..index]
                    .iter()
                    .rev()
                    .find(|previous| previous.identifier)
                    .is_some_and(|previous| previous.text.eq_ignore_ascii_case(first))
        });
        if opens {
            in_clause = true;
        } else if in_clause
            && ![",", ".", "(", ")"].contains(&token.text)
            && !ENGINE_CLAUSES
                .iter()
                .any(|(first, _)| token.text.eq_ignore_ascii_case(first))
        {
            // Another word (ENGINE, SETTINGS, AS, TTL, a column name is
            // fine mid-expression; only clause keywords end it).
            if matches!(
                token.text.to_ascii_uppercase().as_str(),
                "ENGINE" | "SETTINGS" | "AS" | "TTL" | "COMMENT"
            ) {
                in_clause = false;
            }
        }
    }
    in_clause
}

/// Type heads whose raw values make hazardous partition keys: one
/// partition per distinct value.
const HIGH_CARDINALITY_HEADS: [&str; 4] = ["DateTime", "DateTime64", "String", "UUID"];

/// PARTITION BY over a bare declared column of a high-cardinality
/// type, per statement across `sql`: (range of the column use, column
/// name, type head). Wrapped expressions (toYYYYMM(ts)) never flag.
pub(super) fn partition_hazards(sql: &str) -> Vec<(Range<usize>, String, String)> {
    let mut hazards = Vec::new();
    let mut position = 0usize;
    while position <= sql.len() {
        let bounds = statement_bounds(sql, position);
        let Some(statement) = sql.get(bounds.clone()) else {
            break;
        };
        let columns = declared_columns(statement);
        if !columns.is_empty() {
            let tokens = tokenize(statement);
            let mut depth = 0i32;
            for (index, token) in tokens.iter().enumerate() {
                match token.text {
                    "(" => depth += 1,
                    ")" => depth -= 1,
                    _ => {}
                }
                if depth != 0 || !token.text.eq_ignore_ascii_case("BY") {
                    continue;
                }
                let partition = tokens[..index]
                    .iter()
                    .rev()
                    .find(|previous| previous.identifier)
                    .is_some_and(|previous| previous.text.eq_ignore_ascii_case("PARTITION"));
                if !partition {
                    continue;
                }
                // The clause expression: hazard only when it is exactly
                // one bare identifier (a wrap like toYYYYMM(x) means the
                // author already thought about granularity).
                let Some(expression) = tokens.get(index + 1) else {
                    continue;
                };
                if !expression.identifier
                    || tokens
                        .get(index + 2)
                        .is_some_and(|next| next.text == "(" || next.text == ".")
                {
                    continue;
                }
                if let Some(column) = columns
                    .iter()
                    .find(|column| column.name.eq_ignore_ascii_case(expression.text))
                {
                    if HIGH_CARDINALITY_HEADS
                        .iter()
                        .any(|head| column.type_head.eq_ignore_ascii_case(head))
                    {
                        hazards.push((
                            bounds.start + expression.range.start
                                ..bounds.start + expression.range.end,
                            column.name.clone(),
                            column.type_head.clone(),
                        ));
                    }
                }
            }
        }
        if bounds.end >= sql.len() {
            break;
        }
        position = bounds.end + 1;
    }
    hazards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_columns_parse_names_and_type_heads() {
        let sql = "CREATE TABLE db.events (\n\
                   id UInt64,\n\
                   at DateTime64(3, 'UTC') CODEC(Delta),\n\
                   payload String,\n\
                   INDEX idx_p payload TYPE bloom_filter GRANULARITY 4\n\
                   ) ENGINE = MergeTree ORDER BY id";
        let columns = declared_columns(sql);
        let names: Vec<(&str, &str)> = columns
            .iter()
            .map(|column| (column.name.as_str(), column.type_head.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("id", "UInt64"),
                ("at", "DateTime64"),
                ("payload", "String")
            ]
        );
        assert!(declared_columns("SELECT * FROM t (weird)").is_empty());
    }

    #[test]
    fn engine_clause_detection_tracks_the_cursor() {
        let sql = "CREATE TABLE t (id UInt64, at DateTime) ENGINE = MergeTree ORDER BY ";
        assert!(in_engine_clause(sql, sql.len()));
        let sql = "CREATE TABLE t (id UInt64) ENGINE = MergeTree PARTITION BY to";
        assert!(in_engine_clause(sql, sql.len()));
        // Inside the column list, PRIMARY KEY is a list entry.
        let sql = "CREATE TABLE t (id UInt64, PRIMARY KEY (id";
        assert!(!in_engine_clause(sql, sql.len()));
        // SETTINGS ends the clause vocabulary.
        let sql = "CREATE TABLE t (id UInt64) ENGINE = MergeTree ORDER BY id SETTINGS in";
        assert!(!in_engine_clause(sql, sql.len()));
        // Not a CREATE at all.
        assert!(!in_engine_clause("SELECT 1 ORDER BY ", 19));
    }

    #[test]
    fn engine_clauses_complete_the_statements_own_columns() {
        use crate::schema_intelligence::{completions, fixtures, SuggestionKind};
        let snapshot = fixtures::snapshot(None);
        let sql = "CREATE TABLE t (event_time DateTime, id UInt64) \
                   ENGINE = MergeTree ORDER BY ev";
        let items = completions(&snapshot, None, sql, sql.len());
        let declared = items
            .iter()
            .find(|item| item.label == "event_time")
            .expect("declared column offered");
        assert_eq!(declared.kind, SuggestionKind::Column);
        assert_eq!(declared.detail, "DateTime");
    }

    #[test]
    fn raw_high_cardinality_partition_keys_flag_wrapped_ones_do_not() {
        let sql = "CREATE TABLE t (at DateTime, id UInt64) \
                   ENGINE = MergeTree PARTITION BY at ORDER BY id";
        let hazards = partition_hazards(sql);
        assert_eq!(hazards.len(), 1, "{hazards:?}");
        assert_eq!(hazards[0].1, "at");
        assert_eq!(hazards[0].2, "DateTime");
        assert_eq!(&sql[hazards[0].0.clone()], "at");

        for sql in [
            // Wrapped: the author chose a granularity.
            "CREATE TABLE t (at DateTime) ENGINE = MergeTree PARTITION BY toYYYYMM(at) ORDER BY tuple()",
            // A sane key type.
            "CREATE TABLE t (kind LowCardinality(String), at DateTime) \
             ENGINE = MergeTree PARTITION BY kind ORDER BY at",
            // Not a declared column (can't reason).
            "CREATE TABLE t (id UInt64) ENGINE = MergeTree PARTITION BY other ORDER BY id",
        ] {
            assert!(partition_hazards(sql).is_empty(), "{sql}");
        }
    }
}
