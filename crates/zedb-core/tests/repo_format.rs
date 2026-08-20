use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zedb_core::repo::{
    init_repo, render, scaffold_migration, MigrationRepo, RollbackClass, ScaffoldOptions,
};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-repo")
}

#[test]
fn fixture_repo_parses_completely() {
    let repo = MigrationRepo::open_root(&fixture()).unwrap();

    assert_eq!(repo.config.format, 1);
    assert_eq!(repo.config.engine.kind, "clickhouse");
    assert_eq!(repo.config.tracking.database, "default");
    assert!(repo.config.params.contains_key("ttl_days"));
    assert_eq!(repo.config.scopes["db"].param.as_deref(), Some("db"));

    let numbers: Vec<u32> = repo.migrations.iter().map(|m| m.number).collect();
    assert_eq!(numbers, [0, 100, 200]);

    let baseline = repo.migration(0).unwrap();
    assert_eq!(baseline.rollback_class, None);
    assert!(baseline.targeted.is_none());
    assert_eq!(
        baseline.headline().unwrap(),
        "migration 00000: baseline schema."
    );

    let view = repo.migration(100).unwrap();
    assert_eq!(view.rollback_class, Some(RollbackClass::Clean));

    let targeted = repo.migration(200).unwrap();
    assert_eq!(targeted.rollback_class, Some(RollbackClass::Structural));
    assert_eq!(
        targeted.targeted.as_deref(),
        Some(&["demo_pilot".to_string()][..])
    );

    assert!(repo.exclusions.is_excluded("demo_frozen"));
    assert!(!repo.exclusions.is_excluded("demo_other"));

    assert_eq!(repo.next_number(), 300);
}

#[test]
fn open_walks_up_to_the_repo_root() {
    let nested = fixture().join("migrations/2026/08");
    let repo = MigrationRepo::open(&nested).unwrap();
    assert_eq!(repo.root, fixture());
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

#[test]
fn duplicate_numbers_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    let duplicate = dir.path().join("migrations/2026/09/00100");
    std::fs::create_dir_all(&duplicate).unwrap();
    std::fs::write(duplicate.join("upgrade.sql"), "SELECT 1;\n").unwrap();

    let error = MigrationRepo::open_root(dir.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("00100"), "{error}");
    assert!(error.contains("more than once"), "{error}");
}

#[test]
fn misordered_months_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    let earlier = dir.path().join("migrations/2026/07/00300");
    std::fs::create_dir_all(&earlier).unwrap();
    std::fs::write(earlier.join("upgrade.sql"), "SELECT 1;\n").unwrap();

    let error = MigrationRepo::open_root(dir.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("dated before"), "{error}");
}

#[test]
fn unsafe_scope_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    let config_path = dir.path().join("zedb.toml");
    let config = std::fs::read_to_string(&config_path)
        .unwrap()
        .replace("global = { }", "\"../escape\" = { }");
    std::fs::write(config_path, config).unwrap();

    let error = MigrationRepo::open_root(dir.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("scope name"), "{error}");
    assert!(error.contains("unsafe"), "{error}");
}

#[test]
fn invalid_rollback_class_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    std::fs::write(
        dir.path().join("migrations/2026/08/00100/rollback.sql"),
        "-- rollback-class: gentle\nDROP VIEW x;\n",
    )
    .unwrap();

    let error = MigrationRepo::open_root(dir.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("rollback-class"), "{error}");
}

#[test]
fn undeclared_placeholders_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    std::fs::write(
        dir.path().join("migrations/2026/08/00100/upgrade.sql"),
        "CREATE VIEW ${db}.v AS SELECT ${mystery};\n",
    )
    .unwrap();

    let error = MigrationRepo::open_root(dir.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("${mystery}"), "{error}");
    assert!(error.contains("zedb.toml"), "{error}");
}

#[test]
fn malformed_layout_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    let shallow = dir.path().join("migrations/misc");
    std::fs::create_dir_all(&shallow).unwrap();
    std::fs::write(shallow.join("upgrade.sql"), "SELECT 1;\n").unwrap();

    let error = MigrationRepo::open_root(dir.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("YYYY/MM/NNNNN"), "{error}");
}

#[test]
fn init_and_scaffold_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fresh");
    init_repo(&root).unwrap();
    assert!(init_repo(&root).is_err(), "double init must refuse");

    let repo = MigrationRepo::open_root(&root).unwrap();
    assert!(repo.migrations.is_empty());

    let first = scaffold_migration(
        &repo,
        &ScaffoldOptions {
            description: "baseline".into(),
            targeted: false,
            with_rollback: false,
        },
    )
    .unwrap();
    assert!(first.join("upgrade.sql").is_file());
    assert!(!first.join("rollback.sql").exists());

    let repo = MigrationRepo::open_root(&root).unwrap();
    assert_eq!(repo.migrations.len(), 1);
    assert_eq!(repo.migrations[0].number, 0);

    let second = scaffold_migration(
        &repo,
        &ScaffoldOptions {
            description: "pilot flag".into(),
            targeted: true,
            with_rollback: true,
        },
    )
    .unwrap();
    assert_eq!(second.file_name().unwrap(), "00100");

    let repo = MigrationRepo::open_root(&root).unwrap();
    let numbers: Vec<u32> = repo.migrations.iter().map(|m| m.number).collect();
    assert_eq!(numbers, [0, 100]);
    assert_eq!(
        repo.migrations[1].rollback_class,
        Some(RollbackClass::Clean)
    );
    assert_eq!(repo.migrations[1].targeted.as_deref(), Some(&[][..]));
    assert_eq!(
        repo.migrations[1].headline().unwrap(),
        "migration 00100: pilot flag"
    );
}

#[test]
fn set_pinned_version_preserves_the_rest_of_the_config() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    let config_path = dir.path().join("zedb.toml");
    let mut text = std::fs::read_to_string(&config_path).unwrap();
    text.insert_str(0, "# keep this comment\n");
    std::fs::write(&config_path, text).unwrap();

    zedb_core::repo::RepoConfig::set_pinned_version(dir.path(), "26.3.12.3").unwrap();

    let rewritten = std::fs::read_to_string(&config_path).unwrap();
    assert!(rewritten.contains("# keep this comment"), "{rewritten}");
    let repo = MigrationRepo::open_root(dir.path()).unwrap();
    assert_eq!(repo.config.engine.version, "26.3.12.3");
}

#[test]
fn render_validates_identifier_params() {
    let sql = "CREATE TABLE ${db}.t TTL INTERVAL ${ttl_days} DAY;";
    let mut values = BTreeMap::new();
    values.insert("db".to_string(), "demo_a".to_string());
    values.insert("ttl_days".to_string(), "30".to_string());
    assert_eq!(
        render(sql, &values).unwrap(),
        "CREATE TABLE demo_a.t TTL INTERVAL 30 DAY;"
    );

    values.insert("db".to_string(), "demo; DROP TABLE".to_string());
    let error = render(sql, &values).unwrap_err().to_string();
    assert!(error.contains("plain identifier"), "{error}");

    values.remove("db");
    let error = render(sql, &values).unwrap_err().to_string();
    assert!(error.to_lowercase().contains("missing"), "{error}");
}
