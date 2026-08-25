use sha2::{Digest, Sha256};

/// Stable, collision-resistant directory name for a managed checkout.
/// The readable prefix is cosmetic; the full remote identity selects the path.
pub(crate) fn directory_name(remote: &str) -> String {
    let base = zedb_core::git::clone_directory_name(remote);
    let digest = Sha256::digest(remote.trim().as_bytes());
    let suffix: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{base}-{suffix}")
}

/// The checkout path for `remote` under `base`, migrating a checkout
/// made before the suffixed names existed. Without the rename, every
/// pre-existing clone would be silently re-cloned at the new path and
/// the old one orphaned along with any uncommitted work in it.
pub(crate) fn checkout_path(base: &std::path::Path, remote: &str) -> std::path::PathBuf {
    let dest = base.join(directory_name(remote));
    if !dest.join(".git").exists() {
        let legacy = base.join(zedb_core::git::clone_directory_name(remote));
        if legacy != dest && legacy.join(".git").exists() {
            // Best effort: a failed rename leaves the legacy checkout
            // in use rather than abandoned.
            if std::fs::rename(&legacy, &dest).is_err() {
                return legacy;
            }
        }
    }
    dest
}

#[cfg(test)]
mod tests {
    use super::directory_name;

    #[test]
    fn legacy_unsuffixed_checkouts_are_migrated_not_orphaned() {
        let base = tempfile::tempdir().unwrap();
        let url = "git@github.com:one/settings.git";
        let legacy = base.path().join(zedb_core::git::clone_directory_name(url));
        std::fs::create_dir_all(legacy.join(".git")).unwrap();
        std::fs::write(legacy.join("uncommitted.txt"), "work").unwrap();

        let dest = super::checkout_path(base.path(), url);
        assert_eq!(dest, base.path().join(super::directory_name(url)));
        assert!(dest.join(".git").exists(), "the checkout moved");
        assert!(
            dest.join("uncommitted.txt").exists(),
            "local work moved with it"
        );
        assert!(!legacy.exists(), "nothing left behind to go stale");

        // Second call is a no-op returning the same path.
        assert_eq!(super::checkout_path(base.path(), url), dest);
    }

    #[test]
    fn same_basename_from_different_remotes_gets_different_paths() {
        let first = directory_name("git@github.com:one/settings.git");
        let second = directory_name("git@github.com:two/settings.git");
        assert_ne!(first, second);
        assert!(first.starts_with("settings-"));
        assert_eq!(first, directory_name("git@github.com:one/settings.git"));
    }
}
