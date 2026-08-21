//! Commands that move databases along the chain, or report where they sit.

use std::path::Path;

use tokio::runtime::Runtime;
use zedb_ch::runner::{Runner, Targets};
use zedb_core::repo::MigrationRepo;

use super::{open_repo, runtime, terminal_field};
use crate::cli::{ConnectionArgs, ReadConnectionArgs, TargetArgs};

/// The repo, resolved targets, and runtime every fleet command opens with.
/// The `Runner` itself borrows the repo, so it stays with the caller.
fn prepare(root: &Path, targets: &TargetArgs) -> Result<(MigrationRepo, Targets, Runtime), String> {
    Ok((open_repo(root)?, targets.targets()?, runtime()?))
}

pub fn status(
    root: &Path,
    connection: ReadConnectionArgs,
    target_args: TargetArgs,
    json: bool,
) -> Result<(), String> {
    let (repo, targets, runtime) = prepare(root, &target_args)?;
    let runner = Runner::new(&repo, connection.options());
    let statuses = runtime
        .block_on(runner.status(&targets))
        .map_err(|error| error.to_string())?;

    if json {
        let rows: Vec<serde_json::Value> = statuses
            .iter()
            .map(|status| {
                serde_json::json!({
                    "database": status.database,
                    "head": status.head,
                    "latest": status.latest,
                    "pending": status.pending,
                    "customised": status.customised,
                    "failed": status.failed,
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "databases": rows }));
        return Ok(());
    }

    for status in statuses {
        let head = status
            .head
            .map(|head| format!("{head:05}"))
            .unwrap_or_else(|| "none".into());
        let state = if !status.pending.is_empty() {
            let pending: Vec<String> = status.pending.iter().map(|n| format!("{n:05}")).collect();
            format!("pending: {}", pending.join(", "))
        } else {
            "up to date".into()
        };
        let mut line = format!(
            "{}: at {head} of {:05}, {state}",
            terminal_field(&status.database),
            status.latest
        );
        if !status.customised.is_empty() {
            let customised: Vec<String> = status
                .customised
                .iter()
                .map(|n| format!("{n:05}"))
                .collect();
            line.push_str(&format!("; customised: {}", customised.join(", ")));
        }
        if !status.failed.is_empty() {
            let failed: Vec<String> = status
                .failed
                .iter()
                .map(|(n, action)| format!("{n:05} ({})", terminal_field(action)))
                .collect();
            line.push_str(&format!("; FAILED: {}", failed.join(", ")));
        }
        println!("{line}");
    }
    Ok(())
}

pub fn upgrade(
    root: &Path,
    connection: ConnectionArgs,
    target_args: TargetArgs,
    to: Option<u32>,
) -> Result<(), String> {
    let (repo, targets, runtime) = prepare(root, &target_args)?;
    let runner = Runner::new(&repo, connection.options()?);
    runtime
        .block_on(runner.upgrade(&targets, to))
        .map_err(|error| error.to_string())
}

pub fn rollback(
    root: &Path,
    connection: ConnectionArgs,
    target_args: TargetArgs,
    number: Option<u32>,
    to: Option<u32>,
    irreversible: bool,
    targeted: bool,
) -> Result<(), String> {
    let (repo, targets, runtime) = prepare(root, &target_args)?;
    let runner = Runner::new(&repo, connection.options()?);
    match (number, to) {
        (Some(number), None) => runtime
            .block_on(runner.rollback_one(&targets, number, irreversible, targeted))
            .map_err(|error| error.to_string()),
        (None, Some(floor)) => runtime
            .block_on(runner.rollback_to(&targets, floor, irreversible))
            .map_err(|error| error.to_string()),
        _ => Err("pass exactly one of NUMBER or --to TARGET".into()),
    }
}

pub fn stamp(
    root: &Path,
    connection: ConnectionArgs,
    target_args: TargetArgs,
    number: u32,
) -> Result<(), String> {
    let (repo, targets, runtime) = prepare(root, &target_args)?;
    let runner = Runner::new(&repo, connection.options()?);
    runtime
        .block_on(runner.stamp(&targets, number))
        .map_err(|error| error.to_string())
}

pub fn apply(
    root: &Path,
    connection: ConnectionArgs,
    target_args: TargetArgs,
    number: u32,
) -> Result<(), String> {
    let (repo, targets, runtime) = prepare(root, &target_args)?;
    let runner = Runner::new(&repo, connection.options()?);
    runtime
        .block_on(runner.apply_targeted(&targets, number))
        .map_err(|error| error.to_string())
}

pub fn import_tracking(
    root: &Path,
    connection: ConnectionArgs,
    from: String,
) -> Result<(), String> {
    let repo = open_repo(root)?;
    let dry_run = connection.dry_run;
    let runner = Runner::new(&repo, connection.options()?);
    let runtime = runtime()?;
    let imported = runtime
        .block_on(runner.import_tracking(&from))
        .map_err(|error| error.to_string())?;
    if dry_run {
        println!(
            "would import tracking rows from {from} (dry run checks no \
             preconditions; the real run still refuses if tracking \
             already has rows for this repo)"
        );
    } else {
        println!("imported {imported} tracking row(s) from {from}");
    }
    Ok(())
}
