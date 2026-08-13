//! Whole-statement passes: linting unknown names, highlighting known ones,
//! and classifying which databases a statement reads or changes.

use super::bindings::{is_column_reference, resolve_bindings};
use super::tokens::tokenize;
use super::{IdentifierIssue, RecognizedIdentifier, RecognizedKind};
use crate::schema_cache::SchemaSnapshot;

pub fn touched_databases(sql: &str, default_database: Option<&str>) -> Vec<String> {
    let tokens = tokenize(sql);
    let Some(first) = tokens.iter().find(|token| token.identifier) else {
        return Vec::new();
    };
    if !matches!(
        first.text.to_ascii_uppercase().as_str(),
        "CREATE" | "ALTER" | "DROP" | "RENAME" | "TRUNCATE" | "ATTACH" | "DETACH"
    ) {
        return Vec::new();
    }
    for (index, token) in tokens.iter().enumerate() {
        if token.text.eq_ignore_ascii_case("DATABASE") {
            return tokens
                .get(index + 1)
                .filter(|token| token.identifier)
                .map(|token| vec![token.text.to_string()])
                .unwrap_or_default();
        }
        if matches!(
            token.text.to_ascii_uppercase().as_str(),
            "TABLE" | "VIEW" | "DICTIONARY"
        ) {
            let Some(name) = tokens.get(index + 1).filter(|token| token.identifier) else {
                continue;
            };
            if tokens.get(index + 2).is_some_and(|token| token.text == ".") {
                return vec![name.text.to_string()];
            }
            return default_database
                .map(|database| vec![database.to_string()])
                .unwrap_or_default();
        }
    }
    Vec::new()
}

pub fn analyze_sql(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
) -> Vec<IdentifierIssue> {
    let tokens = tokenize(sql);
    let (bindings, table_issues, _) = resolve_bindings(snapshot, default_database, &tokens);
    let mut issues = table_issues;

    for window in tokens.windows(3) {
        if !is_column_reference(window) {
            continue;
        }
        let Some((database, object)) = bindings.aliases.get(&window[0].text.to_ascii_lowercase())
        else {
            continue;
        };
        let Some(columns) = snapshot
            .object(database, object)
            .and_then(|object| object.columns.as_ref())
        else {
            continue;
        };
        if !columns
            .values()
            .any(|column| column.name.eq_ignore_ascii_case(window[2].text))
        {
            issues.push(IdentifierIssue {
                range: window[2].range.clone(),
                message: format!("Unknown column `{}`", window[2].text),
            });
        }
    }
    issues.sort_by_key(|issue| issue.range.start);
    issues.dedup_by(|left, right| left.range == right.range);
    issues
}

/// Every name in the SQL the snapshot can vouch for: databases, tables
/// and views resolved through bindings, and columns whose object's
/// metadata is complete. Feeds the editor's recognized highlighting.
pub fn recognized_identifiers(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
) -> Vec<RecognizedIdentifier> {
    let tokens = tokenize(sql);
    let (bindings, _, mut recognized) = resolve_bindings(snapshot, default_database, &tokens);
    for window in tokens.windows(3) {
        if !is_column_reference(window) {
            continue;
        }
        let Some((database, object)) = bindings.aliases.get(&window[0].text.to_ascii_lowercase())
        else {
            continue;
        };
        let Some(columns) = snapshot
            .object(database, object)
            .and_then(|object| object.columns.as_ref())
        else {
            continue;
        };
        if columns
            .values()
            .any(|column| column.name.eq_ignore_ascii_case(window[2].text))
        {
            recognized.push(RecognizedIdentifier {
                range: window[2].range.clone(),
                kind: RecognizedKind::Column,
            });
        }
    }
    recognized.sort_by_key(|identifier| identifier.range.start);
    recognized.dedup_by(|left, right| left.range == right.range);
    recognized
}

/// Databases whose column metadata the given SQL would use, resolved
/// through the same bindings as analysis. Only databases the snapshot
/// already knows are returned, so callers can warm them safely.
pub fn referenced_databases(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
) -> Vec<String> {
    let tokens = tokenize(sql);
    let (bindings, _, _) = resolve_bindings(snapshot, default_database, &tokens);
    let mut databases: Vec<String> = bindings
        .aliases
        .values()
        .map(|(database, _)| database.clone())
        .collect();
    databases.sort();
    databases.dedup();
    databases
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_intelligence::fixtures::{columns, snapshot};

    #[test]
    fn built_in_databases_never_squiggle() {
        // The cache sweep excludes system and INFORMATION_SCHEMA, but
        // they exist on every server.
        let snapshot = snapshot(Some(columns()));
        let issues = analyze_sql(
            &snapshot,
            Some("analytics"),
            "SELECT database, name FROM system.tables \
             JOIN INFORMATION_SCHEMA.tables i ON i.table_name = name",
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn flags_known_unknown_tables_and_columns() {
        let snapshot = snapshot(Some(columns()));
        let issues = analyze_sql(
            &snapshot,
            Some("analytics"),
            "SELECT e.missing FROM events AS e JOIN absent a ON a.id = e.event_id",
        );
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().any(|issue| issue.message.contains("column")));
        assert!(issues.iter().any(|issue| issue.message.contains("absent")));
    }

    #[test]
    fn stays_quiet_for_uncached_columns_ctes_and_ambiguous_names() {
        let snapshot = snapshot(None);
        let issues = analyze_sql(
            &snapshot,
            Some("analytics"),
            "WITH recent AS (SELECT * FROM events) SELECT r.anything FROM recent r",
        );
        assert!(issues.is_empty());
    }

    #[test]
    fn recognizes_known_databases_tables_and_columns() {
        let snapshot = snapshot(Some(columns()));
        let sql = "SELECT e.event_id, e.missing FROM analytics.events e";
        let recognized = recognized_identifiers(&snapshot, None, sql);
        let kinds: Vec<(RecognizedKind, &str)> = recognized
            .iter()
            .map(|identifier| (identifier.kind, &sql[identifier.range.clone()]))
            .collect();
        assert!(kinds.contains(&(RecognizedKind::Database, "analytics")));
        assert!(kinds.contains(&(RecognizedKind::Object, "events")));
        assert!(kinds.contains(&(RecognizedKind::Column, "event_id")));
        assert!(!kinds.iter().any(|(_, text)| *text == "missing"));
    }

    #[test]
    fn recognizes_backtick_quoted_names() {
        let snapshot = snapshot(Some(columns()));
        let sql = "SELECT e.`event_id` FROM `analytics`.`events` e";
        let recognized = recognized_identifiers(&snapshot, None, sql);
        // The highlighted range spans the backticks; the name matches.
        let kinds: Vec<(RecognizedKind, &str)> = recognized
            .iter()
            .map(|identifier| (identifier.kind, &sql[identifier.range.clone()]))
            .collect();
        assert!(kinds.contains(&(RecognizedKind::Database, "`analytics`")));
        assert!(kinds.contains(&(RecognizedKind::Object, "`events`")));
        assert!(kinds.contains(&(RecognizedKind::Column, "`event_id`")));
    }

    #[test]
    fn reports_databases_referenced_through_bindings() {
        let snapshot = snapshot(None);
        assert_eq!(
            referenced_databases(&snapshot, None, "SELECT e.x FROM analytics.events e"),
            vec!["analytics"]
        );
        assert!(referenced_databases(&snapshot, None, "SELECT 1").is_empty());
    }

    #[test]
    fn classifies_only_schema_changing_statements() {
        assert_eq!(
            touched_databases("ALTER TABLE analytics.events ADD COLUMN x UInt8", None),
            vec!["analytics"]
        );
        assert_eq!(
            touched_databases(
                "CREATE TABLE events (x UInt8) ENGINE=Memory",
                Some("scratch")
            ),
            vec!["scratch"]
        );
        assert!(touched_databases("SELECT * FROM analytics.events", Some("analytics")).is_empty());
    }
}
