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

/// A name the snapshot can vouch for, for positive highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognizedKind {
    Database,
    Object,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognizedIdentifier {
    pub range: Range<usize>,
    pub kind: RecognizedKind,
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
    let (bindings, _, _) = resolve_bindings(snapshot, default_database, &tokenize(sql));
    let mut suggestions = Vec::new();

    let dot_adjacent = tokens
        .last()
        .is_some_and(|token| token.text == "." && token.range.end == replace.start);
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
    let (bindings, _, _) = resolve_bindings(snapshot, default_database, &tokens);
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

/// One `schema_search` match: a dotted path plus a short detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSearchHit {
    pub path: String,
    pub kind: RecognizedKind,
    pub detail: String,
}

/// Case-insensitive substring search over database, object, and column
/// names. Returns up to `limit` hits (databases, then objects, then
/// columns, each alphabetical) plus the total match count.
pub fn schema_search(
    snapshot: &SchemaSnapshot,
    query: &str,
    limit: usize,
) -> (Vec<SchemaSearchHit>, usize) {
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for database in snapshot.databases.values() {
        if database.name.to_lowercase().contains(&needle) {
            hits.push(SchemaSearchHit {
                path: database.name.clone(),
                kind: RecognizedKind::Database,
                detail: format!("{} objects", database.objects.len()),
            });
        }
        for object in database.objects.values() {
            if object.name.to_lowercase().contains(&needle) {
                hits.push(SchemaSearchHit {
                    path: format!("{}.{}", database.name, object.name),
                    kind: RecognizedKind::Object,
                    detail: object.engine.clone(),
                });
            }
            let Some(columns) = object.columns.as_ref() else {
                continue;
            };
            for column in columns.values() {
                if column.name.to_lowercase().contains(&needle) {
                    hits.push(SchemaSearchHit {
                        path: format!("{}.{}.{}", database.name, object.name, column.name),
                        kind: RecognizedKind::Column,
                        detail: column.type_name.clone(),
                    });
                }
            }
        }
    }
    hits.sort_by(|left, right| {
        let rank = |kind: RecognizedKind| match kind {
            RecognizedKind::Database => 0,
            RecognizedKind::Object => 1,
            RecognizedKind::Column => 2,
        };
        rank(left.kind)
            .cmp(&rank(right.kind))
            .then_with(|| left.path.cmp(&right.path))
    });
    let total = hits.len();
    hits.truncate(limit);
    (hits, total)
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
    let tokens = tokenize(sql);
    let (bindings, _, _) = resolve_bindings(snapshot, default_database, &tokens);
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

fn resolve_bindings(
    snapshot: &SchemaSnapshot,
    default_database: Option<&str>,
    tokens: &[Token<'_>],
) -> (Bindings, Vec<IdentifierIssue>, Vec<RecognizedIdentifier>) {
    let mut bindings = Bindings::default();
    let mut issues = Vec::new();
    let mut recognized = Vec::new();
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
        let qualified = tokens.get(index + 2).is_some_and(|token| token.text == ".")
            && tokens.get(index + 3).is_some_and(|token| token.identifier);
        let (database, object, end, object_range, database_token, object_token) = if qualified {
            (
                Some(first.text.to_string()),
                tokens[index + 3].text.to_string(),
                index + 4,
                first.range.start..tokens[index + 3].range.end,
                Some(first.range.clone()),
                tokens[index + 3].range.clone(),
            )
        } else {
            (
                default_database.map(str::to_string),
                first.text.to_string(),
                index + 2,
                first.range.clone(),
                None,
                first.range.clone(),
            )
        };

        let resolved_database =
            database.or_else(|| unique_object(snapshot, &object).map(|hit| hit.0.to_string()));
        if let Some(database) = &resolved_database {
            match snapshot.database(database) {
                Some(cached_database) => {
                    if let Some(database_token) = database_token {
                        recognized.push(RecognizedIdentifier {
                            range: database_token,
                            kind: RecognizedKind::Database,
                        });
                    }
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
                        recognized.push(RecognizedIdentifier {
                            range: object_token,
                            kind: RecognizedKind::Object,
                        });
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
    (bindings, issues, recognized)
}

/// `alias.column` counts only when the three tokens are glued together;
/// `e. from` is an alias, a stray dot, and a keyword, not a reference.
fn is_column_reference(window: &[Token<'_>]) -> bool {
    window[0].identifier
        && window[1].text == "."
        && window[2].identifier
        && window[0].range.end == window[1].range.start
        && window[1].range.end == window[2].range.start
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
    fn schema_search_matches_all_levels_and_caps() {
        let snapshot = snapshot(Some(columns()));
        let (hits, total) = schema_search(&snapshot, "event", 10);
        assert_eq!(total, 2);
        assert_eq!(hits[0].path, "analytics.events");
        assert_eq!(hits[1].path, "analytics.events.event_id");

        let (capped, total) = schema_search(&snapshot, "e", 1);
        assert_eq!(capped.len(), 1);
        assert!(total >= 2);
        assert!(schema_search(&snapshot, "zzz", 10).0.is_empty());
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

/// Replace, insert, or remove the top-level ORDER BY of one statement.
/// Nested clauses (subqueries, window OVER (...)) are untouched. An
/// empty column list removes the clause; a written clause starts on its
/// own line.
pub fn set_order_by(sql: &str, columns: &[(String, bool)]) -> String {
    let (clause, insert_at) = top_level_order_by_span(sql);
    let new_clause = (!columns.is_empty()).then(|| {
        let list = columns
            .iter()
            .map(|(column, ascending)| {
                format!(
                    "`{}` {}",
                    column.replace('`', ""),
                    if *ascending { "ASC" } else { "DESC" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("ORDER BY {list}")
    });

    let join = |head: &str, middle: Option<&str>, rest: &str| {
        let mut out = head.trim_end().to_string();
        if let Some(middle) = middle {
            if !out.is_empty() {
                // The clause reads best on its own line.
                out.push('\n');
            }
            out.push_str(middle);
        }
        let rest = rest.trim_start();
        if !rest.is_empty() {
            if !rest.starts_with(';') {
                out.push(' ');
            }
            out.push_str(rest);
        }
        out
    };

    match clause {
        Some((start, end)) => join(&sql[..start], new_clause.as_deref(), &sql[end..]),
        None => match new_clause {
            Some(new_clause) => match insert_at {
                Some(position) => join(&sql[..position], Some(&new_clause), &sql[position..]),
                None => join(sql, Some(&new_clause), ""),
            },
            None => sql.to_string(),
        },
    }
}

/// Every column of a statement's top-level ORDER BY, in order, with
/// directions, for showing an honest sort indicator.
pub fn top_level_order_by(sql: &str) -> Vec<(String, bool)> {
    let (clause, _) = top_level_order_by_span(sql);
    let Some((start, end)) = clause else {
        return Vec::new();
    };
    let content = sql[start..end]
        .trim_start_matches(|character: char| !character.is_whitespace())
        .trim_start();
    let Some(content) = content
        .get(..2)
        .filter(|prefix| prefix.eq_ignore_ascii_case("by"))
        .map(|_| content[2..].trim_start())
    else {
        return Vec::new();
    };

    // Split entries on commas outside parens, backticks, and quotes.
    let mut entries = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut entry_start = 0;
    for (index, character) in content.char_indices() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
            }
            None => match character {
                '`' | '\'' | '"' => quote = Some(character),
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    entries.push(&content[entry_start..index]);
                    entry_start = index + 1;
                }
                _ => {}
            },
        }
    }
    entries.push(&content[entry_start..]);

    entries
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let upper = entry.to_ascii_uppercase();
            let (name, ascending) = if let Some(prefix) = upper
                .strip_suffix("DESCENDING")
                .or_else(|| upper.strip_suffix("DESC"))
            {
                (&entry[..prefix.len()], false)
            } else if let Some(prefix) = upper
                .strip_suffix("ASCENDING")
                .or_else(|| upper.strip_suffix("ASC"))
            {
                (&entry[..prefix.len()], true)
            } else {
                (entry, true)
            };
            let name = name.trim().trim_matches('`');
            (!name.is_empty()).then(|| (name.to_string(), ascending))
        })
        .collect()
}

/// The byte span of the top-level ORDER BY clause (through the end of
/// its column list), plus the byte position a new clause should be
/// inserted at when none exists (before LIMIT/SETTINGS/FORMAT/...).
fn top_level_order_by_span(sql: &str) -> (Option<(usize, usize)>, Option<usize>) {
    let tokens = tokenize(sql);
    let mut depth = 0i32;
    let mut clause = None;
    let mut insert_at = None;
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        match token.text {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            let upper = token.text.to_ascii_uppercase();
            if clause.is_none()
                && upper == "ORDER"
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| next.text.eq_ignore_ascii_case("BY"))
            {
                let start = token.range.start;
                let mut inner_depth = 0i32;
                let mut end = sql.len();
                let mut cursor = index + 2;
                while let Some(next) = tokens.get(cursor) {
                    match next.text {
                        "(" => inner_depth += 1,
                        ")" => inner_depth -= 1,
                        _ => {}
                    }
                    if inner_depth == 0 {
                        let next_upper = next.text.to_ascii_uppercase();
                        if next.text == ";"
                            || matches!(
                                next_upper.as_str(),
                                "LIMIT" | "OFFSET" | "SETTINGS" | "FORMAT" | "INTO" | "UNION"
                            )
                        {
                            end = next.range.start;
                            break;
                        }
                    }
                    cursor += 1;
                }
                clause = Some((start, end));
            }
            if insert_at.is_none()
                && (token.text == ";"
                    || matches!(
                        upper.as_str(),
                        "LIMIT" | "OFFSET" | "SETTINGS" | "FORMAT" | "INTO"
                    ))
            {
                insert_at = Some(token.range.start);
            }
        }
        index += 1;
    }
    (clause, insert_at)
}

#[cfg(test)]
mod order_by_tests {
    use super::*;

    fn sort(columns: &[(&str, bool)]) -> Vec<(String, bool)> {
        columns
            .iter()
            .map(|(name, ascending)| (name.to_string(), *ascending))
            .collect()
    }

    #[test]
    fn inserts_replaces_and_removes_top_level_order_by() {
        let sql = "SELECT * FROM t LIMIT 10";
        let sorted = set_order_by(sql, &sort(&[("kind", true)]));
        assert_eq!(sorted, "SELECT * FROM t\nORDER BY `kind` ASC LIMIT 10");

        let multi = set_order_by(&sorted, &sort(&[("kind", false), ("day", true)]));
        assert_eq!(
            multi,
            "SELECT * FROM t\nORDER BY `kind` DESC, `day` ASC LIMIT 10"
        );

        assert_eq!(set_order_by(&multi, &[]), "SELECT * FROM t LIMIT 10");
    }

    #[test]
    fn leaves_nested_order_by_alone() {
        let sql = "SELECT count() OVER (ORDER BY id) FROM (SELECT id FROM t ORDER BY id) x;";
        let sorted = set_order_by(sql, &sort(&[("id", false)]));
        assert!(sorted.contains("OVER (ORDER BY id)"));
        assert!(sorted.contains("FROM t ORDER BY id)"));
        assert!(sorted.ends_with("x\nORDER BY `id` DESC;"));
    }

    #[test]
    fn reports_the_active_sort_in_order() {
        assert_eq!(
            top_level_order_by("SELECT * FROM t ORDER BY `kind` DESC LIMIT 5"),
            sort(&[("kind", false)])
        );
        assert_eq!(
            top_level_order_by("SELECT * FROM t ORDER BY day, kind DESC, f(a, b)"),
            sort(&[("day", true), ("kind", false), ("f(a, b)", true)])
        );
        assert!(top_level_order_by("SELECT * FROM t").is_empty());
    }
}
