use std::path::{Path, PathBuf};

use zedb_ch::checks::{check_equivalence, check_sql};
use zedb_ch::regen::{write_tree, Regenerator};
use zedb_core::repo::MigrationRepo;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../zedb-core/tests/fixtures/demo-repo")
}

fn any_cached_binary() -> Option<PathBuf> {
    let root = zedb_ch::binary_cache_dir();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let version = entry.file_name().to_string_lossy().to_string();
        if let Some(binary) = zedb_ch::cached_binary(&version) {
            return Some(binary);
        }
    }
    None
}

fn copy_fixture(destination: &Path) {
    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    copy_tree(&fixture(), destination);
}

/// sql and equivalence checks pass on a healthy repo, and each catches
/// its own class of breakage with a readable message. Skips silently
/// without a cached binary so cold CI stays green.
#[test]
fn checks_pass_healthy_and_catch_breakage() {
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    let repo = MigrationRepo::open_root(dir.path()).unwrap();
    let files = Regenerator::new(&repo, binary.clone())
        .regenerate()
        .unwrap();
    write_tree(&repo, &files).unwrap();

    let sql = check_sql(&binary, &repo).unwrap();
    assert!(sql.errors.is_empty(), "{:?}", sql.errors);
    assert!(sql.checked >= 8, "{}", sql.checked);
    let equivalence = check_equivalence(binary.clone(), &repo).unwrap();
    assert!(
        equivalence.differences.is_empty(),
        "{:?}",
        equivalence.differences
    );
    assert_eq!(equivalence.state_objects, 2);

    // Broken SQL is caught with a file position and the server's message.
    let broken = dir.path().join("migrations/2026/09/00500");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join("upgrade.sql"),
        "CREATE VIEW ${db}.broken AS SELEKT 1;\n",
    )
    .unwrap();
    let repo = MigrationRepo::open_root(dir.path()).unwrap();
    let sql = check_sql(&binary, &repo).unwrap();
    assert_eq!(sql.errors.len(), 1);
    assert!(sql.errors[0].contains("upgrade.sql"), "{:?}", sql.errors);
    assert!(sql.errors[0].contains("SELEKT"), "{:?}", sql.errors);
    std::fs::remove_dir_all(&broken).unwrap();

    // A semantic hand edit to current-state is caught by equivalence.
    let view = dir
        .path()
        .join("current-state/db/00100_01_create_events_daily.sql");
    let text = std::fs::read_to_string(&view)
        .unwrap()
        .replace("count()", "count() + 1");
    std::fs::write(&view, text).unwrap();
    let repo = MigrationRepo::open_root(dir.path()).unwrap();
    let equivalence = check_equivalence(binary, &repo).unwrap();
    assert_eq!(equivalence.differences.len(), 1);
    assert!(
        equivalence.differences[0].contains("definition mismatch: ${db}.events_daily"),
        "{:?}",
        equivalence.differences
    );
}
