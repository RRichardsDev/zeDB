//! EXPLAIN support: runs `EXPLAIN json = 1, indexes = 1` and parses
//! the plan tree, including per-read-node index pruning stats (the
//! "why is this slow" numbers).

use serde::Deserialize;

use crate::error::{ChError, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct ExplainNode {
    pub node_type: String,
    pub description: Option<String>,
    pub indexes: Vec<ExplainIndex>,
    pub children: Vec<ExplainNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExplainIndex {
    pub index_type: String,
    pub keys: Vec<String>,
    pub condition: Option<String>,
    pub initial_parts: u64,
    pub selected_parts: u64,
    pub initial_granules: u64,
    pub selected_granules: u64,
}

#[derive(Deserialize)]
struct RawRoot {
    #[serde(rename = "Plan")]
    plan: RawNode,
}

#[derive(Deserialize)]
struct RawNode {
    #[serde(rename = "Node Type")]
    node_type: String,
    #[serde(rename = "Description")]
    description: Option<String>,
    #[serde(rename = "Indexes", default)]
    indexes: Vec<RawIndex>,
    #[serde(rename = "Plans", default)]
    plans: Vec<RawNode>,
}

#[derive(Deserialize)]
struct RawIndex {
    #[serde(rename = "Type")]
    index_type: String,
    #[serde(rename = "Keys", default)]
    keys: Vec<String>,
    #[serde(rename = "Condition")]
    condition: Option<String>,
    #[serde(rename = "Initial Parts", default)]
    initial_parts: u64,
    #[serde(rename = "Selected Parts", default)]
    selected_parts: u64,
    #[serde(rename = "Initial Granules", default)]
    initial_granules: u64,
    #[serde(rename = "Selected Granules", default)]
    selected_granules: u64,
}

fn convert(raw: RawNode) -> ExplainNode {
    ExplainNode {
        node_type: raw.node_type,
        description: raw.description,
        indexes: raw
            .indexes
            .into_iter()
            .map(|index| ExplainIndex {
                index_type: index.index_type,
                keys: index.keys,
                condition: index.condition,
                initial_parts: index.initial_parts,
                selected_parts: index.selected_parts,
                initial_granules: index.initial_granules,
                selected_granules: index.selected_granules,
            })
            .collect(),
        children: raw.plans.into_iter().map(convert).collect(),
    }
}

/// Parse the single-cell JSON document `EXPLAIN json = 1` returns.
pub fn parse_explain_json(raw: &str) -> Result<ExplainNode> {
    let roots: Vec<RawRoot> = serde_json::from_str(raw)
        .map_err(|error| ChError::Decode(format!("EXPLAIN JSON did not parse: {error}")))?;
    roots
        .into_iter()
        .next()
        .map(|root| convert(root.plan))
        .ok_or_else(|| ChError::Decode("EXPLAIN returned no plan".into()))
}

/// The EXPLAIN statement for a user statement: trailing semicolon
/// stripped so the wrapped form parses.
pub fn explain_statement(sql: &str) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim_end();
    format!("EXPLAIN json = 1, indexes = 1 {trimmed}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plan_with_indexes() {
        let raw = r#"[{"Plan": {
            "Node Type": "Expression",
            "Node Id": "Expression_8",
            "Description": "(Project names + Projection)",
            "Plans": [{
                "Node Type": "ReadFromMergeTree",
                "Node Id": "ReadFromMergeTree_0",
                "Description": "sat.complexBulk",
                "Indexes": [{
                    "Type": "PrimaryKey",
                    "Keys": ["id"],
                    "Condition": "and((id in (-Inf, 5000]), (id in [1000, +Inf)))",
                    "Search Algorithm": "binary search",
                    "Initial Parts": 2,
                    "Selected Parts": 1,
                    "Initial Granules": 123,
                    "Selected Granules": 1
                }]
            }]
        }}]"#;
        let plan = parse_explain_json(raw).unwrap();
        assert_eq!(plan.node_type, "Expression");
        let read = &plan.children[0];
        assert_eq!(read.node_type, "ReadFromMergeTree");
        assert_eq!(read.description.as_deref(), Some("sat.complexBulk"));
        let index = &read.indexes[0];
        assert_eq!(index.index_type, "PrimaryKey");
        assert_eq!(index.selected_granules, 1);
        assert_eq!(index.initial_granules, 123);
    }

    #[test]
    fn explain_statement_strips_semicolon() {
        assert_eq!(
            explain_statement("SELECT 1;"),
            "EXPLAIN json = 1, indexes = 1 SELECT 1"
        );
    }
}
