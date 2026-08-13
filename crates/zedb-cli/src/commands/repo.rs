//! Commands over the migration repo on disk. None of these touch a server.

use std::path::Path;

use zedb_core::repo::{init_repo, scaffold_migration, ScaffoldOptions};

use super::open_repo;

pub fn init(path: &Path) -> Result<(), String> {
    let config = init_repo(path).map_err(|error| error.to_string())?;
    println!("Initialized migration repo: {}", config.display());
    Ok(())
}

pub fn scaffold(
    root: &Path,
    description: String,
    targeted: bool,
    no_rollback: bool,
) -> Result<(), String> {
    let repo = open_repo(root)?;
    let directory = scaffold_migration(
        &repo,
        &ScaffoldOptions {
            description,
            targeted,
            with_rollback: !no_rollback,
        },
    )
    .map_err(|error| error.to_string())?;
    println!("Scaffolded {}", directory.display());
    Ok(())
}

pub fn ls(root: &Path) -> Result<(), String> {
    let repo = open_repo(root)?;
    if repo.migrations.is_empty() {
        println!("No migrations yet; scaffold one with `zedb new`.");
        return Ok(());
    }
    for migration in &repo.migrations {
        let class = migration
            .rollback_class
            .map(|class| class.as_str())
            .unwrap_or("no-rollback");
        let targeted = if migration.targeted.is_some() {
            "  targeted"
        } else {
            ""
        };
        let headline = migration.headline().unwrap_or_default();
        println!(
            "{:05}  {:04}/{:02}  {class:<12}{targeted}  {headline}",
            migration.number, migration.year, migration.month
        );
    }
    Ok(())
}

pub fn show(root: &Path, number: u32) -> Result<(), String> {
    let repo = open_repo(root)?;
    let migration = repo
        .migration(number)
        .ok_or_else(|| format!("no migration {number:05} in the chain"))?;
    println!("migration {:05}", migration.number);
    println!("directory: {}", migration.directory.display());
    match migration.rollback_class {
        Some(class) => println!("rollback-class: {}", class.as_str()),
        None => println!("rollback-class: none (treated as irreversible)"),
    }
    if let Some(allow_list) = &migration.targeted {
        if allow_list.is_empty() {
            println!("targeted: yes (any database)");
        } else {
            println!("targeted: yes, allow list {allow_list:?}");
        }
    }
    let upgrade = migration.upgrade_sql().map_err(|error| error.to_string())?;
    println!("\n-- upgrade.sql\n{upgrade}");
    if let Some(rollback) = migration
        .rollback_sql()
        .map_err(|error| error.to_string())?
    {
        println!("-- rollback.sql\n{rollback}");
    }
    Ok(())
}

pub fn import(ancestor: &Path, destination: &Path) -> Result<(), String> {
    let report =
        zedb_core::repo::import_repo(ancestor, destination).map_err(|error| error.to_string())?;
    println!(
        "imported {} migration(s) into {} (ClickHouse {}, {} exclusion group(s))",
        report.migrations,
        report.destination.display(),
        report.engine_version,
        report.exclusion_groups
    );
    println!("next: zedb pin, zedb regen, zedb check");
    Ok(())
}
