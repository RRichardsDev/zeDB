//! Occurrences of the identifier under the cursor, for the editor's
//! same-name highlighting. Built on the shared tokenizer so it agrees
//! with completions and hover about what an identifier is: strings and
//! comments never match, keywords never light up, and scope is the
//! current statement only. Alias declarations are scope-aware: an
//! alias declared inside a subquery shadows an outer one of the same
//! name, and only occurrences governed by the same declaration light
//! together.

use std::ops::Range;

use super::tokens::{statement_bounds, tokenize, word_range, Token};
use super::vocabulary::KEYWORDS;

/// Ranges of every identifier token in the cursor's statement that
/// refers to the same thing as the identifier under the cursor. Empty
/// when the cursor is not on an identifier, when the word is a
/// keyword, or when there is only one occurrence (a lone highlight
/// relates nothing).
pub fn occurrences_at(sql: &str, offset: usize) -> Vec<Range<usize>> {
    let word = &sql[word_range(sql, offset)];
    if word.is_empty()
        || KEYWORDS
            .iter()
            .any(|keyword| keyword.eq_ignore_ascii_case(word))
    {
        return Vec::new();
    }
    let bounds = statement_bounds(sql, offset);
    let Some(statement) = sql.get(bounds.clone()) else {
        return Vec::new();
    };
    let tokens = tokenize(statement);
    let relative = offset.saturating_sub(bounds.start);
    let regions = paren_regions(&tokens);
    let declarations = declaration_sites(&tokens, word);
    let governing = visible_declaration(&regions, &declarations, relative);
    let hits: Vec<Range<usize>> = tokens
        .iter()
        .filter(|token| token.identifier && token.text.eq_ignore_ascii_case(word))
        .filter(|token| {
            // Same governing declaration = same thing. With no
            // declarations anywhere, everything groups (None == None)
            // and this is plain same-text matching.
            visible_declaration(&regions, &declarations, token.range.start) == governing
        })
        .map(|token| bounds.start + token.range.start..bounds.start + token.range.end)
        .collect();
    if hits.len() < 2 {
        return Vec::new();
    }
    hits
}

/// Byte ranges of every balanced parenthesis pair in the statement.
fn paren_regions(tokens: &[Token<'_>]) -> Vec<Range<usize>> {
    let mut regions = Vec::new();
    let mut stack = Vec::new();
    for token in tokens {
        match token.text {
            "(" => stack.push(token.range.start),
            ")" => {
                if let Some(start) = stack.pop() {
                    regions.push(start..token.range.end);
                }
            }
            _ => {}
        }
    }
    // An unclosed paren (mid-edit) scopes to the end of the statement.
    let end = tokens.last().map(|token| token.range.end).unwrap_or(0);
    for start in stack {
        regions.push(start..end);
    }
    regions
}

/// Byte starts of every place `word` is declared as a table alias:
/// `FROM x word`, `JOIN db.x AS word`, `FROM (subquery) word`.
fn declaration_sites(tokens: &[Token<'_>], word: &str) -> Vec<usize> {
    let mut sites = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if !token.identifier || !token.text.eq_ignore_ascii_case(word) {
            continue;
        }
        let mut back = index;
        // Skip an AS between the table and the alias.
        if back >= 1 && tokens[back - 1].text.eq_ignore_ascii_case("AS") {
            back -= 1;
        }
        if back < 1 {
            continue;
        }
        let anchored = match tokens[back - 1].text {
            // `FROM (subquery) word`: walk to the matching open paren.
            ")" => {
                let mut depth = 0i32;
                let mut open = None;
                for (candidate, prior) in tokens[..back].iter().enumerate().rev() {
                    match prior.text {
                        ")" => depth += 1,
                        "(" => {
                            depth -= 1;
                            if depth == 0 {
                                open = Some(candidate);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                open.is_some_and(|open| is_from_or_join_before(tokens, open))
            }
            // `FROM [db.]table word`: walk back over the name chain.
            _ if tokens[back - 1].identifier => {
                let mut cursor = back - 1;
                while cursor >= 2 && tokens[cursor - 1].text == "." && tokens[cursor - 2].identifier
                {
                    cursor -= 2;
                }
                is_from_or_join_before(tokens, cursor)
            }
            _ => false,
        };
        if anchored {
            sites.push(token.range.start);
        }
    }
    sites
}

fn is_from_or_join_before(tokens: &[Token<'_>], index: usize) -> bool {
    index >= 1
        && (tokens[index - 1].text.eq_ignore_ascii_case("FROM")
            || tokens[index - 1].text.eq_ignore_ascii_case("JOIN"))
}

/// The declaration governing `offset`: the visible one (its innermost
/// enclosing paren region contains the offset) with the smallest such
/// region, i.e. the shadowing inner subquery wins.
fn visible_declaration(
    regions: &[Range<usize>],
    declarations: &[usize],
    offset: usize,
) -> Option<usize> {
    let scope = |position: usize| -> Option<&Range<usize>> {
        regions
            .iter()
            .filter(|region| region.contains(&position))
            .min_by_key(|region| region.len())
    };
    declarations
        .iter()
        .filter(|&&declaration| match scope(declaration) {
            Some(region) => region.contains(&offset),
            None => true,
        })
        .min_by_key(|&&declaration| {
            scope(declaration)
                .map(|region| region.len())
                .unwrap_or(usize::MAX)
        })
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_every_use_of_an_alias_in_the_statement() {
        let sql = "select af.id from events af join dims cd on af.id = cd.id where af.id > 1; \
                   select af.id from other af";
        let cursor = sql.find("af.id").unwrap();
        let hits = occurrences_at(sql, cursor);
        assert_eq!(hits.len(), 4, "{hits:?}");
        assert!(hits.iter().all(|range| &sql[range.clone()] == "af"));
        assert!(
            hits.iter().all(|range| range.end < sql.find(';').unwrap()),
            "the second statement's af stays dark: {hits:?}"
        );
    }

    #[test]
    fn keywords_strings_comments_and_lone_names_stay_dark() {
        assert!(occurrences_at("select x from t", 2).is_empty(), "keyword");
        let sql = "select unique_name from t";
        assert!(
            occurrences_at(sql, sql.find("unique_name").unwrap()).is_empty(),
            "a lone occurrence relates nothing"
        );
        let sql = "select af, 'af af' from t -- af\n where af > 1";
        let hits = occurrences_at(sql, sql.find("af").unwrap());
        assert_eq!(hits.len(), 2, "literal and comment don't match: {hits:?}");
    }

    #[test]
    fn an_inner_alias_shadows_the_outer_one() {
        let sql = "select e.id from events e where e.id in \
                   (select e.err from errors e where e.err > 0)";
        // Cursor on the OUTER declaration: the inner subquery's e stays
        // dark.
        let outer_declaration = sql.find("events e").unwrap() + 7;
        let hits = occurrences_at(sql, outer_declaration);
        assert_eq!(hits.len(), 3, "{hits:?}");
        let subquery = sql.find('(').unwrap();
        assert!(
            hits.iter().all(|range| range.start < subquery),
            "outer only: {hits:?}"
        );

        // Cursor on an INNER use: only the subquery's three light.
        let inner_use = sql.rfind("e.err").unwrap();
        let hits = occurrences_at(sql, inner_use);
        assert_eq!(hits.len(), 3, "{hits:?}");
        assert!(
            hits.iter().all(|range| range.start > subquery),
            "inner only: {hits:?}"
        );
    }

    #[test]
    fn subquery_aliases_anchor_on_the_closing_paren() {
        let sql = "select w.total from (select count() as total from t) w where w.total > 0";
        let hits = occurrences_at(sql, sql.find("w.total").unwrap());
        assert_eq!(hits.len(), 3, "{hits:?}");
    }

    #[test]
    fn empty_or_whitespace_cursor_positions_are_quiet() {
        assert!(occurrences_at("", 0).is_empty());
        let sql = "select a , a from t";
        assert!(occurrences_at(sql, sql.find(',').unwrap()).is_empty());
    }
}
