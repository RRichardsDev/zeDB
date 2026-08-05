use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{template, RepoConfig, RepoError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackClass {
    Clean,
    Structural,
    Irreversible,
}

impl RollbackClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Structural => "structural",
            Self::Irreversible => "irreversible",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "clean" => Some(Self::Clean),
            "structural" => Some(Self::Structural),
            "irreversible" => Some(Self::Irreversible),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct Migration {
    pub number: u32,
    pub directory: PathBuf,
    pub year: u16,
    pub month: u8,
    /// Declared class of `rollback.sql`; `None` when the migration has no
    /// rollback file, which run time treats as irreversible.
    pub rollback_class: Option<RollbackClass>,
    /// `Some` when a `targeted.toml` marker is present; the list is the
    /// allow-list (empty means any database may be targeted).
    pub targeted: Option<Vec<String>>,
}

impl Migration {
    pub fn upgrade_path(&self) -> PathBuf {
        self.directory.join("upgrade.sql")
    }

    pub fn rollback_path(&self) -> Option<PathBuf> {
        let path = self.directory.join("rollback.sql");
        path.is_file().then_some(path)
    }

    pub fn upgrade_sql(&self) -> Result<String, RepoError> {
        Ok(std::fs::read_to_string(self.upgrade_path())?)
    }

    pub fn rollback_sql(&self) -> Result<Option<String>, RepoError> {
        match self.rollback_path() {
            Some(path) => Ok(Some(std::fs::read_to_string(path)?)),
            None => Ok(None),
        }
    }

    /// First comment line of upgrade.sql, conventionally
    /// `-- migration NNNNN: description`.
    pub fn headline(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.upgrade_path()).ok()?;
        let first = text.lines().next()?;
        let comment = first.strip_prefix("--")?.trim();
        Some(comment.to_string())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetedMarker {
    #[serde(default)]
    allow_list: Vec<String>,
}

/// Discover and validate the migration chain under `migrations/`.
///
/// Layout, numbering, marker, and placeholder problems are all reported
/// here so a broken repo fails loudly at open time, not mid-run.
pub fn discover_chain(
    migrations_root: &Path,
    config: &RepoConfig,
) -> Result<Vec<Migration>, RepoError> {
    let mut found = Vec::new();
    if migrations_root.is_dir() {
        collect(migrations_root, migrations_root, &mut found)?;
    }
    found.sort_by_key(|migration: &Migration| migration.number);

    for pair in found.windows(2) {
        if pair[0].number == pair[1].number {
            return Err(RepoError::Layout(format!(
                "migration number {:05} appears more than once",
                pair[0].number
            )));
        }
        if (pair[1].year, pair[1].month) < (pair[0].year, pair[0].month) {
            return Err(RepoError::Layout(format!(
                "migration {:05} is dated before {:05}; month directories must follow migration order",
                pair[1].number, pair[0].number
            )));
        }
    }

    let declared: Vec<&str> = config.declared_params().collect();
    for migration in &found {
        for path in [Some(migration.upgrade_path()), migration.rollback_path()]
            .into_iter()
            .flatten()
        {
            let sql = std::fs::read_to_string(&path)?;
            let unknown = template::undeclared_placeholders(&sql, &declared);
            if let Some(name) = unknown.first() {
                return Err(RepoError::Layout(format!(
                    "{}: uses ${{{name}}}, which is not a built-in or declared in zedb.toml [params]",
                    path.display()
                )));
            }
        }
    }

    Ok(found)
}

fn collect(root: &Path, dir: &Path, found: &mut Vec<Migration>) -> Result<(), RepoError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("upgrade.sql").is_file() {
            found.push(parse_migration(root, &path)?);
        } else {
            collect(root, &path, found)?;
        }
    }
    Ok(())
}

fn parse_migration(root: &Path, directory: &Path) -> Result<Migration, RepoError> {
    let relative = directory
        .strip_prefix(root)
        .expect("migration directory is under the migrations root");
    let parts: Vec<&str> = relative
        .components()
        .map(|part| part.as_os_str().to_str().unwrap_or_default())
        .collect();
    let layout_error = || {
        RepoError::Layout(format!(
            "{} has an invalid migration path; use YYYY/MM/NNNNN/upgrade.sql",
            relative.display()
        ))
    };

    let [year, month, number] = parts.as_slice() else {
        return Err(layout_error());
    };
    if year.len() != 4 || !year.chars().all(|c| c.is_ascii_digit()) {
        return Err(layout_error());
    }
    if month.len() != 2 || !month.chars().all(|c| c.is_ascii_digit()) {
        return Err(layout_error());
    }
    let month_value: u8 = month.parse().map_err(|_| layout_error())?;
    if !(1..=12).contains(&month_value) {
        return Err(layout_error());
    }
    if number.len() != 5 || !number.chars().all(|c| c.is_ascii_digit()) {
        return Err(layout_error());
    }

    let rollback_path = directory.join("rollback.sql");
    let rollback_class = if rollback_path.is_file() {
        let text = std::fs::read_to_string(&rollback_path)?;
        Some(parse_rollback_class(&rollback_path, &text)?)
    } else {
        None
    };

    let targeted_path = directory.join("targeted.toml");
    let targeted = if targeted_path.is_file() {
        let text = std::fs::read_to_string(&targeted_path)?;
        let marker: TargetedMarker = toml::from_str(&text).map_err(|error| RepoError::Config {
            path: targeted_path.clone(),
            message: error.to_string(),
        })?;
        Some(marker.allow_list)
    } else {
        None
    };

    Ok(Migration {
        number: number.parse().expect("checked to be five digits"),
        directory: directory.to_path_buf(),
        year: year.parse().expect("checked to be four digits"),
        month: month_value,
        rollback_class,
        targeted,
    })
}

fn parse_rollback_class(path: &Path, text: &str) -> Result<RollbackClass, RepoError> {
    let first = text.lines().next().unwrap_or_default();
    let declared = first
        .strip_prefix("--")
        .map(str::trim)
        .and_then(|comment| comment.strip_prefix("rollback-class:"))
        .map(str::trim)
        .and_then(RollbackClass::parse);
    declared.ok_or_else(|| {
        RepoError::Layout(format!(
            "{}: line 1 must declare `-- rollback-class: clean | structural | irreversible`",
            path.display()
        ))
    })
}

/// The canonical directory for a newly scaffolded migration.
pub fn new_migration_directory(
    migrations_root: &Path,
    number: u32,
    year: u16,
    month: u8,
) -> PathBuf {
    migrations_root
        .join(format!("{year:04}"))
        .join(format!("{month:02}"))
        .join(format!("{number:05}"))
}
