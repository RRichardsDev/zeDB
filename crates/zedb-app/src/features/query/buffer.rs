/// The end of an `@set` directive line starting at `line_start`, or None
/// when the line is not a directive. Matches the detection in
/// `resolve_query_variables`: `@set` alone or followed by whitespace.
fn directive_line_end(bytes: &[u8], line_start: usize) -> Option<usize> {
    let mut i = line_start;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if !bytes[i..].starts_with(b"@set") {
        return None;
    }
    let after = i + 4;
    if after < bytes.len() && !bytes[after].is_ascii_whitespace() {
        return None;
    }
    let mut end = after;
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    Some(end)
}

use std::ops::Range;

/// Split `text` into statement byte ranges on top-level semicolons, ignoring
/// semicolons inside strings and comments. An `@set` directive line is its
/// own segment: it never rides along with the SQL below it, so running the
/// cursor's statement on a directive does not execute the next query.
/// Ranges exclude the semicolon and may be blank; always returns at least
/// one range.
pub(crate) fn split_statements(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut line_begin = true;
    while i < bytes.len() {
        if line_begin {
            line_begin = false;
            if let Some(end) = directive_line_end(bytes, i) {
                if i > start {
                    segments.push((start, i));
                }
                segments.push((i, end));
                i = (end + 1).min(bytes.len());
                start = i;
                line_begin = true;
                continue;
            }
        }
        if bytes[i] == b'\n' {
            line_begin = true;
            i += 1;
            continue;
        }
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

/// True when `sql` holds no runnable content: only whitespace and comments.
/// Such text must never be sent as a statement (the server rejects it) nor
/// treated as the statement under the cursor.
pub(crate) fn sql_is_blank(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
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
            byte if byte.is_ascii_whitespace() => i += 1,
            _ => return false,
        }
    }
    true
}

/// Byte ranges of `--` and `/* */` comments in `text`, skipping quoted
/// strings so a `--` inside a literal is not mistaken for a comment.
fn comment_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
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
                let start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                spans.push((start, i));
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                spans.push((start, i));
            }
            _ => i += 1,
        }
    }
    spans
}

/// `SET param_<name> = <value>` statements with their buffer positions.
/// Each statement in the app runs as its own stateless HTTP request, so a
/// SET's session state would evaporate immediately; instead these are
/// collected like `@set` declarations and shipped as `param_<name>` URL
/// parameters with every statement they govern.
pub(crate) fn collect_param_declarations(text: &str) -> Vec<(usize, String, String)> {
    let mut declarations = Vec::new();
    for (start, end) in split_statements(text) {
        let raw = &text[start..end.min(text.len())];
        let statement = raw.trim();
        let offset = start + (raw.len() - raw.trim_start().len());
        let Some(rest) = statement
            .get(..3)
            .filter(|head| head.eq_ignore_ascii_case("set"))
            .map(|_| &statement[3..])
        else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(rest) = rest.trim_start().strip_prefix("param_") else {
            continue;
        };
        let name_len = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if name_len == 0 {
            continue;
        }
        let (name, rest) = rest.split_at(name_len);
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        declarations.push((offset, name.to_string(), unquote_literal(value.trim())));
    }
    declarations
}

/// Strip one level of single quotes and unescape (`\x` and `''`); values
/// that are not quoted literals (numbers, arrays) pass through verbatim,
/// which is what the HTTP `param_` form expects.
fn unquote_literal(value: &str) -> String {
    let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    else {
        return value.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            }
            '\'' => {
                out.push('\'');
                chars.next();
            }
            other => out.push(other),
        }
    }
    out
}

/// The query parameters in effect for a statement at `offset`: for each
/// name, the nearest declaration above it, falling back to the first one
/// below (mirroring `@set` scoping). No offset means the whole-buffer
/// view: the last declaration of each name.
pub(crate) fn params_at(
    declarations: &[(usize, String, String)],
    offset: Option<usize>,
) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();
    for (_, name, _) in declarations {
        if result.iter().any(|(seen, _)| seen == name) {
            continue;
        }
        let above = declarations
            .iter()
            .rfind(|(at, declared, _)| declared == name && offset.is_none_or(|o| *at <= o));
        let below = declarations
            .iter()
            .find(|(at, declared, _)| declared == name && offset.is_some_and(|o| *at > o));
        if let Some((_, _, value)) = above.or(below) {
            result.push((name.clone(), value.clone()));
        }
    }
    result
}

/// ClickHouse's Values parser rejects comments between the rows of an
/// `INSERT ... VALUES` data section (verified on 25.8: "expected '('
/// before '-- comment'"), so annotated inserts that every general SQL
/// tool accepts die on the server. Strip comments from the data section
/// client-side before sending; everything up to and including the
/// VALUES keyword, and all string literals, pass through untouched.
pub(crate) fn strip_insert_values_comments(sql: &str) -> std::borrow::Cow<'_, str> {
    let bytes = sql.as_bytes();

    // Only INSERT statements have a Values data section. Leading
    // whitespace and comments may precede the keyword.
    let mut i = 0;
    loop {
        match bytes.get(i) {
            Some(b'-') if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            Some(b'/') if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            Some(byte) if byte.is_ascii_whitespace() => i += 1,
            _ => break,
        }
    }
    let insert = sql[i..]
        .get(..6)
        .is_some_and(|word| word.eq_ignore_ascii_case("insert"));
    if !insert {
        return std::borrow::Cow::Borrowed(sql);
    }

    // Find the top-level VALUES keyword: outside parens, strings, and
    // comments.
    let mut depth = 0usize;
    let mut data_start = None;
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
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if depth == 0 && sql[start..i].eq_ignore_ascii_case("values") {
                    data_start = Some(i);
                    break;
                }
            }
            _ => i += 1,
        }
    }
    let Some(data_start) = data_start else {
        return std::borrow::Cow::Borrowed(sql);
    };

    // Copy the data section with comments removed, strings verbatim.
    let mut out = String::with_capacity(sql.len());
    out.push_str(&sql[..data_start]);
    let mut segment = data_start;
    let mut changed = false;
    let mut i = data_start;
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
                out.push_str(&sql[segment..i]);
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                segment = i;
                changed = true;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(&sql[segment..i]);
                out.push(' ');
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                segment = i;
                changed = true;
            }
            _ => i += 1,
        }
    }
    if !changed {
        return std::borrow::Cow::Borrowed(sql);
    }
    out.push_str(&sql[segment..]);
    std::borrow::Cow::Owned(out)
}

/// `@set name=value` declarations with their buffer positions.
/// Declarations keep their position: a redeclared name takes effect
/// from its own line down, so each `${use}` binds to the nearest
/// `@set` above it rather than the last one anywhere.
pub(crate) fn collect_variable_declarations(
    editor_text: &str,
) -> Result<Vec<(usize, String, String)>, String> {
    let mut declarations: Vec<(usize, String, String)> = Vec::new();
    let mut declaration_offset = 0;
    for (line_index, line) in editor_text.split_inclusive('\n').enumerate() {
        let line_offset = declaration_offset;
        declaration_offset += line.len();
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
        // A trailing semicolon terminates the directive, it is not part
        // of the value; without this there is no way to end an @set line
        // like a statement.
        let value = value.trim();
        let value = value.strip_suffix(';').map_or(value, str::trim_end);
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
        declarations.push((line_offset, name.to_string(), value.to_string()));
    }
    Ok(declarations)
}

/// Resolve editor-local `@set name=value` declarations and `${name}` uses.
/// Declarations come from the full editor buffer, while only declarations in
/// the execution target are removed from the SQL sent to ClickHouse.
pub(crate) fn resolve_query_variables(text: &str, editor_text: &str) -> Result<String, String> {
    let declarations = collect_variable_declarations(editor_text)?;

    // Where the execution target sits in the buffer, to compare use
    // positions against declaration positions. A selection that cannot
    // be located falls back to the last declaration of each name.
    let text_start = if text == editor_text {
        Some(0)
    } else {
        editor_text.find(text)
    };
    let lookup = |name: &str, use_offset: usize| -> Option<&str> {
        fn value(declaration: &(usize, String, String)) -> &str {
            declaration.2.as_str()
        }
        let mut matching = declarations
            .iter()
            .filter(|(_, declared, _)| declared == name);
        let Some(editor_offset) = text_start.map(|start| start + use_offset) else {
            return matching.next_back().map(value);
        };
        let mut nearest_above = None;
        let mut first_below = None;
        for declaration in matching.by_ref() {
            if declaration.0 <= editor_offset {
                nearest_above = Some(declaration);
            } else if first_below.is_none() {
                first_below = Some(declaration);
            }
        }
        // A lone declaration below the use still applies, as it always
        // has; only a redeclaration makes position decide.
        nearest_above.or(first_below).map(value)
    };

    // Placeholders inside comments are prose, not uses: substituting (or
    // erroring on) them would break a query over its own annotations.
    let comments = comment_spans(text);
    let mut sql = String::with_capacity(text.len());
    let mut line_start = 0;
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
            line_start += line.len();
            continue;
        }

        let mut remaining = line;
        while let Some(start) = remaining.find("${") {
            let global = line_start + (line.len() - remaining.len()) + start;
            if comments
                .iter()
                .any(|&(from, to)| global >= from && global < to)
            {
                sql.push_str(&remaining[..start + 2]);
                remaining = &remaining[start + 2..];
                continue;
            }
            sql.push_str(&remaining[..start]);
            let placeholder = &remaining[start + 2..];
            let Some(end) = placeholder.find('}') else {
                return Err("Unclosed query variable placeholder".to_string());
            };
            let name = &placeholder[..end];
            let Some(value) = lookup(name, global) else {
                return Err(format!(
                    "Query variable `${{{name}}}` is not set; add @set {name}=value"
                ));
            };
            sql.push_str(value);
            remaining = &placeholder[end + 1..];
        }
        sql.push_str(remaining);
        line_start += line.len();
    }
    Ok(sql)
}

/// Hover for variable and parameter placeholders: `${db}` shows the
/// `@set` value in effect at that position, `{db:Identifier}` shows the
/// `SET param_db` value. Returns the markdown, the resolved value (for
/// schema enrichment by the caller), and the placeholder span; None
/// when the offset is not on a placeholder.
pub(crate) fn variable_hover(
    sql: &str,
    offset: usize,
) -> Option<(String, Option<String>, Range<usize>)> {
    let offset = offset.min(sql.len());
    let line_start = sql[..offset].rfind('\n').map(|at| at + 1).unwrap_or(0);
    let line_end = sql[offset..]
        .find('\n')
        .map(|at| offset + at)
        .unwrap_or(sql.len());
    let line = &sql[line_start..line_end];

    let line_of = |at: usize| sql[..at].bytes().filter(|byte| *byte == b'\n').count() + 1;

    // `${name}` spans on the hovered line.
    let mut search = 0;
    while let Some(found) = line[search..].find("${") {
        let open = line_start + search + found;
        search += found + 2;
        let Some(close) = sql[open + 2..line_end].find('}').map(|at| open + 2 + at) else {
            continue;
        };
        let span = open..close + 1;
        if offset < span.start || offset > span.end {
            continue;
        }
        let name = &sql[open + 2..close];
        let declarations = collect_variable_declarations(sql).ok()?;
        let (markdown, value) = match params_at(&declarations, Some(open))
            .into_iter()
            .find(|(declared, _)| declared == name)
        {
            Some((_, value)) => {
                let at = declarations
                    .iter()
                    .filter(|(_, declared, _)| declared == name)
                    .rfind(|(declared_at, _, _)| *declared_at <= open)
                    .or_else(|| {
                        declarations
                            .iter()
                            .find(|(_, declared, _)| declared == name)
                    })
                    .map(|(declared_at, _, _)| *declared_at)?;
                (
                    // The tooltip renders \n\n as a plain line break; the
                    // non-breaking-space paragraph is the blank line.
                    format!(
                        "`{name}` from `@set` *(line {})*\n\n\u{a0}\n\n**{value}**",
                        line_of(at)
                    ),
                    Some(value),
                )
            }
            None => (
                format!("`{name}` is not set; add `@set {name}=value`"),
                None,
            ),
        };
        return Some((markdown, value, span));
    }

    // `{name:Type}` spans on the hovered line.
    let mut search = 0;
    while let Some(found) = line[search..].find('{') {
        let open = line_start + search + found;
        search += found + 1;
        let Some(close) = sql[open + 1..line_end].find('}').map(|at| open + 1 + at) else {
            continue;
        };
        let inner = &sql[open + 1..close];
        let Some((name, type_name)) = inner.split_once(':') else {
            continue;
        };
        let valid_name = !name.is_empty()
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if !valid_name || type_name.is_empty() {
            continue;
        }
        let span = open..close + 1;
        if offset < span.start || offset > span.end {
            continue;
        }
        let declarations = collect_param_declarations(sql);
        let (markdown, value) = match params_at(&declarations, Some(open))
            .into_iter()
            .find(|(declared, _)| declared == name)
        {
            Some((_, value)) => {
                let at = declarations
                    .iter()
                    .filter(|(_, declared, _)| declared == name)
                    .rfind(|(declared_at, _, _)| *declared_at <= open)
                    .or_else(|| {
                        declarations
                            .iter()
                            .find(|(_, declared, _)| declared == name)
                    })
                    .map(|(declared_at, _, _)| *declared_at)?;
                (
                    format!(
                        "`{name}` from `SET param_{name}` *(line {})*\n\n\u{a0}\n\n**{value}**",
                        line_of(at)
                    ),
                    Some(value),
                )
            }
            None => (
                format!("`{name}` is not set; add `SET param_{name} = value`"),
                None,
            ),
        };
        return Some((markdown, value, span));
    }
    None
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
    // A cursor past a statement's terminating semicolon but still on its
    // line means that statement: whether the tail is spaces or a trailing
    // `-- comment`, nothing runnable has started yet.
    if idx > 0 && cursor <= text.len() {
        let after_semicolon = &text[segments[idx].0..cursor];
        if !after_semicolon.contains('\n') && sql_is_blank(after_semicolon) {
            idx -= 1;
        }
    }
    let pick = |i: usize| {
        let (start, end) = segments[i];
        let statement = text[start..end.min(text.len())].trim();
        (!sql_is_blank(statement)).then_some(statement)
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
    fn a_trailing_line_comment_stays_with_its_statement() {
        // The comment after the semicolon belongs to SELECT 1; a cursor
        // inside it must not run SELECT 2.
        let text = "SELECT 1; -- note\nSELECT 2";
        let in_comment = text.find("note").unwrap();
        assert_eq!(statement_at_cursor(text, in_comment), Some("SELECT 1"));
        // A comment on its own line heads the next statement (and ships
        // with it, which the server accepts).
        let text = "SELECT 1;\n-- about the next\nSELECT 2";
        let in_comment = text.find("about").unwrap();
        assert_eq!(
            statement_at_cursor(text, in_comment),
            Some("-- about the next\nSELECT 2")
        );
    }

    #[test]
    fn comment_only_segments_are_not_statements() {
        let text = "SELECT 1;\n-- scratch notes";
        assert_eq!(statement_at_cursor(text, text.len()), Some("SELECT 1"));
        assert!(sql_is_blank("-- a\n/* b */\n  "));
        assert!(!sql_is_blank("-- a\nSELECT 1"));
    }

    #[test]
    fn hovering_placeholders_shows_the_value_in_effect() {
        // ${} placeholder: nearest @set above wins, shown with its line.
        // Paragraph breaks (\n\n), not soft breaks: the tooltip renderer
        // ignores trailing-space line breaks and runs lines together.
        let sql = "@set db=xyz\nselect '${db}';\n@set db=abc\nselect '${db}'";
        let first = sql.find("${db}").unwrap() + 2;
        let (markdown, value, span) = variable_hover(sql, first).unwrap();
        assert_eq!(markdown, "`db` from `@set` *(line 1)*\n\n\u{a0}\n\n**xyz**");
        assert_eq!(value.as_deref(), Some("xyz"));
        assert_eq!(&sql[span], "${db}");
        let second = sql.rfind("${db}").unwrap() + 2;
        let (markdown, _, _) = variable_hover(sql, second).unwrap();
        assert!(markdown.contains("**abc**"), "{markdown}");

        // Unset variable says so.
        let sql = "select '${missing}'";
        let offset = sql.find("missing").unwrap();
        let (markdown, value, _) = variable_hover(sql, offset).unwrap();
        assert!(markdown.contains("not set"), "{markdown}");
        assert!(value.is_none());

        // Native param placeholder resolves through SET param_.
        let sql = "SET param_db = 'KPARTS';\nSELECT count() FROM {db:Identifier}.t";
        let offset = sql.find("{db:").unwrap() + 1;
        let (markdown, value, span) = variable_hover(sql, offset).unwrap();
        assert_eq!(
            markdown,
            "`db` from `SET param_db` *(line 1)*\n\n\u{a0}\n\n**KPARTS**"
        );
        assert_eq!(value.as_deref(), Some("KPARTS"));
        assert_eq!(&sql[span], "{db:Identifier}");

        // Plain SQL is not a placeholder.
        assert!(variable_hover("select count() from t", 9).is_none());
    }

    #[test]
    fn collects_native_param_declarations_with_positions() {
        let text =
            "SET param_db = 'KPARTS';\nselect {db:Identifier};\nSET param_db = 'OTHER';\nSET param_n = 42;";
        let declarations = collect_param_declarations(text);
        assert_eq!(declarations.len(), 3);
        assert_eq!(declarations[0].1, "db");
        assert_eq!(declarations[0].2, "KPARTS");
        assert_eq!(declarations[1].2, "OTHER");
        assert_eq!(declarations[2].1, "n");
        assert_eq!(declarations[2].2, "42");
        // Uses take the nearest declaration above; whole-buffer view
        // takes the last.
        let first_use = text.find("select").unwrap();
        // db binds to the declaration above; n only exists below, so it
        // falls forward (a lone declaration anywhere applies, like @set).
        assert_eq!(
            params_at(&declarations, Some(first_use)),
            vec![
                ("db".to_string(), "KPARTS".to_string()),
                ("n".to_string(), "42".to_string())
            ]
        );
        assert_eq!(
            params_at(&declarations, None),
            vec![
                ("db".to_string(), "OTHER".to_string()),
                ("n".to_string(), "42".to_string())
            ]
        );
        // Plain SET of a server setting is not a query parameter.
        assert!(collect_param_declarations("SET max_threads = 4").is_empty());
        // Escapes unwrap.
        let quoted = collect_param_declarations(r"SET param_s = 'it\'s'");
        assert_eq!(quoted[0].2, "it's");
    }

    #[test]
    fn a_set_directive_is_its_own_statement() {
        // No semicolon needed: the directive line never rides along with
        // the SQL below it.
        let text = "@set db=KPARTS\n\nSELECT count() from ${db}.ContactsDim;";
        let statements: Vec<&str> = split_statements(text)
            .into_iter()
            .map(|(start, end)| text[start..end].trim())
            .filter(|statement| !statement.is_empty())
            .collect();
        assert_eq!(
            statements,
            vec!["@set db=KPARTS", "SELECT count() from ${db}.ContactsDim"]
        );
        // Cursor on the directive line stays on the directive.
        assert_eq!(statement_at_cursor(text, 4), Some("@set db=KPARTS"));
        // Cursor on the query gets only the query.
        let in_query = text.find("count").unwrap();
        assert_eq!(
            statement_at_cursor(text, in_query),
            Some("SELECT count() from ${db}.ContactsDim")
        );
    }

    #[test]
    fn a_set_value_may_end_with_a_semicolon() {
        let text = "@set db=KPARTS;\nselect '${db}'";
        assert_eq!(
            resolve_query_variables(text, text).unwrap(),
            "\nselect 'KPARTS'"
        );
    }

    #[test]
    fn insert_values_comments_are_stripped_for_the_server() {
        let sql = "INSERT INTO db.t (a, b) VALUES\n-- first row\n('x -- not a comment', 1),\n -- another row\n('y', 2)";
        let stripped = strip_insert_values_comments(sql);
        assert_eq!(
            stripped.as_ref(),
            "INSERT INTO db.t (a, b) VALUES\n\n('x -- not a comment', 1),\n \n('y', 2)"
        );
        // Statements without a Values data section pass through borrowed.
        assert!(matches!(
            strip_insert_values_comments("SELECT 1 -- note"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            strip_insert_values_comments("INSERT INTO t SELECT a FROM s"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_redeclared_variable_binds_each_use_to_the_nearest_set_above() {
        let editor = "@set db=xyz\nselect 1, '${db}';\n@set db=abc\nselect 2, '${db}'";
        // The whole buffer: each use takes the declaration above it.
        assert_eq!(
            resolve_query_variables(editor, editor).unwrap(),
            "\nselect 1, 'xyz';\n\nselect 2, 'abc'"
        );
        // A single statement resolves at its own position.
        assert_eq!(
            resolve_query_variables("select 1, '${db}';", editor).unwrap(),
            "select 1, 'xyz';"
        );
        assert_eq!(
            resolve_query_variables("select 2, '${db}'", editor).unwrap(),
            "select 2, 'abc'"
        );
    }

    #[test]
    fn placeholders_inside_comments_are_left_alone() {
        let text = "SELECT 1 -- uses ${db} later\n/* also ${db} */";
        assert_eq!(resolve_query_variables(text, text).unwrap(), text);
        let text = "@set db=KPARTS\nSELECT * FROM ${db}.t -- not ${prose}";
        assert_eq!(
            resolve_query_variables(text, text).unwrap(),
            "\nSELECT * FROM KPARTS.t -- not ${prose}"
        );
    }

    #[test]
    fn falls_back_from_blank_space_to_the_nearest_statement() {
        let text = "SELECT 1;\n\n  \nSELECT 2;\n\n";
        assert_eq!(statement_at_cursor(text, text.len()), Some("SELECT 2"));
        assert_eq!(statement_at_cursor(";\nSELECT 9", 0), Some("SELECT 9"));
    }
}
