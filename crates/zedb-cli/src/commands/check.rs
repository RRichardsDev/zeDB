//! `zedb check`: the repo checks, each independently selectable, reported
//! either for a human or as one JSON object for CI.

use std::path::Path;

use super::{open_repo, pinned_binary, runtime};

pub fn check(root: &Path, kind: String, json: bool) -> Result<(), String> {
    let repo = open_repo(root)?;
    let binary = pinned_binary(&repo)?;
    let mut failed = false;
    let mut output = serde_json::Map::new();

    if kind == "sql" || kind == "all" {
        let report =
            zedb_ch::checks::check_sql(&binary, &repo).map_err(|error| error.to_string())?;
        if json {
            output.insert(
                "sql".into(),
                serde_json::json!({
                    "checked": report.checked,
                    "errors": report.errors,
                }),
            );
        } else {
            for error in &report.errors {
                eprintln!("{error}");
            }
            println!(
                "sql: {}/{} SQL files parse as valid ClickHouse",
                report.checked - report.errors.len(),
                report.checked
            );
        }
        failed |= !report.errors.is_empty();
    }

    if kind == "equivalence" || kind == "all" {
        let report = zedb_ch::checks::check_equivalence(binary.clone(), &repo)
            .map_err(|error| error.to_string())?;
        if json {
            output.insert(
                "equivalence".into(),
                serde_json::json!({
                    "state_objects": report.state_objects,
                    "chain_objects": report.chain_objects,
                    "differences": report.differences,
                }),
            );
        } else {
            for difference in &report.differences {
                eprintln!("{difference}");
            }
            println!(
                "equivalence: {} current-state objects vs {} migration-chain objects, {} difference(s)",
                report.state_objects,
                report.chain_objects,
                report.differences.len()
            );
        }
        failed |= !report.differences.is_empty();
    }

    if kind == "lifecycle" || kind == "all" {
        let runtime = runtime()?;
        let report = runtime
            .block_on(zedb_ch::lifecycle::check_lifecycle(&repo, binary.clone()))
            .map_err(|error| error.to_string())?;
        if json {
            output.insert(
                "lifecycle".into(),
                serde_json::json!({
                    "steps": report.steps,
                    "expected_objects": report.expected_objects,
                    "live_objects": report.live_objects,
                    "differences": report.differences,
                }),
            );
        } else {
            for step in &report.steps {
                println!("lifecycle: {step}");
            }
            for difference in &report.differences {
                eprintln!("{difference}");
            }
            println!(
                "lifecycle: {} expected vs {} live objects, {} difference(s)",
                report.expected_objects,
                report.live_objects,
                report.differences.len()
            );
        }
        failed |= !report.differences.is_empty();
    }

    if json {
        output.insert("ok".into(), serde_json::json!(!failed));
        println!("{}", serde_json::Value::Object(output));
    }
    if failed {
        Err("checks failed".into())
    } else {
        Ok(())
    }
}
