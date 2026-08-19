//! Ephemeral `clickhouse local` replay and canonical schema dumps.
//!
//! The one authority on what a pile of DDL produces is ClickHouse itself:
//! statements are applied to an in-process server (the pinned binary, see
//! `pin`) and the canonical `create_table_query` is read back from
//! `system.tables`. Ported from the ancestor tooling's `canonical.py`;
//! see docs/contracts/FORMAT.md.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::Regex;
use zedb_core::repo::{render, RepoConfig};

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("applying SQL to clickhouse local failed:\n{0}")]
    Apply(String),
    #[error("clickhouse format rejected synthesized SQL:\n{stderr}\n{sql}")]
    Format { stderr: String, sql: String },
    #[error("{0}")]
    Template(String),
}

// The cluster name may be a bare identifier or quoted any way
// ClickHouse allows ('name', "name", `name`); all forms must decluster.
static ON_CLUSTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\s+ON\s+CLUSTER\s+('[^']*'|"[^"]*"|`[^`]*`|[A-Za-z_][A-Za-z0-9_]*)"#)
        .expect("static regex")
});
// ReplicatedXMergeTree('/zk/path', '{replica}'[, args]) -> XMergeTree([args]);
// ClickHouse Cloud's SharedXMergeTree takes the same two coordination
// arguments and declusters the same way.
static REPLICATED_ENGINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:Replicated|Shared)(\w*MergeTree)\(\s*'[^']*'\s*,\s*'[^']*'\s*(?:,\s*)?")
        .expect("static regex")
});
// Cloud can also render SharedMergeTree with no coordination clause at
// all (bare or with other args); the prefix still falls away.
static SHARED_BARE_ENGINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:Replicated|Shared)(\w*MergeTree)\b").expect("static regex"));
static ACCESS_CONTROL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(CREATE\s+(USER|ROLE)|GRANT|REVOKE|DROP\s+(USER|ROLE))\b")
        .expect("static regex")
});
static DEFINER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(SQL SECURITY DEFINER|DEFINER = \S+)").expect("static regex"));
static DROP_SYNC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+SYNC\s*$").expect("static regex"));

/// Rewrite rendered SQL for a single ephemeral node, where `ON CLUSTER`
/// and Replicated/Shared engines cannot work: coordination clauses are
/// dropped and Replicated engines (and Cloud's Shared variants) fall
/// back to their plain equivalents. The schema is otherwise identical.
pub fn decluster(sql: &str) -> String {
    let sql = ON_CLUSTER.replace_all(sql, "");
    // Coordination arguments go with the prefix when present; any
    // remaining Replicated/Shared prefix (no such clause) is dropped
    // on its own.
    let sql = REPLICATED_ENGINE.replace_all(&sql, "$1(");
    SHARED_BARE_ENGINE.replace_all(&sql, "$1").into_owned()
}

/// Split on top-level `;`, quote- and comment-aware; chunks keep comments.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            if c == '\\' {
                current.push_str(&sql[i..(i + 2).min(sql.len())]);
                i += 2;
                continue;
            }
            if c == '\'' {
                in_string = false;
            }
            current.push(c);
        } else if c == '\'' {
            in_string = true;
            current.push(c);
        } else if sql[i..].starts_with("--") {
            let end = sql[i..].find('\n').map(|n| i + n).unwrap_or(sql.len());
            current.push_str(&sql[i..end]);
            i = end;
            continue;
        } else if sql[i..].starts_with("/*") {
            let end = sql[i + 2..]
                .find("*/")
                .map(|n| i + 2 + n + 2)
                .unwrap_or(sql.len());
            current.push_str(&sql[i..end]);
            i = end;
            continue;
        } else if c == ';' {
            statements.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        statements.push(current);
    }
    statements
}

/// Is this statement access control (CREATE USER/ROLE, GRANT, ...)?
///
/// Those never appear in `system.tables` and `clickhouse local`'s default
/// user cannot run access management, so replays drop them.
pub fn is_access_control(statement: &str) -> bool {
    let body: String = statement
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    ACCESS_CONTROL.is_match(&body)
}

/// Collision-proof placeholder values for the regen round trip: rendered
/// SQL replays through `clickhouse local` and every value is distinctive
/// enough to be string-replaced back to `${name}` in the canonical dump.
/// `db`/`cluster` need identifier values; other parameters get distinct
/// numbers, valid in both numeric and string positions.
pub fn sentinel_params(config: &RepoConfig) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    params.insert("db".into(), "zz_sentinel_db".into());
    params.insert("cluster".into(), "zz_sentinel_cluster".into());
    let mut sentinel = 739_417u64;
    for (name, param) in &config.params {
        match &param.sentinel {
            Some(value) => params.insert(name.clone(), value.clone()),
            None => {
                let value = sentinel.to_string();
                // Steps chosen so no sentinel is a substring of another.
                sentinel += 20;
                params.insert(name.clone(), value)
            }
        };
    }
    params
}

/// Substitute a side's sentinel values back to `${name}` template form.
pub fn restore_sentinels(text: &str, params: &BTreeMap<String, String>) -> String {
    let mut restored = text.to_string();
    for (name, value) in params {
        restored = restored.replace(value.as_str(), &format!("${{{name}}}"));
    }
    restored
}

pub struct LocalReplay {
    binary: PathBuf,
}

/// One replay side: a name (which becomes its isolated database) and the
/// SQL texts to apply in order.
pub struct ReplaySide {
    pub name: String,
    pub texts: Vec<String>,
}

impl LocalReplay {
    pub fn new(binary: PathBuf) -> Self {
        Self { binary }
    }

    fn run_script(&self, script: &str) -> Result<String, ReplayError> {
        // Refresh scheduling stays off: pending initial refreshes of
        // refreshable MVs otherwise keep clickhouse-local from exiting.
        let mut child = Command::new(&self.binary)
            .args([
                "local",
                "-n",
                "--",
                "--stop_refreshable_materialized_views_on_startup=1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(script.as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(ReplayError::Apply(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Render, decluster, and strip a side's texts for replay.
    ///
    /// `isolate` rewrites the given literal database names to per-side
    /// copies so several sides can replay in one process without
    /// contaminating each other.
    fn prepare(
        side: &ReplaySide,
        params: &BTreeMap<String, String>,
        isolate: &[String],
    ) -> Result<String, ReplayError> {
        let mut params = params.clone();
        params.insert("db".into(), side.name.clone());
        let mut statements = Vec::new();
        for text in &side.texts {
            let rendered =
                render(text, &params).map_err(|error| ReplayError::Template(error.to_string()))?;
            let mut sql = decluster(&rendered);
            for shared in isolate {
                sql = sql.replace(shared.as_str(), &format!("{}__{shared}", side.name));
            }
            for statement in split_statements(&sql) {
                if statement.trim().is_empty() || is_access_control(&statement) {
                    continue;
                }
                let stripped = DEFINER.replace_all(statement.trim(), "");
                let stripped = DROP_SYNC.replace_all(&stripped, "");
                statements.push(format!("{stripped};"));
            }
        }
        Ok(statements.join("\n"))
    }

    /// Replay each side into isolated databases in one run and return per
    /// side `{database.name: create_table_query}` with rendered names
    /// still in place (see `restore_sentinels`).
    pub fn dump_objects(
        &self,
        sides: &[ReplaySide],
        params: &BTreeMap<String, String>,
        shared_databases: &[String],
    ) -> Result<BTreeMap<String, BTreeMap<String, String>>, ReplayError> {
        const MARKER: &str = "__CANONICAL_RESULTS__";
        let side_databases: Vec<(String, Vec<String>)> = sides
            .iter()
            .map(|side| {
                let mut databases = vec![side.name.clone()];
                databases.extend(
                    shared_databases
                        .iter()
                        .map(|shared| format!("{}__{shared}", side.name)),
                );
                (side.name.clone(), databases)
            })
            .collect();
        let all_databases: Vec<&String> = side_databases
            .iter()
            .flat_map(|(_, databases)| databases)
            .collect();

        let mut script = String::new();
        // Every side database is provisioned up front, like the real
        // fleet where databases exist before migrations run; chains
        // whose baseline creates ${db} itself are unaffected.
        for database in &all_databases {
            script.push_str(&format!("CREATE DATABASE IF NOT EXISTS {database};\n"));
        }
        for side in sides {
            script.push_str(&Self::prepare(side, params, shared_databases)?);
            script.push('\n');
        }
        let quoted: Vec<String> = all_databases
            .iter()
            .map(|database| format!("'{database}'"))
            .collect();
        script.push_str(&format!(
            "SELECT '{MARKER}';\nSELECT database, name, create_table_query FROM system.tables \
             WHERE database IN ({}) ORDER BY database, name FORMAT TSV;\n",
            quoted.join(", ")
        ));

        let output = self.run_script(&script)?;
        let results = output
            .split_once(MARKER)
            .map(|(_, rest)| rest)
            .unwrap_or("");
        let mut dumps: BTreeMap<String, BTreeMap<String, String>> = sides
            .iter()
            .map(|side| (side.name.clone(), BTreeMap::new()))
            .collect();
        for line in results.trim().lines() {
            let mut fields = line.splitn(3, '\t');
            let (Some(database), Some(name), Some(create)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            // Refreshable MVs keep staging state in generated .tmp/.inner
            // tables; implementation detail, not schema.
            if name.starts_with(".tmp") || name.starts_with(".inner") {
                continue;
            }
            let Some((side, _)) = side_databases
                .iter()
                .find(|(_, databases)| databases.iter().any(|d| d == database))
            else {
                continue;
            };
            dumps
                .entry(side.clone())
                .or_default()
                .insert(format!("{database}.{name}"), create.replace("\\'", "'"));
        }
        Ok(dumps)
    }

    /// Pretty-print one statement with the pinned ClickHouse formatter.
    pub fn format_sql(&self, sql: &str) -> Result<String, ReplayError> {
        let output = Command::new(&self.binary)
            .args(["format", "--query", sql])
            .output()?;
        if !output.status.success() {
            return Err(ReplayError::Format {
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                sql: sql.to_string(),
            });
        }
        let formatted = String::from_utf8_lossy(&output.stdout);
        Ok(format!("{}\n", formatted.trim_end().trim_end_matches(';')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declusters_every_cluster_name_quoting() {
        for statement in [
            "ALTER TABLE db.t ON CLUSTER main ADD COLUMN a UInt8",
            "ALTER TABLE db.t ON CLUSTER 'main' ADD COLUMN a UInt8",
            "ALTER TABLE db.t ON CLUSTER \"main\" ADD COLUMN a UInt8",
            "ALTER TABLE db.t ON CLUSTER `main` ADD COLUMN a UInt8",
            "ALTER TABLE db.t on cluster 'zedb_cluster' ADD COLUMN a UInt8",
        ] {
            let declustered = decluster(statement);
            assert_eq!(
                declustered, "ALTER TABLE db.t ADD COLUMN a UInt8",
                "failed for: {statement}"
            );
        }
    }

    #[test]
    fn decluster_leaves_unrelated_text_alone() {
        let statement = "SELECT 'ON CLUSTER x is just a string here'";
        assert_eq!(decluster(statement), statement);
    }

    #[test]
    fn declusters_shared_engines_with_and_without_coordination() {
        // The two-argument coordination clause goes with the prefix.
        assert_eq!(
            decluster("ENGINE = SharedMergeTree('/ch/t', '{replica}') ORDER BY x"),
            "ENGINE = MergeTree() ORDER BY x"
        );
        // Cloud can render the clause-less forms too.
        assert_eq!(
            decluster("ENGINE = SharedMergeTree ORDER BY x"),
            "ENGINE = MergeTree ORDER BY x"
        );
        assert_eq!(
            decluster("ENGINE = SharedMergeTree() ORDER BY x"),
            "ENGINE = MergeTree() ORDER BY x"
        );
        assert_eq!(
            decluster("ENGINE = SharedReplacingMergeTree(ver) ORDER BY x"),
            "ENGINE = ReplacingMergeTree(ver) ORDER BY x"
        );
    }
}
