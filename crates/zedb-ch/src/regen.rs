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
    Regex::new(r"Replicated(\w*MergeTree)\(\s*('[^']*')\s*,\s*('[^']*')").expect("static regex")
});
static STATE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<scope>[a-z0-9_]+)/(?P<mig>\d{5})_(?P<seq>\d{2})_(?P<stem>[A-Za-z0-9_]+)\.sql$",
    )
    .expect("static regex")
});
static IF_EXISTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bIF\s+EXISTS\b").expect("static regex"));

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
fn bare(name: &str, fallback: &str) -> String {
    let last = name.split('.').next_back().unwrap_or(name);
    let cleaned = last
        .replace("${db}_", "")
        .replace("${db}", "")
        .trim_matches(|c| c == '\'' || c == '"' || c == '`' || c == '*')
        .to_string();
    let stem = snake(&cleaned);
    if stem.is_empty() {
        fallback.to_string()
    } else {
        stem
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

impl<'a> Regenerator<'a> {
    pub fn new(repo: &'a MigrationRepo, binary: PathBuf) -> Self {
        let db_scope = repo
            .config
            .scopes
            .iter()
            .find(|(_, scope)| scope.param.as_deref() == Some("db"))
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "db".into());
        let global_scope = repo
            .config
            .scopes
            .iter()
            .find(|(_, scope)| scope.param.is_none())
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "global".into());
        Self {
            repo,
            replay: LocalReplay::new(binary),
            db_scope,
            global_scope,
        }
    }

    fn key_scope(&self, key: &Key) -> &str {
        let names = match key {
            Key::Object { name, .. } => vec![name.as_str()],
            Key::Grant { target, grantee } => vec![target.as_str(), grantee.as_str()],
            Key::GrantRole { role, user } => vec![role.as_str(), user.as_str()],
        };
        if names.iter().any(|name| name.contains("${db}")) {
            &self.db_scope
        } else {
            &self.global_scope
        }
    }

    fn provision_order(&self, files: &Files) -> Vec<String> {
        let mut global: Vec<String> = files
            .keys()
            .filter(|path| path.starts_with(&format!("{}/", self.global_scope)))
            .cloned()
            .collect();
        let mut db: Vec<String> = files
            .keys()
            .filter(|path| path.starts_with(&format!("{}/", self.db_scope)))
            .cloned()
            .collect();
        global.sort();
        db.sort();
        global.extend(db);
        global
    }

    /// Literal (non-templated) databases the chain creates: these are
    /// shared across the fleet and isolated per replay side.
    fn shared_databases(&self, chain_texts: &[String]) -> Vec<String> {
        let mut shared = Vec::new();
        for text in chain_texts {
            for chunk in split_statements(text) {
                let (_, body) = partition_chunk(&chunk);
                if let Some(captures) = CREATE.captures(body.trim()) {
                    if captures["kind"].eq_ignore_ascii_case("database") {
                        let name = norm_name(&captures["name"]);
                        if !name.contains("${") && !shared.contains(&name) {
                            shared.push(name);
                        }
                    }
                }
            }
        }
        shared
    }

    /// Replay the migration chain; returns `{relpath: content}` for
    /// current-state/.
    pub fn regenerate(&self) -> Result<Files, RegenError> {
        let chain: Vec<_> = self
            .repo
            .migrations
            .iter()
            .filter(|migration| migration.targeted.is_none())
            .collect();
        let Some(first) = chain.first() else {
            return Ok(Files::new());
        };
        if first.number != 0 {
            return Err(RegenError::Track(
                "baseline migration 00000 not found".into(),
            ));
        }

        let mut files: Files = Files::new();
        let mut key_to_path: HashMap<Key, String> = HashMap::new();

        // Baseline: every statement creates an object.
        let baseline_text = first
            .upgrade_sql()
            .map_err(|error| RegenError::Repo(error.to_string()))?;
        let mut counters: HashMap<String, u32> = HashMap::new();
        for chunk in split_statements(&baseline_text) {
            let (comments, body) = partition_chunk(&chunk);
            if body.is_empty() {
                continue;
            }
            let key = object_key(&body).ok_or_else(|| {
                RegenError::Track(format!(
                    "baseline: cannot identify the object created by {:?}; the baseline may only contain CREATE and GRANT statements",
                    body.lines().next().unwrap_or_default()
                ))
            })?;
            if key_to_path.contains_key(&key) {
                return Err(RegenError::Track(format!(
                    "baseline: {key:?} is defined twice"
                )));
            }
            let path = self.place(&key, &body, 0, &mut counters)?;
            files.insert(path.clone(), format!("{comments}{body};\n"));
            key_to_path.insert(key, path);
        }

        let mut dropped: HashSet<String> = HashSet::new();
        let mut needs_replay = false;
        for migration in chain.iter().skip(1) {
            let text = migration
                .upgrade_sql()
                .map_err(|error| RegenError::Repo(error.to_string()))?;
            let mut counters: HashMap<String, u32> = HashMap::new();
            for chunk in split_statements(&text) {
                let (comments, body) = partition_chunk(&chunk);
                if body.is_empty() {
                    continue;
                }
                let trimmed = body.trim().to_string();
                if ACCESS_UNSUPPORTED.is_match(&trimmed) {
                    return Err(RegenError::Track(format!(
                        "migration {:05}: regen cannot track {:?}; express access-control changes as CREATE [OR REPLACE] USER/ROLE, GRANT, REVOKE, or DROP USER/ROLE",
                        migration.number,
                        trimmed.lines().next().unwrap_or_default()
                    )));
                }
                if let Some(captures) = DROP_ACCESS.captures(&trimmed) {
                    self.apply_drop_access(
                        &captures,
                        migration.number,
                        &key_to_path,
                        &mut dropped,
                        &trimmed,
                    )?;
                    continue;
                }
                if let Some(captures) = DROP.captures(&trimmed) {
                    let target = norm_name(&captures["name"]);
                    let key = key_to_path
                        .keys()
                        .find(|key| matches!(key, Key::Object { name, .. } if *name == target));
                    match key {
                        Some(key) => {
                            dropped.insert(key_to_path[key].clone());
                        }
                        None if IF_EXISTS.is_match(&trimmed) => {}
                        None => {
                            return Err(RegenError::Track(format!(
                                "migration {:05}: DROP of unknown object {target:?}",
                                migration.number
                            )));
                        }
                    }
                    continue;
                }
                if trimmed.to_uppercase().starts_with("RENAME") {
                    let captures = RENAME.captures(&trimmed).ok_or_else(|| {
                        RegenError::Track(format!(
                            "migration {:05}: only a single 'RENAME TABLE a TO b' is supported",
                            migration.number
                        ))
                    })?;
                    self.apply_rename(
                        &captures,
                        migration.number,
                        &mut files,
                        &mut key_to_path,
                        &mut dropped,
                    )?;
                    needs_replay = true;
                    continue;
                }
                if trimmed.to_uppercase().starts_with("REVOKE") {
                    self.apply_revoke(
                        &trimmed,
                        migration.number,
                        &files,
                        &key_to_path,
                        &mut dropped,
                    )?;
                    continue;
                }
                match object_key(&body) {
                    Some(key) => {
                        let content = format!("{comments}{body};\n");
                        if let Some(path) = key_to_path.get(&key) {
                            files.insert(path.clone(), content);
                            dropped.remove(path);
                        } else {
                            let path = self.place(&key, &body, migration.number, &mut counters)?;
                            if files.contains_key(&path) {
                                return Err(RegenError::Track(format!(
                                    "migration {:05}: {path:?} collides with an existing file",
                                    migration.number
                                )));
                            }
                            files.insert(path.clone(), content);
                            key_to_path.insert(key, path);
                        }
                    }
                    None => {
                        // ALTER, EXCHANGE, INSERT, OPTIMIZE, guards, ...:
                        // the canonical replay picks up any consequence.
                        needs_replay = true;
                    }
                }
            }
        }

        files.retain(|path, _| !dropped.contains(path));
        key_to_path.retain(|_, path| files.contains_key(path));
        self.assign_db_scope_paths(&mut files, &mut key_to_path)?;

        if needs_replay {
            let chain_texts: Vec<String> = chain
                .iter()
                .map(|migration| migration.upgrade_sql())
                .collect::<Result<_, _>>()
                .map_err(|error| RegenError::Repo(error.to_string()))?;
            self.canonical_phase(&chain_texts, &mut files, &mut key_to_path)?;
            self.assign_db_scope_paths(&mut files, &mut key_to_path)?;
        }

        Ok(files)
    }

    fn place(
        &self,
        key: &Key,
        body: &str,
        migration: u32,
        counters: &mut HashMap<String, u32>,
    ) -> Result<String, RegenError> {
        let scope = self.key_scope(key).to_string();
        let counter = counters.entry(scope.clone()).or_insert(0);
        *counter += 1;
        if *counter > 99 {
            return Err(RegenError::Track(format!(
                "migration {migration:05} introduces more than 99 {scope} objects"
            )));
        }
        Ok(format!(
            "{scope}/{migration:05}_{:02}_{}.sql",
            counter,
            stem_for(key, body)
        ))
    }

    fn apply_drop_access(
        &self,
        captures: &regex::Captures,
        migration: u32,
        key_to_path: &HashMap<Key, String>,
        dropped: &mut HashSet<String>,
        body: &str,
    ) -> Result<(), RegenError> {
        let kind = captures["kind"].to_uppercase();
        let name = norm_name(&captures["name"]);
        let stripped = name.trim_matches(|c| c == '\'' || c == '"').to_string();
        let key = key_to_path.keys().find(|key| {
            matches!(key, Key::Object { kind: k, name: n }
                if *k == kind && norm_name(n).trim_matches(|c| c == '\'' || c == '"') == stripped)
        });
        let Some(key) = key else {
            if IF_EXISTS.is_match(body) {
                return Ok(());
            }
            return Err(RegenError::Track(format!(
                "migration {migration:05}: DROP {kind} of unknown {name:?}"
            )));
        };
        dropped.insert(key_to_path[key].clone());
        // A dropped user/role takes its grants with it, as on the server.
        for (other, path) in key_to_path {
            let grantee_like: Vec<&String> = match other {
                Key::Grant { grantee, .. } => vec![grantee],
                Key::GrantRole { role, user } => vec![role, user],
                Key::Object { .. } => vec![],
            };
            if grantee_like
                .iter()
                .any(|g| norm_name(g).trim_matches(|c| c == '\'' || c == '"') == stripped)
            {
                dropped.insert(path.clone());
            }
        }
        Ok(())
    }

    fn apply_rename(
        &self,
        captures: &regex::Captures,
        migration: u32,
        files: &mut Files,
        key_to_path: &mut HashMap<Key, String>,
        dropped: &mut HashSet<String>,
    ) -> Result<(), RegenError> {
        let old_raw = captures["old"].to_string();
        let new_raw = captures["new"].to_string();
        let old = norm_name(&old_raw);
        let new = norm_name(&new_raw);
        let key = key_to_path
            .keys()
            .find(|key| matches!(key, Key::Object { name, .. } if *name == old))
            .cloned()
            .ok_or_else(|| {
                RegenError::Track(format!(
                    "migration {migration:05}: RENAME of unknown object {old:?}"
                ))
            })?;
        let old_path = key_to_path.remove(&key).expect("key was found above");
        let mut content = files.remove(&old_path).ok_or_else(|| {
            RegenError::Internal(format!("rename source file {old_path:?} is missing"))
        })?;
        for variant in [old_raw.as_str(), old.as_str()] {
            content = content.replace(variant, &new_raw);
        }
        let Key::Object { kind, .. } = key else {
            unreachable!("rename keys are objects");
        };
        let new_key = Key::Object { kind, name: new };
        let new_path = match STATE_PATH.captures(&old_path) {
            Some(path_captures) if path_captures["scope"] == *self.db_scope => format!(
                "{}/{}_{}_{}.sql",
                &path_captures["scope"],
                &path_captures["mig"],
                &path_captures["seq"],
                stem_for(&new_key, "")
            ),
            _ => old_path.clone(),
        };
        files.insert(new_path.clone(), content);
        key_to_path.insert(new_key, new_path);
        dropped.remove(&old_path);
        Ok(())
    }

    fn apply_revoke(
        &self,
        body: &str,
        migration: u32,
        files: &Files,
        key_to_path: &HashMap<Key, String>,
        dropped: &mut HashSet<String>,
    ) -> Result<(), RegenError> {
        if let Some(captures) = REVOKE.captures(body) {
            let key = Key::Grant {
                target: captures["target"].to_string(),
                grantee: captures["grantee"].to_string(),
            };
            let path = key_to_path.get(&key).ok_or_else(|| {
                RegenError::Track(format!(
                    "migration {migration:05}: REVOKE does not match any tracked GRANT (on {} from {})",
                    &captures["target"], &captures["grantee"]
                ))
            })?;
            let revoked = captures["what"]
                .to_uppercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let (_, granted_body) =
                partition_chunk(files[path].trim_end_matches('\n').trim_end_matches(';'));
            let granted = GRANT
                .captures(granted_body.trim())
                .map(|c| {
                    c["what"]
                        .to_uppercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            if revoked != "ALL" && revoked != "ALL PRIVILEGES" && revoked != granted {
                return Err(RegenError::Track(format!(
                    "migration {migration:05}: partial REVOKE ({revoked:?} of the tracked {granted:?}); REVOKE the full grant and re-GRANT what remains"
                )));
            }
            dropped.insert(path.clone());
            return Ok(());
        }
        if let Some(captures) = REVOKE_ROLE.captures(body) {
            let key = Key::GrantRole {
                role: captures["role"].to_string(),
                user: captures["user"].to_string(),
            };
            let path = key_to_path.get(&key).ok_or_else(|| {
                RegenError::Track(format!(
                    "migration {migration:05}: REVOKE does not match any tracked role grant ({} from {})",
                    &captures["role"], &captures["user"]
                ))
            })?;
            dropped.insert(path.clone());
            return Ok(());
        }
        Err(RegenError::Track(format!(
            "migration {migration:05}: cannot parse REVOKE statement {:?}",
            body.lines().next().unwrap_or_default()
        )))
    }

    /// Renumber db-scope files so every object sorts after what it
    /// references: a deterministic Kahn walk over whole-identifier
    /// references; in the common case nothing moves.
    fn assign_db_scope_paths(
        &self,
        files: &mut Files,
        key_to_path: &mut HashMap<Key, String>,
    ) -> Result<(), RegenError> {
        let prefix = format!("{}/", self.db_scope);
        let scope_paths: Vec<String> = files
            .keys()
            .filter(|path| path.starts_with(&prefix))
            .cloned()
            .collect();
        let mut base_key: HashMap<String, (u32, u32)> = HashMap::new();
        let mut stems: HashMap<String, String> = HashMap::new();
        for path in &scope_paths {
            let captures = STATE_PATH.captures(path).ok_or_else(|| {
                RegenError::Internal(format!(
                    "current-state path {path:?} does not follow scope/NNNNN_SS_name.sql"
                ))
            })?;
            base_key.insert(
                path.clone(),
                (
                    captures["mig"].parse().expect("five digits"),
                    captures["seq"].parse().expect("two digits"),
                ),
            );
            stems.insert(path.clone(), captures["stem"].to_string());
        }

        let referenceable: HashMap<String, String> = key_to_path
            .iter()
            .filter_map(|(key, path)| match key {
                Key::Object { kind, name }
                    if matches!(kind.as_str(), "TABLE" | "VIEW" | "DICTIONARY" | "FUNCTION")
                        && base_key.contains_key(path) =>
                {
                    Some((path.clone(), name.clone()))
                }
                _ => None,
            })
            .collect();

        let file_kind = |content: &str| -> Option<String> {
            let (_, body) = partition_chunk(content);
            CREATE.captures(body.trim()).map(|captures| {
                if captures.name("mat").is_some() {
                    "MATERIALIZED VIEW".to_string()
                } else {
                    captures["kind"].to_uppercase()
                }
            })
        };
        let references = |content: &str, name: &str| -> bool {
            let (_, body) = partition_chunk(content);
            let body = body.replace('`', "");
            let mut start = 0;
            while let Some(found) = body[start..].find(name) {
                let end = start + found + name.len();
                let boundary = body[end..]
                    .chars()
                    .next()
                    .map(|c| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(true);
                if boundary {
                    return true;
                }
                start = start + found + 1;
            }
            false
        };

        let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
        for path in &scope_paths {
            let kind = file_kind(&files[path]);
            let edges = match kind.as_deref() {
                None | Some("USER") | Some("ROLE") | Some("DATABASE") => HashSet::new(),
                _ => referenceable
                    .iter()
                    .filter(|(other, name)| *other != path && references(&files[path], name))
                    .map(|(other, _)| other.clone())
                    .collect(),
            };
            deps.insert(path.clone(), edges);
        }

        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for (path, edges) in &deps {
            for dep in edges {
                dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(path.clone());
            }
        }
        let mut remaining: HashMap<String, HashSet<String>> = deps.clone();
        let mut ready: Vec<String> = scope_paths
            .iter()
            .filter(|path| remaining[*path].is_empty())
            .cloned()
            .collect();
        ready.sort_by_key(|path| base_key[path]);
        let mut assigned: HashMap<String, (u32, u32)> = HashMap::new();
        let mut used: HashSet<(u32, u32)> = HashSet::new();
        while let Some(path) = ready.first().cloned() {
            ready.remove(0);
            let mut candidate = base_key[&path];
            for dep in &deps[&path] {
                let dep_assigned = assigned[dep];
                if dep_assigned >= candidate {
                    candidate = (dep_assigned.0, dep_assigned.1 + 1);
                }
            }
            while used.contains(&candidate) {
                candidate = (candidate.0, candidate.1 + 1);
            }
            if candidate.1 > 99 {
                return Err(RegenError::Track(format!(
                    "ran out of statement-index slots placing {path:?}"
                )));
            }
            assigned.insert(path.clone(), candidate);
            used.insert(candidate);
            if let Some(waiters) = dependents.get(&path) {
                let mut newly_ready = Vec::new();
                for waiter in waiters {
                    let edges = remaining.get_mut(waiter).expect("waiter has deps");
                    edges.remove(&path);
                    if edges.is_empty() {
                        newly_ready.push(waiter.clone());
                    }
                }
                ready.extend(newly_ready);
                ready.sort_by_key(|p| base_key[p]);
                ready.dedup();
            }
        }
        if assigned.len() != scope_paths.len() {
            let mut cycle: Vec<&String> = scope_paths
                .iter()
                .filter(|path| !assigned.contains_key(*path))
                .collect();
            cycle.sort();
            return Err(RegenError::Track(format!(
                "circular references among current-state objects: {cycle:?}"
            )));
        }

        let rename: HashMap<String, String> = scope_paths
            .iter()
            .map(|path| {
                let (mig, seq) = assigned[path];
                (
                    path.clone(),
                    format!("{}/{mig:05}_{seq:02}_{}.sql", self.db_scope, stems[path]),
                )
            })
            .collect();
        let old_files = std::mem::take(files);
        for (path, content) in old_files {
            let new_path = rename.get(&path).cloned().unwrap_or(path);
            files.insert(new_path, content);
        }
        for path in key_to_path.values_mut() {
            if let Some(new_path) = rename.get(path) {
                *path = new_path.clone();
            }
        }
        Ok(())
    }

    fn canonical_phase(
        &self,
        chain_texts: &[String],
        files: &mut Files,
        key_to_path: &mut HashMap<Key, String>,
    ) -> Result<(), RegenError> {
        let params = sentinel_params(&self.repo.config);
        let shared = self.shared_databases(chain_texts);

        let name_of = |key: &Key| -> Option<String> {
            match key {
                Key::Object { name, .. } => Some(name.clone()),
                _ => None,
            }
        };
        let normalize = |dump: &BTreeMap<String, String>, side: &str| -> BTreeMap<String, String> {
            dump.iter()
                .map(|(name, create)| {
                    let strip = |text: &str| {
                        restore_sentinels(
                            &text
                                .replace(&format!("{side}__"), "")
                                .replace(side, "${db}"),
                            &params,
                        )
                    };
                    (strip(name), strip(create))
                })
                .collect()
        };

        let base_kinds = [
            "DATABASE",
            "TABLE",
            "DICTIONARY",
            "FUNCTION",
            "USER",
            "ROLE",
        ];
        let file_kind = |content: &str| -> Option<String> {
            let (_, body) = partition_chunk(content);
            CREATE.captures(body.trim()).map(|captures| {
                if captures.name("mat").is_some() {
                    "MATERIALIZED VIEW".to_string()
                } else {
                    captures["kind"].to_uppercase()
                }
            })
        };

        let base_texts: Vec<String> = self
            .provision_order(files)
            .into_iter()
            .filter(|path| {
                let kind = file_kind(&files[path]);
                kind.is_none()
                    || kind
                        .as_deref()
                        .map(|k| base_kinds.contains(&k))
                        .unwrap_or(true)
            })
            .map(|path| files[&path].clone())
            .collect();

        let run_a = self
            .replay
            .dump_objects(
                &[
                    ReplaySide {
                        name: CHAIN_SIDE.into(),
                        texts: chain_texts.to_vec(),
                    },
                    ReplaySide {
                        name: CAND_SIDE.into(),
                        texts: base_texts,
                    },
                ],
                &params,
                &shared,
            )
            .map_err(|error| RegenError::Replay(error.to_string()))?;

        let chain_norm = normalize(&run_a[CHAIN_SIDE], CHAIN_SIDE);
        let chain_raw: BTreeMap<String, String> = run_a[CHAIN_SIDE]
            .iter()
            .map(|(name, create)| {
                (
                    restore_sentinels(
                        &name
                            .replace(&format!("{CHAIN_SIDE}__"), "")
                            .replace(CHAIN_SIDE, "${db}"),
                        &params,
                    ),
                    create.clone(),
                )
            })
            .collect();
        let mut synthesized: HashSet<String> = HashSet::new();

        let reconcile = |files: &mut Files,
                             key_to_path: &mut HashMap<Key, String>,
                             cand_norm: &BTreeMap<String, String>,
                             synthesized: &mut HashSet<String>,
                             second_pass: bool|
         -> Result<bool, RegenError> {
            let mut changed = false;
            let paths: HashMap<String, String> = key_to_path
                .iter()
                .filter_map(|(key, path)| name_of(key).map(|name| (name, path.clone())))
                .collect();
            let mut names: Vec<&String> = chain_norm.keys().chain(cand_norm.keys()).collect();
            names.sort();
            names.dedup();
            for name in names {
                if !cand_norm.contains_key(name) {
                    if second_pass {
                        return Err(RegenError::Track(format!(
                            "the migration chain produces {name:?} but no migration ever CREATEs it; add a CREATE statement for it"
                        )));
                    }
                    continue;
                }
                if !chain_norm.contains_key(name) {
                    if let Some(path) = paths.get(name) {
                        files.remove(path);
                        key_to_path.retain(|_, p| p != path);
                        changed = true;
                    }
                    continue;
                }
                if chain_norm[name] == cand_norm[name] {
                    continue;
                }
                if synthesized.contains(name) {
                    return Err(RegenError::Internal(format!(
                        "synthesis round-trip mismatch for {name}\n  chain: {}\n  regen: {}",
                        chain_norm[name], cand_norm[name]
                    )));
                }
                let path = paths.get(name).ok_or_else(|| {
                    RegenError::Internal(format!(
                        "replayed object {name:?} maps to no current-state file"
                    ))
                })?;
                let new_content = self.synthesize(&chain_raw[name], &files[path], &params)?;
                files.insert(path.clone(), new_content);
                synthesized.insert(name.clone());
                changed = true;
            }
            Ok(changed)
        };

        reconcile(
            files,
            key_to_path,
            &normalize(&run_a[CAND_SIDE], CAND_SIDE),
            &mut synthesized,
            false,
        )?;

        let run_b = self
            .replay
            .dump_objects(
                &[ReplaySide {
                    name: CAND_SIDE.into(),
                    texts: self
                        .provision_order(files)
                        .into_iter()
                        .map(|path| files[&path].clone())
                        .collect(),
                }],
                &params,
                &shared,
            )
            .map_err(|error| RegenError::Replay(error.to_string()))?;
        let before_second = synthesized.clone();
        let changed_second = reconcile(
            files,
            key_to_path,
            &normalize(&run_b[CAND_SIDE], CAND_SIDE),
            &mut synthesized,
            true,
        )?;

        if changed_second || synthesized != before_second {
            let run_c = self
                .replay
                .dump_objects(
                    &[ReplaySide {
                        name: CAND_SIDE.into(),
                        texts: self
                            .provision_order(files)
                            .into_iter()
                            .map(|path| files[&path].clone())
                            .collect(),
                    }],
                    &params,
                    &shared,
                )
                .map_err(|error| RegenError::Replay(error.to_string()))?;
            let cand_c = normalize(&run_c[CAND_SIDE], CAND_SIDE);
            let mut names: Vec<&String> = chain_norm.keys().chain(cand_c.keys()).collect();
            names.sort();
            names.dedup();
            for name in names {
                if chain_norm.get(name) != cand_c.get(name) {
                    return Err(RegenError::Internal(format!(
                        "synthesis round-trip mismatch for {name}\n  chain: {:?}\n  regen: {:?}",
                        chain_norm.get(name),
                        cand_c.get(name)
                    )));
                }
            }
        }
        Ok(())
    }

    /// Rebuild a current-state file from the chain's canonical dump. The
    /// dump is authoritative for the definition; the old file donates what
    /// the declustered replay cannot know: attached comments, Replicated
    /// engine arguments, definer clauses, and the ON CLUSTER form.
    fn synthesize(
        &self,
        raw_create: &str,
        old_content: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<String, RegenError> {
        let mut render_params = params.clone();
        render_params.insert("db".into(), CHAIN_SIDE.into());
        let (comments, old_body) = partition_chunk(old_content.trim_end_matches('\n'));
        let mut sql = raw_create.to_string();

        // clickhouse-local runs everything as `default`; restore the
        // declared definer, or drop the injected clause entirely.
        static INJECTED: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\s*DEFINER = \w+ SQL SECURITY \w+").expect("static regex")
        });
        static DECLARED_DEFINER: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"DEFINER\s*=\s*(\S+)").expect("static regex"));
        static DECLARED_SECURITY: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?i)SQL\s+SECURITY\s+(\w+)").expect("static regex"));
        if let Some(injected) = INJECTED.find(&sql.clone()) {
            let definer = DECLARED_DEFINER.captures(&old_body);
            let security = DECLARED_SECURITY.captures(&old_body);
            if definer.is_some() || security.is_some() {
                let who_template = definer
                    .map(|c| c[1].to_string())
                    .unwrap_or_else(|| "CURRENT_USER".into());
                let who = render(&who_template, &render_params).unwrap_or(who_template);
                let mode = security
                    .map(|c| c[1].to_string())
                    .unwrap_or_else(|| "DEFINER".into());
                sql.replace_range(
                    injected.range(),
                    &format!(" DEFINER = {who} SQL SECURITY {mode}"),
                );
            } else {
                sql.replace_range(injected.range(), "");
            }
        }

        // Transplant the Replicated engine prefix: zk path and replica are
        // declarations the declustered replay stripped. An explicit
        // non-Replicated engine is deliberate and left alone.
        let engine = REPLICATED.captures(&old_body).map(|captures| {
            (
                captures[1].to_string(),
                captures[2].to_string(),
                captures[3].to_string(),
            )
        });
        if let Some((family, zk_template, replica)) = engine {
            let zk = render(&zk_template, &render_params).unwrap_or(zk_template);
            let with_args =
                Regex::new(&format!(r"ENGINE = {family}\(([^)]*)\)")).expect("family is a word");
            let mut replaced = false;
            sql = with_args
                .replacen(&sql, 1, |captures: &regex::Captures| {
                    replaced = true;
                    let args = captures[1].trim().to_string();
                    let joined = if args.is_empty() {
                        String::new()
                    } else {
                        format!(", {args}")
                    };
                    format!("ENGINE = Replicated{family}({zk}, {replica}{joined})")
                })
                .into_owned();
            if !replaced {
                let bare_engine =
                    Regex::new(&format!(r"ENGINE = {family}\b")).expect("family is a word");
                let mut count = 0;
                sql = bare_engine
                    .replacen(&sql, 1, |_: &regex::Captures| {
                        count += 1;
                        format!("ENGINE = Replicated{family}({zk}, {replica})")
                    })
                    .into_owned();
                if count == 0 {
                    return Err(RegenError::Internal(format!(
                        "could not transplant Replicated{family} engine into:\n{sql}"
                    )));
                }
            }
        }

        static HEAD: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^CREATE (MATERIALIZED VIEW|VIEW|TABLE|DICTIONARY) (\S+)")
                .expect("static regex")
        });
        let head = HEAD.captures(&sql).ok_or_else(|| {
            RegenError::Internal(format!("unexpected canonical dump shape:\n{sql}"))
        })?;
        let kind = head[1].to_string();
        let name = head[2].to_string();
        let guard = if kind == "VIEW" { "OR REPLACE " } else { "" };
        let exists = if kind == "VIEW" { "" } else { "IF NOT EXISTS " };
        // ON CLUSTER travels only where the file declared it; repos
        // without clusters must not gain the clause.
        let cluster = if ON_CLUSTER_DECL.is_match(&old_body) {
            format!(" ON CLUSTER {}", params["cluster"])
        } else {
            String::new()
        };
        let head_len = head.get(0).expect("whole match").end();
        sql = format!(
            "CREATE {guard}{kind} {exists}{name}{cluster}{}",
            &sql[head_len..]
        );

        let formatted = self
            .replay
            .format_sql(&sql)
            .map_err(|error| RegenError::Replay(error.to_string()))?;
        let mut restored = formatted.replace(&format!("{CHAIN_SIDE}__"), "");
        restored = restored.replace(CHAIN_SIDE, "${db}");
        restored = restore_sentinels(&restored, params);
        Ok(format!("{comments}{};\n", restored.trim_end_matches('\n')))
    }
}

static ON_CLUSTER_DECL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bON\s+CLUSTER\b").expect("static regex"));

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
