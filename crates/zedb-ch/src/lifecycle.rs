//! The lifecycle check: everything the fleet has taught us, as a
//! repeatable gate (docs/PHASE-1.md M4, ported from the ancestor's
//! `checks/lifecycle.py`).
//!
//! 1. Start a throwaway single-node *clustered* server: `ON CLUSTER` DDL
//!    goes through the real distributed queue and Replicated engines
//!    coordinate through the embedded Keeper; nothing is declustered at
//!    apply time.
//! 2. Create a `migrator` user shaped like a real provisioning user:
//!    broad DDL and access-management grants, but no OPTIMIZE, TRUNCATE,
//!    or CREATE FUNCTION. When the chain contains statements that user is
//!    refused, the check first proves the run fails without admin
//!    credentials, keeping admin routing load-bearing.
//! 3. Provision a database from the 00000 baseline, `stamp` it, then
//!    `upgrade` the rest of the chain as the migrator (refused statements
//!    route through admin).
//! 4. Smoke-test each targeted customisation (apply, then remove).
//! 5. Walk every rollback from the top down, then upgrade back.
//! 6. Diff the resulting live schema against replayed current-state.

use std::collections::BTreeMap;
use std::path::PathBuf;

use zedb_core::repo::{render, MigrationRepo};

use crate::checks::{current_state_files, dummy_params};
use crate::ephemeral::EphemeralServer;
use crate::replay::{split_statements, LocalReplay, ReplaySide};
use crate::runner::{refused_without_admin, Runner, RunnerOptions, Targets};
use crate::verify::diff_norm;
use crate::{ChClient, ChConfig};

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("could not start the ephemeral clustered server: {0}")]
    Server(String),
    #[error("bootstrap failed: {0}")]
    Bootstrap(String),
    #[error("step {step}: {message}")]
    Step { step: String, message: String },
    #[error("{0}")]
    Repo(String),
}

pub struct LifecycleReport {
    /// Narration of each completed step, for humans reading check output.
    pub steps: Vec<String>,
    pub expected_objects: usize,
    pub live_objects: usize,
    pub differences: Vec<String>,
}

const DB: &str = "lifecycle_db";
const CLUSTER: &str = "lifecycle_cluster";

/// Grants mirroring a real provisioning user (the ancestor re-checked
/// these against staging): SELECT with grant option, INSERT, ALTER
/// DELETE, CREATE, DROP, SET DEFINER, access management, ROLE ADMIN, and
/// CLUSTER (required for any ON CLUSTER query); deliberately no
/// OPTIMIZE, TRUNCATE, or CREATE FUNCTION.
const MIGRATOR_BOOTSTRAP: &[&str] = &[
    "CREATE USER migrator IDENTIFIED WITH plaintext_password BY 'migrator_pw'",
    "GRANT SELECT ON *.* TO migrator WITH GRANT OPTION",
    "GRANT INSERT, ALTER DELETE, CREATE, DROP, SET DEFINER ON *.* TO migrator",
    "GRANT ROLE ADMIN ON *.* TO migrator",
    "GRANT CREATE USER, ALTER USER, DROP USER, CREATE ROLE, ALTER ROLE, DROP ROLE ON *.* TO migrator",
    "GRANT CLUSTER ON *.* TO migrator",
];

fn provision_params(repo: &MigrationRepo) -> BTreeMap<String, String> {
    let mut params = dummy_params(repo);
    params.insert("db".into(), DB.into());
    params.insert("cluster".into(), CLUSTER.into());
    params
}

fn config(url: &str, user: &str, password: Option<&str>) -> ChConfig {
    ChConfig {
        url: url.to_string(),
        user: user.to_string(),
        password: password.map(ToString::to_string),
        database: None,
        read_only: false,
        driver: Default::default(),
    }
}

pub async fn check_lifecycle(
    repo: &MigrationRepo,
    binary: PathBuf,
) -> Result<LifecycleReport, LifecycleError> {
    let mut steps = Vec::new();
    let server = EphemeralServer::start_clustered(&binary, CLUSTER)
        .map_err(|error| LifecycleError::Server(error.to_string()))?;
    let admin = ChClient::new(config(&server.http_url, "default", None));

    // Bootstrap: the migrator user and the shared databases cluster
    // bootstrap would provide.
    for statement in MIGRATOR_BOOTSTRAP {
        admin
            .execute(statement)
            .await
            .map_err(|error| LifecycleError::Bootstrap(error.to_string()))?;
    }
    for shared in &repo.config.replay.shared_databases {
        admin
            .execute(&format!(
                "CREATE DATABASE IF NOT EXISTS {shared} ON CLUSTER {CLUSTER}"
            ))
            .await
            .map_err(|error| LifecycleError::Bootstrap(error.to_string()))?;
    }

    let step = |steps: &mut Vec<String>, text: String| {
        steps.push(text);
    };

    // [1] Provision from the baseline, exactly as written (ON CLUSTER,
    // Replicated engines), as admin: provisioning is bootstrap's job.
    let chain: Vec<_> = repo
        .migrations
        .iter()
        .filter(|migration| migration.targeted.is_none())
        .collect();
    let Some(baseline) = chain.first().filter(|migration| migration.number == 0) else {
        return Err(LifecycleError::Repo(
            "baseline migration 00000 not found".into(),
        ));
    };
    let params = provision_params(repo);
    let baseline_sql = baseline
        .upgrade_sql()
        .map_err(|error| LifecycleError::Repo(error.to_string()))?;
    let rendered =
        render(&baseline_sql, &params).map_err(|error| LifecycleError::Repo(error.to_string()))?;
    for statement in split_statements(&rendered) {
        let has_sql = statement
            .lines()
            .any(|line| !line.trim().is_empty() && !line.trim().starts_with("--"));
        if !has_sql {
            continue;
        }
        admin
            .execute(statement.trim())
            .await
            .map_err(|error| LifecycleError::Step {
                step: "provision baseline".into(),
                message: error.to_string(),
            })?;
    }
    step(
        &mut steps,
        format!("provisioned {DB} from the 00000 baseline ON CLUSTER {CLUSTER}"),
    );

    // Everything after provisioning runs as the migrator, through the
    // same runner real fleets use; refused statements route to admin.
    let overrides: BTreeMap<String, String> = params
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let runner_options = |admin: bool| RunnerOptions {
        server: config(&server.http_url, "migrator", Some("migrator_pw")),
        admin: admin.then(|| config(&server.http_url, "default", None)),
        cluster: Some(CLUSTER.into()),
        no_cluster: false,
        write: true,
        dry_run: false,
        overrides: overrides.clone(),
    };
    let runner = Runner::new(repo, runner_options(true));
    let targets = Targets::Databases(vec![DB.into()]);
    let fail = |step: &str, error: String| LifecycleError::Step {
        step: step.into(),
        message: error,
    };

    // [2] Stamp the baseline. When the chain contains statements the
    // migration user is genuinely refused, first prove the run *fails*
    // without admin credentials: that keeps admin routing load-bearing.
    runner
        .stamp(&targets, 0)
        .await
        .map_err(|error| fail("stamp 00000", error.to_string()))?;
    let chain_needs_admin = chain.iter().skip(1).any(|migration| {
        migration
            .upgrade_sql()
            .map(|sql| {
                split_statements(&sql)
                    .iter()
                    .any(|s| refused_without_admin(s))
            })
            .unwrap_or(false)
    });
    if chain_needs_admin {
        let no_admin = Runner::new(repo, runner_options(false));
        if no_admin.upgrade(&targets, None).await.is_ok() {
            return Err(fail(
                "no-admin assertion",
                "the chain contains statements the migration user should be refused,                  but the upgrade succeeded without admin credentials"
                    .into(),
            ));
        }
        step(
            &mut steps,
            "upgrade without admin credentials failed as it must".into(),
        );
    }
    runner
        .upgrade(&targets, None)
        .await
        .map_err(|error| fail("upgrade with admin routing", error.to_string()))?;
    let statuses = runner
        .status(&targets)
        .await
        .map_err(|error| fail("status", error.to_string()))?;
    let status = statuses.first().expect("one database was targeted");
    if !status.pending.is_empty() {
        return Err(fail(
            "status after upgrade",
            format!("still pending: {:?}", status.pending),
        ));
    }
    step(
        &mut steps,
        format!(
            "stamped 00000 and upgraded to {:05} as migrator",
            status.latest
        ),
    );

    // [3] Smoke-test targeted customisations: apply, then remove.
    let targeted: Vec<u32> = repo
        .migrations
        .iter()
        .filter(|migration| migration.targeted.is_some())
        .map(|migration| migration.number)
        .collect();
    for number in &targeted {
        runner
            .apply_targeted_for_check(&targets, *number)
            .await
            .map_err(|error| fail(&format!("apply {number:05}"), error.to_string()))?;
        runner
            .rollback_one(&targets, *number, true, true)
            .await
            .map_err(|error| fail(&format!("remove {number:05}"), error.to_string()))?;
    }
    if !targeted.is_empty() {
        step(
            &mut steps,
            format!(
                "smoke-tested {} targeted customisation(s) (allow lists bypassed \
                 for the ephemeral database)",
                targeted.len()
            ),
        );
    }

    // [4] Walk every rollback from the top down (irreversible included:
    // the ephemeral server has nothing to lose), then upgrade back.
    let rollbackable: Vec<u32> = chain
        .iter()
        .rev()
        .filter(|migration| migration.rollback_class.is_some())
        .map(|migration| migration.number)
        .collect();
    for number in &rollbackable {
        runner
            .rollback_one(&targets, *number, true, false)
            .await
            .map_err(|error| fail(&format!("rollback {number:05}"), error.to_string()))?;
    }
    runner
        .upgrade(&targets, None)
        .await
        .map_err(|error| fail("upgrade back to the top", error.to_string()))?;
    step(
        &mut steps,
        format!(
            "walked {} rollback(s) down and upgraded back",
            rollbackable.len()
        ),
    );

    // [5] Diff the live schema against replayed current-state.
    let shared_predicates: String = repo
        .config
        .replay
        .shared_databases
        .iter()
        .map(|shared| format!(" OR (database = '{shared}' AND name LIKE '{DB}\\_%')"))
        .collect();
    let live_rows = admin
        .query(&format!(
            "SELECT database, name, create_table_query FROM system.tables \
             WHERE (database = '{DB}'{shared_predicates}) AND name NOT LIKE '.%'"
        ))
        .await
        .map_err(|error| fail("live schema dump", error.to_string()))?;
    let live: BTreeMap<String, String> = live_rows
        .rows
        .iter()
        .filter_map(|row| {
            let database = row.first()?.to_string();
            let name = row.get(1)?.to_string();
            let create = row.get(2)?.to_string();
            Some((
                format!("{database}.{name}").replace(DB, "${db}"),
                diff_norm(&create, DB),
            ))
        })
        .collect();

    let state_texts: Vec<String> = current_state_files(repo)
        .map_err(|error| LifecycleError::Repo(error.to_string()))?
        .iter()
        .map(std::fs::read_to_string)
        .collect::<Result<_, _>>()
        .map_err(|error| LifecycleError::Repo(error.to_string()))?;
    let replay = LocalReplay::new(binary);
    let dumps = replay
        .dump_objects(
            &[ReplaySide {
                name: DB.into(),
                texts: state_texts,
            }],
            &dummy_params(repo),
            &repo.config.replay.shared_databases,
        )
        .map_err(|error| fail("expected-side replay", error.to_string()))?;
    let strip_isolation = |text: &str| text.replace(&format!("{DB}__"), "");
    let expected: BTreeMap<String, String> = dumps[DB]
        .iter()
        .map(|(name, create)| {
            (
                strip_isolation(name).replace(DB, "${db}"),
                diff_norm(&strip_isolation(create), DB),
            )
        })
        // Match the live query's scope: the database itself, plus this
        // database's objects parked in shared databases. Fully shared
        // objects (cluster bootstrap state) are out of a per-database
        // lifecycle's scope, same as the ancestor's diff.
        .filter(|(name, _)| {
            name.starts_with("${db}.")
                || name
                    .split_once('.')
                    .is_some_and(|(_, object)| object.starts_with("${db}_"))
        })
        .collect();

    let mut differences = Vec::new();
    let mut names: Vec<&String> = expected.keys().chain(live.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        match (expected.get(name), live.get(name)) {
            (Some(_), None) => differences.push(format!("missing on server: {name}")),
            (None, Some(_)) => differences.push(format!("unexpected on server: {name}")),
            (Some(expected), Some(live)) if expected != live => differences.push(format!(
                "mismatch: {name}\n  server:   {live}\n  expected: {expected}"
            )),
            _ => {}
        }
    }
    step(
        &mut steps,
        "diffed live schema against replayed current-state".into(),
    );

    Ok(LifecycleReport {
        steps,
        expected_objects: expected.len(),
        live_objects: live.len(),
        differences,
    })
}
