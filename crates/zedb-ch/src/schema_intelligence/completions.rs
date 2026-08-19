//! Cursor-position completion of database, object, and column names.

use std::ops::Range;

use super::bindings::{resolve_bindings, unique_object};
use super::tokens::{current_statement, starts_with_case_insensitive, tokenize, word_range};
use super::vocabulary::{FUNCTIONS, KEYWORDS, PARAM_TYPES};
use super::{SchemaSuggestion, SuggestionKind};
use crate::schema_cache::{CachedColumn, CachedObject, SchemaSnapshot};

pub fn completions(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    cursor: usize,
) -> Vec<SchemaSuggestion> {
    completions_with_placeholders(snapshot, default_database, sql, cursor, &[], &[])
}

/// Like [`completions`], with the editor's resolved placeholder values:
/// `variables` for `${name}` (@set) and `params` for `{name:Type}`
/// (SET param_). A placeholder before a dot then qualifies the dot the
/// same way a typed name would, so `{db:Identifier}.` offers that
/// database's tables.
pub fn completions_with_placeholders(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    cursor: usize,
    variables: &[(String, String)],
    params: &[(String, String)],
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

    // Inside a `{name:` query-parameter placeholder the only thing that
    // can follow the colon is a type; nothing else applies there.
    let type_position = tokens.len() >= 3
        && tokens[tokens.len() - 1].text == ":"
        && tokens[tokens.len() - 2].identifier
        && tokens[tokens.len() - 3].text == "{";
    if type_position {
        for (name, hint) in PARAM_TYPES {
            if starts_with_case_insensitive(name, prefix) {
                suggestions.push(SchemaSuggestion {
                    label: (*name).to_string(),
                    detail: (*hint).to_string(),
                    kind: SuggestionKind::Type,
                    replace: replace.clone(),
                });
            }
        }
        // PARAM_TYPES is in priority order; keep it.
        return suggestions;
    }

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
        // A placeholder before the dot qualifies it too: resolve
        // `{db:Identifier}.` / `${db}.` to the declared value and treat
        // that as the typed qualifier.
        let qualifier = qualifier.or_else(|| {
            let dot = tokens.last()?.range.start;
            let before_dot = &sql[..dot];
            if !before_dot.ends_with('}') {
                return None;
            }
            let open = before_dot.rfind('{')?;
            if before_dot[open..].contains('\n') {
                return None;
            }
            let inner = &before_dot[open + 1..before_dot.len() - 1];
            let dollar = open > 0 && before_dot.as_bytes()[open - 1] == b'$';
            let (name, declarations) = if dollar {
                (inner, variables)
            } else {
                (inner.split_once(':')?.0, params)
            };
            declarations
                .iter()
                .find(|(declared, _)| declared == name)
                .map(|(_, value)| value.as_str())
        });
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
        // Other databases stay reachable from a table position: typing
        // the database name qualifies the follow-up dot completion.
        for database in snapshot.databases.values() {
            if !prefix.is_empty() && starts_with_case_insensitive(&database.name, prefix) {
                suggestions.push(SchemaSuggestion {
                    label: database.name.clone(),
                    detail: "database".to_string(),
                    kind: SuggestionKind::Database,
                    replace: replace.clone(),
                });
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
        // Vocabulary rides along with bare words only once something is
        // typed: an empty prefix would wall every keystroke with keywords.
        if !prefix.is_empty() {
            for (name, signature) in FUNCTIONS {
                if starts_with_case_insensitive(name, prefix) {
                    suggestions.push(SchemaSuggestion {
                        label: (*name).to_string(),
                        detail: (*signature).to_string(),
                        kind: SuggestionKind::Function,
                        replace: replace.clone(),
                    });
                }
            }
            for keyword in KEYWORDS {
                if starts_with_case_insensitive(keyword, prefix) {
                    suggestions.push(SchemaSuggestion {
                        label: (*keyword).to_string(),
                        detail: String::new(),
                        kind: SuggestionKind::Keyword,
                        replace: replace.clone(),
                    });
                }
            }
        }
    }
    // Schema names outrank vocabulary: what the user's own data calls
    // things is almost always what a prefix means.
    suggestions.sort_by(|left, right| {
        kind_rank(&left.kind)
            .cmp(&kind_rank(&right.kind))
            .then_with(|| {
                left.label
                    .to_ascii_lowercase()
                    .cmp(&right.label.to_ascii_lowercase())
            })
    });
    suggestions
}

fn kind_rank(kind: &SuggestionKind) -> u8 {
    match kind {
        SuggestionKind::Column | SuggestionKind::Type => 0,
        SuggestionKind::Object => 1,
        SuggestionKind::Database => 2,
        SuggestionKind::Function => 3,
        SuggestionKind::Keyword => 4,
    }
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
    fn offers_functions_and_keywords_behind_schema_names() {
        let snapshot = snapshot(Some(columns()));
        // "eve" matches the event_id column; schema outranks vocabulary.
        let sql = "SELECT eve FROM analytics.events";
        let cursor = sql.find("eve ").unwrap() + 3;
        let items = completions(&snapshot, None, sql, cursor);
        assert_eq!(items[0].kind, SuggestionKind::Column);

        // "toSta" matches only functions.
        let sql = "SELECT toSta FROM analytics.events";
        let cursor = sql.find("toSta").unwrap() + 5;
        let items = completions(&snapshot, None, sql, cursor);
        assert!(!items.is_empty());
        assert!(items
            .iter()
            .all(|item| item.kind == SuggestionKind::Function));
        assert!(items.iter().any(|item| item.label == "toStartOfDay"));

        // "grou" surfaces both groupArray and GROUP BY, functions first.
        let sql = "SELECT * FROM analytics.events grou";
        let items = completions(&snapshot, None, sql, sql.len());
        assert!(items.iter().any(|item| item.label == "groupArray"));
        assert!(items.iter().any(|item| item.label == "GROUP BY"));

        // An empty prefix stays schema-only: no keyword wall.
        let sql = "SELECT  FROM analytics.events";
        let cursor = sql.find("  ").unwrap() + 1;
        let items = completions(&snapshot, None, sql, cursor);
        assert!(items.iter().all(|item| item.kind == SuggestionKind::Column));
    }

    #[test]
    fn a_placeholder_qualifier_completes_like_its_value() {
        let snapshot = snapshot(Some(columns()));
        let params = vec![("db".to_string(), "analytics".to_string())];
        let variables = vec![("db".to_string(), "analytics".to_string())];

        // {db:Identifier}. offers the tables of the database it names.
        let sql = "SELECT count() FROM {db:Identifier}.";
        let items = completions_with_placeholders(&snapshot, None, sql, sql.len(), &[], &params);
        assert_eq!(items[0].label, "events");

        // ${db}. resolves through @set declarations the same way.
        let sql = "SELECT count() FROM ${db}.ev";
        let items = completions_with_placeholders(&snapshot, None, sql, sql.len(), &variables, &[]);
        assert_eq!(items[0].label, "events");

        // An unresolved placeholder stays quiet rather than guessing.
        let sql = "SELECT count() FROM {other:Identifier}.";
        let items = completions_with_placeholders(&snapshot, None, sql, sql.len(), &[], &params);
        assert!(items.is_empty());
    }

    #[test]
    fn offers_types_after_a_parameter_colon() {
        let snapshot = snapshot(Some(columns()));
        // Immediately after the colon: the full type list, priority
        // order, nothing else.
        let sql = "SELECT count() FROM {db:";
        let items = completions(&snapshot, Some("analytics"), sql, sql.len());
        assert!(!items.is_empty());
        assert!(items.iter().all(|item| item.kind == SuggestionKind::Type));
        assert_eq!(items[0].label, "Identifier");

        // A typed prefix filters case-insensitively.
        let sql = "SELECT count() FROM {db:iden";
        let items = completions(&snapshot, Some("analytics"), sql, sql.len());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Identifier");

        // A colon outside a placeholder does not force types.
        let sql = "SELECT eve FROM analytics.events";
        let cursor = sql.find("eve ").unwrap() + 3;
        let items = completions(&snapshot, None, sql, cursor);
        assert!(items.iter().any(|item| item.kind == SuggestionKind::Column));
    }

    #[test]
    fn offers_databases_in_table_position() {
        let snapshot = snapshot(Some(columns()));
        // Even with a default database set, other databases stay
        // reachable by name from FROM.
        let sql = "SELECT * FROM analyt";
        let items = completions(&snapshot, Some("analytics"), sql, sql.len());
        assert!(items
            .iter()
            .any(|item| item.kind == SuggestionKind::Database && item.label == "analytics"));
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
