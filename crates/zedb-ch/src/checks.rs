//! Repo checks: `sql` and `equivalence` (docs/PHASE-1.md M4).
//!
//! `sql` renders every SQL file with dummy parameters and pipes it through
//! `clickhouse format`, the real server grammar: anything it rejects would
//! also be rejected at apply time. Nothing executes and no server runs.
//!
//! `equivalence` enforces "migrations are canonical, current-state is
//! derived": both are replayed into one `clickhouse local` process and
//! every object's canonical definition is diffed. Any mismatch means regen
//! has a bug or current-state/ was edited by hand.
//!
//! The lifecycle check arrives with the live runner (M5), which it drives.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use zedb_core::repo::{render, MigrationRepo};

use crate::replay::{split_statements, LocalReplay, ReplaySide};

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Repo(String),
    #[error("replaying failed:\n{0}")]
    Replay(String),
}

/// Dummy values for every placeholder: `check sql` and `check equivalence`
/// render with exactly these. Declared parameters use their default when
/// one exists, a distinctive number otherwise.
pub fn dummy_params(repo: &MigrationRepo) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    params.insert("db".into(), "rendertest".into());
    params.insert("cluster".into(), "rendertest_cluster".into());
    for (name, config) in &repo.config.params {
        params.insert(
            name.clone(),
            config.default.clone().unwrap_or_else(|| "42".into()),
        );
    }
    params
}

static POSITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(line (\d+), col (\d+)\)").expect("static regex"));
static INSERT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*INSERT\s").expect("static regex"));
static VALUES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)^(.*?\bVALUES\b)").expect("static regex"));

/// `clickhouse format` parses inline VALUES data with the client's
/// one-row parser, so multi-row INSERTs (valid on the real server) are
/// rejected. Truncate after the VALUES keyword: the insert header still
/// gets validated, row data is a runtime concern.
fn truncate_insert_values(sql: &str) -> String {
    let mut out = Vec::new();
    let mut changed = false;
    for statement in split_statements(sql) {
        let first_sql = statement
            .lines()
            .find(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
            .unwrap_or_default();
        if INSERT.is_match(first_sql.trim()) {
            let body: String = statement
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(captures) = VALUES.captures(&body) {
                out.push(captures[1].to_string());
                changed = true;
                continue;
            }
        }
        out.push(statement);
    }
    if !changed {
        return sql.to_string();
    }
    let mut joined = out
        .iter()
        .map(|statement| statement.trim())
        .filter(|statement| !statement.is_empty())
        .collect::<Vec<_>>()
        .join(";\n");
    joined.push(';');
    joined
}

/// Check one file; returns a `path:line:col: message` string on failure.
pub fn check_sql_file(
    binary: &Path,
    path: &Path,
    params: &BTreeMap<String, String>,
) -> Result<Option<String>, CheckError> {
    let text = std::fs::read_to_string(path)?;
    // A file using an undeclared placeholder fails here, which is exactly
    // right: it would also fail at apply time.
    let sql = match render(&text, params) {
        Ok(sql) => sql,
        Err(error) => return Ok(Some(format!("{}:1:1: {error}", path.display()))),
    };
    let sql = truncate_insert_values(&sql);

    // -n / multiquery: migration files hold several statements.
    let output = Command::new(binary)
        .args(["format", "-n", "--query", &sql])
        .output()?;
    if output.status.success() {
        return Ok(None);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr
        .trim()
        .lines()
        .next()
        .unwrap_or("parse error")
        .to_string();
    let (line, col) = POSITION
        .captures(&message)
        .map(|captures| (captures[1].to_string(), captures[2].to_string()))
        .unwrap_or(("1".into(), "1".into()));
    Ok(Some(format!("{}:{line}:{col}: {message}", path.display())))
}

/// Every SQL file under migrations/ and current-state/, sorted.
pub fn all_sql_files(repo: &MigrationRepo) -> Vec<PathBuf> {
    fn collect(dir: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "sql") {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(&repo.root.join("current-state"), &mut files);
    collect(&repo.root.join("migrations"), &mut files);
    files.sort();
    files
}

pub struct SqlCheckReport {
    pub checked: usize,
    pub errors: Vec<String>,
}

/// Syntax-check every SQL file with the real ClickHouse parser.
pub fn check_sql(binary: &Path, repo: &MigrationRepo) -> Result<SqlCheckReport, CheckError> {
    let params = dummy_params(repo);
    let files = all_sql_files(repo);
    let mut errors = Vec::new();
    for file in &files {
        if let Some(error) = check_sql_file(binary, file, &params)? {
            errors.push(error);
        }
    }
    Ok(SqlCheckReport {
        checked: files.len(),
        errors,
    })
}

pub struct EquivalenceReport {
    pub state_objects: usize,
    pub chain_objects: usize,
    pub differences: Vec<String>,
}

const STATE_SIDE: &str = "eqv_currentstate";
const CHAIN_SIDE: &str = "eqv_migrations";

/// current-state/ in provisioning order: scopes without a parameter
/// first, then templated scopes, each sorted.
pub fn current_state_files(repo: &MigrationRepo) -> Result<Vec<PathBuf>, CheckError> {
    let root = repo.root.join("current-state");
    let mut unparameterized = Vec::new();
    let mut parameterized = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    for entry in entries.flatten() {
        let scope_dir = entry.path();
        if !scope_dir.is_dir() {
            continue;
        }
        let scope_name = entry.file_name().to_string_lossy().to_string();
        let has_param = repo
            .config
            .scopes
            .get(&scope_name)
            .map(|scope| scope.param.is_some())
            .unwrap_or(scope_name != "global");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&scope_dir)?
            .flatten()
            .map(|file| file.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .collect();
        files.sort();
        if has_param {
            parameterized.extend(files);
        } else {
            unparameterized.extend(files);
        }
    }
    unparameterized.sort();
    parameterized.sort();
    unparameterized.extend(parameterized);
    Ok(unparameterized)
}

/// Replay current-state/ and the migration chain side by side; diff every
/// object's canonical definition.
pub fn check_equivalence(
    binary: PathBuf,
    repo: &MigrationRepo,
) -> Result<EquivalenceReport, CheckError> {
    let read =
        |path: &PathBuf| -> Result<String, CheckError> { Ok(std::fs::read_to_string(path)?) };
    let state_texts: Vec<String> = current_state_files(repo)?
        .iter()
        .map(read)
        .collect::<Result<_, _>>()?;
    let chain_texts: Vec<String> = repo
        .migrations
        .iter()
        .filter(|migration| migration.targeted.is_none())
        .map(|migration| {
            migration
                .upgrade_sql()
                .map_err(|error| CheckError::Repo(error.to_string()))
        })
        .collect::<Result<_, _>>()?;

    let params = dummy_params(repo);
    let replay = LocalReplay::new(binary);
    let dumps = replay
        .dump_objects(
            &[
                ReplaySide {
                    name: STATE_SIDE.into(),
                    texts: state_texts,
                },
                ReplaySide {
                    name: CHAIN_SIDE.into(),
                    texts: chain_texts,
                },
            ],
            &params,
            &[],
        )
        .map_err(|error| CheckError::Replay(error.to_string()))?;

    let normalize = |dump: &BTreeMap<String, String>, side: &str| -> BTreeMap<String, String> {
        dump.iter()
            .map(|(name, create)| {
                let strip = |text: &str| {
                    text.replace(&format!("{side}__"), "")
                        .replace(side, "${db}")
                };
                (strip(name), strip(create))
            })
            .collect()
    };
    let state = normalize(&dumps[STATE_SIDE], STATE_SIDE);
    let chain = normalize(&dumps[CHAIN_SIDE], CHAIN_SIDE);

    let mut differences = Vec::new();
    let mut names: Vec<&String> = state.keys().chain(chain.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        match (state.get(name), chain.get(name)) {
            (Some(_), None) => differences.push(format!("only in current-state: {name}")),
            (None, Some(_)) => differences.push(format!("only in migration chain: {name}")),
            (Some(from_state), Some(from_chain)) if from_state != from_chain => {
                differences.push(format!(
                    "definition mismatch: {name}\n  current-state: {from_state}\n  migrations:    {from_chain}"
                ));
            }
            _ => {}
        }
    }

    Ok(EquivalenceReport {
        state_objects: state.len(),
        chain_objects: chain.len(),
        differences,
    })
}
