use super::*;

/// Write the generated tree under `current-state/`; returns
/// (changed, removed) paths.
pub fn write_tree(
    repo: &MigrationRepo,
    files: &Files,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), RegenError> {
    let root = repo.root.join("current-state");
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    for (relative, content) in files {
        let target = root.join(relative);
        let existing = std::fs::read_to_string(&target).ok();
        if existing.as_deref() != Some(content) {
            std::fs::create_dir_all(target.parent().expect("state files have parents"))?;
            std::fs::write(&target, content)?;
            changed.push(target);
        }
    }
    let mut stale = Vec::new();
    collect_sql(&root, &mut stale)?;
    for existing in stale {
        let relative = existing
            .strip_prefix(&root)
            .expect("collected under root")
            .to_string_lossy()
            .to_string();
        if !files.contains_key(&relative) {
            std::fs::remove_file(&existing)?;
            removed.push(existing);
        }
    }
    Ok((changed, removed))
}

/// Compare the generated tree to disk; returns human-readable differences.
pub fn diff_tree(repo: &MigrationRepo, files: &Files) -> Result<Vec<String>, RegenError> {
    let root = repo.root.join("current-state");
    let mut problems = Vec::new();
    for (relative, content) in files {
        let target = root.join(relative);
        match std::fs::read_to_string(&target) {
            Err(_) => problems.push(format!("missing: {relative}")),
            Ok(existing) if existing != *content => {
                problems.push(format!("stale (regen would rewrite): {relative}"));
            }
            Ok(_) => {}
        }
    }
    let mut on_disk = Vec::new();
    collect_sql(&root, &mut on_disk)?;
    on_disk.sort();
    for existing in on_disk {
        let relative = existing
            .strip_prefix(&root)
            .expect("collected under root")
            .to_string_lossy()
            .to_string();
        if !files.contains_key(&relative) {
            problems.push(format!(
                "extraneous (not derived from migrations): {relative}"
            ));
        }
    }
    Ok(problems)
}

fn collect_sql(dir: &std::path::Path, found: &mut Vec<PathBuf>) -> Result<(), RegenError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            collect_sql(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "sql") {
            found.push(path);
        }
    }
    Ok(())
}
