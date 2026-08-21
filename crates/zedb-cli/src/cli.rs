//! The command surface: what `zedb` accepts, and how its shared argument
//! groups translate into the driver's option types.
//!
//! Doc comments on these types are the user-visible `--help` text, so they
//! read as documentation rather than as notes to the next maintainer.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "zedb", version, about = "ClickHouse migration engine")]
pub struct Cli {
    /// Repo root or any directory inside it (defaults to the working directory).
    #[arg(long, global = true, default_value = ".")]
    pub repo: PathBuf,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
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
        #[arg(long, requires = "server", conflicts_with = "pin_version")]
        user: Option<String>,
        /// Read the server password for discovery from this file.
        #[arg(
            long = "password-file",
            value_name = "FILE",
            value_parser = read_secret_file,
            requires = "server",
            conflicts_with = "pin_version"
        )]
        password: Option<String>,
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
        connection: ReadConnectionArgs,
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
        connection: ReadConnectionArgs,
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
        #[arg(long, requires = "server")]
        user: Option<String>,
        /// Read the server password from this file.
        #[arg(
            long = "password-file",
            value_name = "FILE",
            value_parser = read_secret_file,
            requires = "server"
        )]
        password: Option<String>,
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
pub struct ReadConnectionArgs {
    /// Server HTTP URL, e.g. http://localhost:8123.
    #[arg(long)]
    pub server: String,
    #[arg(long, default_value = "default")]
    pub user: String,
    /// Read the server password from this file.
    #[arg(long = "password-file", value_name = "FILE", value_parser = read_secret_file)]
    pub password: Option<String>,
}

#[derive(clap::Args)]
pub struct ConnectionArgs {
    /// Server HTTP URL, e.g. http://localhost:8123.
    #[arg(long)]
    pub server: String,
    #[arg(long, default_value = "default")]
    pub user: String,
    /// Read the server password from this file.
    #[arg(long = "password-file", value_name = "FILE", value_parser = read_secret_file)]
    pub password: Option<String>,
    /// Value for ${cluster}; DDL runs ON CLUSTER as written.
    #[arg(long)]
    pub cluster: Option<String>,
    /// Render for a single node: ON CLUSTER dropped, Replicated engines
    /// declustered.
    #[arg(long, conflicts_with = "cluster")]
    pub no_cluster: bool,
    /// Elevated user for statements the migration user is refused
    /// (OPTIMIZE, TRUNCATE, structural ALTER, functions, SYSTEM).
    #[arg(long)]
    pub admin_user: Option<String>,
    /// Read the elevated user's password from this file.
    #[arg(
        long = "admin-password-file",
        value_name = "FILE",
        value_parser = read_secret_file,
        requires = "admin_user"
    )]
    pub admin_password: Option<String>,
    /// Consent to mutate; without it mutating commands refuse.
    #[arg(long)]
    pub write: bool,
    /// Print what would run without executing.
    #[arg(long)]
    pub dry_run: bool,
    /// Template parameter override, name=value (repeatable).
    #[arg(long = "param", value_parser = parse_param)]
    pub params: Vec<(String, String)>,
    /// Read a template parameter from a file, name=FILE (repeatable).
    #[arg(long = "param-file", value_parser = parse_param_file)]
    pub param_files: Vec<(String, String)>,
}

impl ReadConnectionArgs {
    pub fn options(&self) -> zedb_ch::runner::RunnerOptions {
        zedb_ch::runner::RunnerOptions {
            server: zedb_ch::ChConfig {
                url: self.server.clone(),
                user: self.user.clone(),
                password: self.password.clone(),
                database: None,
                read_only: true,
                driver: Default::default(),
                native_port: None,
            },
            admin: None,
            cluster: None,
            no_cluster: false,
            write: false,
            dry_run: false,
            overrides: BTreeMap::new(),
        }
    }
}

#[derive(clap::Args)]
pub struct TargetArgs {
    /// Target database (repeatable).
    #[arg(long = "db")]
    pub databases: Vec<String>,
    /// Target every database in an exclusion group.
    #[arg(long, conflicts_with = "databases")]
    pub group: Option<String>,
    /// Target every discovered database, minus exclusion groups.
    #[arg(long, conflicts_with_all = ["databases", "group"])]
    pub all: bool,
}

fn parse_param(text: &str) -> Result<(String, String), String> {
    let (name, value) = text
        .split_once('=')
        .ok_or_else(|| format!("expected name=value, got {text:?}"))?;
    validate_param_name(name)?;
    Ok((name.to_string(), value.to_string()))
}

fn parse_param_file(text: &str) -> Result<(String, String), String> {
    let (name, path) = text
        .split_once('=')
        .ok_or_else(|| format!("expected name=FILE, got {text:?}"))?;
    validate_param_name(name)?;
    Ok((name.to_string(), read_secret_file(path)?))
}

fn validate_param_name(name: &str) -> Result<(), String> {
    if !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        Ok(())
    } else {
        Err(format!(
            "parameter name {name:?} must contain only ASCII letters, digits, and underscores"
        ))
    }
}

fn read_secret_file(path: &str) -> Result<String, String> {
    const MAX_SECRET_BYTES: usize = 64 * 1024;

    let bytes = std::fs::read(path).map_err(|error| format!("cannot read {path:?}: {error}"))?;
    if bytes.len() > MAX_SECRET_BYTES {
        return Err(format!(
            "secret file {path:?} is too large (maximum {MAX_SECRET_BYTES} bytes)"
        ));
    }
    let secret =
        String::from_utf8(bytes).map_err(|_| format!("secret file {path:?} is not valid UTF-8"))?;
    let secret = secret.trim_end_matches(['\r', '\n']).to_string();
    if secret.is_empty() {
        return Err(format!("secret file {path:?} is empty"));
    }
    Ok(secret)
}

impl ConnectionArgs {
    pub fn options(&self) -> Result<zedb_ch::runner::RunnerOptions, String> {
        let mut overrides = BTreeMap::new();
        for (name, value) in self.params.iter().chain(&self.param_files) {
            if overrides.insert(name.clone(), value.clone()).is_some() {
                return Err(format!(
                    "template parameter {name:?} was supplied more than once"
                ));
            }
        }
        Ok(zedb_ch::runner::RunnerOptions {
            server: zedb_ch::ChConfig {
                url: self.server.clone(),
                user: self.user.clone(),
                password: self.password.clone(),
                database: None,
                read_only: false,
                driver: Default::default(),
                native_port: None,
            },
            admin: self.admin_user.as_ref().map(|user| zedb_ch::ChConfig {
                url: self.server.clone(),
                user: user.clone(),
                password: self.admin_password.clone(),
                database: None,
                read_only: false,
                driver: Default::default(),
                native_port: None,
            }),
            cluster: self.cluster.clone(),
            no_cluster: self.no_cluster,
            write: self.write,
            dry_run: self.dry_run,
            overrides,
        })
    }
}

impl TargetArgs {
    pub fn targets(&self) -> Result<zedb_ch::runner::Targets, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use zedb_ch::runner::Targets;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("should parse")
    }

    #[test]
    fn command_definition_is_valid() {
        // Catches conflicting ids, bad defaults, and duplicate flags that
        // would otherwise only surface as a panic at runtime.
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_param_splits_on_the_first_equals() {
        assert_eq!(
            parse_param("db=analytics"),
            Ok(("db".into(), "analytics".into()))
        );
        // Values may themselves contain '='.
        assert_eq!(parse_param("expr=a=b"), Ok(("expr".into(), "a=b".into())));
        assert!(parse_param("nope").is_err());
    }

    #[test]
    fn targets_prefer_databases_then_group_then_all() {
        let cli = parse(&["zedb", "upgrade", "--server", "u", "--db", "a", "--db", "b"]);
        let Command::Upgrade { targets, .. } = cli.command else {
            panic!("expected upgrade");
        };
        assert_eq!(
            targets.targets(),
            Ok(Targets::Databases(vec!["a".into(), "b".into()]))
        );

        let cli = parse(&["zedb", "upgrade", "--server", "u", "--group", "g"]);
        let Command::Upgrade { targets, .. } = cli.command else {
            panic!("expected upgrade");
        };
        assert_eq!(targets.targets(), Ok(Targets::Group("g".into())));

        let cli = parse(&["zedb", "upgrade", "--server", "u", "--all"]);
        let Command::Upgrade { targets, .. } = cli.command else {
            panic!("expected upgrade");
        };
        assert_eq!(targets.targets(), Ok(Targets::All));
    }

    #[test]
    fn targets_refuse_an_unscoped_command() {
        // Silently defaulting to every database would be the dangerous
        // reading of "no target given".
        let cli = parse(&["zedb", "upgrade", "--server", "u"]);
        let Command::Upgrade { targets, .. } = cli.command else {
            panic!("expected upgrade");
        };
        assert!(targets.targets().is_err());
    }

    #[test]
    fn omitted_password_files_become_none() {
        let cli = parse(&["zedb", "status", "--server", "http://h:8123", "--all"]);
        let Command::Status { connection, .. } = cli.command else {
            panic!("expected status");
        };
        let options = connection.options();
        assert_eq!(options.server.password, None);
        assert!(options.server.read_only);
        assert!(options.admin.is_none());
        assert!(!options.write);
    }

    #[test]
    fn connection_args_carry_admin_and_overrides_through() {
        let directory = tempfile::tempdir().unwrap();
        let password = directory.path().join("admin-password");
        std::fs::write(&password, "s3cret\n").unwrap();
        let password = password.to_str().unwrap();
        let cli = parse(&[
            "zedb",
            "upgrade",
            "--server",
            "http://h:8123",
            "--all",
            "--write",
            "--admin-user",
            "root",
            "--admin-password-file",
            password,
            "--param",
            "db=analytics",
            "--param",
            "shard=01",
        ]);
        let Command::Upgrade { connection, .. } = cli.command else {
            panic!("expected upgrade");
        };
        let options = connection.options().unwrap();
        assert!(options.write);
        let admin = options.admin.expect("admin configured");
        assert_eq!(admin.user, "root");
        assert_eq!(admin.password.as_deref(), Some("s3cret"));
        assert_eq!(
            options.overrides.get("db").map(String::as_str),
            Some("analytics")
        );
        assert_eq!(
            options.overrides.get("shard").map(String::as_str),
            Some("01")
        );
    }

    #[test]
    fn duplicate_parameter_names_are_rejected_across_sources() {
        let directory = tempfile::tempdir().unwrap();
        let secret = directory.path().join("secret");
        std::fs::write(&secret, "second\n").unwrap();
        let secret_arg = format!("token={}", secret.display());
        let cli = parse(&[
            "zedb",
            "upgrade",
            "--server",
            "u",
            "--all",
            "--param",
            "token=first",
            "--param-file",
            &secret_arg,
        ]);
        let Command::Upgrade { connection, .. } = cli.command else {
            panic!("expected upgrade");
        };
        let error = match connection.options() {
            Ok(_) => panic!("duplicate parameter should be rejected"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            "template parameter \"token\" was supplied more than once"
        );
    }

    #[test]
    fn secret_files_are_bounded_nonempty_utf8_and_trim_line_endings() {
        let directory = tempfile::tempdir().unwrap();
        let secret = directory.path().join("secret");
        std::fs::write(&secret, "value\r\n").unwrap();
        assert_eq!(
            read_secret_file(secret.to_str().unwrap()),
            Ok("value".into())
        );

        std::fs::write(&secret, "\n").unwrap();
        assert!(read_secret_file(secret.to_str().unwrap()).is_err());
        std::fs::write(&secret, [0xff]).unwrap();
        assert!(read_secret_file(secret.to_str().unwrap()).is_err());
        std::fs::write(&secret, vec![b'x'; 64 * 1024 + 1]).unwrap();
        assert!(read_secret_file(secret.to_str().unwrap()).is_err());
    }

    #[test]
    fn mutually_exclusive_flags_are_rejected() {
        // Each of these pairs would otherwise silently pick a winner.
        for args in [
            vec![
                "zedb", "upgrade", "--server", "u", "--db", "a", "--group", "g",
            ],
            vec!["zedb", "upgrade", "--server", "u", "--all", "--db", "a"],
            vec![
                "zedb",
                "upgrade",
                "--server",
                "u",
                "--all",
                "--cluster",
                "c",
                "--no-cluster",
            ],
            vec![
                "zedb", "rollback", "--server", "u", "--all", "5", "--to", "3",
            ],
            vec!["zedb", "pin", "--server", "u", "--version", "24.1.1.1"],
        ] {
            assert!(
                Cli::try_parse_from(&args).is_err(),
                "should have been rejected: {args:?}"
            );
        }
    }

    #[test]
    fn credentials_require_the_endpoint_or_identity_that_uses_them() {
        for args in [
            vec!["zedb", "pin", "--version", "24.1.1.1", "--user", "root"],
            vec!["zedb", "mcp", "--user", "root"],
            vec![
                "zedb",
                "upgrade",
                "--server",
                "u",
                "--all",
                "--admin-password-file",
                "missing",
            ],
        ] {
            assert!(
                Cli::try_parse_from(&args).is_err(),
                "should have been rejected: {args:?}"
            );
        }
    }

    #[test]
    fn direct_password_arguments_are_not_part_of_the_interface() {
        for args in [
            vec![
                "zedb",
                "status",
                "--server",
                "u",
                "--all",
                "--password",
                "secret",
            ],
            vec![
                "zedb",
                "upgrade",
                "--server",
                "u",
                "--all",
                "--admin-user",
                "root",
                "--admin-password",
                "secret",
            ],
        ] {
            assert!(Cli::try_parse_from(&args).is_err());
        }
    }

    #[test]
    fn read_commands_do_not_accept_write_or_admin_options() {
        for args in [
            vec!["zedb", "status", "--server", "u", "--all", "--write"],
            vec!["zedb", "verify", "--server", "u", "--all", "--dry-run"],
            vec![
                "zedb",
                "status",
                "--server",
                "u",
                "--all",
                "--admin-user",
                "root",
            ],
        ] {
            assert!(
                Cli::try_parse_from(&args).is_err(),
                "read command should reject unused authority: {args:?}"
            );
        }
    }

    #[test]
    fn check_kind_is_restricted_to_known_checks() {
        assert!(Cli::try_parse_from(["zedb", "check", "sql"]).is_ok());
        assert!(Cli::try_parse_from(["zedb", "check", "nonsense"]).is_err());
        // Defaults to running everything.
        let cli = parse(&["zedb", "check"]);
        let Command::Check { kind, json } = cli.command else {
            panic!("expected check");
        };
        assert_eq!(kind, "all");
        assert!(!json);
    }

    #[test]
    fn repo_defaults_to_the_working_directory_and_is_global() {
        assert_eq!(parse(&["zedb", "ls"]).repo, PathBuf::from("."));
        // --repo is global, so it is accepted after the subcommand too.
        assert_eq!(
            parse(&["zedb", "ls", "--repo", "/tmp/x"]).repo,
            PathBuf::from("/tmp/x")
        );
    }

    #[test]
    fn a_connection_is_required_for_fleet_commands() {
        assert!(Cli::try_parse_from(["zedb", "status", "--all"]).is_err());
        // ...but not for MCP, whose repo tools work unconnected.
        assert!(Cli::try_parse_from(["zedb", "mcp"]).is_ok());
    }
}
