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
}
