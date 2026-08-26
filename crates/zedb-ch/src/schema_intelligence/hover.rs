//! Describing the name under a text offset, and resolving it to an object.

use super::bindings::{resolve_bindings, unique_object};
use super::tokens::{current_statement, tokenize, word_range};
use super::HoverInfo;
use crate::schema_cache::{CachedObject, SchemaSnapshot};

/// Server descriptions carry link targets written for the docs repo,
/// not for a browser: root-relative (`/operations/...`), markdown
/// paths (`../../sql-reference/data-types/date.md#x`), and bare
/// anchors (`#concat`). Resolve them against clickhouse.com/docs
/// (shapes verified against the live site); display text is never
/// touched. `anchor_page` names the docs page bare anchors belong to
/// (setting descriptions anchor into the settings page); without one
/// an anchor is unresolvable, and a link that errors is worse than
/// text, so it unlinks.
pub(super) fn absolutize_doc_links(text: &str, anchor_page: Option<&str>) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("](") {
        let target_start = open + 2;
        let Some(close) = rest[target_start..].find(')') else {
            break;
        };
        let target = &rest[target_start..target_start + close];
        output.push_str(&rest[..open]);
        match resolve_doc_target(target, anchor_page) {
            Some(resolved) => {
                output.push_str("](");
                output.push_str(&resolved);
                output.push(')');
            }
            None => {
                // Unresolvable: drop the link markup, keep the text.
                // The `[` this `](` pairs with is the last unmatched
                // one already written; remove it.
                if let Some(bracket) = output.rfind('[') {
                    output.remove(bracket);
                }
            }
        }
        rest = &rest[target_start + close + 1..];
    }
    output.push_str(rest);
    output
}

fn resolve_doc_target(target: &str, anchor_page: Option<&str>) -> Option<String> {
    if target.starts_with("http://") || target.starts_with("https://") {
        return Some(target.to_string());
    }
    let docs_url = |path: &str| {
        // The docs site serves pages without the repo's .md suffix.
        let path = path
            .replace(".md/#", "#")
            .replace(".md#", "#")
            .trim_end_matches(".md")
            .to_string();
        format!(
            "https://clickhouse.com/docs/{}",
            path.trim_start_matches('/')
        )
    };
    if let Some(anchor) = target.strip_prefix('#') {
        let page = anchor_page?;
        return Some(format!("{}#{anchor}", docs_url(page)));
    }
    if let Some(path) = target.strip_prefix("/docs/") {
        return Some(docs_url(path));
    }
    if target.starts_with('/') {
        return Some(docs_url(target));
    }
    if target.starts_with("../") {
        let path = target.trim_start_matches("../");
        return Some(docs_url(path));
    }
    None
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
    // A setting name inside a SETTINGS clause: the server's own card,
    // with the layer a query-level value would override.
    if !snapshot.settings.is_empty()
        && super::settings::settings_names(sql)
            .iter()
            .any(|(name_range, _)| name_range.contains(&offset) || name_range.end == offset)
    {
        if let Some(setting) = snapshot.setting(word) {
            let mut markdown = format!("**{}**", setting.name);
            if !setting.type_name.is_empty() {
                markdown.push_str(&format!("\n\nType: `{}`", setting.type_name));
            }
            let (layer, value) = super::settings::override_parts(setting);
            markdown.push_str(&format!("\n\n**Overrides:** _{layer}_ {value}"));
            if !setting.description.is_empty() {
                // The rule separates zeDB's metadata above from the
                // server's own prose below, relayed verbatim (links
                // absolutized so they open).
                markdown.push_str(&format!(
                    "\n\n---\n\n{}",
                    absolutize_doc_links(
                        &setting.description,
                        Some("operations/settings/settings"),
                    )
                ));
            }
            return Some(HoverInfo { range, markdown });
        }
    }
    // A function call (the word sits directly on a parenthesis): the
    // server's own card, combinator-aware. Gated on the parenthesis so
    // a column that happens to share a function's name still hovers as
    // the column.
    if !snapshot.functions.is_empty() && sql[range.end..].trim_start().starts_with('(') {
        if let Some(resolved) = super::functions::resolve_function(snapshot, word) {
            return Some(HoverInfo {
                markdown: super::functions::function_markdown(word, &resolved),
                range,
            });
        }
    }
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

    // The word is a bound alias: hover the table it stands for, saying
    // so (hover a variable, see its definition). Checked before the
    // database and column fallbacks; the alias is the most local name.
    if let Some((database, object)) = bindings.aliases.get(&word.to_ascii_lowercase()) {
        if let Some(cached) = snapshot.object(database, object) {
            return Some(HoverInfo {
                range,
                markdown: format!("_{word}_ → {}", object_hover_markdown(database, cached)),
            });
        }
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
    fn hovering_an_alias_shows_the_table_it_references() {
        let snapshot = snapshot(Some(columns()));
        // Both the declaration ("events e") and a later usage resolve;
        // the card is the table's own hover, prefixed with the alias
        // so the indirection is visible.
        let sql = "SELECT * FROM analytics.events e WHERE e.event_id > 1 AND e > 0";
        let declaration = hover(&snapshot, None, sql, sql.find(" e ").unwrap() + 1).unwrap();
        assert!(declaration.markdown.starts_with("_e_ →"), "{declaration:?}");
        assert!(declaration.markdown.contains("analytics.events"));
        assert!(declaration.markdown.contains("Engine:"));

        let usage = hover(&snapshot, None, sql, sql.rfind("e >").unwrap()).unwrap();
        assert!(usage.markdown.contains("analytics.events"));
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
