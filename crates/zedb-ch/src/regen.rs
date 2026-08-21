//! Regenerate `current-state/` from the migration chain.
//!
//! Port of the ancestor tooling's `regen.py` onto the format-v1 repo
//! model. current-state is a build artifact: migrations are plain SQL and
//! regen works out the consequences, replaying through the pinned
//! `clickhouse local` when text tracking alone cannot (ALTER, RENAME,
//! EXCHANGE, ...). Files ClickHouse proves untouched are kept
//! byte-for-byte; a data-only migration causes zero churn. The synthesis
//! is self-verifying: regen re-replays its own output and fails loudly on
//! any residual difference.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use zedb_core::repo::{render, MigrationRepo};

use crate::replay::{
    restore_sentinels, sentinel_params, split_statements, LocalReplay, ReplaySide,
};

mod canonical;
mod synthesis;
mod tracking;
mod tree;

pub use tree::{diff_tree, write_tree};

#[derive(Debug, thiserror::Error)]
pub enum RegenError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Repo(String),
    #[error("replaying the migration chain failed:\n{0}")]
    Replay(String),
    #[error("{0}")]
    Track(String),
    #[error("internal: {0}")]
    Internal(String),
    #[error("unsafe current-state path: {0}")]
    UnsafePath(String),
}

const CHAIN_SIDE: &str = "zz_chain";
const CAND_SIDE: &str = "zz_cand";

static CREATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^CREATE\s+(?:OR\s+REPLACE\s+)?(?P<mat>MATERIALIZED\s+)?(?P<kind>TABLE|VIEW|DATABASE|USER|ROLE|DICTIONARY|FUNCTION)\s+(?:IF\s+NOT\s+EXISTS\s+)?(?P<name>[^\s(;]+)",
    )
    .expect("static regex")
});
static DROP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^DROP\s+(?:TABLE|VIEW|DICTIONARY|FUNCTION)\s+(?:IF\s+EXISTS\s+)?(?P<name>[^\s;]+)",
    )
    .expect("static regex")
});
static GRANT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^GRANT\s+(?P<what>.+?)\s+ON\s+(?P<target>\S+)\s+TO\s+(?P<grantee>\S+)")
        .expect("static regex")
});
static GRANT_ROLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^GRANT\s+(?P<role>\S+)\s+TO\s+(?P<user>\S+)").expect("static regex")
});
static REVOKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^REVOKE\s+(?P<what>.+?)\s+ON\s+(?P<target>\S+)\s+FROM\s+(?P<grantee>\S+)")
        .expect("static regex")
});
static REVOKE_ROLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^REVOKE\s+(?P<role>\S+)\s+FROM\s+(?P<user>\S+)").expect("static regex")
});
static RENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^RENAME\s+TABLE\s+(?P<old>[^\s,;]+)\s+TO\s+(?P<new>[^\s,;]+)\s*(?:ON\s+CLUSTER\s+\S+\s*)?$",
    )
    .expect("static regex")
});
static ACCESS_UNSUPPORTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:ALTER\s+(?:USER|ROLE)|CREATE\s+(?:ROW\s+POLICY|QUOTA|SETTINGS\s+PROFILE|NAMED\s+COLLECTION))\b",
    )
    .expect("static regex")
});
static DROP_ACCESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^DROP\s+(?P<kind>USER|ROLE)\s+(?:IF\s+EXISTS\s+)?(?P<name>[^\s;]+)")
        .expect("static regex")
});
static REPLICATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(Replicated|Shared)(\w*MergeTree)\(\s*('[^']*')\s*,\s*('[^']*')")
        .expect("static regex")
});
static STATE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<scope>[a-z0-9_]+)/(?P<mig>\d{5})_(?P<seq>\d{2})_(?P<stem>[A-Za-z0-9_]+)\.sql$",
    )
    .expect("static regex")
});
static IF_EXISTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bIF\s+EXISTS\b").expect("static regex"));
static ON_CLUSTER_DECL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bON\s+CLUSTER\b").expect("static regex"));

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Key {
    Object { kind: String, name: String },
    Grant { target: String, grantee: String },
    GrantRole { role: String, user: String },
}

/// Split a statement chunk into (attached comments, body): comment lines
/// contiguous with the statement attach to it; earlier blocks do not.
fn partition_chunk(chunk: &str) -> (String, String) {
    let lines: Vec<&str> = chunk.split('\n').collect();
    let first_sql = lines
        .iter()
        .position(|line| !line.trim().is_empty() && !line.trim_start().starts_with("--"));
    let Some(first_sql) = first_sql else {
        return (String::new(), String::new());
    };
    let mut attached_start = first_sql;
    while attached_start > 0 && lines[attached_start - 1].trim_start().starts_with("--") {
        attached_start -= 1;
    }
    let comments = if attached_start < first_sql {
        format!("{}\n", lines[attached_start..first_sql].join("\n"))
    } else {
        String::new()
    };
    let body = lines[first_sql..].join("\n").trim_matches('\n').to_string();
    (comments, body)
}

/// Backticks are optional in ClickHouse identifiers; normalize them away
/// so `${db}.X` and `${db}.`X`` map to the same object.
fn norm_name(name: &str) -> String {
    name.replace('`', "")
}

fn object_key(body: &str) -> Option<Key> {
    let body = body.trim();
    if let Some(captures) = CREATE.captures(body) {
        return Some(Key::Object {
            kind: captures["kind"].to_uppercase(),
            name: norm_name(&captures["name"]),
        });
    }
    if let Some(captures) = GRANT.captures(body) {
        return Some(Key::Grant {
            target: captures["target"].to_string(),
            grantee: captures["grantee"].to_string(),
        });
    }
    if let Some(captures) = GRANT_ROLE.captures(body) {
        return Some(Key::GrantRole {
            role: captures["role"].to_string(),
            user: captures["user"].to_string(),
        });
    }
    None
}

fn snake(text: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in text.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// Reduce a (possibly qualified/quoted) object name to a stem chunk.
/// Objects living in a shared (non-templated) database carry that
/// database's initials as a prefix, so `RefreshableViews.${db}_X` and
/// `${db}.X` never share a stem (the ancestor hardcoded `rv_`).
fn bare(name: &str, fallback: &str) -> String {
    let (qualifier, last) = match name.rsplit_once('.') {
        Some((qualifier, last)) => (qualifier, last),
        None => ("", name),
    };
    // Only per-database objects parked in a shared database need the
    // disambiguating prefix; fully shared objects keep bare stems.
    let prefix = if !qualifier.is_empty() && !qualifier.contains("${db}") && last.contains("${db}")
    {
        let initials: String = qualifier
            .chars()
            .filter(|c| c.is_ascii_uppercase())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let prefix = if initials.len() >= 2 {
            initials
        } else {
            snake(qualifier)
        };
        format!("{prefix}_")
    } else {
        String::new()
    };
    let cleaned = last
        .replace("${db}_", "")
        .replace("${db}", "")
        .trim_matches(|c| c == '\'' || c == '"' || c == '`' || c == '*')
        .to_string();
    let stem = snake(&cleaned);
    if stem.is_empty() {
        fallback.to_string()
    } else {
        format!("{prefix}{stem}")
    }
}

fn stem_for(key: &Key, body: &str) -> String {
    match key {
        Key::GrantRole { role, user } => {
            format!("grant_{}_to_{}", bare(role, "role"), bare(user, "user"))
        }
        Key::Grant { target, .. } => {
            let what = GRANT
                .captures(body.trim())
                .and_then(|captures| {
                    let what = captures["what"].to_string();
                    what.split_whitespace()
                        .next()
                        .map(|w| format!("{}_", w.to_lowercase()))
                })
                .unwrap_or_default();
            format!("grant_{what}{}", bare(target, "db"))
        }
        Key::Object { kind, name } => {
            let fallback = match kind.as_str() {
                "DATABASE" => "db",
                "USER" => "user",
                "ROLE" => "role",
                _ => "object",
            };
            format!("create_{}", bare(name, fallback))
        }
    }
}

pub struct Regenerator<'a> {
    repo: &'a MigrationRepo,
    replay: LocalReplay,
    /// Directory name for objects in the templated per-database scope.
    db_scope: String,
    /// Directory name for everything else.
    global_scope: String,
}

type Files = BTreeMap<String, String>;

#[cfg(test)]
mod engine_prefix_tests {
    use super::REPLICATED;

    #[test]
    fn captures_replicated_and_shared_alike() {
        for (body, kind) in [
            (
                "ENGINE = ReplicatedMergeTree('/zk/t', '{replica}') ORDER BY x",
                "Replicated",
            ),
            (
                "ENGINE = SharedMergeTree('/ch/t', '{replica}') ORDER BY x",
                "Shared",
            ),
        ] {
            let captures = REPLICATED.captures(body).expect(body);
            assert_eq!(&captures[1], kind);
            assert_eq!(&captures[2], "MergeTree");
        }
        assert!(REPLICATED
            .captures("ENGINE = MergeTree() ORDER BY x")
            .is_none());
    }
}
