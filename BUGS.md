# Bugs

Known defects, found during use or development, that are not fixed
yet. One line per bug: where, what, how seen. Remove the line in the
same commit that fixes it. (Fixed-on-sight bugs never land here; this
file is the queue, not the history.)

## Open

- zedb-ch pin cache never re-verifies on macOS (Apple Silicon): the OS
  rewrites adhoc linker-signed binaries in place on first execution
  (verified 2026-08-21: fresh clickhouse-macos-aarch64 26.3.12.3
  hashes f69ad394... matching the trust manifest, and 8c552906...
  after one `local --version` run), so `verify_cached_binary`'s
  at-rest recompute can never match the manifest digest after
  `ensure_exact_binary`'s own version probe. Every fleet
  verify/check/regen therefore re-downloads ~850 MB. Likely fix:
  persist a post-first-execution digest sidecar at verified-download
  time and check continuity against it (manifest still anchors the
  download itself).
- Query tab content stops painting after closing the history drawer
  with its × while an advise-run tab is active: editor text, grid
  rows, and the schema sidebar section all render empty; tab
  switching does not recover; the process stays alive and idle
  (seen 2026-08-17 during the README screenshot session; the drawer
  path was Saved -> lightbulb "Run & advise" -> close drawer).
  Relaunch recovers.
- Grid header filter on an aggregated column (an alias of count(),
  sum(), ...) rewrites the predicate into WHERE, which ClickHouse
  rejects with ILLEGAL_AGGREGATION; it belongs in HAVING when the
  column is an aggregate alias (seen 2026-08-17 filtering the `hits`
  column of a GROUP BY query).
