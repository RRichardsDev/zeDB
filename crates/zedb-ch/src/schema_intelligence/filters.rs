use super::order_by::starts_with_clause;
use super::*;

/// The byte span of the top-level WHERE clause content (after the
/// keyword, through the end of its predicate), plus where a new clause
/// would be inserted when none exists.
fn top_level_where_span(sql: &str) -> (Option<(usize, usize, usize)>, Option<usize>) {
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
            if clause.is_none() && upper == "WHERE" {
                let keyword_start = token.range.start;
                let content_start = token.range.end;
                let mut inner_depth = 0i32;
                let mut end = sql.len();
                let mut cursor = index + 1;
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
                                "GROUP"
                                    | "HAVING"
                                    | "WINDOW"
                                    | "ORDER"
                                    | "LIMIT"
                                    | "OFFSET"
                                    | "SETTINGS"
                                    | "FORMAT"
                                    | "INTO"
                                    | "UNION"
                            )
                        {
                            end = next.range.start;
                            break;
                        }
                    }
                    cursor += 1;
                }
                clause = Some((keyword_start, content_start, end));
            }
            if insert_at.is_none()
                && (token.text == ";"
                    || matches!(
                        upper.as_str(),
                        "GROUP"
                            | "HAVING"
                            | "WINDOW"
                            | "ORDER"
                            | "LIMIT"
                            | "OFFSET"
                            | "SETTINGS"
                            | "FORMAT"
                            | "INTO"
                    ))
            {
                insert_at = Some(token.range.start);
            }
        }
        index += 1;
    }
    (clause, insert_at)
}

/// Split a WHERE body into top-level AND conjuncts. A top-level OR
/// makes the whole body one opaque conjunct (parenthesised on rebuild).
fn split_conjuncts(content: &str) -> Vec<String> {
    let tokens = tokenize(content);
    let mut depth = 0i32;
    let mut boundaries = Vec::new();
    for token in &tokens {
        match token.text {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 && token.text.eq_ignore_ascii_case("or") {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return Vec::new();
            }
            return vec![format!("({trimmed})")];
        }
        if depth == 0 && token.text.eq_ignore_ascii_case("and") {
            boundaries.push((token.range.start, token.range.end));
        }
    }
    let mut parts = Vec::new();
    let mut start = 0;
    for (boundary_start, boundary_end) in boundaries {
        parts.push(content[start..boundary_start].trim().to_string());
        start = boundary_end;
    }
    parts.push(content[start..].trim().to_string());
    parts.retain(|part| !part.is_empty());
    parts
}

/// The column a conjunct is about: a backticked name (ours) or a bare
/// (possibly dotted) identifier followed by anything but a call. None
/// for parenthesised or computed conjuncts.
fn conjunct_column(conjunct: &str) -> Option<String> {
    let trimmed = conjunct.trim();
    // A parenthesised group attributes to a column only when every
    // OR'd part inside is about that same column (e.g. our
    // `IN (...) OR IS NULL` unions). Mixed groups stay opaque.
    if let Some(inner) = strip_matching_parens(trimmed) {
        let tokens = tokenize(inner);
        let mut depth = 0i32;
        let mut parts = Vec::new();
        let mut start = 0;
        for token in &tokens {
            match token.text {
                "(" => depth += 1,
                ")" => depth -= 1,
                _ => {}
            }
            if depth == 0 && token.text.eq_ignore_ascii_case("or") {
                parts.push(&inner[start..token.range.start]);
                start = token.range.end;
            }
        }
        parts.push(&inner[start..]);
        let first = conjunct_column(parts.first()?)?;
        for part in &parts[1..] {
            if conjunct_column(part)? != first {
                return None;
            }
        }
        return Some(first);
    }
    if let Some(inner) = trimmed.strip_prefix('`') {
        let close = inner.find('`')?;
        return Some(inner[..close].to_string());
    }
    let tokens = tokenize(trimmed);
    let mut index = 0;
    let mut last: Option<String> = None;
    while let Some(token) = tokens.get(index) {
        if !token.identifier {
            break;
        }
        last = Some(token.text.to_string());
        if tokens.get(index + 1).map(|next| next.text) == Some(".") {
            index += 2;
        } else {
            index += 1;
            break;
        }
    }
    let last = last?;
    if tokens.get(index).map(|next| next.text) == Some("(") {
        return None;
    }
    if matches!(
        last.to_ascii_uppercase().as_str(),
        "NOT" | "EXISTS" | "TRUE" | "FALSE" | "CASE" | "INTERVAL"
    ) {
        return None;
    }
    Some(last)
}

/// Replace, add, or remove this column's managed filter conjunct in the
/// top-level WHERE. `predicate` is the full conjunct (backtick-quoted
/// column first); hand-written conjuncts survive untouched.
pub fn set_column_filter(sql: &str, column: &str, predicate: Option<&str>) -> String {
    let (clause, insert_at) = top_level_where_span(sql);
    let mut conjuncts = match clause {
        Some((_, content_start, end)) => split_conjuncts(&sql[content_start..end]),
        None => Vec::new(),
    };
    conjuncts.retain(|conjunct| conjunct_column(conjunct).as_deref() != Some(column));
    if let Some(predicate) = predicate {
        conjuncts.push(predicate.to_string());
    }
    let new_clause = (!conjuncts.is_empty()).then(|| format!("WHERE {}", conjuncts.join(" AND ")));

    let join = |head: &str, middle: Option<&str>, rest: &str| {
        let mut out = head.trim_end().to_string();
        if let Some(middle) = middle {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(middle);
        }
        let rest = rest.trim_start();
        if !rest.is_empty() {
            if rest.starts_with(';') {
            } else if starts_with_clause(rest) {
                out.push('\n');
            } else {
                out.push(' ');
            }
            out.push_str(rest);
        }
        out
    };

    match clause {
        Some((keyword_start, _, end)) => {
            join(&sql[..keyword_start], new_clause.as_deref(), &sql[end..])
        }
        None => match new_clause {
            Some(new_clause) => match insert_at {
                Some(position) => join(&sql[..position], Some(&new_clause), &sql[position..]),
                None => join(sql, Some(&new_clause), ""),
            },
            None => sql.to_string(),
        },
    }
}

/// This column's filter conjunct (ours or hand-written), if any.
pub fn column_filter(sql: &str, column: &str) -> Option<String> {
    let (clause, _) = top_level_where_span(sql);
    let (_, content_start, end) = clause?;
    split_conjuncts(&sql[content_start..end])
        .into_iter()
        .find(|conjunct| conjunct_column(conjunct).as_deref() == Some(column))
}

/// The inside of a fully parenthesised expression, when the opening
/// paren closes exactly at the end.
fn strip_matching_parens(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('(')?;
    let mut depth = 1i32;
    for (index, character) in inner.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (index == inner.len() - 1).then(|| &inner[..index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Column-attributable top-level WHERE conjuncts, ours or
/// hand-written, as (column, conjunct) pairs for the header UI.
pub fn column_filters(sql: &str) -> Vec<(String, String)> {
    let Some((_, content_start, end)) = top_level_where_span(sql).0 else {
        return Vec::new();
    };
    split_conjuncts(&sql[content_start..end])
        .into_iter()
        .filter_map(|conjunct| conjunct_column(&conjunct).map(|column| (column, conjunct)))
        .collect()
}

/// Columns the top-level WHERE filters on.
pub fn filtered_columns(sql: &str) -> Vec<String> {
    column_filters(sql)
        .into_iter()
        .map(|(column, _)| column)
        .collect()
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    #[test]
    fn adds_replaces_and_removes_managed_filters() {
        let sql = "SELECT * FROM t ORDER BY id LIMIT 5";
        let filtered = set_column_filter(sql, "kind", Some("`kind` IN ('a', 'b')"));
        assert_eq!(
            filtered,
            "SELECT * FROM t\nWHERE `kind` IN ('a', 'b')\nORDER BY id LIMIT 5"
        );

        let replaced = set_column_filter(&filtered, "kind", Some("`kind` = 'c'"));
        assert_eq!(
            replaced,
            "SELECT * FROM t\nWHERE `kind` = 'c'\nORDER BY id LIMIT 5"
        );
        assert_eq!(
            column_filter(&replaced, "kind").as_deref(),
            Some("`kind` = 'c'")
        );
        assert_eq!(filtered_columns(&replaced), vec!["kind".to_string()]);

        assert_eq!(
            set_column_filter(&replaced, "kind", None),
            "SELECT * FROM t\nORDER BY id LIMIT 5"
        );
    }

    #[test]
    fn preserves_hand_written_conjuncts_and_or_bodies() {
        let sql = "SELECT * FROM t WHERE day > '2026-01-01' AND kind != 'x'";
        assert_eq!(
            filtered_columns(sql),
            vec!["day".to_string(), "kind".to_string()]
        );
        assert_eq!(column_filter(sql, "kind").as_deref(), Some("kind != 'x'"));

        // A hand-written conjunct about the same column is replaced;
        // conjuncts about other columns survive.
        let filtered = set_column_filter(sql, "kind", Some("`kind` = 'a'"));
        assert!(filtered.contains("day > '2026-01-01' AND `kind` = 'a'"));
        assert!(!filtered.contains("kind != 'x'"));

        let or_sql = "SELECT * FROM t WHERE a = 1 OR b = 2";
        let wrapped = set_column_filter(or_sql, "kind", Some("`kind` = 'a'"));
        assert!(wrapped.contains("WHERE (a = 1 OR b = 2) AND `kind` = 'a'"));
    }

    #[test]
    fn null_union_groups_attribute_to_their_column() {
        let sql = "SELECT * FROM t WHERE (`kind` IN ('a') OR `kind` IS NULL) AND day > 1";
        assert_eq!(
            filtered_columns(sql),
            vec!["kind".to_string(), "day".to_string()]
        );
        let replaced = set_column_filter(sql, "kind", Some("`kind` = 'b'"));
        assert!(replaced.contains("WHERE day > 1 AND `kind` = 'b'"));

        // A mixed OR group stays opaque and survives.
        let mixed = "SELECT * FROM t WHERE (a = 1 OR b = 2)";
        assert!(filtered_columns(mixed).is_empty());
    }
}
