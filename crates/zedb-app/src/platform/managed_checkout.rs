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

#[cfg(test)]
mod tests {
    use super::directory_name;

    #[test]
    fn same_basename_from_different_remotes_gets_different_paths() {
        let first = directory_name("git@github.com:one/settings.git");
        let second = directory_name("git@github.com:two/settings.git");
        assert_ne!(first, second);
        assert!(first.starts_with("settings-"));
        assert_eq!(first, directory_name("git@github.com:one/settings.git"));
    }
}
