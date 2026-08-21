//! Commands over the pinned ClickHouse build: caching it, and replaying the
//! chain through it to regenerate `current-state/`.

use std::path::Path;

use super::{open_repo, pinned_binary, runtime, terminal_field, terminal_text};

pub fn pin(
    root: &Path,
    server: Option<String>,
    user: Option<String>,
    password: Option<String>,
    pin_version: Option<String>,
) -> Result<(), String> {
    let repo = open_repo(root)?;
    let runtime = runtime()?;
    let version = match (pin_version, server) {
        (Some(version), _) => version,
        (None, Some(url)) => {
            let discovered = runtime
                .block_on(zedb_ch::discover_server_version(zedb_ch::ChConfig {
                    url,
                    user: user.unwrap_or_else(|| "default".into()),
                    password,
                    database: None,
                    read_only: true,
                    driver: Default::default(),
                    native_port: None,
                }))
                .map_err(|error| error.to_string())?;
            println!("server runs ClickHouse {}", terminal_field(&discovered));
            discovered
        }
        (None, None) => repo.config.engine.version.clone(),
    };
    let binary = runtime
        .block_on(zedb_ch::ensure_binary(&version))
        .map_err(|error| error.to_string())?;
    zedb_ch::smoke_replay(&binary).map_err(|error| error.to_string())?;
    if version != repo.config.engine.version {
        zedb_core::repo::RepoConfig::set_pinned_version(&repo.root, &version)
            .map_err(|error| error.to_string())?;
        println!(
            "zedb.toml [engine].version updated to {}",
            terminal_field(&version)
        );
    }
    println!(
        "pinned: ClickHouse {} at {}",
        terminal_field(&version),
        terminal_field(&binary.display().to_string())
    );
    Ok(())
}

pub fn regen(root: &Path, check: bool) -> Result<(), String> {
    let repo = open_repo(root)?;
    let binary = pinned_binary(&repo)?;
    let regenerator = zedb_ch::regen::Regenerator::new(&repo, binary);
    let files = regenerator
        .regenerate()
        .map_err(|error| error.to_string())?;
    if check {
        let problems =
            zedb_ch::regen::diff_tree(&repo, &files).map_err(|error| error.to_string())?;
        if problems.is_empty() {
            println!(
                "current-state/ matches the migration chain ({} files)",
                files.len()
            );
            return Ok(());
        }
        for problem in &problems {
            eprintln!("{}", terminal_text(problem));
        }
        return Err(format!(
            "current-state/ is out of date ({} problem(s)); run `zedb regen`",
            problems.len()
        ));
    }
    let (changed, removed) =
        zedb_ch::regen::write_tree(&repo, &files).map_err(|error| error.to_string())?;
    for path in &changed {
        println!("wrote {}", terminal_field(&path.display().to_string()));
    }
    for path in &removed {
        println!("removed {}", terminal_field(&path.display().to_string()));
    }
    println!(
        "current-state/: {} files ({} written, {} removed, {} unchanged)",
        files.len(),
        changed.len(),
        removed.len(),
        files.len() - changed.len()
    );
    Ok(())
}
