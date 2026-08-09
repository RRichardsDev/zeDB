//! Coloring for ClickHouse type strings (DESCRIBE's type column, the
//! schema inspector's columns tab, the cell inspector header): a tiny
//! lexer over the type text mapped onto the palette. tree-sitter has
//! no grammar for bare type expressions, and the strings are small
//! and regular, so custom rules cost less than bending a grammar.

use std::ops::Range;

use gpui::{rgb, HighlightStyle, Hsla, StyledText};

use crate::theme;

/// Whitespace-collapsed single-line form (DESCRIBE emits named tuple
/// types across lines).
pub fn collapse(type_name: &str) -> String {
    if type_name.contains('\n') {
        type_name.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        type_name.to_string()
    }
}

/// Container and modifier types: the structure around a payload.
const CONTAINERS: &[&str] = &[
    "Array",
    "Map",
    "Tuple",
    "Nullable",
    "LowCardinality",
    "Nested",
    "AggregateFunction",
    "SimpleAggregateFunction",
    "Variant",
    "Dynamic",
];

/// Highlight runs over a (single-line) type string: container types
/// in a cool structural blue, leaf types in the editor's type tint,
/// numbers and quoted strings as literals, punctuation dim.
/// Named-tuple field names (lowercase) stay plain.
pub fn runs(type_name: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    let dark = theme::is_dark();
    let number: Hsla = if dark {
        rgb(0xd8c88a).into()
    } else {
        rgb(0x8a6d1a).into()
    };
    let string: Hsla = if dark {
        rgb(0x83b97c).into()
    } else {
        rgb(0x3f8049).into()
    };
    let container: Hsla = if dark {
        rgb(0x82a8de).into()
    } else {
        rgb(0x2c62a8).into()
    };
    let color = |color: Hsla| HighlightStyle {
        color: Some(color),
        ..Default::default()
    };

    let bytes = type_name.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\'' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push((start..i.min(bytes.len()), color(string)));
        } else if byte.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            out.push((start..i, color(number)));
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            // Uppercase-initial words are type names (String, Array,
            // LowCardinality); named-tuple fields are lowercase.
            let word = &type_name[start..i];
            let is_type = word.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            if is_type {
                // Nullable is a property, not structure: muted so it
                // annotates without shouting.
                let tint = if word == "Nullable" {
                    theme::text_dim()
                } else if CONTAINERS.contains(&word) {
                    container
                } else {
                    theme::table_tint()
                };
                out.push((start..i, color(tint)));
            }
        } else {
            let start = i;
            while i < bytes.len() {
                let byte = bytes[i];
                if byte == b'\'' || byte.is_ascii_alphanumeric() || byte == b'_' {
                    break;
                }
                i += 1;
            }
            out.push((start..i, color(theme::text_dim())));
        }
    }
    out
}

/// The type string as a colored inline element.
pub fn styled(type_name: &str) -> StyledText {
    let text = collapse(type_name);
    let runs = runs(&text);
    StyledText::new(text).with_highlights(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_numbers_strings_and_fields() {
        let text = "Tuple(city String, population UInt64)";
        let runs = runs(text);
        let colored: Vec<&str> = runs
            .iter()
            .filter(|(_, style)| style.color.is_some())
            .map(|(range, _)| &text[range.clone()])
            .collect();
        // Field names stay plain; Tuple/String/UInt64 and punctuation
        // are colored.
        assert!(colored.contains(&"Tuple"));
        assert!(colored.contains(&"String"));
        assert!(colored.contains(&"UInt64"));
        assert!(!colored.contains(&"city"));
        assert!(!colored.contains(&"population"));

        let text = "Enum8('a' = 1, 'b' = 2)";
        let enum_runs = super::runs(text);
        let spans: Vec<&str> = enum_runs
            .iter()
            .map(|(range, _)| &text[range.clone()])
            .collect();
        assert!(spans.contains(&"'a'"));
        assert!(spans.contains(&"1"));
    }

    #[test]
    fn collapse_flattens_describe_tuples() {
        assert_eq!(
            collapse("Tuple(\n    city String,\n    population UInt64)"),
            "Tuple( city String, population UInt64)"
        );
    }
}
