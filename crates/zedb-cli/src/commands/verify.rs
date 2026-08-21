//! `zedb verify`: diff each live database against its own chain position.
//! Drift is a non-zero exit, so this is usable as a gate.

use std::path::Path;

use zedb_ch::runner::Runner;

use super::{open_repo, pinned_binary, runtime, terminal_field, terminal_text};
use crate::cli::{ReadConnectionArgs, TargetArgs};

pub fn verify(
    root: &Path,
    connection: ReadConnectionArgs,
    target_args: TargetArgs,
    json: bool,
) -> Result<(), String> {
    let repo = open_repo(root)?;
    let binary = pinned_binary(&repo)?;
    let runner = Runner::new(&repo, connection.options());
    let verifier = zedb_ch::verify::Verifier::new(&repo, &runner, binary);
    let targets = target_args.targets()?;
    let runtime = runtime()?;
    let drifts = runtime
        .block_on(verifier.verify(&targets))
        .map_err(|error| error.to_string())?;

    if json {
        let rows: Vec<serde_json::Value> = drifts
            .iter()
            .map(|drift| {
                serde_json::json!({
                    "database": drift.database,
                    "head": drift.head,
                    "findings": drift.findings,
                })
            })
            .collect();
        let clean = drifts.iter().all(|drift| drift.findings.is_empty());
        println!(
            "{}",
            serde_json::json!({ "databases": rows, "clean": clean })
        );
        return if clean {
            Ok(())
        } else {
            Err("drift detected".into())
        };
    }

    let mut drifted = false;
    for drift in &drifts {
        let head = drift
            .head
            .map(|head| format!("{head:05}"))
            .unwrap_or_else(|| "none".into());
        if drift.findings.is_empty() {
            println!("{}: clean at {head}", terminal_field(&drift.database));
        } else {
            drifted = true;
            println!(
                "{}: {} drift finding(s) at {head}",
                terminal_field(&drift.database),
                drift.findings.len()
            );
            for finding in &drift.findings {
                println!("  {}", terminal_text(finding));
            }
        }
    }
    if drifted {
        Err("drift detected".into())
    } else {
        Ok(())
    }
}
