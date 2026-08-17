# Bugs

Known defects, found during use or development, that are not fixed
yet. One line per bug: where, what, how seen. Remove the line in the
same commit that fixes it. (Fixed-on-sight bugs never land here; this
file is the queue, not the history.)

## Open

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
