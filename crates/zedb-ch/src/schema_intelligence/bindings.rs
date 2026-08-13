//! Resolving the tables a statement puts in scope.
//!
//! Every name-resolving entry point (analysis, completion, hover) runs
//! through `resolve_bindings` so they all agree on what an alias refers to,
//! and so the neutrality rules for CTEs and uncached objects live in one
//! place rather than being reimplemented per caller.

use std::collections::HashMap;

use super::tokens::{is_clause_keyword, Token};
use super::{IdentifierIssue, RecognizedIdentifier, RecognizedKind};
use crate::schema_cache::{CachedObject, SchemaSnapshot};

#[derive(Default)]
pub(super) struct Bindings {
    pub(super) aliases: HashMap<String, (String, String)>,
    pub(super) ctes: Vec<String>,
}

pub(super) fn resolve_bindings(
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
pub(super) fn is_column_reference(window: &[Token<'_>]) -> bool {
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

pub(super) fn unique_object<'a>(
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
