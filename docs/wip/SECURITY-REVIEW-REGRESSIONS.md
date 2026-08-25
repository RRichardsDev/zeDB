# Security-review regression triage

The August hardening pass (ee4c6d8, b22a851, 64e795b, 0ff2654, cd872df,
1f5b814, 38c02aa) is suspected of introducing guardrails that land in
interactive user paths and silently override explicit user choices, as
opposed to pure safety on agent/untrusted paths. Three such bugs were
already found in the wild and fixed (streaming decoder value budget vs
Max rows; streaming 1 GiB response cap vs Unlimited; agent child PATH).
This is the targeted sweep of everything else the pass introduced.

Test applied to every limit: does it guard an untrusted or agent path
(keep), or can it silently override a deliberate user action (fix or
surface)?

Spot-verified items are marked [verified]; the rest are reviewer
findings with file:line evidence that still need a look before fixing.

## Tier 1: overrides explicit user intent in hands-on paths (fix)

STATUS: all 18 items below fixed on feature/query-analytics
(commits 718dbc5 zedb-ch, 2b231b6 zedb-core, 3126cd5 zedb-acp, and
the zedb-app checkout/rotation commit), plus the process-group
PID-recycle race and the droppable Cancel from the notes at the end.
Tier 2 remains open for product decisions.

1. [verified] `HTTP_REQUEST_TIMEOUT` 5 min on the shared reqwest client
   (`zedb-ch/src/client.rs:21`, installed at `:102`). Reqwest's builder
   timeout covers connect through end-of-body, so it wall-clocks every
   editor streaming run, live-tail poll, schema/ops read, and every
   migration statement (`runner/execution.rs`). It overrides the user's
   own `max_execution_time` driver setting, and a migration ALTER that
   takes >5 min gets recorded as failed while the server keeps running
   it. The un-fixed sibling of the two streaming-cap regressions.
2. [verified] `MAX_COLLECTION_ITEMS` 100_000 (`rowbinary.rs:17`) on
   interactive results: any `groupArray`-style aggregation returning
   >100k elements fails the entire result. Ordinary ClickHouse usage,
   no user control to raise it. (Fine as a bound on the agent path.)
3. [verified] `NATIVE_STREAM_IDLE_TIMEOUT` 60 s closes the live tail on
   a merely-quiet table (`native.rs:25`, `:296`). A tail is explicitly
   a long-lived, possibly idle subscription; overnight or filtered
   tails die with "native stream idle deadline exceeded".
4. `MAX_DECODED_VALUES` 2M whole-response on the materialized decoder
   (`rowbinary.rs:18`, budget at `:212`): the un-fixed twin of the
   streaming fix; backs schema browsing / ops on large fleets
   (rows x columns > 2M fails the whole read). Keep for the agent path,
   or scale with an explicit caller-declared expectation.
5. [verified] `HANDSHAKE_TIMEOUT` 30 s covers the npx package download
   on first agent launch (`zedb-acp/src/lib.rs:28`, used at `:252`).
   Cold npm cache routinely exceeds it and the surfaced error blames
   auth. Needs a separate spawn/first-frame budget.
6. [verified] `MAX_ACP_FRAME_BYTES` 2 MiB inbound tears down the whole
   agent connection on one oversized frame (`lib.rs:23`, `:142`), and
   zeDB's own MCP output cap is 4 MiB (`zedb-ch/src/mcp.rs:26`), so the
   app can emit tool output it refuses to read back when the agent
   echoes it. Minimum fix: raise inbound above MCP output and make an
   oversized frame drop that frame, not the connection.
7. `REPLAY_PROCESS_TIMEOUT` 120 s over the whole chain replay
   (`replay.rs:20`, used at `:193`): chain length is user-controlled
   and grows monotonically; drift/verify/regen on a mature repo will
   SIGKILL mid-run with an error that reads like a real failure.
8. `MAX_REPO_FILE_BYTES` 16 MiB makes the whole repo unopenable when
   one migration file exceeds it (`zedb-core/src/repo/mod.rs:41`);
   bulk INSERT migrations are legitimate. Bound parse, not open.
9. Symlinked migration directories silently skipped during chain
   discovery (`repo/chain.rs:163-167`): a shortened chain that upgrade
   then acts on, with no warning. Silent omission on an apply path is
   the worst failure mode; also `regen/tree.rs` hard-errors on the
   same layout. Should at minimum be a loud error, consistently.
10. `BINARY_PROCESS_TIMEOUT` 30 s on the first exec of a freshly
    downloaded ClickHouse binary (`pin.rs:24`, `:257`, `:957`), the
    exact operation the code documents as unpredictably slow under
    Gatekeeper assessment.
11. [verified] Managed checkout directory renaming
    (`zedb-app/src/platform/managed_checkout.rs`): the new SHA suffix
    changes every existing checkout's path, so `fleet_clone_repo`
    silently re-clones and orphans the old checkout with any
    uncommitted work. Needs a one-time migration (rename old path or
    prefer it when present).
12. [verified] Settings sync can only add connections
    (`zedb-core/src/sync.rs:80-104`): edits to an existing
    connection's user/database/driver never propagate and the sync
    reports success. Silent drop of a pushed change (0ff2654 removed
    the field-update path 64e795b still had).
13. Local-state 64 MiB read bound swallowed into empty-then-overwrite
    (`store.rs:11`; `saved_tabs.rs:76`, `history.rs:49`): an oversized
    or symlinked (O_NOFOLLOW) state file loads as empty and the next
    save persists the loss. Bounding fine; silent data loss not.
    Related: saved tabs newly truncate to 200 on load (`saved_tabs.rs:81`).
14. Forced git config on explicit commit/push (`git.rs:33-62`):
    `commit.gpgSign=false` silently unsigns a signing user's commits;
    `credential.helper=` cleared with only macOS re-added breaks auth
    elsewhere; `core.sshCommand=ssh` discards corporate SSH wrappers.
    Fine on the background status path; wrong on explicit commit/push.
15. `GIT_NETWORK_TIMEOUT` 120 s kills legitimate large/slow clones and
    pulls with no override (`git.rs:20`), aggravated by forced
    `core.fsmonitor=false` on big worktrees; `MAX_GIT_OUTPUT` 16 MiB
    silently truncates `changed_paths` on huge trees (`git.rs:23`).
16. `EXPORT_IDLE_TIMEOUT` 60 s between chunks (`client/export.rs:8`):
    a heavy GROUP BY/ORDER BY export can take >60 s before its first
    block; dies "stalled" and leaves the partial file on disk.
17. Cloud password rotation result discarded after the control plane
    already rotated (`connections/cloud.rs:998-1010`): on a
    form-identity mismatch the new password is thrown away and the
    saved connection keeps the stale one. Fail toward persisting.
18. Endpoint acceptability (`client.rs:372-380`) refuses scheme-less
    saved endpoints (`localhost:8123`) everywhere including ping, with
    no normalization on load. Normalize; keep the credential-in-URL
    refusal.

## Tier 2: deliberate policy, but silent; surface or soften (decide)

- Always-allow permission grants now ignored with no notice
  (`agent/preferences` + events/messages deletions). Rationale sound
  (tool titles aren't authority identities); the silent drop of a
  stored explicit grant is the problem. Either honor with a
  fingerprint-based identity or say so in the permission card.
- Trust-manifest gate + fallback alias (`pin.rs:632`, `:339-403`):
  with a one-entry manifest (26.3.12.3), any other server version gets
  replay/format/drift/regen silently computed on 26.3.12.3, against
  the module's stated exact-version contract. Known mechanism; the
  severity is manifest breadth + silence. At minimum: badge results
  computed on a fallback version. (A manifest entry for 26.6.2.160 was
  already parked as an offer.)
- Agent result overflow `break` to `throw` (`client.rs:191-196`): the
  agent now gets a hard server error instead of the first 200 rows,
  and the graceful "capped at N" branch in `mcp/handlers.rs:309` is
  dead code. Decide which contract the agent should see.
- Fleet re-confirm context match uses literal `repo_root` PathBuf
  equality and `write_unlocked` coupling (`fleet/view.rs:127-131`,
  `execution.rs:113-120`): a transient health blip mid-confirmation
  discards the reviewed dry run. Policy right, friction real.
- `apply_in_place_allowed` denies when tier is unknown
  (`schema/model.rs:40-46`): stricter than the rule it replaced;
  fail-closed is defensible but the flash message names a condition
  the user can't act on.
- Sync exclusions for `custom_agents` / `fleet_cluster`
  (`sync.rs:33-47`): arguably machine-local, but silently exempted
  from an explicit sync.
- Persisted agent transcript lossier than live (32 KiB/entry, 200
  entries vs 600 live; `agent/mod.rs:25-26`): bound is fine, silent
  cut on reopen is the surprise.
- CLI interface removals (`--password`; `status`/`verify` losing
  `--cluster`/`--param`; `cli.rs:187-212`): deliberate break, worth a
  release-note callout rather than code change.
- Import destination must not exist at all, even empty
  (`repo/import.rs:170-176`); pin version regex rejects suffixed
  versions like `24.8.1.1-lts` (`import.rs:149-160`).
- Tracking-database / import-table identifier allowlists reject
  hyphenated names on read-only paths (`runner/targets.rs:48`,
  `actions.rs:249`); `backtick_identifier` exists and clusters
  already got this treatment in 0bc31dd.
- Linux cache-hit re-hashes the whole 220 MB archive per invocation
  and requires the retained tgz (`pin.rs:134-142`): correctness fine,
  cost persistent.

## Tier 3: verified fine, keep as is

Decoder header/type bounds (columns 16k, type depth/length, tuple
4096, value 64 MiB, fixed-string 64 MiB); materialized 1 GiB response
cap; error-body 1 MiB; redirect policy none; unauthenticated ping;
native read allowlist (WITH over HTTP fallback is latency only);
plaintext-native gating; MCP framing/truncation with markers; bridge
queue/token/read-timeout bounds; permission option-ID allowlist and
request-id pairing; event-channel backpressure (0bc31dd); stderr line
bound (non-fatal); pending-request cap; 24 h prompt timeout; pinned
adapter versions; update-install signature checks and absolute tool
paths; fleet `repo_owned` segment matching; scaffold description
rules; audit-log 0600/O_NOFOLLOW; git error-summary ellipsis; import
outside-ancestor rule; repo depth 8; sync payload 5 MiB; process
runner 64 MiB child-output cap.

Two code-correctness notes found along the way (not user-facing
policy): `process.rs:58` SIGKILLs the process group after the child
was already reaped (PID-recycle race, could kill an unrelated group);
and `notify()` still uses `try_send`, so a Cancel click is the one
user command that can be dropped under a full writer queue
(`zedb-acp/src/lib.rs:236`). The updates installer also depends on
exact `zipinfo -t` output wording (`updates.rs:181-188`) and fails all
updates closed if that wording ever changes.

Borderline item worth a note: `MAX_STREAM_BUFFER_BYTES` 64 MiB acts as
a de-facto single-row cap in streaming (`rowbinary.rs:13`); a row
holding one large blob plus other columns is undecodable even at Max
rows: 1. Keep the bound, but the error should say "row exceeds" rather
than "incomplete data".
