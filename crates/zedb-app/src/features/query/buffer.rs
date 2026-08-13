use std::collections::HashMap;

/// Split `text` into statement byte ranges on top-level semicolons, ignoring
/// semicolons inside strings and comments. Ranges exclude the semicolon and may
/// be blank; always returns at least one range.
pub(crate) fn split_statements(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && quote != b'`' {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b';' => {
                segments.push((start, i));
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    segments.push((start, text.len()));
    segments
}

/// Resolve editor-local `@set name=value` declarations and `${name}` uses.
/// Declarations come from the full editor buffer, while only declarations in
/// the execution target are removed from the SQL sent to ClickHouse.
pub(crate) fn resolve_query_variables(text: &str, editor_text: &str) -> Result<String, String> {
    let mut variables = HashMap::new();
    for (line_index, line) in editor_text.lines().enumerate() {
        let trimmed = line.trim();
        let directive = if trimmed == "@set" {
            Some("")
        } else {
            trimmed
                .strip_prefix("@set")
                .filter(|rest| rest.starts_with(char::is_whitespace))
        };
        let Some(directive) = directive else {
            continue;
        };
        let Some((name, value)) = directive.trim().split_once('=') else {
            return Err(format!(
                "Invalid query variable on line {}: use @set name=value",
                line_index + 1
            ));
        };
        let name = name.trim();
        let value = value.trim();
        let valid_name = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if !valid_name {
            return Err(format!(
                "Invalid query variable name `{name}` on line {}",
                line_index + 1
            ));
        }
        if value.is_empty() {
            return Err(format!(
                "Query variable `{name}` has no value on line {}",
                line_index + 1
            ));
        }
        variables.insert(name.to_string(), value.to_string());
    }

    let mut sql = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim();
        let is_directive = trimmed == "@set"
            || trimmed
                .strip_prefix("@set")
                .is_some_and(|rest| rest.starts_with(char::is_whitespace));
        if is_directive {
            if line.ends_with('\n') {
                sql.push('\n');
            }
            continue;
        }

        let mut remaining = line;
        while let Some(start) = remaining.find("${") {
            sql.push_str(&remaining[..start]);
            let placeholder = &remaining[start + 2..];
            let Some(end) = placeholder.find('}') else {
                return Err("Unclosed query variable placeholder".to_string());
            };
            let name = &placeholder[..end];
            let Some(value) = variables.get(name) else {
                return Err(format!(
                    "Query variable `${{{name}}}` is not set; add @set {name}=value"
                ));
            };
            sql.push_str(value);
            remaining = &placeholder[end + 1..];
        }
        sql.push_str(remaining);
    }
    Ok(sql)
}

/// Find the occurrence of `needle` closest to `cursor`.
pub(crate) fn nearest_occurrence(text: &str, needle: &str, cursor: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    text.match_indices(needle)
        .map(|(start, _)| {
            let end = start + needle.len();
            let distance = if cursor < start {
                start - cursor
            } else {
                cursor.saturating_sub(end)
            };
            (distance, start)
        })
        .min()
        .map(|(_, start)| start)
}

/// Return the statement at `cursor`, falling back to the nearest non-empty
/// statement before it and then after it.
pub(crate) fn statement_at_cursor(text: &str, cursor: usize) -> Option<&str> {
    let segments = split_statements(text);
    let mut idx = segments
        .iter()
        .position(|&(start, end)| cursor >= start && cursor <= end)
        .unwrap_or(segments.len() - 1);
    if idx > 0 && cursor <= text.len() {
        let line_start = text[..cursor].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
        let line_before_cursor = text[line_start..cursor].trim_end();
        if line_before_cursor.ends_with(';') && segments[idx].0 >= line_start {
            idx -= 1;
        }
    }
    let pick = |i: usize| {
        let (start, end) = segments[i];
        let statement = text[start..end.min(text.len())].trim();
        (!statement.is_empty()).then_some(statement)
    };
    pick(idx)
        .or_else(|| (0..idx).rev().find_map(pick))
        .or_else(|| ((idx + 1)..segments.len()).find_map(pick))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_statements_in_order() {
        let text = "SELECT 1;\n-- a; comment\nSELECT ';';\n\nSELECT 3";
        let statements: Vec<&str> = split_statements(text)
            .into_iter()
            .map(|(start, end)| text[start..end].trim())
            .filter(|statement| !statement.is_empty())
            .collect();
        assert_eq!(
            statements,
            vec!["SELECT 1", "-- a; comment\nSELECT ';'", "SELECT 3"]
        );
    }

    #[test]
    fn removes_and_substitutes_query_variables() {
        let text = "@set db=KPARTS\nselect count() from ${db}.ContactsDim";
        assert_eq!(
            resolve_query_variables(text, text).unwrap(),
            "\nselect count() from KPARTS.ContactsDim"
        );
    }

    #[test]
    fn applies_query_variables_to_a_selected_statement() {
        let editor = "@set db=KPARTS\nselect 1;\nselect count() from ${db}.ContactsDim";
        let selected = "select count() from ${db}.ContactsDim";
        assert_eq!(
            resolve_query_variables(selected, editor).unwrap(),
            "select count() from KPARTS.ContactsDim"
        );
    }

    #[test]
    fn reports_invalid_or_missing_query_variables() {
        assert_eq!(
            resolve_query_variables("select ${db}.table", "select ${db}.table").unwrap_err(),
            "Query variable `${db}` is not set; add @set db=value"
        );
        assert_eq!(
            resolve_query_variables("@set db\nselect 1", "@set db\nselect 1").unwrap_err(),
            "Invalid query variable on line 1: use @set name=value"
        );
    }

    #[test]
    fn picks_statement_under_cursor() {
        let text = "SELECT 1;\nSELECT 2;\nSELECT 3";
        assert_eq!(statement_at_cursor(text, 3), Some("SELECT 1"));
        assert_eq!(statement_at_cursor(text, 12), Some("SELECT 2"));
        assert_eq!(statement_at_cursor(text, text.len()), Some("SELECT 3"));
    }

    #[test]
    fn end_of_line_stays_on_that_statement() {
        let text = "DESCRIBE sat.arrayValues;\ndescribe sat.complexTypes;";
        let after_first_semicolon = text.find(';').unwrap() + 1;
        assert_eq!(
            statement_at_cursor(text, after_first_semicolon),
            Some("DESCRIBE sat.arrayValues")
        );
        let text = "SELECT 1;  \nSELECT 2;";
        assert_eq!(statement_at_cursor(text, 10), Some("SELECT 1"));
        assert_eq!(statement_at_cursor(text, 11), Some("SELECT 1"));
        assert_eq!(statement_at_cursor(text, 12), Some("SELECT 2"));
        let text = "SELECT 1; SELECT 2;";
        assert_eq!(statement_at_cursor(text, 14), Some("SELECT 2"));
    }

    #[test]
    fn handles_single_or_empty_statement_buffers() {
        assert_eq!(statement_at_cursor("SELECT 1", 4), Some("SELECT 1"));
        assert_eq!(statement_at_cursor("", 0), None);
        assert_eq!(statement_at_cursor("  \n ; ; ", 3), None);
    }

    #[test]
    fn ignores_semicolons_in_strings_and_comments() {
        let text = "SELECT ';' AS a; -- trailing; comment\nSELECT /* not; here */ 2";
        assert_eq!(statement_at_cursor(text, 4), Some("SELECT ';' AS a"));
        assert_eq!(
            statement_at_cursor(text, text.len()),
            Some("-- trailing; comment\nSELECT /* not; here */ 2")
        );
    }

    #[test]
    fn falls_back_from_blank_space_to_the_nearest_statement() {
        let text = "SELECT 1;\n\n  \nSELECT 2;\n\n";
        assert_eq!(statement_at_cursor(text, text.len()), Some("SELECT 2"));
        assert_eq!(statement_at_cursor(";\nSELECT 9", 0), Some("SELECT 9"));
    }
}
