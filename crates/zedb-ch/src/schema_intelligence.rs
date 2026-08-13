//! Conservative SQL name intelligence over an immutable schema snapshot.
//!
//! The analyzer only reports an unknown name when its containing database or
//! object's metadata is complete. Ambiguous, unqualified, CTE, and uncached
//! cases stay neutral.

use std::{collections::HashMap, ops::Range};

use crate::schema_cache::{CachedColumn, CachedObject, SchemaSnapshot};

mod filters;
mod limit;
mod order_by;

pub use filters::{column_filter, column_filters, filtered_columns, set_column_filter};
pub use limit::strip_top_level_limit;
pub use order_by::{aggregate_projection, has_group_by, set_order_by, top_level_order_by};

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
            // Built-in databases are excluded from the cache sweep, so
            // the linter can't see inside them; they are real all the
            // same and must never squiggle.
            if matches!(
                database.to_ascii_lowercase().as_str(),
                "system" | "information_schema"
            ) {
                index = end;
                continue;
            }
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

/// The slice of `sql` for the statement containing `cursor`, split on
/// top-level semicolons. Semicolons inside strings and comments are
/// skipped by the tokenizer, so they do not break statements.
fn current_statement(sql: &str, cursor: usize) -> &str {
    let cursor = cursor.min(sql.len());
    let semicolons: Vec<usize> = tokenize(sql)
        .into_iter()
        .filter(|token| token.text == ";")
        .map(|token| token.range.start)
        .collect();
    let start = semicolons
        .iter()
        .filter(|&&pos| pos < cursor)
        .max()
        .map(|&pos| pos + 1)
        .unwrap_or(0);
    let end = semicolons
        .iter()
        .find(|&&pos| pos >= cursor)
        .copied()
        .unwrap_or(sql.len());
    &sql[start..end.max(start)]
}

/// Byte length of the UTF-8 character starting at `index`, so the
/// byte-wise tokenizer advances whole characters and never slices mid
/// character (a multibyte char in the SQL otherwise panics the app, since
/// this runs on the editor buffer as you type/paste). At least 1.
fn char_len(sql: &str, index: usize) -> usize {
    sql[index..].chars().next().map_or(1, |c| c.len_utf8())
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
        } else if bytes[index] == b'`' {
            // Backticks quote an identifier in ClickHouse (db/table/
            // column names with special chars). Emit it as an
            // identifier token: `text` is the inner name so it matches
            // schema names, `range` spans the backticks for
            // highlighting.
            let start = index;
            index += 1;
            let inner_start = index;
            let mut inner_end = index;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index =
                        (index + 1 + char_len(sql, (index + 1).min(sql.len()))).min(bytes.len());
                    inner_end = index;
                } else if bytes[index] == b'`' {
                    inner_end = index;
                    index += 1;
                    break;
                } else {
                    index += char_len(sql, index);
                    inner_end = index;
                }
            }
            tokens.push(Token {
                text: &sql[inner_start..inner_end.min(sql.len())],
                range: start..index,
                identifier: true,
            });
        } else if matches!(bytes[index], b'\'' | b'"') {
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
            index += char_len(sql, index);
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

    #[test]
    fn multibyte_sql_does_not_panic_the_tokenizer() {
        // The byte-wise tokenizer runs on the editor buffer as you type or
        // paste; a multibyte character (arrow, em-dash, accent, emoji, a
        // backtick-quoted non-ASCII name) must never slice mid character.
        for sql in [
            "SELECT * FROM t WHERE amount = 475.51 → note",
            "SELECT * FROM t WHERE name = 'café' AND note = '— dash'",
            "SELECT * FROM t WHERE `café` = 1 ORDER BY `naïve` DESC",
            "SELECT 😀 FROM t WHERE x > 1",
            "-- comment with →\nSELECT * FROM t WHERE a = 1",
        ] {
            // Must not panic.
            let _ = tokenize(sql);
            let _ = column_filters(sql);
            let _ = top_level_order_by(sql);
        }
    }

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
                        total_bytes: None,
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
