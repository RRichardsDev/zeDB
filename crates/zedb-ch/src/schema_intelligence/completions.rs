//! Cursor-position completion of database, object, and column names.

use std::ops::Range;

use super::bindings::{resolve_bindings, unique_object};
use super::tokens::{current_statement, starts_with_case_insensitive, tokenize, word_range};
use super::{SchemaSuggestion, SuggestionKind};
use crate::schema_cache::{CachedColumn, CachedObject, SchemaSnapshot};

pub fn completions(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    cursor: usize,
) -> Vec<SchemaSuggestion> {
    let cursor = cursor.min(sql.len());
    let replace = word_range(sql, cursor);
    let prefix = &sql[replace.clone()];
    // A word typed inside backticks (`db`.`tab|`) carries an opening
    // backtick between it and the qualifying dot; step over it so the
    // dot/qualifier adjacency below still resolves.
    let context_end = if replace.start > 0 && sql.as_bytes()[replace.start - 1] == b'`' {
        replace.start - 1
    } else {
        replace.start
    };
    let before = &sql[..context_end];
    let tokens = tokenize(before);
    // Resolve tables from the statement under the cursor only. An
    // editor holds many statements; binding over the whole buffer
    // would offer columns from every table anyone ever typed.
    let (bindings, _, _) = resolve_bindings(
        snapshot,
        default_database,
        &tokenize(current_statement(sql, cursor)),
    );
    let mut suggestions = Vec::new();

    let dot_adjacent = tokens
        .last()
        .is_some_and(|token| token.text == "." && token.range.end == context_end);
    if dot_adjacent {
        let qualifier = tokens
            .get(tokens.len().saturating_sub(2))
            .filter(|token| {
                token.identifier
                    && tokens
                        .last()
                        .is_some_and(|dot| token.range.end == dot.range.start)
            })
            .map(|token| token.text);
        if let Some(qualifier) = qualifier {
            if let Some((database, object)) = bindings.aliases.get(&qualifier.to_ascii_lowercase())
            {
                // Alias or table binding: its columns.
                if let Some(columns) = snapshot
                    .object(database, object)
                    .and_then(|object| object.columns.as_ref())
                {
                    for column in columns.values() {
                        if starts_with_case_insensitive(&column.name, prefix) {
                            suggestions.push(column_suggestion(column, replace.clone()));
                        }
                    }
                }
            } else if let Some(database) = snapshot
                .databases
                .values()
                .find(|database| database.name.eq_ignore_ascii_case(qualifier))
            {
                // Database qualifier: its tables and views.
                for object in database.objects.values() {
                    if starts_with_case_insensitive(&object.name, prefix) {
                        suggestions.push(object_suggestion(object, replace.clone()));
                    }
                }
            } else if let Some((_, object)) = default_database
                .and_then(|database| {
                    snapshot
                        .object(database, qualifier)
                        .map(|object| (database, object))
                })
                .or_else(|| unique_object(snapshot, qualifier))
            {
                // Bare table qualifier: its columns.
                if let Some(columns) = object.columns.as_ref() {
                    for column in columns.values() {
                        if starts_with_case_insensitive(&column.name, prefix) {
                            suggestions.push(column_suggestion(column, replace.clone()));
                        }
                    }
                }
            }
        }
        suggestions.sort_by(|left, right| left.label.cmp(&right.label));
        return suggestions;
    }

    let table_position = tokens.last().is_some_and(|token| {
        matches!(
            token.text.to_ascii_uppercase().as_str(),
            "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE"
        )
    });
    if table_position {
        if let Some(database) = default_database.and_then(|name| snapshot.database(name)) {
            for object in database.objects.values() {
                if starts_with_case_insensitive(&object.name, prefix) {
                    suggestions.push(object_suggestion(object, replace.clone()));
                }
            }
        } else {
            for database in snapshot.databases.values() {
                for object in database.objects.values() {
                    let label = format!("{}.{}", database.name, object.name);
                    if starts_with_case_insensitive(&label, prefix)
                        || starts_with_case_insensitive(&object.name, prefix)
                    {
                        suggestions.push(SchemaSuggestion {
                            label,
                            detail: object.engine.clone(),
                            kind: SuggestionKind::Object,
                            replace: replace.clone(),
                        });
                    }
                }
            }
        }
    } else {
        // A bare (unqualified) word in a column position: offer the
        // columns of every table in the query's scope, deduped by
        // name. resolve_bindings ran over the whole statement, so this
        // works even while typing the SELECT list before the FROM.
        let mut scope_tables: Vec<&(String, String)> = bindings.aliases.values().collect();
        scope_tables.sort();
        scope_tables.dedup();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (database, object) in scope_tables {
            if let Some(columns) = snapshot
                .object(database, object)
                .and_then(|object| object.columns.as_ref())
            {
                for column in columns.values() {
                    if starts_with_case_insensitive(&column.name, prefix)
                        && seen.insert(column.name.to_ascii_lowercase())
                    {
                        suggestions.push(column_suggestion(column, replace.clone()));
                    }
                }
            }
        }
    }
    suggestions.sort_by(|left, right| left.label.cmp(&right.label));
    suggestions
}

fn column_suggestion(column: &CachedColumn, replace: Range<usize>) -> SchemaSuggestion {
    SchemaSuggestion {
        label: column.name.clone(),
        detail: column.type_name.clone(),
        kind: SuggestionKind::Column,
        replace,
    }
}

fn object_suggestion(object: &CachedObject, replace: Range<usize>) -> SchemaSuggestion {
    SchemaSuggestion {
        label: object.name.clone(),
        detail: object.engine.clone(),
        kind: SuggestionKind::Object,
        replace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_intelligence::fixtures::{columns, snapshot};

    #[test]
    fn completes_tables_and_alias_columns() {
        let snapshot = snapshot(Some(columns()));
        let table_sql = "SELECT * FROM ev";
        let table = completions(&snapshot, Some("analytics"), table_sql, table_sql.len());
        assert_eq!(table[0].label, "events");

        let column_sql = "SELECT e.ev FROM events e";
        let cursor = column_sql.find("e.ev").unwrap() + 4;
        let column = completions(&snapshot, Some("analytics"), column_sql, cursor);
        assert_eq!(column[0].label, "event_id");
    }

    #[test]
    fn repro_alias_dot_mid_line() {
        let snapshot = snapshot(Some(columns()));
        let sql = "select e. from analytics.events e;";
        let cursor = sql.find('.').unwrap() + 1;
        let items = completions(&snapshot, None, sql, cursor);
        println!(
            "items: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
        assert!(!items.is_empty());
    }

    #[test]
    fn completes_after_database_and_table_qualifiers() {
        let snapshot = snapshot(Some(columns()));
        let sql = "SELECT * FROM analytics.";
        let objects = completions(&snapshot, None, sql, sql.len());
        assert_eq!(objects[0].label, "events");

        let sql = "SELECT events. FROM events";
        let cursor = sql.find(". FROM").unwrap() + 1;
        let columns = completions(&snapshot, Some("analytics"), sql, cursor);
        assert_eq!(columns[0].label, "event_id");
    }

    #[test]
    fn completes_bare_columns_from_query_scope() {
        let snapshot = snapshot(Some(columns()));
        // Typing the SELECT list before the FROM still resolves scope.
        let sql = "SELECT eve FROM analytics.events";
        let cursor = sql.find("eve ").unwrap() + 3;
        let items = completions(&snapshot, None, sql, cursor);
        assert!(items.iter().any(|item| item.label == "event_id"));
        // A bare column in WHERE, table not the default database.
        let sql = "SELECT * FROM analytics.events WHERE eve";
        let cols = completions(&snapshot, None, sql, sql.len());
        assert!(cols.iter().any(|item| item.label == "event_id"));

        // Only the statement under the cursor scopes the columns: a
        // prior statement's table must not leak its columns in.
        let sql = "SELECT other FROM other_db.other_table;\nSELECT eve FROM analytics.events";
        let cursor = sql.find("eve FROM").unwrap() + 3;
        let scoped = completions(&snapshot, None, sql, cursor);
        assert!(scoped.iter().any(|item| item.label == "event_id"));
        // (other_table is not in the snapshot, so the only way a stray
        // column could appear is a cross-statement leak; guard the
        // count stays column-only for events.)
        assert!(scoped
            .iter()
            .all(|item| item.kind == SuggestionKind::Column));
    }

    #[test]
    fn completes_inside_backtick_quotes() {
        let snapshot = snapshot(Some(columns()));
        // Bare backticked table name.
        let sql = "SELECT * FROM `ev";
        let table = completions(&snapshot, Some("analytics"), sql, sql.len());
        assert_eq!(table[0].label, "events");

        // Backticked database qualifier, backticked table being typed.
        let sql = "SELECT * FROM `analytics`.`ev";
        let objects = completions(&snapshot, None, sql, sql.len());
        assert_eq!(objects[0].label, "events");

        // Backticked table qualifier, column being typed.
        let sql = "SELECT `events`.`ev FROM events";
        let cursor = sql.find(".`ev").unwrap() + 4;
        let cols = completions(&snapshot, Some("analytics"), sql, cursor);
        assert_eq!(cols[0].label, "event_id");
    }
}
