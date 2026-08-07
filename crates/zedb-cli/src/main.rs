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
    /// Regenerate current-state/ by replaying the migration chain.
    Regen {
        /// Verify the committed tree matches instead of writing (CI mode).
        #[arg(long)]
        check: bool,
    },
    /// Run repo checks against the pinned binary.
    Check {
        /// Which check to run (default: all).
        #[arg(value_parser = ["sql", "equivalence", "lifecycle", "all"], default_value = "all")]
        kind: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show each database's chain position.
    Status {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[command(flatten)]
        targets: TargetArgs,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Apply pending fleet migrations.
    Upgrade {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[command(flatten)]
        targets: TargetArgs,
        /// Stop after this migration number.
        #[arg(long)]
        to: Option<u32>,
    },
    /// Roll back migrations (peel one from the top, or walk down with --to).
    Rollback {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[command(flatten)]
        targets: TargetArgs,
        /// The migration to roll back (must be the latest applied).
        number: Option<u32>,
        /// Walk rollbacks from the top down to (not including) this number.
        #[arg(long, conflicts_with = "number")]
        to: Option<u32>,
        /// Acknowledge an irreversible rollback.
        #[arg(long)]
        irreversible: bool,
        /// Confirm removing a targeted customisation.
        #[arg(long)]
        targeted: bool,
    },
    /// Record migrations as applied without executing (adopt existing DBs).
    Stamp {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[command(flatten)]
        targets: TargetArgs,
        /// Stamp through this migration number.
        number: u32,
    },
    /// Convert an analytics-clickhouse-ddl repo into a format-1 repo.
    Import {
        /// Path to the ancestor repo.
        ancestor: PathBuf,
        /// Destination directory for the new repo.
        destination: PathBuf,
    },
    /// Copy ancestor tracking rows (default.schema_migrations) into the
    /// format-1 tracking tables.
    ImportTracking {
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Source table holding the ancestor rows.
        #[arg(long, default_value = "default.schema_migrations")]
        from: String,
    },
    /// Diff live schemas against each database's applied chain position.
    Verify {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[command(flatten)]
        targets: TargetArgs,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Serve read-only fleet and query tools to AI agents over MCP
    /// (stdio). Connection optional: repo tools work without one.
    Mcp {
        /// Server HTTP URL, e.g. http://localhost:8123.
        #[arg(long)]
        server: Option<String>,
        #[arg(long, default_value = "default")]
        user: String,
        #[arg(long, default_value = "")]
        password: String,
        /// Serve schema_search and lint_sql from the zeDB app's schema
        /// cache for this connection name (as shown in the app sidebar).
        #[arg(long)]
        cache_connection: Option<String>,
    },
    /// Apply one targeted migration to specific databases.
    Apply {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[command(flatten)]
        targets: TargetArgs,
        /// The targeted migration number.
        number: u32,
    },
}

#[derive(clap::Args)]
struct ConnectionArgs {
    /// Server HTTP URL, e.g. http://localhost:8123.
    #[arg(long)]
    server: String,
    #[arg(long, default_value = "default")]
    user: String,
    #[arg(long, default_value = "")]
    password: String,
    /// Value for ${cluster}; DDL runs ON CLUSTER as written.
    #[arg(long)]
    cluster: Option<String>,
    /// Render for a single node: ON CLUSTER dropped, Replicated engines
    /// declustered.
    #[arg(long, conflicts_with = "cluster")]
    no_cluster: bool,
    /// Elevated user for statements the migration user is refused
    /// (OPTIMIZE, TRUNCATE, structural ALTER, functions, SYSTEM).
    #[arg(long)]
    admin_user: Option<String>,
    #[arg(long, default_value = "")]
    admin_password: String,
    /// Consent to mutate; without it mutating commands refuse.
    #[arg(long)]
    write: bool,
    /// Print what would run without executing.
    #[arg(long)]
    dry_run: bool,
    /// Template parameter override, name=value (repeatable).
    #[arg(long = "param", value_parser = parse_param)]
    params: Vec<(String, String)>,
}

#[derive(clap::Args)]
struct TargetArgs {
    /// Target database (repeatable).
    #[arg(long = "db")]
    databases: Vec<String>,
    /// Target every database in an exclusion group.
    #[arg(long, conflicts_with = "databases")]
    group: Option<String>,
    /// Target every discovered database, minus exclusion groups.
    #[arg(long, conflicts_with_all = ["databases", "group"])]
    all: bool,
}

fn parse_param(text: &str) -> Result<(String, String), String> {
    text.split_once('=')
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .ok_or_else(|| format!("expected name=value, got {text:?}"))
}

impl ConnectionArgs {
    fn options(&self) -> zedb_ch::runner::RunnerOptions {
        zedb_ch::runner::RunnerOptions {
            server: zedb_ch::ChConfig {
                url: self.server.clone(),
                user: self.user.clone(),
                password: (!self.password.is_empty()).then(|| self.password.clone()),
                database: None,
                read_only: false,
            },
            admin: self.admin_user.as_ref().map(|user| zedb_ch::ChConfig {
                url: self.server.clone(),
                user: user.clone(),
                password: (!self.admin_password.is_empty()).then(|| self.admin_password.clone()),
                database: None,
                read_only: false,
            }),
            cluster: self.cluster.clone(),
            no_cluster: self.no_cluster,
            write: self.write,
            dry_run: self.dry_run,
            overrides: self.params.iter().cloned().collect(),
        }
    }
}

impl TargetArgs {
    fn targets(&self) -> Result<zedb_ch::runner::Targets, String> {
        use zedb_ch::runner::Targets;
        if !self.databases.is_empty() {
            Ok(Targets::Databases(self.databases.clone()))
        } else if let Some(group) = &self.group {
            Ok(Targets::Group(group.clone()))
        } else if self.all {
            Ok(Targets::All)
        } else {
            Err("pass --db NAME (repeatable), --group NAME, or --all".into())
        }
    }
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
        Command::Regen { check } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let version = &repo.config.engine.version;
            let binary = zedb_ch::cached_binary(version).ok_or_else(|| {
                format!("pinned ClickHouse {version} is not cached; run `zedb pin` first")
            })?;
            let regenerator = zedb_ch::regen::Regenerator::new(&repo, binary);
            let files = regenerator
                .regenerate()
                .map_err(|error| error.to_string())?;
            if check {
                let problems =
                    zedb_ch::regen::diff_tree(&repo, &files).map_err(|error| error.to_string())?;
                if problems.is_empty() {
                    println!(
                        "current-state/ matches the migration chain ({} files)",
                        files.len()
                    );
                    Ok(())
                } else {
                    for problem in &problems {
                        eprintln!("{problem}");
                    }
                    Err(format!(
                        "current-state/ is out of date ({} problem(s)); run `zedb regen`",
                        problems.len()
                    ))
                }
            } else {
                let (changed, removed) =
                    zedb_ch::regen::write_tree(&repo, &files).map_err(|error| error.to_string())?;
                for path in &changed {
                    println!("wrote {}", path.display());
                }
                for path in &removed {
                    println!("removed {}", path.display());
                }
                println!(
                    "current-state/: {} files ({} written, {} removed, {} unchanged)",
                    files.len(),
                    changed.len(),
                    removed.len(),
                    files.len() - changed.len()
                );
                Ok(())
            }
        }
        Command::Check { kind, json } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let version = &repo.config.engine.version;
            let binary = zedb_ch::cached_binary(version).ok_or_else(|| {
                format!("pinned ClickHouse {version} is not cached; run `zedb pin` first")
            })?;
            let mut failed = false;
            let mut output = serde_json::Map::new();
            if kind == "sql" || kind == "all" {
                let report = zedb_ch::checks::check_sql(&binary, &repo)
                    .map_err(|error| error.to_string())?;
                if json {
                    output.insert(
                        "sql".into(),
                        serde_json::json!({
                            "checked": report.checked,
                            "errors": report.errors,
                        }),
                    );
                } else {
                    for error in &report.errors {
                        eprintln!("{error}");
                    }
                    println!(
                        "sql: {}/{} SQL files parse as valid ClickHouse",
                        report.checked - report.errors.len(),
                        report.checked
                    );
                }
                failed |= !report.errors.is_empty();
            }
            if kind == "equivalence" || kind == "all" {
                let report = zedb_ch::checks::check_equivalence(binary.clone(), &repo)
                    .map_err(|error| error.to_string())?;
                if json {
                    output.insert(
                        "equivalence".into(),
                        serde_json::json!({
                            "state_objects": report.state_objects,
                            "chain_objects": report.chain_objects,
                            "differences": report.differences,
                        }),
                    );
                } else {
                    for difference in &report.differences {
                        eprintln!("{difference}");
                    }
                    println!(
                        "equivalence: {} current-state objects vs {} migration-chain objects, {} difference(s)",
                        report.state_objects,
                        report.chain_objects,
                        report.differences.len()
                    );
                }
                failed |= !report.differences.is_empty();
            }
            if kind == "lifecycle" || kind == "all" {
                let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
                let report = runtime
                    .block_on(zedb_ch::lifecycle::check_lifecycle(&repo, binary.clone()))
                    .map_err(|error| error.to_string())?;
                if json {
                    output.insert(
                        "lifecycle".into(),
                        serde_json::json!({
                            "steps": report.steps,
                            "expected_objects": report.expected_objects,
                            "live_objects": report.live_objects,
                            "differences": report.differences,
                        }),
                    );
                } else {
                    for step in &report.steps {
                        println!("lifecycle: {step}");
                    }
                    for difference in &report.differences {
                        eprintln!("{difference}");
                    }
                    println!(
                        "lifecycle: {} expected vs {} live objects, {} difference(s)",
                        report.expected_objects,
                        report.live_objects,
                        report.differences.len()
                    );
                }
                failed |= !report.differences.is_empty();
            }
            if json {
                output.insert("ok".into(), serde_json::json!(!failed));
                println!("{}", serde_json::Value::Object(output));
            }
            if failed {
                Err("checks failed".into())
            } else {
                Ok(())
            }
        }
        Command::Status {
            connection,
            targets,
            json,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let runner = zedb_ch::runner::Runner::new(&repo, connection.options());
            let targets = targets.targets()?;
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            let statuses = runtime
                .block_on(runner.status(&targets))
                .map_err(|error| error.to_string())?;
            if json {
                let rows: Vec<serde_json::Value> = statuses
                    .iter()
                    .map(|status| {
                        serde_json::json!({
                            "database": status.database,
                            "head": status.head,
                            "latest": status.latest,
                            "pending": status.pending,
                            "customised": status.customised,
                            "failed": status.failed,
                        })
                    })
                    .collect();
                println!("{}", serde_json::json!({ "databases": rows }));
                return Ok(());
            }
            for status in statuses {
                let head = status
                    .head
                    .map(|head| format!("{head:05}"))
                    .unwrap_or_else(|| "none".into());
                let state = if !status.pending.is_empty() {
                    let pending: Vec<String> =
                        status.pending.iter().map(|n| format!("{n:05}")).collect();
                    format!("pending: {}", pending.join(", "))
                } else {
                    "up to date".into()
                };
                let mut line = format!(
                    "{}: at {head} of {:05}, {state}",
                    status.database, status.latest
                );
                if !status.customised.is_empty() {
                    let customised: Vec<String> = status
                        .customised
                        .iter()
                        .map(|n| format!("{n:05}"))
                        .collect();
                    line.push_str(&format!("; customised: {}", customised.join(", ")));
                }
                if !status.failed.is_empty() {
                    let failed: Vec<String> = status
                        .failed
                        .iter()
                        .map(|(n, action)| format!("{n:05} ({action})"))
                        .collect();
                    line.push_str(&format!("; FAILED: {}", failed.join(", ")));
                }
                println!("{line}");
            }
            Ok(())
        }
        Command::Upgrade {
            connection,
            targets,
            to,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let runner = zedb_ch::runner::Runner::new(&repo, connection.options());
            let targets = targets.targets()?;
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            runtime
                .block_on(runner.upgrade(&targets, to))
                .map_err(|error| error.to_string())
        }
        Command::Rollback {
            connection,
            targets,
            number,
            to,
            irreversible,
            targeted,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let runner = zedb_ch::runner::Runner::new(&repo, connection.options());
            let targets = targets.targets()?;
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            match (number, to) {
                (Some(number), None) => runtime
                    .block_on(runner.rollback_one(&targets, number, irreversible, targeted))
                    .map_err(|error| error.to_string()),
                (None, Some(floor)) => runtime
                    .block_on(runner.rollback_to(&targets, floor, irreversible))
                    .map_err(|error| error.to_string()),
                _ => Err("pass exactly one of NUMBER or --to TARGET".into()),
            }
        }
        Command::Stamp {
            connection,
            targets,
            number,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let runner = zedb_ch::runner::Runner::new(&repo, connection.options());
            let targets = targets.targets()?;
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            runtime
                .block_on(runner.stamp(&targets, number))
                .map_err(|error| error.to_string())
        }
        Command::Import {
            ancestor,
            destination,
        } => {
            let report = zedb_core::repo::import_repo(&ancestor, &destination)
                .map_err(|error| error.to_string())?;
            println!(
                "imported {} migration(s) into {} (ClickHouse {}, {} exclusion group(s))",
                report.migrations,
                report.destination.display(),
                report.engine_version,
                report.exclusion_groups
            );
            println!("next: zedb pin, zedb regen, zedb check");
            Ok(())
        }
        Command::Mcp {
            server,
            user,
            password,
            cache_connection,
        } => {
            // The repo is optional here: query tools alone are useful.
            let repo = MigrationRepo::open(&cli.repo).ok();
            let config = server.map(|url| zedb_ch::ChConfig {
                url,
                user,
                password: (!password.is_empty()).then_some(password),
                database: None,
                read_only: true,
            });
            let mut mcp = zedb_ch::mcp::McpServer::new(repo, config, Default::default());
            if let Some(name) = cache_connection {
                if let Some(root) = dirs::cache_dir() {
                    mcp = mcp.with_schema_cache(zedb_ch::schema_cache::connection_snapshot_path(
                        &root.join("zedb").join("schema"),
                        &name,
                    ));
                }
            }
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            runtime
                .block_on(zedb_ch::mcp::serve_stdio(mcp))
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        Command::ImportTracking { connection, from } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let runner = zedb_ch::runner::Runner::new(&repo, connection.options());
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            let imported = runtime
                .block_on(runner.import_tracking(&from))
                .map_err(|error| error.to_string())?;
            println!("imported {imported} tracking row(s) from {from}");
            Ok(())
        }
        Command::Verify {
            connection,
            targets,
            json,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let version = &repo.config.engine.version;
            let binary = zedb_ch::cached_binary(version).ok_or_else(|| {
                format!("pinned ClickHouse {version} is not cached; run `zedb pin` first")
            })?;
            let runner = zedb_ch::runner::Runner::new(&repo, connection.options());
            let verifier = zedb_ch::verify::Verifier::new(&repo, &runner, binary);
            let targets = targets.targets()?;
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            let drifts = runtime
                .block_on(verifier.verify(&targets))
                .map_err(|error| error.to_string())?;
            if json {
                let rows: Vec<serde_json::Value> = drifts
                    .iter()
                    .map(|drift| {
                        serde_json::json!({
                            "database": drift.database,
                            "head": drift.head,
                            "findings": drift.findings,
                        })
                    })
                    .collect();
                let clean = drifts.iter().all(|drift| drift.findings.is_empty());
                println!(
                    "{}",
                    serde_json::json!({ "databases": rows, "clean": clean })
                );
                return if clean {
                    Ok(())
                } else {
                    Err("drift detected".into())
                };
            }
            let mut drifted = false;
            for drift in &drifts {
                let head = drift
                    .head
                    .map(|head| format!("{head:05}"))
                    .unwrap_or_else(|| "none".into());
                if drift.findings.is_empty() {
                    println!("{}: clean at {head}", drift.database);
                } else {
                    drifted = true;
                    println!(
                        "{}: {} drift finding(s) at {head}",
                        drift.database,
                        drift.findings.len()
                    );
                    for finding in &drift.findings {
                        println!("  {finding}");
                    }
                }
            }
            if drifted {
                Err("drift detected".into())
            } else {
                Ok(())
            }
        }
        Command::Apply {
            connection,
            targets,
            number,
        } => {
            let repo = MigrationRepo::open(&cli.repo).map_err(|error| error.to_string())?;
            let runner = zedb_ch::runner::Runner::new(&repo, connection.options());
            let targets = targets.targets()?;
            let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
            runtime
                .block_on(runner.apply_targeted(&targets, number))
                .map_err(|error| error.to_string())
        }
    }
}
