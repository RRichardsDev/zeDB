//! Function-name intelligence over the server's own catalog
//! (system.functions), including aggregate combinator resolution:
//! `quantileIf` is not in the catalog, but `quantile` is, and `-If`
//! has fixed semantics worth explaining in place.

use crate::schema_cache::{CachedFunction, SchemaSnapshot};

/// Aggregate combinator suffixes, longest-match first, with the
/// explanation hover shows. Semantics are stable ClickHouse rules;
/// the base function still comes from the server.
const COMBINATORS: &[(&str, &str)] = &[
    (
        "SimpleState",
        "returns the SimpleAggregateFunction state instead of the final value",
    ),
    (
        "MergeState",
        "merges partial states and returns the merged state, not the final value",
    ),
    (
        "OrDefault",
        "returns the return type's default value when no rows were aggregated",
    ),
    (
        "OrNull",
        "returns NULL when no rows were aggregated (return type becomes Nullable)",
    ),
    (
        "Distinct",
        "aggregates each distinct combination of arguments only once",
    ),
    (
        "ForEach",
        "aggregates arrays element-wise, returning an array of results",
    ),
    (
        "Resample",
        "splits rows into intervals by a key column and aggregates each interval separately",
    ),
    (
        "ArgMin",
        "aggregates only the rows where an extra argument reaches its minimum",
    ),
    (
        "ArgMax",
        "aggregates only the rows where an extra argument reaches its maximum",
    ),
    (
        "Array",
        "takes array arguments and aggregates across all their elements",
    ),
    (
        "Map",
        "takes Map arguments and aggregates each key's values separately",
    ),
    (
        "State",
        "returns the intermediate aggregation state (for AggregatingMergeTree / later -Merge)",
    ),
    (
        "Merge",
        "takes intermediate states (from -State) and finishes the aggregation",
    ),
    (
        "If",
        "takes an extra condition as the last argument; only rows where it holds are aggregated",
    ),
];

/// A resolved function name: the catalog entry, plus any combinator
/// suffixes that were peeled off an aggregate (outermost last).
pub(super) struct ResolvedFunction<'a> {
    pub(super) function: &'a CachedFunction,
    pub(super) combinators: Vec<&'static (&'static str, &'static str)>,
}

/// Resolve `name` against the catalog: exact (or server-flagged
/// case-insensitive) match first, then aggregate combinator stripping
/// (`quantileIf`, `sumArrayIf`, ...). Combinators only apply to
/// aggregates, and only when the remaining base really is one.
pub(super) fn resolve_function<'a>(
    snapshot: &'a SchemaSnapshot,
    name: &str,
) -> Option<ResolvedFunction<'a>> {
    if let Some(function) = snapshot.function(name) {
        return Some(ResolvedFunction {
            function,
            combinators: Vec::new(),
        });
    }
    let mut remaining = name.to_string();
    let mut combinators = Vec::new();
    // At most a few combinators stack in practice; bound the walk.
    for _ in 0..4 {
        let Some(combinator) = COMBINATORS
            .iter()
            .find(|(suffix, _)| remaining.len() > suffix.len() && remaining.ends_with(suffix))
        else {
            break;
        };
        remaining.truncate(remaining.len() - combinator.0.len());
        combinators.push(combinator);
        if let Some(function) = snapshot.function(&remaining) {
            if function.is_aggregate {
                combinators.reverse();
                return Some(ResolvedFunction {
                    function,
                    combinators,
                });
            }
            return None;
        }
    }
    None
}

/// The hover card for a resolved function.
pub(super) fn function_markdown(name: &str, resolved: &ResolvedFunction<'_>) -> String {
    let function = resolved.function;
    let mut markdown = format!("**{name}**");
    let kind = if function.is_aggregate {
        "aggregate function"
    } else {
        "function"
    };
    if resolved.combinators.is_empty() {
        markdown.push_str(&format!("\n\n{kind}"));
    } else {
        markdown.push_str(&format!("\n\n**{}** {kind} with:", function.name));
        for (suffix, explanation) in &resolved.combinators {
            markdown.push_str(&format!("\n- **-{suffix}**: {explanation}"));
        }
    }
    if !function.alias_to.is_empty() {
        markdown.push_str(&format!("\n\nAlias of `{}`", function.alias_to));
    }
    if !function.syntax.is_empty() {
        markdown.push_str(&format!("\n\n```sql\n{}\n```", function.syntax.trim()));
    }
    let mut prose = String::new();
    if !function.description.is_empty() {
        prose.push_str(function.description.trim());
    }
    if !function.arguments.is_empty() {
        if !prose.is_empty() {
            prose.push_str("\n\n");
        }
        prose.push_str(function.arguments.trim());
    }
    if !function.returned_value.is_empty() {
        if !prose.is_empty() {
            prose.push_str("\n\n");
        }
        prose.push_str(function.returned_value.trim());
    }
    if !prose.is_empty() {
        // Same separator contract as the setting card: zeDB's lines
        // above, the server's prose verbatim below (links absolutized
        // so they open).
        markdown.push_str(&format!(
            "\n\n---\n\n{}",
            super::hover::absolutize_doc_links(&prose, None)
        ));
    }
    markdown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_intelligence::fixtures::snapshot;
    use crate::schema_intelligence::hover;

    #[test]
    fn exact_names_and_combinator_stacks_resolve() {
        let snapshot = snapshot(None);
        let plain = resolve_function(&snapshot, "quantile").unwrap();
        assert!(plain.combinators.is_empty());

        let stacked = resolve_function(&snapshot, "quantileArrayIf").unwrap();
        assert_eq!(stacked.function.name, "quantile");
        let suffixes: Vec<&str> = stacked
            .combinators
            .iter()
            .map(|(suffix, _)| *suffix)
            .collect();
        assert_eq!(suffixes, ["Array", "If"], "outermost last");

        // Combinators never apply to non-aggregates or unknown bases.
        assert!(resolve_function(&snapshot, "lowerIf").is_none());
        assert!(resolve_function(&snapshot, "nonsenseIf").is_none());
    }

    #[test]
    fn doc_links_resolve_in_every_shape_the_server_writes() {
        use crate::schema_cache::{CachedFunction, SchemaSnapshot};
        let mut snapshot = SchemaSnapshot::default();
        snapshot.functions = vec![CachedFunction {
            name: "toDate".into(),
            description: "Root [a](/operations/settings/settings#x), \
                          md [b](../../sql-reference/data-types/date.md), \
                          md anchor [c](../../engines/mergetree.md/#projections), \
                          docs [d](/docs/en/interfaces/cli), \
                          external [e](https://example.test), \
                          anchor [concat](#concat)."
                .into(),
            ..Default::default()
        }];
        let sql = "select toDate(x)";
        let info = hover(&snapshot, None, sql, sql.find("toDate").unwrap()).unwrap();
        for expected in [
            "](https://clickhouse.com/docs/operations/settings/settings#x)",
            "](https://clickhouse.com/docs/sql-reference/data-types/date)",
            "](https://clickhouse.com/docs/engines/mergetree#projections)",
            "](https://clickhouse.com/docs/en/interfaces/cli)",
            "](https://example.test)",
        ] {
            assert!(info.markdown.contains(expected), "{expected}\n{info:?}");
        }
        // A bare anchor in a FUNCTION card has no resolvable page: the
        // link markup drops, the text stays (a dead link throws OS
        // errors; text does not).
        assert!(info.markdown.contains("anchor concat."), "{info:?}");
        assert!(!info.markdown.contains("[concat]"), "{info:?}");
    }

    #[test]
    fn setting_card_anchors_resolve_into_the_settings_page() {
        use crate::schema_intelligence::fixtures;
        let mut snapshot = fixtures::snapshot(None);
        snapshot.settings[0].description = "See also [max_memory_usage](#max_memory_usage).".into();
        let sql = "select 1 settings max_threads = 4";
        let info = hover(&snapshot, None, sql, sql.find("max_threads").unwrap()).unwrap();
        assert!(
            info.markdown.contains(
                "](https://clickhouse.com/docs/operations/settings/settings#max_memory_usage)"
            ),
            "{info:?}"
        );
    }

    #[test]
    fn case_insensitivity_follows_the_server_flag() {
        let snapshot = snapshot(None);
        assert!(resolve_function(&snapshot, "LOWER").is_some());
        assert!(
            resolve_function(&snapshot, "QUANTILE").is_none(),
            "quantile is case-sensitive per the catalog"
        );
    }

    #[test]
    fn hovering_a_function_call_shows_the_card() {
        let snapshot = snapshot(None);
        let sql = "select quantileIf(0.5)(ms, ok) from t";
        let info = hover(&snapshot, None, sql, sql.find("quantileIf").unwrap() + 3).unwrap();
        assert!(info.markdown.contains("**quantileIf**"), "{info:?}");
        assert!(info.markdown.contains("**quantile** aggregate function"));
        assert!(info.markdown.contains("-If"), "{info:?}");
        assert!(info.markdown.contains("condition"), "{info:?}");
        assert!(
            info.markdown.contains("Computes an approximate quantile"),
            "server prose rides along: {info:?}"
        );

        // Not followed by a parenthesis: not a call, no function card.
        let sql = "select quantile from t";
        let quantile_hover = hover(&snapshot, None, sql, sql.find("quantile").unwrap());
        assert!(
            quantile_hover.is_none()
                || !quantile_hover
                    .unwrap()
                    .markdown
                    .contains("aggregate function")
        );
    }
}
