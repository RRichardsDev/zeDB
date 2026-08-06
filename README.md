# zeDB

A fast, native database tool for ClickHouse: an explorer with Zed-grade
responsiveness, and a git-backed migration engine that knows the schema
state of an entire fleet.

## What it is

zeDB is two tools that share one core:

- **An explorer.** A GPU-accelerated desktop app (built with GPUI, the UI
  framework behind the Zed editor) for browsing databases, writing SQL, and
  scrolling through very large result sets without jank. Connections are
  tiered (dev / staging / production), read-only by default, and passwords
  live in the macOS Keychain, not in config files.
- **A migration engine.** Migrations are plain SQL files in a plain git
  repository. Instead of parsing your SQL or asking you to re-describe your
  schema declaratively, zeDB replays migrations through a real,
  version-pinned `clickhouse local` and lets the database itself answer
  every semantic question. It can then tell you, across hundreds of
  databases on multiple clusters, exactly which migrations have been
  applied where, what has drifted, and what an apply would do before you
  run it.

## Why it exists

ClickHouse deserves better tooling. General-purpose database GUIs treat it
shallowly, and no existing migration tool understands fleets: many
databases sharing one migration history, spread across clusters.

The design bets are simple:

1. **Replay, don't interpret.** The tool never reimplements ClickHouse
   semantics; a real pinned ClickHouse is the parser and the referee.
2. **Safety is architecture.** Mutating production requires layered,
   explicit consent: visual environment identity, read-only defaults,
   mandatory dry-run diffs, rollback-class acknowledgement, and an audit
   log. Not a confirmation dialog.
3. **Headless core.** All logic lives in library crates; the GUI and the
   CLI are thin clients. CI runs everything without a window server.
4. **BYO git.** Migration repos are ordinary directories in any git remote.
   No forge coupling, no hosted service.

## Layout

| Crate | Role |
|---|---|
| `crates/zedb-core` | Shared domain model: connections, preferences, secrets |
| `crates/zedb-ch` | ClickHouse client, replay engine, migrations, fleet state |
| `crates/zedb-cli` | Command-line interface for the same operations |
| `crates/zedb-app` | The GPUI desktop app (macOS) |

`docs/SPEC.md` is the full design document; `docs/devlog.md` records
findings and gotchas as development goes.

## Building

Rust stable on macOS. GPUI needs Apple's Metal toolchain, which no longer
ships with the Command Line Tools:

```sh
xcodebuild -downloadComponent MetalToolchain   # one-time, ~700MB
cargo build
cargo run -p zedb-app
```

Packaged, signed builds come from `scripts/bundle-macos.sh`; releases are
cut by tagging `v*` (see `packaging/TODO.md` for the pipeline details).

## Status

Early and moving fast. Built in the open for the ClickHouse community;
opinionated about migration layout and workflow, and those opinions are the
product. Other database engines are welcome as guests later via a
capability-based driver model, but ClickHouse is first-class.

## License

Apache-2.0
