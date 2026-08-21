//! Integration coverage for the vendored SQL highlighter boundary.
//!
//! ClickHouse statements the sequel grammar cannot parse (DESCRIBE,
//! OPTIMIZE, KILL, ...) must still get keyword coloring via the
//! vendored ERROR-region salvage patch.

use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};

#[test]
fn describe_gets_keyword_color() {
    let text = "describe sat.complexTypes;";
    let mut hl = SyntaxHighlighter::new("sql");
    hl.update(None, &gpui_component::Rope::from(text));
    let theme = HighlightTheme::default_dark();
    let styles = hl.styles(&(0..text.len()), &theme);
    let describe_colored = styles.iter().any(|(range, style)| {
        range.start == 0 && range.end == "describe".len() && style.color.is_some()
    });
    assert!(describe_colored, "describe uncolored: {styles:?}");
    // The table segment after the dot colors like parsed object
    // references ("describe sat." is 13 bytes; "complexTypes" follows).
    let table_colored = styles
        .iter()
        .any(|(range, style)| range.start == 13 && range.end == 25 && style.color.is_some());
    assert!(table_colored, "table name uncolored: {styles:?}");
}

#[test]
fn multibyte_text_in_error_region_does_not_panic() {
    // A styles() range cutting a multibyte character inside an ERROR
    // region must clamp, not panic (Rope::slice is boundary-strict).
    let text = "describe caf\u{e9}";
    let mut hl = SyntaxHighlighter::new("sql");
    hl.update(None, &gpui_component::Rope::from(text));
    let theme = HighlightTheme::default_dark();
    for end in 0..=text.len() {
        let _ = hl.styles(&(0..end), &theme);
    }
}
