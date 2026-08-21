//! `zedb import`: converting an analytics-clickhouse-ddl repo into format 1.
//!
//! This is a one-shot, one-way conversion of someone's real repo, so the
//! cases that matter most are the refusals: it must not half-convert into a
//! directory that already holds a repo, and it must not accept a directory
//! that only looks like an ancestor.

use std::path::Path;

use zedb_core::repo::{import_repo, MigrationRepo};

/// A minimal but genuine ancestor: the chain layout is unchanged between
/// generations, so these files are what `import` copies verbatim.
fn ancestor(root: &Path, version: &str) {
    let chain = root.join("migrations/2026/08");
    std::fs::create_dir_all(chain.join("00000")).unwrap();
    std::fs::write(
        chain.join("00000/upgrade.sql"),
        "-- migration 00000: baseline.\nCREATE TABLE ${db}.events (id UInt64) ENGINE = MergeTree ORDER BY id;\n",
    )
    .unwrap();
    std::fs::create_dir_all(chain.join("00100")).unwrap();
    std::fs::write(
        chain.join("00100/upgrade.sql"),
        "-- migration 00100: rollup.\nCREATE VIEW ${db}.events_daily AS SELECT id FROM ${db}.events;\n",
    )
    .unwrap();
    std::fs::write(
        chain.join("00100/rollback.sql"),
        "-- rollback-class: clean\nDROP VIEW IF EXISTS ${db}.events_daily;\n",
    )
    .unwrap();
    pin(root, &format!("CH_VERSION = \"{version}\"\n"));
}

fn pin(root: &Path, contents: &str) {
    let dir = root.join("clickhouse_ddl");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pin.py"), contents).unwrap();
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn imports_the_chain_and_writes_a_format_1_config() {
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("imported");
    ancestor(&from, "24.3.1.1");

    let report = import_repo(&from, &to).expect("import should succeed");
    assert_eq!(report.migrations, 2);
    assert_eq!(report.engine_version, "24.3.1.1");
    assert_eq!(report.exclusion_groups, 0);
    assert_eq!(report.destination, to);

    // Migrations copy byte for byte: the conversion supplies configuration,
    // it does not rewrite anyone's SQL.
    for relative in [
        "migrations/2026/08/00000/upgrade.sql",
        "migrations/2026/08/00100/upgrade.sql",
        "migrations/2026/08/00100/rollback.sql",
    ] {
        assert_eq!(
            std::fs::read_to_string(from.join(relative)).unwrap(),
            std::fs::read_to_string(to.join(relative)).unwrap(),
            "{relative} should be copied verbatim"
        );
    }

    let config = std::fs::read_to_string(to.join("zedb.toml")).unwrap();
    assert!(config.contains("format = 1"), "{config}");
    assert!(config.contains(r#"version = "24.3.1.1""#), "{config}");
    // The ancestor's dummy and sentinel values come across, so checks and
    // regen render exactly as they did before the conversion.
    assert!(
        config.contains(r#"dummy = "rendertest_password""#),
        "{config}"
    );
    assert!(
        config.contains(r#"sentinel = "zz_sentinel_password""#),
        "{config}"
    );

    // The result is a repo the rest of the tooling can open, which is the
    // real proof the generated config is valid rather than merely present.
    let repo = MigrationRepo::open_root(&to).expect("imported repo should open");
    assert_eq!(
        repo.migrations.iter().map(|m| m.number).collect::<Vec<_>>(),
        [0, 100]
    );
    assert_eq!(repo.config.engine.version, "24.3.1.1");
}

#[test]
fn current_state_is_created_empty_rather_than_copied() {
    // It is a generated artifact; `zedb regen` derives it under this
    // tooling's own naming and `zedb check equivalence` proves the match.
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("imported");
    ancestor(&from, "24.3.1.1");
    std::fs::create_dir_all(from.join("current-state")).unwrap();
    std::fs::write(from.join("current-state/events.sql"), "CREATE TABLE stale;").unwrap();

    import_repo(&from, &to).expect("import should succeed");

    assert!(to.join("current-state").is_dir());
    assert_eq!(
        std::fs::read_dir(to.join("current-state")).unwrap().count(),
        0,
        "the ancestor's generated tree must not come across"
    );
}

#[test]
fn exclusion_groups_are_carried_over_and_counted() {
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("imported");
    ancestor(&from, "24.3.1.1");
    let exclusions = "[groups.frozen]\nreason = \"Under an incident freeze.\"\n\
                      databases = [\"demo_frozen\"]\n\n\
                      [groups.retired]\nreason = \"Retired; kept for reads.\"\n\
                      databases = [\"demo_retired\"]\n";
    std::fs::write(from.join("exceptions.toml"), exclusions).unwrap();

    let report = import_repo(&from, &to).expect("import should succeed");

    assert_eq!(report.exclusion_groups, 2);
    assert_eq!(
        std::fs::read_to_string(to.join("exclusions.toml")).unwrap(),
        exclusions
    );
    let repo = MigrationRepo::open_root(&to).unwrap();
    assert!(repo.exclusions.is_excluded("demo_frozen"));
    assert!(!repo.exclusions.is_excluded("demo_other"));
}

#[test]
fn an_ancestor_without_exclusions_imports_cleanly() {
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("imported");
    ancestor(&from, "24.3.1.1");

    let report = import_repo(&from, &to).expect("import should succeed");

    assert_eq!(report.exclusion_groups, 0);
    assert!(!to.join("exclusions.toml").exists());
}

#[test]
fn refuses_a_destination_that_is_already_a_repo() {
    // Converting into a live repo would interleave two chains.
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("imported");
    ancestor(&from, "24.3.1.1");
    std::fs::create_dir_all(&to).unwrap();
    std::fs::write(to.join("zedb.toml"), "format = 1\n").unwrap();

    let error = import_repo(&from, &to).expect_err("should refuse");
    assert!(
        error.to_string().contains("imported"),
        "should name the destination: {error}"
    );
    // The existing config is left exactly as it was.
    assert_eq!(
        std::fs::read_to_string(to.join("zedb.toml")).unwrap(),
        "format = 1\n"
    );
}

#[test]
fn refuses_a_directory_that_is_not_an_ancestor_repo() {
    let dir = temp();
    let from = dir.path().join("not-a-repo");
    std::fs::create_dir_all(&from).unwrap();

    let error = import_repo(&from, &dir.path().join("imported")).expect_err("should refuse");
    assert!(
        error.to_string().contains("analytics-clickhouse-ddl"),
        "the error should say what was expected: {error}"
    );
}

#[test]
fn refuses_when_the_pinned_version_is_missing_or_unreadable() {
    // No pin.py at all.
    let dir = temp();
    let from = dir.path().join("ancestor");
    std::fs::create_dir_all(from.join("migrations")).unwrap();
    let error = import_repo(&from, &dir.path().join("a")).expect_err("should refuse");
    assert!(error.to_string().contains("pinned version"), "{error}");

    // A pin.py that never assigns CH_VERSION.
    pin(&from, "SOMETHING_ELSE = \"24.3.1.1\"\n");
    let error = import_repo(&from, &dir.path().join("b")).expect_err("should refuse");
    assert!(error.to_string().contains("CH_VERSION"), "{error}");
}

#[test]
fn the_pinned_version_is_read_unquoted_and_untrimmed_of_content() {
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("imported");
    ancestor(&from, "24.3.1.1");
    // Surrounding noise and spacing must not end up in the version.
    pin(
        &from,
        "import os\n\n  CH_VERSION   =   \"25.1.2.3\"  \n\nOTHER = 1\n",
    );

    let report = import_repo(&from, &to).expect("import should succeed");
    assert_eq!(report.engine_version, "25.1.2.3");
}

#[test]
fn refuses_any_preexisting_destination_without_changing_it() {
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("occupied");
    ancestor(&from, "24.3.1.1");
    std::fs::create_dir_all(&to).unwrap();
    std::fs::write(to.join("keep.txt"), "keep").unwrap();

    let error = import_repo(&from, &to).expect_err("occupied destination must be refused");
    assert!(
        error.to_string().contains("must not already exist"),
        "{error}"
    );
    assert_eq!(
        std::fs::read_to_string(to.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(!to.join("zedb.toml").exists());
}

#[test]
fn refuses_a_destination_nested_inside_the_ancestor() {
    let dir = temp();
    let from = dir.path().join("ancestor");
    ancestor(&from, "24.3.1.1");
    let to = from.join("migrations/imported");

    let error = import_repo(&from, &to).expect_err("overlapping destination must be refused");
    assert!(
        error.to_string().contains("outside the ancestor"),
        "{error}"
    );
    assert!(!to.exists());
}

#[test]
fn refuses_versions_that_can_change_generated_toml() {
    let dir = temp();
    let from = dir.path().join("ancestor");
    let to = dir.path().join("imported");
    ancestor(&from, "24.3.1.1");
    pin(
        &from,
        "CH_VERSION = \"24.3.1.1\\\"\\n[tracking]\\ndatabase = \\\"attacker\"\"\n",
    );

    let error = import_repo(&from, &to).expect_err("unsafe version must be refused");
    assert!(
        error.to_string().contains("unsafe ClickHouse version"),
        "{error}"
    );
    assert!(
        !to.exists(),
        "validation must finish before destination creation"
    );
}

#[cfg(unix)]
#[test]
fn refuses_file_and_directory_symlinks_in_the_ancestor_tree() {
    use std::os::unix::fs::symlink;

    for directory_link in [false, true] {
        let dir = temp();
        let from = dir.path().join("ancestor");
        let to = dir.path().join("imported");
        ancestor(&from, "24.3.1.1");
        let migration = from.join("migrations/2026/08/00100");

        if directory_link {
            let outside = dir.path().join("outside-directory");
            std::fs::create_dir(&outside).unwrap();
            std::fs::write(outside.join("private.txt"), "private").unwrap();
            symlink(&outside, migration.join("linked-directory")).unwrap();
        } else {
            let outside = dir.path().join("outside-file");
            std::fs::write(&outside, "private").unwrap();
            symlink(&outside, migration.join("linked-file.sql")).unwrap();
        }

        let error = import_repo(&from, &to).expect_err("source symlink must be refused");
        assert!(error.to_string().contains("symlink"), "{error}");
        assert!(
            !to.exists(),
            "validation must finish before destination creation"
        );
    }
}
