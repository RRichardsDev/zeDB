use super::tokens::tokenize;

/// Remove the top-level LIMIT (and its OFFSET) from one statement, for
/// probes that need to see past the visible window.
pub fn strip_top_level_limit(sql: &str) -> String {
    let tokens = tokenize(sql);
    let mut depth = 0i32;
    let mut span: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        match token.text {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 && span.is_none() && token.text.eq_ignore_ascii_case("limit") {
            let start = token.range.start;
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
                    let upper = next.text.to_ascii_uppercase();
                    if next.text == ";"
                        || matches!(upper.as_str(), "SETTINGS" | "FORMAT" | "INTO" | "UNION")
                    {
                        end = next.range.start;
                        break;
                    }
                }
                cursor += 1;
            }
            span = Some((start, end));
            break;
        }
        index += 1;
    }
    let Some((start, end)) = span else {
        return sql.to_string();
    };
    let head = sql[..start].trim_end();
    let rest = sql[end..].trim_start();
    if rest.is_empty() {
        head.to_string()
    } else if rest.starts_with(';') {
        format!("{head}{rest}")
    } else {
        format!("{head} {rest}")
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn strips_only_the_top_level_limit() {
        assert_eq!(
            strip_top_level_limit("SELECT * FROM t ORDER BY id LIMIT 10 OFFSET 5;"),
            "SELECT * FROM t ORDER BY id;"
        );
        assert_eq!(
            strip_top_level_limit("SELECT * FROM (SELECT a FROM t LIMIT 3) x LIMIT 7 SETTINGS x=1"),
            "SELECT * FROM (SELECT a FROM t LIMIT 3) x SETTINGS x=1"
        );
        assert_eq!(strip_top_level_limit("SELECT 1"), "SELECT 1");
    }
}
