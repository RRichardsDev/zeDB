//! The executing-scope honesty check for hand-written SQL.
//!
//! zeDB never rewrites the SQL a user typed: with a cluster selected
//! in "Executing on", a DDL statement without ON CLUSTER still runs
//! on the connected node only. These helpers make that divergence
//! visible (a hint diagnostic on the statement) and offer the fix as
//! an explicit edit (the context menu's Add ON CLUSTER), which lands
//! in the buffer for the user to see before they run anything.

use std::ops::Range;

use super::buffer::split_statements;

/// Verbs whose statements ClickHouse distributes with ON CLUSTER.
const DDL_VERBS: [&str; 9] = [
    "CREATE", "ALTER", "DROP", "TRUNCATE", "RENAME", "ATTACH", "DETACH", "OPTIMIZE", "EXCHANGE",
];

/// One word-ish token: (byte range, uppercased text). Backticked
/// identifiers come back verbatim (not uppercased) with their ticks.
fn tokens(statement: &str) -> Vec<(Range<usize>, String)> {
    let bytes = statement.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if statement[i..].starts_with("--") {
            i += statement[i..]
                .find('\n')
                .map(|n| n + 1)
                .unwrap_or(statement.len() - i);
        } else if statement[i..].starts_with("/*") {
            i += statement[i..]
                .find("*/")
                .map(|n| n + 2)
                .unwrap_or(statement.len() - i);
        } else if c == b'\'' || c == b'"' {
            // String literal: skip to the closing quote, honoring \\.
            let quote = c;
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j += 2;
                } else if bytes[j] == quote {
                    j += 1;
                    break;
                } else {
                    j += 1;
                }
            }
            i = j.min(bytes.len());
        } else if c == b'`' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'`' {
                j += 1;
            }
            let end = (j + 1).min(statement.len());
            out.push((i..end, statement[i..end].to_string()));
            i = end;
        } else if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.')
            {
                j += 1;
            }
            out.push((i..j, statement[i..j].to_ascii_uppercase()));
            i = j;
        } else {
            out.push((i..i + 1, statement[i..i + 1].to_string()));
            i += 1;
        }
    }
    out
}

fn is_identifier(token: &str) -> bool {
    token.starts_with('`')
        || token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Hints for every DDL statement in `sql` that lacks ON CLUSTER while
/// the executing scope is `cluster`: (range of the leading verb, the
/// message). SELECT/INSERT and friends are never flagged; neither is
/// SQL that already says ON CLUSTER.
pub(crate) fn cluster_scope_hints(sql: &str, cluster: &str) -> Vec<(Range<usize>, String)> {
    let mut hints = Vec::new();
    for (start, end) in split_statements(sql) {
        let statement = &sql[start..end];
        let tokens = tokens(statement);
        let Some((verb_range, verb)) = tokens.first() else {
            continue;
        };
        if !DDL_VERBS.contains(&verb.as_str()) {
            continue;
        }
        let has_on_cluster = tokens
            .windows(2)
            .any(|pair| pair[0].1 == "ON" && pair[1].1 == "CLUSTER");
        if has_on_cluster {
            continue;
        }
        hints.push((
            start + verb_range.start..start + verb_range.end,
            format!(
                "Executing on is cluster {cluster}, but this statement runs on the connected \
                 node only. Add ON CLUSTER {cluster} (right-click) if it should run cluster-wide."
            ),
        ));
    }
    hints
}

/// Whether the statement under `offset` is clusterable DDL without an
/// ON CLUSTER clause: the context menu's gate for offering the fix.
pub(crate) fn statement_wants_on_cluster(sql: &str, offset: usize) -> bool {
    split_statements(sql)
        .into_iter()
        .find(|(start, end)| (*start..=*end).contains(&offset))
        .is_some_and(|(start, end)| {
            let tokens = tokens(&sql[start..end]);
            tokens
                .first()
                .is_some_and(|(_, verb)| DDL_VERBS.contains(&verb.as_str()))
                && !tokens
                    .windows(2)
                    .any(|pair| pair[0].1 == "ON" && pair[1].1 == "CLUSTER")
        })
}

/// Where ` ON CLUSTER x` belongs inside `statement`: right after the
/// target object's name, for the forms whose grammar is unambiguous.
/// None means no confident spot; the hint then has no one-click fix.
pub(crate) fn on_cluster_insertion(statement: &str) -> Option<usize> {
    let tokens = tokens(statement);
    let mut it = tokens.iter().peekable();
    let (_, verb) = it.next()?;

    // Modifier words that may precede the object kind.
    const MODIFIERS: [&str; 6] = [
        "OR",
        "REPLACE",
        "TEMPORARY",
        "MATERIALIZED",
        "LIVE",
        "WINDOW",
    ];
    const KINDS: [&str; 5] = ["TABLE", "VIEW", "DICTIONARY", "DATABASE", "FUNCTION"];

    match verb.as_str() {
        "CREATE" | "ATTACH" | "DROP" | "DETACH" | "TRUNCATE" | "OPTIMIZE" | "ALTER" => {
            // Skip modifiers up to the kind word. TRUNCATE allows a
            // bare name (TABLE optional); ALTER requires TABLE.
            while let Some((_, token)) = it.peek() {
                if MODIFIERS.contains(&token.as_str()) {
                    it.next();
                } else {
                    break;
                }
            }
            let mut saw_kind = false;
            if let Some((_, token)) = it.peek() {
                if KINDS.contains(&token.as_str()) {
                    saw_kind = true;
                    it.next();
                }
            }
            if verb == "ALTER" && !saw_kind {
                return None;
            }
            // IF [NOT] EXISTS
            if it.peek().is_some_and(|(_, t)| t == "IF") {
                it.next();
                if it.peek().is_some_and(|(_, t)| t == "NOT") {
                    it.next();
                }
                if it.peek().is_some_and(|(_, t)| t == "EXISTS") {
                    it.next();
                } else {
                    return None;
                }
            }
            let (name_range, name) = it.next()?;
            if !is_identifier(name) {
                return None;
            }
            Some(name_range.end)
        }
        // RENAME and EXCHANGE place ON CLUSTER at the end, after all
        // pairs; too shape-dependent to edit confidently.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_clusterable_ddl_without_on_cluster_is_flagged() {
        let sql = "SELECT 1;\n\
                   CREATE TABLE t (x UInt64) ENGINE = MergeTree ORDER BY x;\n\
                   ALTER TABLE db.t ON CLUSTER prod DROP COLUMN x;\n\
                   -- DROP TABLE commented_out;\n\
                   DROP TABLE IF EXISTS t;";
        let hints = cluster_scope_hints(sql, "prod");
        assert_eq!(hints.len(), 2, "{hints:?}");
        assert!(sql[hints[0].0.clone()].eq_ignore_ascii_case("CREATE"));
        assert!(sql[hints[1].0.clone()].eq_ignore_ascii_case("DROP"));
        assert!(hints[0].1.contains("cluster prod"));
    }

    #[test]
    fn string_literals_do_not_hide_or_fake_on_cluster() {
        let hints = cluster_scope_hints("CREATE TABLE t (s String) COMMENT 'ON CLUSTER'", "c");
        assert_eq!(hints.len(), 1, "a quoted ON CLUSTER does not count");
    }

    #[test]
    fn insertion_lands_after_the_object_name() {
        for (statement, after) in [
            ("CREATE TABLE t (x UInt64)", "CREATE TABLE t"),
            (
                "CREATE TABLE IF NOT EXISTS db.t (x UInt64)",
                "CREATE TABLE IF NOT EXISTS db.t",
            ),
            (
                "CREATE MATERIALIZED VIEW mv TO t AS SELECT 1",
                "CREATE MATERIALIZED VIEW mv",
            ),
            ("ALTER TABLE `we ird` DROP COLUMN x", "ALTER TABLE `we ird`"),
            ("DROP TABLE IF EXISTS t", "DROP TABLE IF EXISTS t"),
            ("TRUNCATE TABLE t", "TRUNCATE TABLE t"),
            ("TRUNCATE t", "TRUNCATE t"),
            ("OPTIMIZE TABLE t FINAL", "OPTIMIZE TABLE t"),
        ] {
            let offset = on_cluster_insertion(statement)
                .unwrap_or_else(|| panic!("no insertion for {statement:?}"));
            assert_eq!(&statement[..offset], after, "for {statement:?}");
        }
    }

    #[test]
    fn unconfident_forms_get_no_insertion() {
        for statement in [
            "RENAME TABLE a TO b",
            "EXCHANGE TABLES a AND b",
            "ALTER ROLE r RENAME TO q",
        ] {
            assert_eq!(on_cluster_insertion(statement), None, "{statement:?}");
        }
    }
}

use gpui::{Context, Window};

use crate::{Diagnostic, DiagnosticSeverity, Workspace};

impl Workspace {
    /// Hint diagnostics for hand-written DDL diverging from the
    /// executing scope; empty when the scope is a single node.
    pub(crate) fn cluster_hint_diagnostics(&self, sql: &str) -> Vec<Diagnostic> {
        let Some(cluster) = self.view_scope_cluster() else {
            return Vec::new();
        };
        cluster_scope_hints(sql, &cluster)
            .into_iter()
            .map(|(range, message)| {
                let range = crate::byte_range_to_lsp(sql, range);
                Diagnostic {
                    range: range.start..range.end,
                    severity: DiagnosticSeverity::Hint,
                    source: Some("zeDB scope".into()),
                    message: message.into(),
                    ..Default::default()
                }
            })
            .collect()
    }

    /// The context menu's explicit fix: put ON CLUSTER into the buffer
    /// at the statement under `offset`, visibly, for the user to run.
    pub(crate) fn add_on_cluster_at(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cluster) = self.view_scope_cluster() else {
            self.flash_warning("Pick a cluster under Executing on first", cx);
            return;
        };
        let Some(tab) = self.query.tabs.get(self.query.active_tab) else {
            return;
        };
        let editor = tab.editor.clone();
        let sql = editor.read(cx).value().to_string();
        let Some((start, end)) = split_statements(&sql)
            .into_iter()
            .find(|(start, end)| (*start..=*end).contains(&offset))
        else {
            return;
        };
        let Some(insert_at) = on_cluster_insertion(&sql[start..end]) else {
            self.flash_warning(
                "No confident spot for ON CLUSTER in this statement; add it by hand",
                cx,
            );
            return;
        };
        let clause = format!(" ON CLUSTER `{}`", cluster.replace('`', "``"));
        let mut updated = sql.clone();
        updated.insert_str(start + insert_at, &clause);
        editor.update(cx, |editor, cx| {
            editor.set_value(updated, window, cx);
        });
        self.refresh_schema_diagnostics(cx);
    }
}
