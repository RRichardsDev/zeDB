//! Query advisor (Phase 9, Part A) — deterministic, explainable
//! recommendations from a query's execution, not a model.
//!
//! Increment 1 ships one finding: the primary key isn't filtering the
//! query (scanned a lot to return a little, and `EXPLAIN indexes` shows
//! the primary key barely pruned). Every input is ClickHouse-sourced (the
//! run's `read_rows` / `result_rows` plus the `EXPLAIN json = 1,
//! indexes = 1` plan), so a finding is a fact, not a guess. The fix is
//! copyable DDL naming the real WHERE column(s), with the skip-index type
//! (minmax / set / bloom_filter) chosen from each column's type.
//!
//! Mirrors the shape of [`crate::storage_advisor`]: pure rules over a
//! facts struct, unit-tested, returning ranked findings.

use zedb_ch::explain::ExplainNode;

/// Below this many rows scanned we never advise: small scans are cheap
/// and indexing them is noise.
const MIN_SCAN_ROWS: u64 = 1_000_000;
/// Scanned-to-returned ratio under which the filter is not selective
/// enough for an index to help (you are returning most of what you read).
const MIN_SELECTIVITY: u64 = 100;
/// If the primary key kept fewer than this fraction of its granules it is
/// already doing its job, so we stay quiet.
const WELL_PRUNED: f64 = 0.5;
/// Rows per granule (ClickHouse's default `index_granularity`). Used to
/// estimate rows scanned from `EXPLAIN`'s granule counts, which is robust
/// for fast queries the live progress poll (100ms) never sampled.
const GRANULE_ROWS: u64 = 8192;
/// At or below this many distinct values a `set` index beats a bloom
/// filter (few enough values to store outright).
const SET_MAX_DISTINCT: u64 = 1_000;
/// Above this many distinct values the column is high-cardinality enough
/// to justify a tighter bloom-filter false-positive rate.
const BLOOM_TIGHT_DISTINCT: u64 = 100_000;

/// Everything the rules need about one executed query, drawn from the run
/// summary and its EXPLAIN plan.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct QueryFacts {
    /// Rows ClickHouse read to answer the query (`system.processes`).
    pub read_rows: u64,
    /// Rows the query returned into the grid.
    pub result_rows: u64,
    /// Bytes read.
    pub read_bytes: u64,
    /// True when the grid capped the result, so `result_rows` is a floor,
    /// not the real count (we then stay quiet to avoid a false positive).
    pub result_capped: bool,
    /// `db.table` of the single MergeTree read, when there is exactly one.
    pub table: Option<String>,
    /// The primary-key columns (ORDER BY leading keys).
    pub order_by: Vec<String>,
    /// Columns the query's WHERE filters on, parsed from the SQL (with
    /// their type and whether the filter is a range), so the fix DDL names
    /// the real column and picks the right skip-index type.
    pub filter_columns: Vec<FilterColumn>,
    /// The plan contains a Filter node, i.e. the query has a WHERE. Guards
    /// against advising on an intentional full scan (`SELECT count(*)`).
    pub has_filter: bool,
    pub pk_present: bool,
    pub pk_initial_granules: u64,
    pub pk_selected_granules: u64,
    /// How many MergeTree reads the plan had (a join has more than one).
    pub merge_tree_reads: u32,
}

/// One column the WHERE filters on, with what we need to size its index.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FilterColumn {
    pub name: String,
    /// The column's ClickHouse type from `system.columns` (e.g. `UInt64`,
    /// `LowCardinality(String)`), empty when unknown.
    pub base_type: String,
    /// True when the filter is a range (`<`, `>`, `BETWEEN`) rather than an
    /// equality / `IN`. Ranges want `minmax`; point lookups want a set or
    /// bloom filter.
    pub is_range: bool,
    /// Distinct values (`uniqCombined`), probed for equality filters to
    /// choose between a set and a bloom filter. `None` when not probed.
    pub distinct: Option<u64>,
}

/// A type that is inherently low-cardinality (dictionary-encoded), so a
/// `set` index fits without probing the value count.
pub fn is_inherently_low_cardinality(base_type: &str) -> bool {
    base_type.contains("LowCardinality") || base_type.contains("Enum")
}

/// Whether an equality filter on this column needs a cardinality probe to
/// choose its index type (ranges and inherently low-cardinality types do
/// not).
pub fn needs_cardinality_probe(column: &FilterColumn) -> bool {
    !column.is_range && !is_inherently_low_cardinality(&column.base_type)
}

/// The skip-index type best suited to a filtered column, driven by the
/// filter operator and the column's cardinality: `minmax` for ranges,
/// `set` for few distinct values, and a `bloom_filter` whose false-
/// positive rate tightens (to 1%) only for very high-cardinality columns.
fn index_type_for(column: &FilterColumn) -> &'static str {
    if column.is_range {
        return "minmax GRANULARITY 4";
    }
    if is_inherently_low_cardinality(&column.base_type) {
        return "set(0) GRANULARITY 4";
    }
    match column.distinct {
        Some(distinct) if distinct <= SET_MAX_DISTINCT => "set(0) GRANULARITY 4",
        Some(distinct) if distinct > BLOOM_TIGHT_DISTINCT => {
            // Very high cardinality earns a tighter (1%) false-positive
            // rate than ClickHouse's 0.025 default.
            "bloom_filter(0.01) GRANULARITY 1"
        }
        // Moderate or unknown cardinality: the default bloom rate, no
        // guess at a tighter one.
        _ => "bloom_filter GRANULARITY 1",
    }
}

/// A single ranked recommendation.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryFinding {
    pub title: String,
    /// Plain-language diagnosis (the "why"), all ClickHouse-sourced.
    pub detail: String,
    /// What to change.
    pub fix: String,
    /// Copyable DDL to open in the editor, when a single table is known.
    pub editor_sql: Option<String>,
    /// Rows scanned, used to rank findings by rough impact.
    pub rows_scanned: u64,
}

/// Build facts from a run summary and its parsed EXPLAIN plan.
pub fn facts_from_plan(
    plan: &ExplainNode,
    read_rows: u64,
    result_rows: u64,
    read_bytes: u64,
    result_capped: bool,
) -> QueryFacts {
    let mut facts = QueryFacts {
        read_rows,
        result_rows,
        read_bytes,
        result_capped,
        ..Default::default()
    };
    walk(plan, &mut facts);
    // Only keep a table for templated DDL when the plan read exactly one
    // MergeTree table; a join makes "which table" ambiguous.
    if facts.merge_tree_reads != 1 {
        facts.table = None;
    }
    // The live progress poll misses fast queries (it samples every 100ms),
    // so fall back to an EXPLAIN-derived estimate from the granules the
    // primary key selected. Take whichever is larger: the poll is exact
    // when it fired, the estimate covers when it didn't.
    let granule_estimate = facts.pk_selected_granules.saturating_mul(GRANULE_ROWS);
    facts.read_rows = facts.read_rows.max(granule_estimate);
    facts
}

fn walk(node: &ExplainNode, facts: &mut QueryFacts) {
    // A WHERE shows up either as a `Filter` node or, in current
    // ClickHouse, folded into an `Expression` node whose description
    // reads "(WHERE + ...)". Catch both so a filtered query is recognized.
    let describes_where = node
        .description
        .as_deref()
        .map(|desc| desc.contains("WHERE"))
        .unwrap_or(false);
    if node.node_type.contains("Filter") || describes_where {
        facts.has_filter = true;
    }
    if node.node_type == "ReadFromMergeTree" {
        facts.merge_tree_reads += 1;
        if facts.table.is_none() {
            facts.table = node.description.as_deref().and_then(table_ident);
        }
        for index in &node.indexes {
            if index.index_type.eq_ignore_ascii_case("primarykey") {
                facts.pk_present = true;
                facts.pk_initial_granules += index.initial_granules;
                facts.pk_selected_granules += index.selected_granules;
                if facts.order_by.is_empty() {
                    facts.order_by = index.keys.clone();
                }
            }
        }
    }
    for child in &node.children {
        walk(child, facts);
    }
}

/// A ReadFromMergeTree description is the table (`db.table`). Accept it
/// only when it looks like a bare qualified identifier, so we never build
/// DDL from a decorated description.
fn table_ident(description: &str) -> Option<String> {
    let name = description.trim();
    let ok = !name.is_empty()
        && !name.contains(char::is_whitespace)
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    ok.then(|| name.to_string())
}

/// Whether a statement is a plain read we can advise on: its first
/// keyword is SELECT or WITH (a CTE), so `EXPLAIN` wraps cleanly and we
/// are not looking at DDL/DML.
pub fn is_advisable_select(sql: &str) -> bool {
    let head = sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    head == "select" || head == "with"
}

/// Rank findings by rows scanned (rough impact), most first.
pub fn advise(facts: &QueryFacts) -> Vec<QueryFinding> {
    let mut findings = Vec::new();
    if let Some(finding) = primary_key_not_filtering(facts) {
        findings.push(finding);
    }
    findings.sort_by_key(|finding| std::cmp::Reverse(finding.rows_scanned));
    findings
}

fn primary_key_not_filtering(facts: &QueryFacts) -> Option<QueryFinding> {
    if facts.read_rows < MIN_SCAN_ROWS {
        return None;
    }
    // A WHERE must exist and the result must be small relative to the scan;
    // otherwise the scan is intentional or an index cannot help.
    if !facts.has_filter || facts.result_capped || facts.result_rows == 0 {
        return None;
    }
    if facts.read_rows / facts.result_rows.max(1) < MIN_SELECTIVITY {
        return None;
    }
    // Stay quiet if the primary key already pruned well.
    if facts.pk_present && facts.pk_initial_granules > 0 {
        let kept = facts.pk_selected_granules as f64 / facts.pk_initial_granules as f64;
        if kept < WELL_PRUNED {
            return None;
        }
    }

    let kept_pct = if facts.pk_initial_granules > 0 {
        (facts.pk_selected_granules as f64 / facts.pk_initial_granules as f64 * 100.0).round()
            as u64
    } else {
        100
    };

    let index_clause = if facts.pk_present {
        let order = if facts.order_by.is_empty() {
            String::new()
        } else {
            format!(" (ORDER BY {})", facts.order_by.join(", "))
        };
        format!(
            "the primary key kept {}/{} granules ({}%), so the filter isn't served by the table's sort order{}",
            facts.pk_selected_granules, facts.pk_initial_granules, kept_pct, order
        )
    } else {
        "no primary-key pruning applied, so the whole table was scanned".to_string()
    };

    let filter_desc = if facts.filter_columns.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = facts
            .filter_columns
            .iter()
            .map(|column| unquote(&column.name).to_string())
            .collect();
        format!(" Your WHERE filters on {}.", join_idents(&names))
    };

    let detail = format!(
        "Scanned about {} rows to return {}, and {}.{}",
        thousands(facts.read_rows),
        thousands(facts.result_rows),
        index_clause,
        filter_desc,
    );

    // The columns worth indexing are those the query filters on that
    // aren't already the table's leading sort key (filtering the leading
    // key would have pruned). Names come straight from the WHERE.
    let leading_key = facts
        .order_by
        .first()
        .map(|key| unquote(key).to_ascii_lowercase());
    let mut candidates: Vec<FilterColumn> = Vec::new();
    for column in &facts.filter_columns {
        let name = unquote(&column.name).to_string();
        if leading_key.as_deref() == Some(name.to_ascii_lowercase().as_str()) {
            continue;
        }
        if candidates.iter().any(|existing| existing.name == name) {
            continue;
        }
        candidates.push(FilterColumn {
            name,
            base_type: column.base_type.clone(),
            is_range: column.is_range,
            distinct: column.distinct,
        });
    }

    let fix = if candidates.is_empty() {
        "Order the table by the column(s) you filter on, or add a skip index on them.".to_string()
    } else {
        let names: Vec<String> = candidates
            .iter()
            .map(|column| column.name.clone())
            .collect();
        format!(
            "Add a skip index on {}, or move it into the table's ORDER BY.",
            join_idents(&names)
        )
    };

    let editor_sql = facts.table.as_ref().map(|table| {
        if candidates.is_empty() {
            format!(
                "-- Replace <column> with the column(s) in your WHERE.\n\
                 ALTER TABLE {table} ADD INDEX idx_<column> <column> TYPE minmax GRANULARITY 4;\n\
                 ALTER TABLE {table} MATERIALIZE INDEX idx_<column>;"
            )
        } else {
            index_ddl(table, &candidates)
        }
    });

    Some(QueryFinding {
        title: "Primary key isn't filtering this query".to_string(),
        detail,
        fix,
        editor_sql,
        rows_scanned: facts.read_rows,
    })
}

/// Strip a single pair of surrounding backticks from an identifier.
fn unquote(ident: &str) -> &str {
    ident.trim().trim_matches('`')
}

/// `a`, `b` and `c` -> "`a`, `b`, `c`" for prose.
fn join_idents(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| format!("`{column}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One `ADD INDEX` + `MATERIALIZE INDEX` per filtered column, each with the
/// index type chosen for that column (see [`index_type_for`]).
fn index_ddl(table: &str, columns: &[FilterColumn]) -> String {
    let mut out = String::new();
    for column in columns {
        let name = &column.name;
        out.push_str(&format!(
            "ALTER TABLE {table} ADD INDEX idx_{name} {name} TYPE {};\n\
             ALTER TABLE {table} MATERIALIZE INDEX idx_{name};\n",
            index_type_for(column)
        ));
    }
    out.trim_end().to_string()
}

/// Group digits for readability (12345678 -> "12,345,678"). Locale-free,
/// matching the app's plain number style.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let bytes = digits.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use zedb_ch::explain::{ExplainIndex, ExplainNode};

    fn fcol(name: &str, base_type: &str, is_range: bool) -> FilterColumn {
        fcol_card(name, base_type, is_range, None)
    }

    fn fcol_card(
        name: &str,
        base_type: &str,
        is_range: bool,
        distinct: Option<u64>,
    ) -> FilterColumn {
        FilterColumn {
            name: name.to_string(),
            base_type: base_type.to_string(),
            is_range,
            distinct,
        }
    }

    fn pk_index(initial: u64, selected: u64, keys: &[&str]) -> ExplainIndex {
        ExplainIndex {
            index_type: "PrimaryKey".to_string(),
            keys: keys.iter().map(|k| k.to_string()).collect(),
            condition: Some("some condition".to_string()),
            initial_parts: 4,
            selected_parts: 4,
            initial_granules: initial,
            selected_granules: selected,
        }
    }

    /// Expression -> Filter -> ReadFromMergeTree(table, pk index).
    fn plan(table: &str, index: ExplainIndex) -> ExplainNode {
        ExplainNode {
            node_type: "Expression".to_string(),
            description: None,
            indexes: vec![],
            children: vec![ExplainNode {
                node_type: "Filter".to_string(),
                description: None,
                indexes: vec![],
                children: vec![ExplainNode {
                    node_type: "ReadFromMergeTree".to_string(),
                    description: Some(table.to_string()),
                    indexes: vec![index],
                    children: vec![],
                }],
            }],
        }
    }

    #[test]
    fn fires_when_pk_does_not_prune_a_selective_filter() {
        // Kept 118/120 granules for a huge scan returning a handful.
        let facts = facts_from_plan(
            &plan("db.events", pk_index(120, 118, &["id"])),
            2_000_000,
            12,
            0,
            false,
        );
        assert!(facts.has_filter);
        assert_eq!(facts.table.as_deref(), Some("db.events"));
        assert_eq!(facts.order_by, vec!["id".to_string()]);
        let findings = advise(&facts);
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert!(finding.detail.contains("2,000,000"));
        assert!(finding.detail.contains("ORDER BY id"));
        assert!(finding.editor_sql.as_ref().unwrap().contains("db.events"));
    }

    #[test]
    fn recognizes_where_folded_into_an_expression_node() {
        // Current ClickHouse renders the WHERE as an Expression node
        // "(WHERE + ...)" rather than a Filter node.
        let read = ExplainNode {
            node_type: "ReadFromMergeTree".to_string(),
            description: Some("explain.me".to_string()),
            indexes: vec![pk_index(368, 368, &["user_id", "at"])],
            children: vec![],
        };
        let where_expr = ExplainNode {
            node_type: "Expression".to_string(),
            description: Some("(WHERE + Change column names to column identifiers)".to_string()),
            indexes: vec![],
            children: vec![read],
        };
        let facts = facts_from_plan(&where_expr, 3_000_000, 30, 0, false);
        assert!(facts.has_filter);
        let findings = advise(&facts);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("368/368"));
    }

    #[test]
    fn names_the_real_filter_column_in_the_fix() {
        let mut facts = facts_from_plan(
            &plan("db.events", pk_index(120, 118, &["id"])),
            2_000_000,
            12,
            0,
            false,
        );
        facts.filter_columns = vec![fcol("amount", "Decimal(18, 4)", false)];
        let finding = &advise(&facts)[0];
        assert!(finding.detail.contains("`amount`"));
        assert!(finding.fix.contains("`amount`"));
        let sql = finding.editor_sql.as_ref().unwrap();
        // Equality on a high-cardinality numeric -> bloom_filter.
        assert!(sql.contains("ADD INDEX idx_amount amount TYPE bloom_filter"));
        assert!(sql.contains("MATERIALIZE INDEX idx_amount"));
        assert!(!sql.contains("<column>"));
    }

    #[test]
    fn picks_the_index_type_from_operator_type_and_cardinality() {
        // Range -> minmax, regardless of type/cardinality.
        assert_eq!(
            index_type_for(&fcol_card("ts", "DateTime", true, Some(9_000_000))),
            "minmax GRANULARITY 4"
        );
        // Inherently low-cardinality types -> set, no probe needed.
        assert_eq!(
            index_type_for(&fcol("country", "LowCardinality(String)", false)),
            "set(0) GRANULARITY 4"
        );
        assert_eq!(
            index_type_for(&fcol("kind", "Enum8('a' = 1)", false)),
            "set(0) GRANULARITY 4"
        );
        // Equality, cardinality-driven:
        // few distinct -> set,
        assert_eq!(
            index_type_for(&fcol_card("status", "String", false, Some(6))),
            "set(0) GRANULARITY 4"
        );
        // moderate -> default bloom rate,
        assert_eq!(
            index_type_for(&fcol_card("city", "String", false, Some(20_000))),
            "bloom_filter GRANULARITY 1"
        );
        // very high -> tighter 1% bloom,
        assert_eq!(
            index_type_for(&fcol_card(
                "amount",
                "Decimal(18, 4)",
                false,
                Some(2_900_000)
            )),
            "bloom_filter(0.01) GRANULARITY 1"
        );
        // unknown (probe failed) -> default bloom rate, no guess.
        assert_eq!(
            index_type_for(&fcol("user_id", "UInt64", false)),
            "bloom_filter GRANULARITY 1"
        );
    }

    #[test]
    fn skips_the_leading_sort_key_when_choosing_index_columns() {
        // Filtering the leading key (id) shouldn't suggest indexing it;
        // the non-key column (amount) should.
        let mut facts = facts_from_plan(
            &plan("db.events", pk_index(120, 118, &["id"])),
            2_000_000,
            12,
            0,
            false,
        );
        facts.filter_columns = vec![
            fcol("id", "UInt64", false),
            fcol("amount", "Decimal(18, 4)", false),
        ];
        let sql = advise(&facts)[0].editor_sql.clone().unwrap();
        assert!(sql.contains("idx_amount"));
        assert!(!sql.contains("idx_id"));
    }

    #[test]
    fn quiet_when_pk_prunes_well() {
        // Kept only 3/120 granules: the primary key is doing its job.
        let facts = facts_from_plan(
            &plan("db.events", pk_index(120, 3, &["id"])),
            2_000_000,
            12,
            0,
            false,
        );
        assert!(advise(&facts).is_empty());
    }

    #[test]
    fn quiet_without_a_filter() {
        // No Filter node -> intentional full scan (e.g. count(*)).
        let read = ExplainNode {
            node_type: "ReadFromMergeTree".to_string(),
            description: Some("db.events".to_string()),
            indexes: vec![pk_index(120, 120, &["id"])],
            children: vec![],
        };
        let facts = facts_from_plan(&read, 2_000_000, 1, 0, false);
        assert!(!facts.has_filter);
        assert!(advise(&facts).is_empty());
    }

    #[test]
    fn quiet_when_result_is_capped() {
        let facts = facts_from_plan(
            &plan("db.events", pk_index(120, 118, &["id"])),
            2_000_000,
            1000,
            0,
            true,
        );
        assert!(advise(&facts).is_empty());
    }

    #[test]
    fn quiet_on_small_scans() {
        let facts = facts_from_plan(
            &plan("db.events", pk_index(10, 10, &["id"])),
            5_000,
            1,
            0,
            false,
        );
        assert!(advise(&facts).is_empty());
    }

    #[test]
    fn quiet_when_filter_matches_most_rows() {
        // Returning most of what was scanned: an index would not help.
        let facts = facts_from_plan(
            &plan("db.events", pk_index(120, 118, &["id"])),
            2_000_000,
            1_500_000,
            0,
            false,
        );
        assert!(advise(&facts).is_empty());
    }

    #[test]
    fn no_table_ddl_for_joins() {
        // Two MergeTree reads -> ambiguous table, so no templated DDL.
        let two = ExplainNode {
            node_type: "Join".to_string(),
            description: None,
            indexes: vec![],
            children: vec![
                plan("db.left", pk_index(120, 118, &["id"])),
                plan("db.right", pk_index(80, 79, &["id"])),
            ],
        };
        let facts = facts_from_plan(&two, 2_000_000, 12, 0, false);
        assert_eq!(facts.merge_tree_reads, 2);
        assert!(facts.table.is_none());
        let findings = advise(&facts);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].editor_sql.is_none());
    }

    #[test]
    fn advisable_select_recognizes_reads_only() {
        assert!(is_advisable_select("SELECT * FROM t"));
        assert!(is_advisable_select(
            "  with x as (select 1) select * from x"
        ));
        assert!(!is_advisable_select("INSERT INTO t VALUES (1)"));
        assert!(!is_advisable_select("ALTER TABLE t ADD COLUMN c Int32"));
        assert!(!is_advisable_select(""));
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(12), "12");
        assert_eq!(thousands(1_234), "1,234");
        assert_eq!(thousands(12_345_678), "12,345,678");
    }
}
