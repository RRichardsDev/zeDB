//! Byte-wise SQL tokenizer and the cursor-position helpers built on it.
//!
//! Everything here is shared by the analysis, completion, hover, and clause
//! rewriting modules, so the token shape is `pub(super)` rather than private.

use std::ops::Range;

#[derive(Debug, Clone)]
pub(super) struct Token<'a> {
    pub(super) text: &'a str,
    pub(super) range: Range<usize>,
    pub(super) identifier: bool,
}

pub(super) fn starts_with_case_insensitive(value: &str, prefix: &str) -> bool {
    value.len() >= prefix.len() && value[..prefix.len()].eq_ignore_ascii_case(prefix)
}

pub(super) fn word_range(text: &str, offset: usize) -> Range<usize> {
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
pub(super) fn current_statement(sql: &str, cursor: usize) -> &str {
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
pub(super) fn char_len(sql: &str, index: usize) -> usize {
    sql[index..].chars().next().map_or(1, |c| c.len_utf8())
}

pub(super) fn tokenize(sql: &str) -> Vec<Token<'_>> {
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

pub(super) fn is_clause_keyword(value: &str) -> bool {
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
    use super::*;
    use crate::schema_intelligence::{column_filters, top_level_order_by};

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
}
