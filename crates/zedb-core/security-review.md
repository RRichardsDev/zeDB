# `zedb-core` security review

## Review metadata

| Field | Value |
| --- | --- |
| Status | Complete; fifteen confirmed findings remediated, four documented as accepted or deferred |
| Baseline commit | `0bc31dd0bfd10346d0ba78d0f6a90f7b3577e7dd` |
| Branch | `security/zedb-cli-2026-aug` |
| Review host | Apple silicon, macOS 26.2 |
| Rust | `rustc 1.94.1 (e408947bf 2026-03-25)` |
| Cargo | `cargo 1.94.1 (29ea6fb6a 2026-03-24)` |
| Started | 2026-08-21 |
| Scope | `crates/zedb-core` (repo format, git subprocess, secrets/session, sync/preferences, local persistence, value formatting) and the settings-sync apply path in `crates/zedb-app` |

The review starts from the completed `zedb-ch`, `zedb-acp`, and `zedb-cli`
security commits. Those crates are authoritative for their own surfaces;
`zedb-core` is revisited as the foundation they stand on, plus the one
consumer boundary (settings-sync apply) whose fix could not be contained to
core.

## Evidence status

| Evidence | Status | Notes |
| --- | --- | --- |
| Formatting | Passed | `cargo fmt --all --check` |
| Clippy | Passed | `cargo clippy --workspace --all-targets` clean |
| Core tests | Passed | 37 unit plus repo integration; new tests for symlink walk, oversized files, decimal scale, git host/injection, sync merge |
| Workspace tests | Passed | Full suite green, including the git subprocess tests through the new timeout wrapper |
| Manual threat model | Complete | Repo parsing, git subprocess, Keychain, session/store, settings sync, local persistence, value formatting traced through consumers |
| Change-scope analysis | Reviewed | `impact` on the edited symbols; `route_message`-class hubs avoided; git host parse and sync merge verified against callers |

## Threat model

### Assets and authority

- Database and Cloud credentials in the Keychain, keyed by connection name.
- The GitHub/git broker token used for authenticated pushes.
- ClickHouse schema and migration integrity.
- Local files outside a migration repository.
- The user's machine: no repository, remote, or synced document should be
  able to run code merely because zeDB opened or synced it.

### Actors and failure sources

- A shared or third-party migration/settings repository (its files and its
  `.git` configuration).
- A crafted or hostile remote URL supplied to clone/probe.
- A compromised settings-sync account or repo pushing arbitrary payloads.
- A malicious or corrupted ClickHouse server returning crafted values.
- A co-resident local user reading world-readable state files.

### Principal boundaries

| Boundary | Input | Sensitive sink | Required control |
| --- | --- | --- | --- |
| Repo files to parser | zedb.toml, SQL, markers, directory names | SQL, filesystem walk, memory | No symlink follow, bounded reads/depth, identifier grammar |
| Repo `.git` to git | fsmonitor/hooks config | Subprocess execution | Neutralize repo-local hooks/fsmonitor, bound runtime |
| Remote URL to git | clone/ls-remote argument | git option parser, broker token | `--` separator, reject option-like URLs, correct host parse |
| Sync payload to local state | connection list, preferences | Keychain routing, DDL, execution | Preserve local endpoint/safety, validate, bound size |
| In-memory secrets to disk | SQL, tokens, endpoints | Local files | Private modes, atomic writes |
| Server value to UI | decimal scale, strings | Display formatting | No panic, no control-character passthrough at the sink |

## Findings register

| ID | Severity | Confidence | Status | Title | CWE |
| --- | --- | --- | --- | --- | --- |
| ZCORE-01 | Critical | High | Fixed | Synced endpoint/safety change exfiltrates a connection's Keychain password | CWE-522, CWE-829 |
| ZCORE-02 | High | High | Fixed | Broker token handed to an attacker host via URL userinfo confusion | CWE-346, CWE-522 |
| ZCORE-03 | High | High | Fixed | Argument injection in clone/ls-remote via option-like remote URL | CWE-88 |
| ZCORE-04 | High | Medium | Fixed | Repo-local git config (fsmonitor/hooks) runs code on open/watch | CWE-426 |
| ZCORE-05 | Medium | High | Fixed | Migration walk follows symlinked directories (loop crash / read outside repo) | CWE-59, CWE-674 |
| ZCORE-06 | Medium | High | Fixed | Server-controlled decimal scale panics the grid (overflow / divide-by-zero) | CWE-369 |
| ZCORE-07 | Medium | High | Fixed | Query history, saved tabs, session written world-readable with credential-bearing SQL | CWE-732 |
| ZCORE-08 | Medium | High | Fixed | Keychain namespace collision lets a connection name read/destroy a token | CWE-706 |
| ZCORE-09 | Medium | High | Fixed | Remote payload weakens `read_only`/`tier` on existing connections | CWE-602 |
| ZCORE-10 | Medium | High | Fixed | `fleet_repos`/`fleet_cluster` neither stripped from nor pinned against sync | CWE-15, CWE-200 |
| ZCORE-11 | Low | High | Fixed | No timeout on git subprocesses; a wedged child pins a pool thread | CWE-400 |
| ZCORE-12 | Low | High | Fixed | Unbounded reads of repo SQL/TOML and the sync payload | CWE-400, CWE-789 |
| ZCORE-13 | Low | High | Fixed | Non-atomic history write can silently erase history | CWE-404 |
| ZCORE-14 | Low | High | Fixed | Load paths do not re-apply the on-disk caps | CWE-400 |
| ZCORE-15 | Low | High | Fixed | Control characters in git branch/path and migration headline reach the UI | CWE-150 |
| ZCORE-16 | Low | High | Accepted | Cloud control-plane API key stored at the plain (silent-read) Keychain tier | CWE-522 |
| ZCORE-17 | Low | Medium | Deferred | `Value::String`/`Enum` Display passes control characters to the grid copy path | CWE-150 |
| ZCORE-18 | Info | High | Accepted | FNV-1a content hash gates reconciliation (not a security digest) | CWE-328 |
| ZCORE-19 | Low | Medium | Deferred | Duplicate connection names within a local set (sync payload deduped; local not) | CWE-694 |

## Findings

### ZCORE-01: synced endpoint/safety change exfiltrates a Keychain password (Fixed)

A connection's name is its Keychain account key, and settings-sync applied a
pulled payload's connection list wholesale (`features/settings/sync.rs`). A
compromised sync account could keep the name `prod`, repoint its endpoint to
an attacker host (or flip `read_only` off, `tier` to dev), and on the next
connect zeDB would fetch the real `prod` password by name and send it to the
attacker, with only a passive notice.

Fix: `sync::merge_synced_connections` (new) merges secure-by-default: for a
name already present locally the endpoints, `read_only`, `tier`, and Cloud
provenance are kept from the local record; only new connections are added, and
only when every endpoint is a plain `http(s)` URL. The app apply path calls it
instead of replacing the list. Decision recorded with the user: preserve local
(no confirmation UI), and expand this commit into `zedb-app` to close it.

### ZCORE-02: broker token handed to an attacker host (Fixed)

`git::auth_envs` derived the host as the first `@`-or-`/` token after
`https://`, so `https://github.com@evil.com/x.git` was classified as
`github.com`. git connects to `evil.com` (the part after the userinfo) and
calls the broker, which returns the github.com token to `evil.com`. Fix:
parse the authority, then take the host after the last `@` (userinfo) and
before the port. Regression test `broker_refuses_userinfo_host_spoofing`.

### ZCORE-03: argument injection in clone/ls-remote (Fixed)

`git clone <url> <dest>` and `git ls-remote --exit-code <url> HEAD` passed the
URL positionally with no `--`, so a value like `--upload-pack=<cmd>` became a
git option (command execution). Fix: `--` before every URL, plus an
`is_option_like` guard that refuses a `-`-leading URL before spawning.

### ZCORE-04: repo-local git config runs code on open/watch (Fixed)

git honours `core.fsmonitor` and hook programs named in a checkout's
`.git/config`. Because zeDB runs `git status` automatically when a repo is
opened or watched (and the agent/git-broker path reaches it with no human
gesture), a shared repo could execute code just by being opened. Fix: every
git invocation now runs through `git_command`, which sets
`-c core.fsmonitor=false -c core.hooksPath=/dev/null` and
`GIT_OPTIONAL_LOCKS=0`. Confidence Medium: the vector requires attacker
control of `.git/config`, which a fresh clone does not inherit; the fix is
cheap defense-in-depth regardless.

### ZCORE-05: migration walk follows symlinked directories (Fixed)

`repo::chain::collect` used `path.is_dir()`, which follows symlinks, and
recursed with no depth bound. A symlink under `migrations/` pointing at an
ancestor produced unbounded recursion (stack-overflow abort on repo open, a
crash any teammate could trigger); an outward symlink pulled files from
outside the repo into the chain. Fix: use `entry.file_type()` (from readdir,
no follow) to skip symlinks, and bound recursion at `MAX_MIGRATION_DEPTH`.
Import already rejected symlinks (ZCLI-005); this closes the primary open
path. Regression test `symlinked_migration_directories_are_not_followed`.

### ZCORE-06: server-controlled decimal scale panics the grid (Fixed)

`Value::Decimal` Display computed `10i128.pow(scale)` with a server-supplied
`scale` (native protocol truncates the wire scale to `u8` unchecked). Past
scale 38 this overflows: a debug-build panic, and in release a wrapped
zero divisor causing a divide-by-zero panic the moment the grid formats the
cell. Fix: `checked_pow`, falling back to `{value}e-{scale}`. Regression test
`decimal_display_survives_out_of_range_scale`.

### ZCORE-07: local state files world-readable with secret SQL (Fixed)

`history.json`, `saved-tabs.json`, `session.json`, `connections.json`,
`settings.json`, and the sync payload/state were written with default umask
(0644 under umask 022). History and tabs record every statement run,
including DDL like `CREATE USER ... IDENTIFIED BY '...'`. Fix: a shared
`store::write_private_atomic` creates the parent 0700 and writes the file
0600 at open time (no chmod race), atomically. All six writers route through
it; `session::take_at` also clears a stale `*.tmp`.

### ZCORE-08: Keychain namespace collision (Fixed)

Plain tokens (Cloud API key, OAuth, GitHub/git broker) shared the legacy
`"zedb"` service with per-connection legacy passwords. A connection named
like a token account (e.g. `zedb-clickhouse-cloud-<org>`) could, through
`get_password`'s legacy fallback, read the token as its password and migrate
or delete it. Fix: tokens now live in their own `dev.zedb.app.tokens`
service; `get_plain` migrates any legacy-service token into it on first read,
and `delete_plain` clears both. Password accounts and token accounts no
longer intersect.

### ZCORE-09: remote payload weakens `read_only`/`tier` (Fixed)

Same root as ZCORE-01: a pulled payload could turn `read_only` off (the whole
of the `readonly=2` guard) and downgrade `tier` (the production badge) on an
existing connection. `merge_synced_connections` keeps both from the local
record. Regression test
`merge_preserves_local_endpoint_and_safety_for_existing_names`.

### ZCORE-10: `fleet_repos`/`fleet_cluster` sync gap (Fixed)

The ee4c6d8 sync hardening stripped and pinned the agent fields and
`fleet_repo`, but missed `fleet_repos` (per-connection local checkout paths)
and `fleet_cluster` (rendered into `${cluster}` for fleet DDL). The former
leaked local paths into the shared repo; the latter let a payload feed a
remote-controlled value into DDL substitution. Fix: both are cleared in
`sanitized_preferences` and pinned to local in `apply_preferences`.
Regression test `sanitize_and_apply_pin_fleet_repos_and_cluster`.

### ZCORE-11: no git subprocess timeout (Fixed)

Every git call used blocking `.output()` with no wall-clock bound, so a
malicious fsmonitor daemon or a stalled remote could pin a blocking-pool
thread forever. Fix: `run_capture` spawns the child, drains stdout/stderr on
threads, and kills it past a 30 s (local) or 120 s (network) deadline.

### ZCORE-12: unbounded reads (Fixed)

Repo SQL/TOML files and the sync payload were slurped with `read_to_string`
and no size cap; a multi-GB file OOM'd on open/tick. Fix:
`repo::read_repo_file` caps repo files at 16 MiB (and rejects symlinks); the
sync payload is capped at 5 MiB; captured git output is capped at 16 MiB.

### ZCORE-13: non-atomic history write (Fixed)

`save_history` wrote in place; a torn write left truncated JSON that
`load_history` silently discarded, erasing up to 1000 entries. Fix: it now
goes through `write_private_atomic` (temp + rename), matching the other
persisters.

### ZCORE-14: load paths did not re-apply caps (Fixed)

`load_history`/`load_at` deserialized whatever was on disk; the cap applied
only on insert, so an oversized file stayed oversized in memory. Fix:
`truncate(HISTORY_CAP)` and `truncate(SAVED_TAB_CAP)` on load.

### ZCORE-15: control characters reach the UI (Fixed)

Git branch names and changed paths (porcelain v2 without `-z`) and a
migration's `headline()` (first SQL comment) were returned verbatim,
including terminal escapes. Fix: `git::strip_controls` on branch and paths,
and `headline()` filters control characters, matching the scaffold's
write-side rule.

### ZCORE-16: Cloud API key at the plain Keychain tier (Accepted)

The Cloud control-plane API key uses `set_plain` (no user-presence), the same
tier as a silent-read convenience token, though it can enumerate services and
wake them (spend money). The default Keychain ACL still limits silent reads to
the signed app. A middle tier (protected keychain without a biometric prompt)
would be an improvement but is a product decision about prompt friction;
documented, not changed. `set_plain`'s doc now states the privilege ceiling.

### ZCORE-17: `Value` Display control characters in the copy path (Deferred)

`Value::String`/`Enum` Display passes control characters through; in-app GPUI
rendering does not interpret them, but the grid's TSV copy/export path can be
row-spoofed by an embedded tab/newline. The `Display` impl is the legitimate
render path, so the correct fix (sanitize at the copy/export sink) lives in
`zedb-app`; deferred to the `zedb-app` review with the grid copy work.

### ZCORE-18: FNV-1a reconciliation hash (Accepted)

`sync::content_hash` uses FNV-1a to detect which side changed. A repo
attacker who could force a collision could suppress a victim's push, but
already controls the repo and cannot make anything malicious apply through
this path. Not a security digest; documented, left as is.

### ZCORE-19: duplicate local connection names (Deferred)

`merge_synced_connections` now dedupes names within a pulled payload, closing
the sync vector. A duplicate arising purely from local edits is a
connections-controller (zedb-app) concern; deferred there.

## Checked and clean

- Template rendering (`repo::template::render`) validates `${db}`/`${cluster}`
  as plain identifiers before substitution; missing values hard-error.
- Repo config: `deny_unknown_fields`, pinned format, engine kind restricted,
  scope names validated to one lowercase path component.
- Import hardening (ZCLI-005) holds: symlink rejection, fresh-destination
  requirement, canonicalized outside-ancestor check, validated pin version.
- No plaintext secret fallback exists off the Apple target; no password field
  serializes to any on-disk struct (asserted by existing tests).
- Sign-out deletion, token expiry, and corrupt-file robustness verified in the
  secrets/session/store audit.
- The agent-field sync hardening from ee4c6d8 is complete for the three fields
  it covers; the gap was ZCORE-10 only.
