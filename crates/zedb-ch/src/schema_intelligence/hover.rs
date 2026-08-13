//! Describing the name under a text offset, and resolving it to an object.

use super::bindings::{resolve_bindings, unique_object};
use super::tokens::{current_statement, tokenize, word_range};
use super::HoverInfo;
use crate::schema_cache::{CachedObject, SchemaSnapshot};

pub fn hover(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    offset: usize,
) -> Option<HoverInfo> {
    let range = word_range(sql, offset.min(sql.len()));
    if range.is_empty() {
        return None;
    }
    let word = &sql[range.clone()];
    // Bindings from the statement under the offset only, so an
    // editor full of statements does not resolve names against
    // tables from other queries (see completions).
    let (bindings, _, _) = resolve_bindings(
        snapshot,
        default_database,
        &tokenize(current_statement(sql, offset)),
    );
    let qualifier = sql[..range.start]
        .strip_suffix('.')
        .map(|before| &before[word_range(before, before.len())])
        .map(str::to_ascii_lowercase);
    if let Some((database, object)) = qualifier
        .as_ref()
        .and_then(|qualifier| bindings.aliases.get(qualifier))
    {
        let column = snapshot.column(database, object, word)?;
        let mut markdown = format!(
            "**{}.{}.**_{}_\n\nType: `{}`",
            database, object, column.name, column.type_name
        );
        if !column.codec_expression.is_empty() {
            markdown.push_str(&format!("\n\n{}", column.codec_expression));
        }
        if !column.comment.is_empty() {
            markdown.push_str(&format!("\n\n{}", column.comment));
        }
        return Some(HoverInfo { range, markdown });
    }

    // Database-qualified object: `zedb_kappa.events_daily` resolves even
    // when the bare name is ambiguous across databases.
    if let Some(database) = qualifier.as_ref().and_then(|qualifier| {
        snapshot
            .databases
            .values()
            .find(|database| database.name.eq_ignore_ascii_case(qualifier))
    }) {
        let object = database
            .objects
            .values()
            .find(|object| object.name.eq_ignore_ascii_case(word))?;
        return Some(HoverInfo {
            range,
            markdown: object_hover_markdown(&database.name, object),
        });
    }

    // The word itself is a database name.
    if let Some(database) = snapshot
        .databases
        .values()
        .find(|database| database.name.eq_ignore_ascii_case(word))
    {
        return Some(HoverInfo {
            range,
            markdown: format!(
                "**{}**\n\nDatabase with {} objects",
                database.name,
                database.objects.len()
            ),
        });
    }

    // A bare column: resolve it against the tables in scope. When
    // exactly one of them has a column by this name, hover it as
    // `db.table.column` + type, same as the qualified form.
    let mut scope_tables: Vec<&(String, String)> = bindings.aliases.values().collect();
    scope_tables.sort();
    scope_tables.dedup();
    let mut column_match = None;
    for (database, object) in scope_tables {
        if let Some(column) = snapshot.column(database, object, word) {
            if column_match.is_some() {
                // Ambiguous across tables; don't guess.
                column_match = None;
                break;
            }
            column_match = Some((database.clone(), object.clone(), column));
        }
    }
    if let Some((database, object, column)) = column_match {
        let mut markdown = format!(
            "**{}.{}.**_{}_\n\nType: `{}`",
            database, object, column.name, column.type_name
        );
        if !column.codec_expression.is_empty() {
            markdown.push_str(&format!("\n\n{}", column.codec_expression));
        }
        if !column.comment.is_empty() {
            markdown.push_str(&format!("\n\n{}", column.comment));
        }
        return Some(HoverInfo { range, markdown });
    }

    let object = default_database
        .and_then(|database| {
            snapshot
                .object(database, word)
                .map(|object| (database, object))
        })
        .or_else(|| unique_object(snapshot, word))?;
    Some(HoverInfo {
        range,
        markdown: object_hover_markdown(object.0, object.1),
    })
}

/// The object a text offset refers to: database-qualified, a bound
/// alias, or a bare name resolvable through the default database or
/// uniqueness. Returns snapshot-canonical (database, object) names.
pub fn object_at(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    offset: usize,
) -> Option<(String, String)> {
    let range = word_range(sql, offset.min(sql.len()));
    if range.is_empty() {
        return None;
    }
    let word = &sql[range.clone()];
    // Bindings from the statement under the offset only, so an
    // editor full of statements does not resolve names against
    // tables from other queries (see completions).
    let (bindings, _, _) = resolve_bindings(
        snapshot,
        default_database,
        &tokenize(current_statement(sql, offset)),
    );
    let qualifier = sql[..range.start]
        .strip_suffix('.')
        .map(|before| &before[word_range(before, before.len())])
        .map(str::to_ascii_lowercase);
    if let Some(qualifier) = qualifier.as_ref() {
        if let Some(database) = snapshot
            .databases
            .values()
            .find(|database| database.name.eq_ignore_ascii_case(qualifier))
        {
            let object = database
                .objects
                .values()
                .find(|object| object.name.eq_ignore_ascii_case(word))?;
            return Some((database.name.clone(), object.name.clone()));
        }
        // An alias or table qualifier means the word is a column.
        return None;
    }
    if let Some((database, object)) = bindings.aliases.get(&word.to_ascii_lowercase()) {
        return Some((database.clone(), object.clone()));
    }
    let (database, object) = default_database
        .and_then(|database| {
            snapshot
                .object(database, word)
                .map(|object| (database, object))
        })
        .or_else(|| unique_object(snapshot, word))?;
    Some((database.to_string(), object.name.clone()))
}

fn object_hover_markdown(database: &str, object: &CachedObject) -> String {
    let mut markdown = format!(
        "**{}.{}**\n\nEngine: `{}`",
        database, object.name, object.engine
    );
    if let Some(rows) = object.total_rows {
        markdown.push_str(&format!("\n\nApproximate rows: {rows}"));
    }
    if let Some(columns) = object.columns.as_ref() {
        markdown.push_str(&format!("\n\n{} columns", columns.len()));
    }
    if !object.comment.is_empty() {
        markdown.push_str(&format!("\n\n{}", object.comment));
    }
    markdown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_intelligence::fixtures::{columns, snapshot};

    #[test]
    fn hover_describes_columns_and_tables() {
        let snapshot = snapshot(Some(columns()));
        let sql = "SELECT e.event_id FROM events e";
        let info = hover(
            &snapshot,
            Some("analytics"),
            sql,
            sql.find("event_id").unwrap(),
        )
        .unwrap();
        assert!(info.markdown.contains("UInt64"));
        assert!(info.markdown.contains("Primary event id"));
    }

    #[test]
    fn hover_resolves_bare_columns_from_scope() {
        let snapshot = snapshot(Some(columns()));
        // Unqualified column, single table in scope: resolves to
        // db.table.column and its type.
        let sql = "SELECT event_id FROM analytics.events";
        let info = hover(&snapshot, None, sql, sql.find("event_id").unwrap()).unwrap();
        // db.table. bold, column italic.
        assert!(info.markdown.contains("analytics.events."));
        assert!(info.markdown.contains("event_id"));
        assert!(info.markdown.contains("UInt64"));
    }

    #[test]
    fn hover_resolves_database_qualified_objects_and_databases() {
        let snapshot = snapshot(None);
        let sql = "SELECT * FROM analytics.events";
        let table = hover(&snapshot, None, sql, sql.find("events").unwrap()).unwrap();
        assert!(table.markdown.contains("MergeTree"));

        let database = hover(&snapshot, None, sql, sql.find("analytics").unwrap()).unwrap();
        assert!(database.markdown.contains("Database with 1 objects"));
    }

    #[test]
    fn object_at_resolves_qualified_names_and_aliases() {
        let snapshot = snapshot(None);
        let sql = "SELECT e.x FROM analytics.events e";
        let qualified = object_at(&snapshot, None, sql, sql.rfind("events").unwrap());
        assert_eq!(qualified, Some(("analytics".into(), "events".into())));

        let alias = object_at(&snapshot, None, sql, sql.len() - 1);
        assert_eq!(alias, Some(("analytics".into(), "events".into())));

        assert_eq!(
            object_at(&snapshot, None, sql, sql.find('x').unwrap()),
            None
        );
    }
}
