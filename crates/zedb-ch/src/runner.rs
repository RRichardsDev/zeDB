//! Live execution against real servers: tracking bootstrap, status,
//! upgrade, rollback with class enforcement, stamp, and targeted apply.
//! Ported from the ancestor's runner.py.
//!
//! Safety is architecture: mutating entry points refuse read-only
//! connections, every run records to the tracking table and a local audit
//! log, structural rollbacks warn, irreversible ones require explicit
//! acknowledgement, and rollbacks only peel from the top of the chain.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::time::Instant;

use zedb_core::repo::{placeholders, render, Migration, MigrationRepo, RollbackClass};

use crate::replay::{decluster, split_statements};
use crate::{ChClient, ChConfig};

mod actions;
mod execution;
mod status;
mod targets;
mod tracking;

pub(super) use status::{host_of, replace_host};
pub use status::{is_system, needs_admin, refused_without_admin, DatabaseStatus};

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Server(String),
    #[error("{0}")]
    Repo(String),
    #[error("{0}")]
    Refused(String),
}

pub struct RunnerOptions {
    pub server: ChConfig,
    /// Optional elevated connection: statements needing grants the
    /// migration user deliberately lacks (OPTIMIZE, TRUNCATE, structural
    /// ALTER, functions, SYSTEM, definers) route here, exactly as the
    /// ancestor tooling did.
    pub admin: Option<ChConfig>,
    pub cluster: Option<String>,
    /// Render for a single node: `ON CLUSTER` dropped, Replicated engines
    /// declustered.
    pub no_cluster: bool,
    /// Explicit consent to mutate; without it every mutating entry point
    /// refuses.
    pub write: bool,
    pub dry_run: bool,
    pub overrides: BTreeMap<String, String>,
}

pub struct Runner<'a> {
    repo: &'a MigrationRepo,
    client: ChClient,
    admin: Option<ChClient>,
    options: RunnerOptions,
    run_id: String,
}

/// Which databases an operation targets; `All` discovers from the server
/// and skips exclusion groups out loud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Targets {
    Databases(Vec<String>),
    Group(String),
    All,
}

/// The outcome of resolving `Targets`: what runs, and what `All` skipped
/// (database, exclusion group) so callers can say so out loud.
pub struct ResolvedTargets {
    pub databases: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

pub(crate) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Backtick-quote a name for identifier position in DDL. Closes the
/// injection without rejecting legal non-plain names (hyphenated
/// clusters, most commonly).
pub(crate) fn backtick_identifier(name: &str) -> String {
    format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
}

fn is_plain_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(character) if character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn validate_identifier(value: &str, label: &str) -> Result<(), RunnerError> {
    if is_plain_identifier(value) {
        Ok(())
    } else {
        Err(RunnerError::Refused(format!(
            "{label} must be a plain ClickHouse identifier, got {value:?}"
        )))
    }
}

fn validate_qualified_table(value: &str) -> Result<(), RunnerError> {
    let parts: Vec<&str> = value.split('.').collect();
    if (parts.len() == 1 || parts.len() == 2) && parts.iter().all(|part| is_plain_identifier(part))
    {
        Ok(())
    } else {
        Err(RunnerError::Refused(format!(
            "source table must be TABLE or DATABASE.TABLE using plain identifiers, got {value:?}"
        )))
    }
}

fn terminal_field(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn new_run_id() -> String {
    // v4-shaped from OS randomness; no uuid dependency needed.
    let mut bytes = [0u8; 16];
    getrandom(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = |range: std::ops::Range<usize>| {
        bytes[range]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}-{}-{}-{}",
        h(0..4),
        h(4..6),
        h(6..8),
        h(8..10),
        h(10..16)
    )
}

fn getrandom(buffer: &mut [u8]) {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    // RandomState seeds from OS entropy; fold hashes of a counter into
    // the buffer. Not cryptographic, but run ids only need uniqueness.
    let state = RandomState::new();
    for (index, chunk) in buffer.chunks_mut(8).enumerate() {
        let mut hasher = state.build_hasher();
        hasher.write_usize(index);
        let hash = hasher.finish().to_le_bytes();
        for (target, source) in chunk.iter_mut().zip(hash.iter()) {
            *target = *source;
        }
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn accepts_only_plain_sql_identifiers_and_qualified_tables() {
        for identifier in ["default", "cluster_01", "_internal"] {
            assert!(validate_identifier(identifier, "test").is_ok());
        }
        for identifier in ["", "db-name", "db name", "db/*x*/", "db.settings"] {
            assert!(validate_identifier(identifier, "test").is_err());
        }

        for table in ["schema_migrations", "default.schema_migrations"] {
            assert!(validate_qualified_table(table).is_ok());
        }
        for table in [
            "",
            "default.schema.migrations",
            "url('https://example.test')",
            "default.schema_migrations WHERE 1",
            "default.schema_migrations/*comment*/",
        ] {
            assert!(validate_qualified_table(table).is_err());
        }
    }

    #[test]
    fn terminal_fields_escape_control_sequences() {
        assert_eq!(terminal_field("db\n\u{1b}[2J"), "db\\n\\u{1b}[2J");
    }

    #[test]
    fn sql_string_literals_escape_quotes_and_backslashes() {
        assert_eq!(quote("db'\\name"), "'db\\'\\\\name'");
    }

    #[test]
    fn backticked_identifiers_escape_backticks_and_backslashes() {
        assert_eq!(backtick_identifier("ch-prod-cluster"), "`ch-prod-cluster`");
        assert_eq!(backtick_identifier("x`\\y"), "`x\\`\\\\y`");
    }
}
