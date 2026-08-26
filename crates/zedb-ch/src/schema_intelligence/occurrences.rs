//! Occurrences of the identifier under the cursor, for the editor's
//! same-name highlighting. Built on the shared tokenizer so it agrees
//! with completions and hover about what an identifier is: strings and
//! comments never match, keywords never light up, and scope is the
//! current statement only.

use std::ops::Range;

use super::tokens::{statement_bounds, tokenize, word_range};
use super::vocabulary::KEYWORDS;

/// Ranges of every identifier token in the cursor's statement whose
/// text matches the identifier under the cursor. Empty when the cursor
/// is not on an identifier, when the word is a keyword, or when the
/// name appears only once (a lone highlight relates nothing).
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
    let hits: Vec<Range<usize>> = tokenize(&sql[bounds.clone()])
        .into_iter()
        .filter(|token| token.identifier && token.text.eq_ignore_ascii_case(word))
        .map(|token| bounds.start + token.range.start..bounds.start + token.range.end)
        .collect();
    if hits.len() < 2 {
        return Vec::new();
    }
    hits
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
    fn empty_or_whitespace_cursor_positions_are_quiet() {
        assert!(occurrences_at("", 0).is_empty());
        let sql = "select a , a from t";
        assert!(occurrences_at(sql, sql.find(',').unwrap()).is_empty());
    }
}
