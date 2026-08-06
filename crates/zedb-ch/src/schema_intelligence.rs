//! Conservative SQL name intelligence over an immutable schema snapshot.
//!
//! The analyzer only reports an unknown name when its containing database or
//! object's metadata is complete. Ambiguous, unqualified, CTE, and uncached
//! cases stay neutral.

use std::{collections::HashMap, ops::Range};

use crate::schema_cache::{CachedColumn, CachedObject, SchemaSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierIssue {
    pub range: Range<usize>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionKind {
    Database,
    Object,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSuggestion {
    pub label: String,
    pub detail: String,
    pub kind: SuggestionKind,
    pub replace: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: Range<usize>,
    pub markdown: String,
}

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

#[derive(Debug, Clone)]
struct Token<'a> {
    text: &'a str,
    range: Range<usize>,
    identifier: bool,
}

#[derive(Default)]
struct Bindings {
    aliases: HashMap<String, (String, String)>,
    ctes: Vec<String>,
}

pub fn analyze_sql(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
) -> Vec<IdentifierIssue> {
    let tokens = tokenize(sql);
    let (bindings, table_issues) = resolve_bindings(snapshot, default_database, &tokens);
    let mut issues = table_issues;

    for window in tokens.windows(3) {
        if !window[0].identifier || window[1].text != "." || !window[2].identifier {
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

pub fn completions(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    sql: &str,
    cursor: usize,
) -> Vec<SchemaSuggestion> {
    let cursor = cursor.min(sql.len());
    let replace = word_range(sql, cursor);
    let prefix = &sql[replace.clone()];
    let before = &sql[..replace.start];
    let tokens = tokenize(before);
    let (bindings, _) = resolve_bindings(snapshot, default_database, &tokenize(sql));
    let mut suggestions = Vec::new();

    if tokens.last().is_some_and(|token| token.text == ".") {
        let qualifier = tokens
            .get(tokens.len().saturating_sub(2))
            .filter(|token| token.identifier)
            .map(|token| token.text.to_ascii_lowercase());
        if let Some((database, object)) = qualifier
            .as_ref()
            .and_then(|qualifier| bindings.aliases.get(qualifier))
        {
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
        }
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
    }
    suggestions.sort_by(|left, right| left.label.cmp(&right.label));
    suggestions
}

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
    let tokens = tokenize(sql);
    let (bindings, _) = resolve_bindings(snapshot, default_database, &tokens);
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
            "**{}.{}.{}**\n\n`{}`",
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
    let mut markdown = format!(
        "**{}.{}**\n\nEngine: `{}`",
        object.0, object.1.name, object.1.engine
    );
    if let Some(rows) = object.1.total_rows {
        markdown.push_str(&format!("\n\nApproximate rows: {rows}"));
    }
    if !object.1.comment.is_empty() {
        markdown.push_str(&format!("\n\n{}", object.1.comment));
    }
    Some(HoverInfo { range, markdown })
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
    let (bindings, _) = resolve_bindings(snapshot, default_database, &tokens);
    let mut databases: Vec<String> = bindings
        .aliases
        .values()
        .map(|(database, _)| database.clone())
        .collect();
    databases.sort();
    databases.dedup();
    databases
}

fn resolve_bindings(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    tokens: &[Token<'_>],
) -> (Bindings, Vec<IdentifierIssue>) {
    let mut bindings = Bindings::default();
    let mut issues = Vec::new();
    for window in tokens.windows(4) {
        if window[0].text.eq_ignore_ascii_case("WITH")
            && window[1].identifier
            && window[2].text.eq_ignore_ascii_case("AS")
            && window[3].text == "("
        {
            bindings.ctes.push(window[1].text.to_ascii_lowercase());
        }
    }

    let mut index = 0;
    while index < tokens.len() {
        if !matches!(
            tokens[index].text.to_ascii_uppercase().as_str(),
            "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE"
        ) {
            index += 1;
            continue;
        }
        let Some(first) = tokens.get(index + 1).filter(|token| token.identifier) else {
            index += 1;
            continue;
        };
        if bindings.ctes.contains(&first.text.to_ascii_lowercase()) {
            index += 2;
            continue;
        }
        let (database, object, end, object_range) =
            if tokens.get(index + 2).is_some_and(|token| token.text == ".")
                && tokens.get(index + 3).is_some_and(|token| token.identifier)
            {
                (
                    Some(first.text.to_string()),
                    tokens[index + 3].text.to_string(),
                    index + 4,
                    first.range.start..tokens[index + 3].range.end,
                )
            } else {
                (
                    default_database.map(str::to_string),
                    first.text.to_string(),
                    index + 2,
                    first.range.clone(),
                )
            };

        let resolved_database =
            database.or_else(|| unique_object(snapshot, &object).map(|hit| hit.0.to_string()));
        if let Some(database) = &resolved_database {
            match snapshot.database(database) {
                Some(cached_database) => {
                    if cached_database
                        .objects
                        .values()
                        .all(|candidate| !candidate.name.eq_ignore_ascii_case(&object))
                    {
                        issues.push(IdentifierIssue {
                            range: object_range,
                            message: format!("Unknown table or view `{database}.{object}`"),
                        });
                    } else {
                        let alias = table_alias(tokens, end).unwrap_or_else(|| object.clone());
                        bindings.aliases.insert(
                            alias.to_ascii_lowercase(),
                            (database.clone(), object.clone()),
                        );
                    }
                }
                None if !snapshot.databases.is_empty() => issues.push(IdentifierIssue {
                    range: object_range,
                    message: format!("Unknown database `{database}`"),
                }),
                None => {}
            }
        }
        index = end;
    }
    (bindings, issues)
}

fn table_alias(tokens: &[Token<'_>], end: usize) -> Option<String> {
    let next = tokens.get(end)?;
    if next.text.eq_ignore_ascii_case("AS") {
        return tokens
            .get(end + 1)
            .filter(|token| token.identifier)
            .map(|token| token.text.into());
    }
    if next.identifier && !is_clause_keyword(next.text) {
        return Some(next.text.into());
    }
    None
}

fn unique_object<'a>(
    snapshot: &'a SchemaSnapshot,
    name: &str,
) -> Option<(&'a str, &'a CachedObject)> {
    let mut hits = snapshot.databases.values().filter_map(|database| {
        database
            .objects
            .values()
            .find(|object| object.name.eq_ignore_ascii_case(name))
            .map(|object| (database.name.as_str(), object))
    });
    let hit = hits.next()?;
    hits.next().is_none().then_some(hit)
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

fn starts_with_case_insensitive(value: &str, prefix: &str) -> bool {
    value.len() >= prefix.len() && value[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn word_range(text: &str, offset: usize) -> Range<usize> {
    let mut start = offset.min(text.len());
    let mut end = start;
    while start > 0
        && (text.as_bytes()[start - 1].is_ascii_alphanumeric()
            || text.as_bytes()[start - 1] == b'_')
    {
        start -= 1;
    }
    while end < text.len()
        && (text.as_bytes()[end].is_ascii_alphanumeric() || text.as_bytes()[end] == b'_')
    {
        end += 1;
    }
    start..end
}

fn tokenize(sql: &str) -> Vec<Token<'_>> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes[index] == b'-' && bytes.get(index + 1) == Some(&b'-') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
        } else if matches!(bytes[index], b'\'' | b'"' | b'`') {
            let quote = bytes[index];
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else if bytes[index] == quote {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
        } else if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(Token {
                text: &sql[start..index],
                range: start..index,
                identifier: true,
            });
        } else {
            let start = index;
            index += 1;
            tokens.push(Token {
                text: &sql[start..index],
                range: start..index,
                identifier: false,
            });
        }
    }
    tokens
}

fn is_clause_keyword(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "WHERE"
            | "PREWHERE"
            | "JOIN"
            | "LEFT"
            | "RIGHT"
            | "INNER"
            | "FULL"
            | "CROSS"
            | "ON"
            | "USING"
            | "GROUP"
            | "ORDER"
            | "LIMIT"
            | "SETTINGS"
            | "FORMAT"
            | "UNION"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::schema_cache::{CachedDatabase, CachedObjectKind};

    use super::*;

    fn snapshot(columns: Option<HashMap<String, CachedColumn>>) -> SchemaSnapshot {
        let mut snapshot = SchemaSnapshot::default();
        snapshot.databases.insert(
            "analytics".into(),
            CachedDatabase {
                name: "analytics".into(),
                touched: 1,
                objects: HashMap::from([(
                    "events".into(),
                    CachedObject {
                        name: "events".into(),
                        engine: "MergeTree".into(),
                        kind: CachedObjectKind::Table,
                        total_rows: Some(42),
                        comment: "Event stream".into(),
                        columns,
                    },
                )]),
            },
        );
        snapshot
    }

    fn columns() -> HashMap<String, CachedColumn> {
        HashMap::from([(
            "event_id".into(),
            CachedColumn {
                name: "event_id".into(),
                type_name: "UInt64".into(),
                codec_expression: "CODEC(Delta)".into(),
                comment: "Primary event id".into(),
            },
        )])
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
