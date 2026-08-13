use super::tokens::tokenize;

/// Whether trailing statement text begins with a top-level clause
/// keyword that reads best on its own line.
pub(super) fn starts_with_clause(rest: &str) -> bool {
    let word: String = rest
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .collect();
    matches!(
        word.to_ascii_uppercase().as_str(),
        "WHERE"
            | "GROUP"
            | "HAVING"
            | "WINDOW"
            | "ORDER"
            | "LIMIT"
            | "OFFSET"
            | "SETTINGS"
            | "FORMAT"
            | "UNION"
            | "INTO"
    )
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

/// Whether the statement has a top-level `GROUP BY` (an aggregation over
/// the whole scan), ignoring any inside subqueries or function calls.
/// Distinguishes a real rollup from a global aggregate like
/// `SELECT count(*)`, so the advisor only suggests a projection when there
/// is something to pre-aggregate.
pub fn has_group_by(sql: &str) -> bool {
    let tokens = tokenize(sql);
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate() {
        match token.text {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ if depth == 0
                && token.text.eq_ignore_ascii_case("GROUP")
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| next.text.eq_ignore_ascii_case("BY")) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Build an aggregate projection from a grouped `SELECT`: its projection
/// body (`SELECT <list> GROUP BY <cols>`, with FROM/WHERE/ORDER/LIMIT
/// dropped) and a derived projection name. Deterministic, no model. Returns
/// `None` when the statement isn't a plain `SELECT ... FROM ... GROUP BY`
/// (a CTE, a missing clause, etc.) so the advisor stays honest.
pub fn aggregate_projection(sql: &str) -> Option<(String, String)> {
    let tokens = tokenize(sql);
    // Must start with SELECT; WITH (CTE) is out of scope for a mechanical
    // projection body.
    let first = tokens.iter().find(|token| !token.text.trim().is_empty())?;
    if !first.text.eq_ignore_ascii_case("SELECT") {
        return None;
    }

    let mut depth = 0i32;
    let select_end = first.range.end;
    let mut from_start: Option<usize> = None;
    let mut group_span: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        match token.text {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ if depth == 0 => {
                if from_start.is_none() && token.text.eq_ignore_ascii_case("FROM") {
                    from_start = Some(token.range.start);
                }
                if group_span.is_none()
                    && token.text.eq_ignore_ascii_case("GROUP")
                    && tokens
                        .get(index + 1)
                        .is_some_and(|next| next.text.eq_ignore_ascii_case("BY"))
                {
                    // The GROUP BY clause runs until the next top-level
                    // clause that a projection can't carry.
                    let start = token.range.start;
                    let mut inner = 0i32;
                    let mut end = sql.len();
                    let mut cursor = index + 2;
                    while let Some(next) = tokens.get(cursor) {
                        match next.text {
                            "(" => inner += 1,
                            ")" => inner -= 1,
                            _ if inner == 0 => {
                                let upper = next.text.to_ascii_uppercase();
                                if next.text == ";"
                                    || matches!(
                                        upper.as_str(),
                                        "HAVING"
                                            | "ORDER"
                                            | "LIMIT"
                                            | "OFFSET"
                                            | "SETTINGS"
                                            | "FORMAT"
                                            | "UNION"
                                            | "WITH"
                                    )
                                {
                                    end = next.range.start;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        cursor += 1;
                    }
                    group_span = Some((start, end));
                }
            }
            _ => {}
        }
        index += 1;
    }

    let from_start = from_start?;
    let (group_start, group_end) = group_span?;
    let select_list = sql.get(select_end..from_start)?.trim();
    let group_clause = sql.get(group_start..group_end)?.trim();
    if select_list.is_empty() || group_clause.is_empty() {
        return None;
    }

    // No leading indent here: the caller indents the body as one block, so
    // SELECT and GROUP BY stay at the same level.
    let body = format!("SELECT {select_list}\n{group_clause}");
    let name = projection_name(&group_clause["GROUP BY".len().min(group_clause.len())..]);
    Some((body, name))
}

/// A stable, readable projection name from the GROUP BY columns: their
/// identifier characters, joined by underscores, prefixed `proj_`.
fn projection_name(group_cols: &str) -> String {
    let mut slug = String::new();
    let mut last_underscore = false;
    for character in group_cols.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore && !slug.is_empty() {
            slug.push('_');
            last_underscore = true;
        }
    }
    let slug = slug.trim_matches('_');
    let slug: String = slug.chars().take(40).collect();
    if slug.is_empty() {
        "proj_rollup".to_string()
    } else {
        format!("proj_{slug}")
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
        assert_eq!(sorted, "SELECT * FROM t\nORDER BY `kind` ASC\nLIMIT 10");

        let multi = set_order_by(&sorted, &sort(&[("kind", false), ("day", true)]));
        assert_eq!(
            multi,
            "SELECT * FROM t\nORDER BY `kind` DESC, `day` ASC\nLIMIT 10"
        );

        assert_eq!(set_order_by(&multi, &[]), "SELECT * FROM t\nLIMIT 10");
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

    #[test]
    fn detects_top_level_group_by_only() {
        assert!(has_group_by("SELECT k, count() FROM t GROUP BY k"));
        assert!(has_group_by(
            "select day, sum(x) from t group by day order by day"
        ));
        // Global aggregate, no GROUP BY.
        assert!(!has_group_by("SELECT count(*) FROM t"));
        // GROUP BY only inside a subquery is not the outer query's rollup.
        assert!(!has_group_by(
            "SELECT * FROM (SELECT k, count() FROM t GROUP BY k) x"
        ));
    }

    #[test]
    fn builds_an_aggregate_projection_body_and_name() {
        let (body, name) =
            aggregate_projection("SELECT kind, count() FROM explain.me GROUP BY kind").unwrap();
        assert_eq!(body, "SELECT kind, count()\nGROUP BY kind");
        assert_eq!(name, "proj_kind");

        // FROM/WHERE/ORDER/LIMIT dropped; multi-column group keeps both.
        let (body, name) = aggregate_projection(
            "select country, kind, sum(amount) from t where at > now() group by country, kind order by 1 limit 10",
        )
        .unwrap();
        assert_eq!(
            body,
            "SELECT country, kind, sum(amount)\ngroup by country, kind"
        );
        assert_eq!(name, "proj_country_kind");
    }

    #[test]
    fn no_projection_body_without_group_by_or_from() {
        assert!(aggregate_projection("SELECT count(*) FROM t").is_none());
        assert!(aggregate_projection("WITH x AS (SELECT 1) SELECT * FROM x GROUP BY a").is_none());
    }
}
