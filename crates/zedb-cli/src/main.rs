//! zedb: thin CLI over zedb-core (docs/contracts/SPEC.md).

use std::process::ExitCode;

use clap::Parser;

mod cli;
mod commands;

use cli::{Cli, Command};

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let Cli { repo, command } = cli;
    let root = repo.as_path();
    match command {
        Command::Init { path } => commands::repo::init(&path),
        Command::New {
            description,
            targeted,
            no_rollback,
        } => commands::repo::scaffold(root, description, targeted, no_rollback),
        Command::Ls => commands::repo::ls(root),
        Command::Show { number } => commands::repo::show(root, number),
        Command::Import {
            ancestor,
            destination,
        } => commands::repo::import(&ancestor, &destination),
        Command::Pin {
            server,
            user,
            password,
            pin_version,
        } => commands::engine::pin(root, server, user, password, pin_version),
        Command::Regen { check } => commands::engine::regen(root, check),
        Command::Check { kind, json } => commands::check::check(root, kind, json),
        Command::Status {
            connection,
            targets,
            json,
        } => commands::fleet::status(root, connection, targets, json),
        Command::Upgrade {
            connection,
            targets,
            to,
        } => commands::fleet::upgrade(root, connection, targets, to),
        Command::Rollback {
            connection,
            targets,
            number,
            to,
            irreversible,
            targeted,
        } => commands::fleet::rollback(
            root,
            connection,
            targets,
            number,
            to,
            irreversible,
            targeted,
        ),
        Command::Stamp {
            connection,
            targets,
            number,
        } => commands::fleet::stamp(root, connection, targets, number),
        Command::Apply {
            connection,
            targets,
            number,
        } => commands::fleet::apply(root, connection, targets, number),
        Command::ImportTracking { connection, from } => {
            commands::fleet::import_tracking(root, connection, from)
        }
        Command::Verify {
            connection,
            targets,
            json,
        } => commands::verify::verify(root, connection, targets, json),
        Command::Mcp {
            server,
            user,
            password,
            cache_connection,
        } => commands::mcp::serve(root, server, user, password, cache_connection),
    }
}
