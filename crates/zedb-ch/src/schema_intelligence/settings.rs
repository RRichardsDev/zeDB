//! SETTINGS-clause intelligence: knowing when the cursor sits inside a
//! query's SETTINGS clause, which names it lists, and how a name would
//! layer over the server's and the connection's values.

use std::ops::Range;

use super::tokens::{statement_bounds, tokenize, Token};
use crate::schema_cache::CachedSetting;

/// Where a cursor sits inside a SETTINGS clause.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SettingsCursor {
    /// Typing a setting name (right after SETTINGS or a comma).
    Name,
    /// Typing the value of `setting`, right after its `=`.
    Value { setting: String },
}

/// Clause keywords that end a SETTINGS clause when they follow it
/// (FORMAT is the only thing ClickHouse allows after; UNION starts the
/// next leg).
const CLAUSE_ENDERS: [&str; 3] = ["FORMAT", "UNION", "INTO"];

fn settings_tokens(sql: &str, cursor: usize) -> Option<(Vec<Token<'_>>, usize, usize)> {
    let bounds = statement_bounds(sql, cursor);
    let statement = sql.get(bounds.clone())?;
    let tokens = tokenize(statement);
    let relative = cursor - bounds.start;
    // The most recent SETTINGS keyword before the cursor, unless a
    // clause ender intervenes.
    let mut opened = None;
    for (index, token) in tokens.iter().enumerate() {
        if token.range.start >= relative {
            break;
        }
        if token.text.eq_ignore_ascii_case("SETTINGS") {
            opened = Some(index);
        } else if CLAUSE_ENDERS
            .iter()
            .any(|ender| token.text.eq_ignore_ascii_case(ender))
        {
            opened = None;
        }
    }
    opened.map(|index| (tokens, index, relative))
}

/// The cursor's position inside a SETTINGS clause, or None when the
/// cursor is not in one.
pub(super) fn settings_cursor(sql: &str, cursor: usize) -> Option<SettingsCursor> {
    let (tokens, opened, relative) = settings_tokens(sql, cursor)?;
    // Tokens strictly between SETTINGS and the cursor, excluding the
    // word currently being typed (it ends at or beyond the cursor).
    let between: Vec<&Token<'_>> = tokens
        .iter()
        .skip(opened + 1)
        .take_while(|token| token.range.end < relative || token.range.start >= relative)
        .filter(|token| token.range.end <= relative)
        .collect();
    match between.last() {
        None => Some(SettingsCursor::Name),
        Some(token) if token.text == "," => Some(SettingsCursor::Name),
        Some(token) if token.text == "=" => {
            let setting = between
                .iter()
                .rev()
                .nth(1)
                .filter(|token| token.identifier)?
                .text
                .to_string();
            Some(SettingsCursor::Value { setting })
        }
        // Mid-name (the cursor touches the word being typed): name
        // position when the token before that word says so.
        Some(token) if token.identifier => {
            let before: Vec<&&Token<'_>> = between
                .iter()
                .filter(|candidate| candidate.range.end <= token.range.start)
                .collect();
            match before.last() {
                None => Some(SettingsCursor::Name),
                Some(prior) if prior.text == "," => Some(SettingsCursor::Name),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Every setting-name token in SETTINGS clauses across `sql`:
/// identifiers in name position (after SETTINGS or a comma).
pub(super) fn settings_names(sql: &str) -> Vec<(Range<usize>, String)> {
    let tokens = tokenize(sql);
    let mut names = Vec::new();
    let mut in_clause = false;
    let mut name_position = false;
    for token in &tokens {
        if token.text.eq_ignore_ascii_case("SETTINGS") {
            in_clause = true;
            name_position = true;
            continue;
        }
        if !in_clause {
            continue;
        }
        if token.text == ";"
            || CLAUSE_ENDERS
                .iter()
                .any(|ender| token.text.eq_ignore_ascii_case(ender))
        {
            in_clause = false;
            continue;
        }
        if token.text == "," {
            name_position = true;
            continue;
        }
        if name_position && token.identifier {
            names.push((token.range.clone(), token.text.to_string()));
        }
        name_position = false;
    }
    names
}

/// What a query-level value for this setting would override, split as
/// (layer, value) so callers can style the layer word: the
/// connection's driver setting, the server's changed value, or the
/// ClickHouse default. Hover renders `**Overrides:** _layer_ value`;
/// completion details are plain text.
pub(super) fn override_parts(setting: &CachedSetting) -> (&'static str, String) {
    if let Some(connection) = &setting.connection_value {
        return ("connection setting", connection.clone());
    }
    if setting.changed {
        if setting.default_value.is_empty() || setting.default_value == setting.value {
            return ("server value", setting.value.clone());
        }
        return (
            "server value",
            format!(
                "{} (ClickHouse default {})",
                setting.value, setting.default_value
            ),
        );
    }
    let default = if setting.default_value.is_empty() {
        &setting.value
    } else {
        &setting.default_value
    };
    ("default", default.clone())
}

pub(super) fn override_line(setting: &CachedSetting) -> String {
    let (layer, value) = override_parts(setting);
    format!("{layer} {value}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_intelligence::fixtures::snapshot;
    use crate::schema_intelligence::{analyze_sql, completions, hover, SuggestionKind};

    #[test]
    fn names_complete_with_type_and_override_context() {
        let snapshot = snapshot(None);
        let sql = "select * from analytics.events settings max_";
        let items = completions(&snapshot, None, sql, sql.len());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "max_threads");
        assert_eq!(items[0].kind, SuggestionKind::Setting);
        assert!(
            items[0].detail.contains("Overrides: default 8"),
            "{}",
            items[0].detail
        );

        let sql = "select * from analytics.events settings join_";
        let items = completions(&snapshot, None, sql, sql.len());
        assert!(
            items[0].detail.contains("Overrides: connection setting 1"),
            "the nearest layer wins the wording: {}",
            items[0].detail
        );
    }

    #[test]
    fn values_complete_with_their_layers() {
        let snapshot = snapshot(None);
        let sql = "select 1 settings join_use_nulls = ";
        let items = completions(&snapshot, None, sql, sql.len());
        let labels: Vec<(&str, &str)> = items
            .iter()
            .map(|item| (item.label.as_str(), item.detail.as_str()))
            .collect();
        assert!(labels.contains(&("1", "on")), "{labels:?}");
        assert!(labels.contains(&("0", "off")), "{labels:?}");
    }

    #[test]
    fn hovering_a_setting_shows_its_card() {
        let snapshot = snapshot(None);
        let sql = "select 1 settings max_threads = 4";
        let info = hover(&snapshot, None, sql, sql.find("max_threads").unwrap() + 2).unwrap();
        assert!(info.markdown.contains("**max_threads**"), "{info:?}");
        assert!(info.markdown.contains("**Overrides:** _default_ 8"));
        assert!(
            info.markdown
                .contains("---\n\nMaximum query processing threads"),
            "a rule separates zeDB's metadata from the server's prose: {info:?}"
        );
    }

    #[test]
    fn unknown_settings_squiggle_known_ones_do_not() {
        let snapshot = snapshot(None);
        let issues = analyze_sql(
            &snapshot,
            None,
            "select 1 settings max_threads = 4, max_thread = 4",
        );
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].message.contains("max_thread"), "{issues:?}");
        assert!(issues[0].message.contains("this server"));

        // No catalog (no connection yet): no claims.
        let empty = crate::schema_cache::SchemaSnapshot::default();
        assert!(analyze_sql(&empty, None, "select 1 settings zzz = 1").is_empty());
    }

    #[test]
    fn cursor_positions_inside_the_clause_are_classified() {
        let sql = "select * from t settings max_threads = 4, ";
        assert_eq!(settings_cursor(sql, sql.len()), Some(SettingsCursor::Name));

        let sql = "select * from t settings max_";
        assert_eq!(settings_cursor(sql, sql.len()), Some(SettingsCursor::Name));

        let sql = "select * from t settings max_threads = ";
        assert_eq!(
            settings_cursor(sql, sql.len()),
            Some(SettingsCursor::Value {
                setting: "max_threads".into()
            })
        );

        // Outside the clause: nothing.
        let sql = "select * from t where x = 1";
        assert_eq!(settings_cursor(sql, sql.len()), None);
        let sql = "select * from t settings max_threads = 4 format JSON ";
        assert_eq!(settings_cursor(sql, sql.len()), None);
    }

    #[test]
    fn names_are_collected_per_statement() {
        let sql = "select 1 settings max_threads = 4, join_use_nulls = 1; select 2";
        let names = settings_names(sql);
        let labels: Vec<&str> = names.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(labels, ["max_threads", "join_use_nulls"]);
        assert_eq!(&sql[names[0].0.clone()], "max_threads");
    }

    #[test]
    fn override_lines_name_the_layer_being_overridden() {
        let mut setting = CachedSetting {
            name: "max_threads".into(),
            value: "8".into(),
            default_value: "8".into(),
            ..Default::default()
        };
        assert_eq!(override_line(&setting), "default 8");

        setting.changed = true;
        setting.value = "16".into();
        assert_eq!(
            override_line(&setting),
            "server value 16 (ClickHouse default 8)"
        );

        setting.connection_value = Some("4".into());
        assert_eq!(override_line(&setting), "connection setting 4");
    }
}
