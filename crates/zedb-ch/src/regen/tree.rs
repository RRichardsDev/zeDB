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
        ensure_safe_parent(&root, &target)?;
        let existing = std::fs::read_to_string(&target).ok();
        if existing.as_deref() != Some(content) {
            let parent = target.parent().expect("state files have parents");
            std::fs::create_dir_all(parent)?;
            ensure_safe_parent(&root, &target)?;
            if std::fs::symlink_metadata(&target)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(RegenError::UnsafePath(format!(
                    "refusing to replace symlink {target:?}"
                )));
            }
            let mut temporary = tempfile::Builder::new()
                .prefix(".zedb-regen-")
                .tempfile_in(parent)?;
            use std::io::Write as _;
            temporary.write_all(content.as_bytes())?;
            temporary.as_file().sync_all()?;
            temporary
                .persist(&target)
                .map_err(|error| RegenError::Io(error.error))?;
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
        ensure_safe_parent(&root, &target)?;
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
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(RegenError::UnsafePath(format!(
                "refusing to traverse symlink {path:?}"
            )));
        }
        if metadata.is_dir() {
            collect_sql(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "sql") {
            found.push(path);
        }
    }
    Ok(())
}

fn ensure_safe_parent(root: &std::path::Path, target: &std::path::Path) -> Result<(), RegenError> {
    let relative = target.strip_prefix(root).map_err(|_| {
        RegenError::UnsafePath(format!("path {target:?} escapes current-state {root:?}"))
    })?;
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(RegenError::UnsafePath(format!(
            "path {target:?} is not a confined relative path"
        )));
    }
    let mut current = root.to_path_buf();
    if std::fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(RegenError::UnsafePath(format!(
            "current-state root is a symlink: {root:?}"
        )));
    }
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    for component in parent.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(RegenError::UnsafePath(format!(
                    "path contains symlink {current:?}"
                )))
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(RegenError::UnsafePath(format!(
                    "path component is not a directory: {current:?}"
                )))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
