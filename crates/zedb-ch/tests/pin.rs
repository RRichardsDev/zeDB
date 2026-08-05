use zedb_ch::{binary_cache_dir, cached_binary, smoke_replay};

#[test]
fn cache_layout_is_per_version() {
    let dir = binary_cache_dir();
    assert!(dir.ends_with("zedb/clickhouse"), "{}", dir.display());
}

#[test]
fn absent_versions_report_no_cache_hit() {
    assert!(cached_binary("0.0.0.0").is_none());
}

/// Exercises the cache-hit and smoke-replay paths when any real binary is
/// already cached (seeded by `zedb pin`); skips silently on a cold machine
/// so CI without a cache stays green.
#[test]
fn cached_binaries_pass_smoke_replay() {
    let root = binary_cache_dir();
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!("skipping: no binary cache at {}", root.display());
        return;
    };
    for entry in entries.flatten() {
        let version = entry.file_name().to_string_lossy().to_string();
        if let Some(binary) = cached_binary(&version) {
            smoke_replay(&binary).unwrap();
            return;
        }
    }
    eprintln!("skipping: cache exists but holds no usable binary");
}
