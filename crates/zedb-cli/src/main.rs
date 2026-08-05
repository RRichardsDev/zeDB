//! zedb: thin CLI over zedb-core (docs/SPEC.md, docs/PHASE-1.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use zedb_core::repo::{init_repo, scaffold_migration, MigrationRepo, ScaffoldOptions};

#[derive(Parser)]
#[command(name = "zedb", version, about = "ClickHouse migration engine")]
struct Cli {
    /// Repo root or any directory inside it (defaults to the working directory).
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an empty migration repo.
    Init {
        /// Directory to initialize (created if missing).
        path: PathBuf,
    },
    /// Scaffold the next migration in the chain.
    New {
        /// One-line description for the migration header.
        description: String,
        /// Mark the migration targeted (opt-in per database).
        #[arg(long)]
        targeted: bool,
        /// Skip the rollback.sql template (the migration is irreversible).
        #[arg(long)]
        no_rollback: bool,
    },
    /// List the migration chain.
    Ls,
    /// Show one migration's files and metadata.
    Show { number: u32 },
    /// Ensure the pinned ClickHouse binary is cached, discovering the
    /// version from a server or taking it explicitly.
    Pin {
        /// Discover the version from this server (http://host:port).
        #[arg(long, conflicts_with = "pin_version")]
        server: Option<String>,
        /// Server user for discovery.
        #[arg(long, default_value = "default")]
        user: String,
        /// Server password for discovery.
        #[arg(long, default_value = "")]
        password: String,
        /// Pin this exact version instead of discovering one.
        #[arg(long = "version", id = "pin_version")]
        pin_version: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Init { path } => {
            let config = init_repo(&path).map_err(|error| error.to_string())?;
            println!("Initialized migration repo: {}", config.display());
            Ok(())
        }
        Command::New {
            description,
            targeted,
            no_rollback,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let directory = scaffold_migration(
                &repo,
                &ScaffoldOptions {
                    description,
                    targeted,
                    with_rollback: !no_rollback,
                },
            )
            .map_err(|error| error.to_string())?;
            println!("Scaffolded {}", directory.display());
            Ok(())
        }
        Command::Ls => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            if repo.migrations.is_empty() {
                println!("No migrations yet; scaffold one with `zedb new`.");
                return Ok(());
            }
            for migration in &repo.migrations {
                let class = migration
                    .rollback_class
                    .map(|class| class.as_str())
                    .unwrap_or("no-rollback");
                let targeted = if migration.targeted.is_some() {
                    "  targeted"
                } else {
                    ""
                };
                let headline = migration.headline().unwrap_or_default();
                println!(
                    "{:05}  {:04}/{:02}  {class:<12}{targeted}  {headline}",
                    migration.number, migration.year, migration.month
                );
            }
            Ok(())
        }
        Command::Show { number } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let migration = repo
                .migration(number)
                .ok_or_else(|| format!("no migration {number:05} in the chain"))?;
            println!("migration {:05}", migration.number);
            println!("directory: {}", migration.directory.display());
            match migration.rollback_class {
                Some(class) => println!("rollback-class: {}", class.as_str()),
                None => println!("rollback-class: none (treated as irreversible)"),
            }
            if let Some(allow_list) = &migration.targeted {
                if allow_list.is_empty() {
                    println!("targeted: yes (any database)");
                } else {
                    println!("targeted: yes, allow list {allow_list:?}");
                }
            }
            let upgrade = migration.upgrade_sql().map_err(|error| error.to_string())?;
            println!("\n-- upgrade.sql\n{upgrade}");
            if let Some(rollback) = migration
                .rollback_sql()
                .map_err(|error| error.to_string())?
            {
                println!("-- rollback.sql\n{rollback}");
            }
            Ok(())
        }
        Command::Pin {
            server,
            user,
            password,
            pin_version,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            let version = match (pin_version, server) {
                (Some(version), _) => version,
                (None, Some(url)) => {
                    let discovered = runtime
                        .block_on(zedb_ch::discover_server_version(zedb_ch::ChConfig {
                            url,
                            user,
                            password: (!password.is_empty()).then_some(password),
                            database: None,
                            read_only: true,
                        }))
                        .map_err(|error| error.to_string())?;
                    println!("server runs ClickHouse {discovered}");
                    discovered
                }
                (None, None) => repo.config.engine.version.clone(),
            };
            let binary = runtime
                .block_on(zedb_ch::ensure_binary(&version))
                .map_err(|error| error.to_string())?;
            zedb_ch::smoke_replay(&binary).map_err(|error| error.to_string())?;
            if version != repo.config.engine.version {
                zedb_core::repo::RepoConfig::set_pinned_version(&repo.root, &version)
                    .map_err(|error| error.to_string())?;
                println!("zedb.toml [engine].version updated to {version}");
            }
            println!("pinned: ClickHouse {version} at {}", binary.display());
            Ok(())
        }
    }
}
