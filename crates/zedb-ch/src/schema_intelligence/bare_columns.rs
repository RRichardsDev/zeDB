//! Unqualified column references: `WHERE name LIKE 'x'` as opposed to
//! `WHERE c.name LIKE 'x'`.
//!
//! The qualified form names its table, so a miss is certain. A bare name
//! has to be resolved against every table the statement reads, and it is
//! only a column at all when it sits where an operand belongs. This pass
//! claims nothing unless it can be sure: a SELECT whose sources are all
//! cached tables with column metadata, no CTEs, subqueries, table
//! functions, or comma joins. Aliases the statement defines (explicit
//! `AS x`, implicit `count() cnt`), lambda parameters, and the
//! documented virtual columns are never reported.

use std::collections::HashSet;
use std::ops::Range;

use super::bindings::{resolve_bindings, unique_object};
use super::tokens::Token;
use crate::schema_cache::{CachedObject, SchemaSnapshot};

pub(super) struct BareReference<'a> {
    pub(super) range: Range<usize>,
    pub(super) text: &'a str,
    /// Some table in scope has this column.
    pub(super) known: bool,
    /// The tables the name was resolved against, `database.object`.
    pub(super) scope: Vec<String>,
}

/// Every bare identifier in operand position across the statements of
/// `sql`, resolved against the tables its statement binds. Statements
/// this pass cannot vouch for contribute nothing.
pub(super) fn bare_column_references<'a>(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    tokens: &[Token<'a>],
) -> Vec<BareReference<'a>> {
    let mut references = Vec::new();
    for statement in tokens.split(|token| token.text == ";") {
        statement_references(snapshot, default_database, sql, statement, &mut references);
    }
    references
}

fn statement_references<'a>(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    tokens: &[Token<'a>],
    references: &mut Vec<BareReference<'a>>,
) {
    let Some(first) = tokens.iter().find(|token| token.identifier) else {
        return;
    };
    if !matches!(first.text.to_ascii_uppercase().as_str(), "SELECT" | "WITH") {
        return;
    }
    let (bindings, _, _) = resolve_bindings(snapshot, default_database, tokens);
    if !bindings.ctes.is_empty() {
        return;
    }
    let Some(scope) = source_tables(snapshot, default_database, tokens) else {
        return;
    };
    let scope_names: Vec<String> = scope
        .iter()
        .map(|(database, object)| format!("{database}.{}", object.name))
        .collect();

    let mut skip: HashSet<String> = bindings.aliases.keys().cloned().collect();
    skip.extend(declared_names(sql, tokens));

    // `quiet` covers the regions where a bare identifier is a table,
    // a setting, or a format rather than an expression.
    let mut quiet = false;
    // Past the end of a parameterized type (`AS Nullable(String)`,
    // `::DateTime64(3)`), whose arguments are types, not columns.
    let mut skip_until = 0;
    for (index, token) in tokens.iter().enumerate() {
        if index < skip_until || !token.identifier {
            continue;
        }
        let previous = index.checked_sub(1).map(|i| &tokens[i]);
        let next = tokens.get(index + 1);
        if next.is_some_and(|next| next.text == "(") && is_type_position(sql, tokens, index) {
            skip_until = closing_paren(tokens, index + 1);
            continue;
        }
        if is_reserved(token.text) {
            let upper = token.text.to_ascii_uppercase();
            let array_join = upper == "JOIN"
                && previous.is_some_and(|token| token.text.eq_ignore_ascii_case("ARRAY"));
            quiet = matches!(
                upper.as_str(),
                "FROM" | "JOIN" | "INTO" | "TABLE" | "UPDATE" | "SETTINGS" | "FORMAT"
            ) && !array_join;
            continue;
        }
        if quiet || skip.contains(&token.text.to_ascii_lowercase()) {
            continue;
        }
        if is_virtual_column(token.text) {
            continue;
        }
        // Functions, qualified names, and `::Type` casts.
        if next.is_some_and(|next| next.text == "(" || next.text == ".") {
            continue;
        }
        if previous.is_some_and(|previous| previous.text == ".") {
            continue;
        }
        let Some(before) = last_non_space_char(sql, token.range.start) else {
            continue;
        };
        let operand = if before.is_ascii_alphanumeric() || before == '_' || before == '`' {
            previous.is_some_and(|previous| previous.identifier && leads_expression(previous.text))
        } else {
            matches!(
                before,
                '(' | ','
                    | '='
                    | '<'
                    | '>'
                    | '+'
                    | '-'
                    | '*'
                    | '/'
                    | '%'
                    | '!'
                    | '['
                    | '|'
                    | '&'
                    | '^'
                    | '?'
            )
        };
        if !operand {
            continue;
        }
        let known = scope.iter().any(|(_, object)| {
            object.columns.as_ref().is_some_and(|columns| {
                columns
                    .values()
                    .any(|c| c.name.eq_ignore_ascii_case(token.text))
            })
        });
        references.push(BareReference {
            range: token.range.clone(),
            text: token.text,
            known,
            scope: scope_names.clone(),
        });
    }
}

/// The tables the statement reads, provided every one of them is a
/// cached table with complete column metadata. `None` as soon as any
/// source is something else (subquery, table function, comma join,
/// built-in database, uncached object): the scope would be incomplete
/// and every miss a guess.
fn source_tables<'a>(
    snapshot: &'a SchemaSnapshot,
    default_database: Option<&str>,
    tokens: &[Token<'_>],
) -> Option<Vec<(&'a str, &'a CachedObject)>> {
    let mut sources: Vec<(&str, &CachedObject)> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if !matches!(token.text.to_ascii_uppercase().as_str(), "FROM" | "JOIN") {
            continue;
        }
        if index > 0 && tokens[index - 1].text.eq_ignore_ascii_case("ARRAY") {
            continue;
        }
        let first = tokens.get(index + 1).filter(|token| token.identifier)?;
        let qualified = tokens.get(index + 2).is_some_and(|token| token.text == ".")
            && tokens.get(index + 3).is_some_and(|token| token.identifier);
        let (database, object, end) = if qualified {
            (Some(first.text), tokens[index + 3].text, index + 4)
        } else {
            (default_database, first.text, index + 2)
        };
        // A table function, or a comma join hiding a second source.
        if tokens.get(end).is_some_and(|token| token.text == "(") {
            return None;
        }
        let mut after = end;
        if tokens
            .get(after)
            .is_some_and(|token| token.text.eq_ignore_ascii_case("AS"))
        {
            after += 1;
        }
        if tokens
            .get(after)
            .is_some_and(|token| token.identifier && !is_reserved(token.text))
        {
            after += 1;
        }
        if tokens.get(after).is_some_and(|token| token.text == ",") {
            return None;
        }
        let (database, cached) = match database {
            Some(database) => {
                if matches!(
                    database.to_ascii_lowercase().as_str(),
                    "system" | "information_schema"
                ) {
                    return None;
                }
                // Database names are case-sensitive in ClickHouse, and
                // the binding pass already squiggles a wrong case.
                let cached_database = snapshot.database(database)?;
                let cached = cached_database
                    .objects
                    .values()
                    .find(|candidate| candidate.name.eq_ignore_ascii_case(object))?;
                (cached_database.name.as_str(), cached)
            }
            None => unique_object(snapshot, object)?,
        };
        cached.columns.as_ref()?;
        sources.push((database, cached));
    }
    (!sources.is_empty()).then_some(sources)
}

/// Names the statement itself introduces, lowercased: explicit `AS x`
/// aliases, implicit `expr x` aliases, and lambda parameters. Also
/// catches type names after `AS` in a CAST, which is harmless.
fn declared_names(sql: &str, tokens: &[Token<'_>]) -> HashSet<String> {
    let mut names = HashSet::new();
    for (index, token) in tokens.iter().enumerate() {
        if !token.identifier || is_reserved(token.text) {
            continue;
        }
        let previous = index.checked_sub(1).map(|i| &tokens[i]);
        let next = tokens.get(index + 1);
        if next.is_some_and(|next| next.text == "(" || next.text == ".") {
            continue;
        }
        if previous.is_some_and(|previous| previous.text.eq_ignore_ascii_case("AS")) {
            names.insert(token.text.to_ascii_lowercase());
            continue;
        }
        // `x -> ...` and `(x, y) -> ...`.
        if next.is_some_and(|next| next.text == "-")
            && tokens.get(index + 2).is_some_and(|token| token.text == ">")
        {
            names.insert(token.text.to_ascii_lowercase());
            continue;
        }
        if tokens.get(index + 1).is_some_and(|t| t.text == ")")
            && tokens.get(index + 2).is_some_and(|t| t.text == "-")
            && tokens.get(index + 3).is_some_and(|t| t.text == ">")
        {
            let mut back = index;
            while back > 0 && tokens[back - 1].text != "(" {
                back -= 1;
                if tokens[back].identifier {
                    names.insert(tokens[back].text.to_ascii_lowercase());
                }
            }
            names.insert(token.text.to_ascii_lowercase());
            continue;
        }
        // Implicit alias: an identifier straight after a closed
        // expression (`count() cnt`, `arr[1] first_tag`, `'x' label`,
        // `e.name contact`).
        let Some(before) = last_non_space_char(sql, token.range.start) else {
            continue;
        };
        let implicit = match before {
            ')' | ']' | '\'' | '"' | '`' => true,
            _ if before.is_ascii_alphanumeric() || before == '_' => {
                previous.is_some_and(|previous| previous.identifier && !is_reserved(previous.text))
            }
            _ => false,
        };
        if implicit {
            names.insert(token.text.to_ascii_lowercase());
        }
    }
    names
}

/// The identifier at `index` names a type: it follows `AS` (a CAST) or
/// `::`.
fn is_type_position(sql: &str, tokens: &[Token<'_>], index: usize) -> bool {
    let previous = index.checked_sub(1).map(|i| &tokens[i]);
    previous.is_some_and(|previous| previous.text.eq_ignore_ascii_case("AS"))
        || last_non_space_char(sql, tokens[index].range.start) == Some(':')
}

/// Index just past the parenthesis matching the `(` at `open`.
fn closing_paren(tokens: &[Token<'_>], open: usize) -> usize {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.text {
            "(" => depth += 1,
            ")" => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }
    tokens.len()
}

fn last_non_space_char(sql: &str, end: usize) -> Option<char> {
    sql[..end].chars().rev().find(|c| !c.is_whitespace())
}

/// Keywords after which the next identifier is an operand.
fn leads_expression(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "SELECT"
            | "DISTINCT"
            | "WHERE"
            | "PREWHERE"
            | "HAVING"
            | "QUALIFY"
            | "AND"
            | "OR"
            | "NOT"
            | "BY"
            | "ON"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "CASE"
            | "LIKE"
            | "ILIKE"
            | "BETWEEN"
            | "USING"
            | "ALL"
    )
}

/// Words that are never a column, whichever position they sit in.
fn is_reserved(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "SELECT"
            | "FROM"
            | "WHERE"
            | "PREWHERE"
            | "HAVING"
            | "QUALIFY"
            | "GROUP"
            | "ORDER"
            | "BY"
            | "LIMIT"
            | "OFFSET"
            | "FETCH"
            | "JOIN"
            | "LEFT"
            | "RIGHT"
            | "INNER"
            | "FULL"
            | "CROSS"
            | "OUTER"
            | "ANY"
            | "ALL"
            | "ASOF"
            | "SEMI"
            | "ANTI"
            | "PASTE"
            | "GLOBAL"
            | "ARRAY"
            | "ON"
            | "USING"
            | "AS"
            | "AND"
            | "OR"
            | "NOT"
            | "IN"
            | "LIKE"
            | "ILIKE"
            | "BETWEEN"
            | "ESCAPE"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
            | "DISTINCT"
            | "UNION"
            | "EXCEPT"
            | "INTERSECT"
            | "WITH"
            | "SAMPLE"
            | "FINAL"
            | "FORMAT"
            | "SETTINGS"
            | "INTERVAL"
            | "ASC"
            | "DESC"
            | "NULLS"
            | "FIRST"
            | "LAST"
            | "NULL"
            | "TRUE"
            | "FALSE"
            | "NAN"
            | "INF"
            | "IS"
            | "EXISTS"
            | "TOTALS"
            | "ROLLUP"
            | "CUBE"
            | "TIES"
            | "FILL"
            | "STEP"
            | "TO"
            | "INTERPOLATE"
            | "OVER"
            | "PARTITION"
            | "WINDOW"
            | "ROWS"
            | "RANGE"
            | "GROUPS"
            | "UNBOUNDED"
            | "PRECEDING"
            | "FOLLOWING"
            | "CURRENT"
            | "ROW"
            | "TOP"
            | "CAST"
            | "EXTRACT"
            | "VALUES"
            | "INTO"
            | "TABLE"
            | "UPDATE"
            | "DELETE"
            | "INSERT"
            | "CREATE"
            | "ALTER"
            | "DROP"
            | "APPLY"
            | "REPLACE"
            | "COLUMNS"
            | "GROUPING"
            | "SETS"
            | "MOD"
            | "DIV"
            | "DATE"
            | "TIMESTAMP"
            | "NANOSECOND"
            | "MICROSECOND"
            | "MILLISECOND"
            | "SECOND"
            | "MINUTE"
            | "HOUR"
            | "DAY"
            | "WEEK"
            | "MONTH"
            | "QUARTER"
            | "YEAR"
            | "EXPLAIN"
    )
}

/// MergeTree and Distributed virtual columns: real, and absent from
/// system.columns.
fn is_virtual_column(word: &str) -> bool {
    matches!(
        word,
        "_part"
            | "_part_index"
            | "_part_uuid"
            | "_part_offset"
            | "_part_data_version"
            | "_partition_id"
            | "_partition_value"
            | "_sample_factor"
            | "_row_exists"
            | "_block_number"
            | "_block_offset"
            | "_table"
            | "_database"
            | "_shard_num"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::{analyze_sql, recognized_identifiers, RecognizedKind};
    use crate::schema_cache::CachedColumn;
    use crate::schema_intelligence::fixtures::snapshot;

    fn columns() -> HashMap<String, CachedColumn> {
        ["event_id", "name", "ts", "tags"]
            .into_iter()
            .map(|name| {
                (
                    name.to_string(),
                    CachedColumn {
                        name: name.into(),
                        type_name: "String".into(),
                        codec_expression: String::new(),
                        comment: String::new(),
                    },
                )
            })
            .collect()
    }

    fn unknown(sql: &str) -> Vec<String> {
        let snapshot = snapshot(Some(columns()));
        analyze_sql(&snapshot, Some("analytics"), sql)
            .into_iter()
            .filter(|issue| issue.message.starts_with("Unknown column"))
            .map(|issue| sql[issue.range].to_string())
            .collect()
    }

    #[test]
    fn flags_a_bare_unknown_column_against_the_table_in_scope() {
        // The report: `name` does not exist on the table, and got no
        // squiggle because only `alias.column` was ever checked.
        let snapshot = snapshot(Some(columns()));
        let sql = "select * from analytics.events where nmae like 'test';";
        let issues = analyze_sql(&snapshot, Some("analytics"), sql);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(&sql[issues[0].range.clone()], "nmae");
        assert_eq!(
            issues[0].message,
            "Unknown column `nmae` on analytics.events"
        );
    }

    #[test]
    fn known_bare_columns_are_recognized_not_flagged() {
        let sql = "SELECT name, ts FROM events WHERE name LIKE 'x' ORDER BY ts DESC NULLS LAST";
        assert!(unknown(sql).is_empty());
        let snapshot = snapshot(Some(columns()));
        let recognized: Vec<&str> = recognized_identifiers(&snapshot, Some("analytics"), sql)
            .into_iter()
            .filter(|identifier| identifier.kind == RecognizedKind::Column)
            .map(|identifier| &sql[identifier.range])
            .collect();
        assert_eq!(recognized, vec!["name", "ts", "name", "ts"]);
    }

    #[test]
    fn aliases_functions_keywords_and_params_are_not_columns() {
        for sql in [
            "SELECT count() AS total, name FROM events GROUP BY name HAVING total > 1 ORDER BY total",
            "SELECT count() total FROM events ORDER BY total",
            "SELECT 'x' label, name AS contact FROM events ORDER BY label, contact",
            "SELECT toStartOfDay(ts) AS day FROM events GROUP BY day WITH TOTALS",
            "SELECT * FROM events WHERE ts > now() - INTERVAL 1 DAY AND name IS NOT NULL",
            "SELECT CAST(ts AS Nullable(String)), ts::DateTime64(3) FROM events",
            "SELECT arrayMap(x -> x + 1, tags), arrayFilter((k, v) -> k = v, tags) FROM events",
            "SELECT name FROM events WHERE name = {who:String} SETTINGS max_threads = 4 FORMAT JSONEachRow",
            "SELECT CASE WHEN name = 'a' THEN 1 ELSE 0 END, _part, _partition_id FROM events FINAL",
            "SELECT sum(event_id) OVER (PARTITION BY name ORDER BY ts ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM events",
            "SELECT * EXCEPT (name) FROM events e WHERE e.ts > 1 LIMIT 10 BY name",
            "SELECT name FROM events WHERE name IN other_table",
            "SELECT tag FROM events ARRAY JOIN tags AS tag WHERE tag = 'x'",
        ] {
            assert!(unknown(sql).is_empty(), "{sql}: {:?}", unknown(sql));
        }
    }

    #[test]
    fn array_join_is_an_expression_not_a_table() {
        let snapshot = snapshot(Some(columns()));
        let issues = analyze_sql(
            &snapshot,
            Some("analytics"),
            "SELECT tag FROM events LEFT ARRAY JOIN tags AS tag",
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn stays_quiet_when_the_scope_is_not_fully_known() {
        // CTEs, subqueries, table functions, comma joins, system tables,
        // uncached columns, non-SELECT statements: any miss would be a
        // guess, so nothing is claimed.
        for sql in [
            "WITH recent AS (SELECT * FROM events) SELECT anything FROM recent",
            "SELECT anything FROM (SELECT name FROM events)",
            "SELECT number FROM numbers(10)",
            "SELECT anything FROM events, events",
            "SELECT anything FROM system.tables",
            "SELECT anything FROM events JOIN missing m ON m.id = event_id",
            "INSERT INTO events SELECT anything FROM events",
        ] {
            assert!(unknown(sql).is_empty(), "{sql}: {:?}", unknown(sql));
        }
        let snapshot = snapshot(None);
        assert!(
            analyze_sql(&snapshot, Some("analytics"), "SELECT anything FROM events").is_empty()
        );
    }

    #[test]
    fn resolves_across_every_joined_table() {
        let sql =
            "SELECT name, missing FROM events e JOIN analytics.events x ON e.event_id = x.event_id";
        assert_eq!(unknown(sql), vec!["missing"]);
    }

    #[test]
    fn each_statement_has_its_own_scope() {
        let sql = "SELECT name FROM events; SELECT 1; SELECT nope FROM events";
        assert_eq!(unknown(sql), vec!["nope"]);
    }
}
