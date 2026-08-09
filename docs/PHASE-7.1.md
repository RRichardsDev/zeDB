# Phase 7.1: the query editor grows up

Status: SHIPPED across v0.1.15 (2026-08-09). The daily-driver slice
of the phase-7.1 ideas list (remaining ideas carried to
PHASE-7.2-IDEAS.md): the things a first-class browser needs so the
editor remembers, explains, and exports.

## What shipped

- **Query history + saved queries** (v0.1.15). A resizable drawer
  beside the editor with History and Saved tabs and live search.
  Every run records locally per connection (sql, time, duration,
  rows, or the error) in history.json, newest-first with consecutive
  dedup and a 1000 cap; history stays machine-local. Bookmarking
  saves instantly under a name derived from the query; saved queries
  are named snippets in settings.json (so they sync), with favorites
  pinned first, inline rename, and full-statement tree-sitter hover
  cards. Click inserts at the cursor as its own paragraph; a renamed
  save inserts with a "-- Saved: name" comment.
- **EXPLAIN, visualized** (v0.1.15). Palette-only "Explain query"
  draws the plan for the statement under the cursor as a colored
  tree in the results pane, with per-read index-pruning bars
  (selected vs initial parts and granules). Works back to 25.8;
  scrolls both axes.
- **Errors grew hands** (v0.1.15). The error bar offers Copy and Ask
  (last-used agent, by logo). Ask opens the agent pane, auto-sends
  the error once the session is ready with the failing tab and SQL
  attached invisibly, and an agent-proposed fix replaces the failed
  statement in place.
- **Export current query results** (v0.1.15). Palette-only two-step
  dialog: scope (tab cap or all rows), then format (CSV / Parquet /
  JSONEachRow) with the location defaulting quietly to Downloads.
  Streams the server's output format straight to disk past decode
  and the grid, with live bytes and transfer rate; Cancel aborts and
  cleans up the partial file.

## Also in this stretch

- The Run button absorbed Cancel (Running -> Cancel on hover), Run
  all became Execute with a list-play glyph, shortcuts moved to
  tooltips, connection pills shrank to tag size.
- vendor/gpui created to carry the upstream key-status deadlock
  backport (zed#51035); see docs/VENDOR-PATCHES.md.
- Test/demo databases on local docker: sat.complexTypes /
  sat.complexBulk (composite types), explain.me (a deliberately
  gnarly plan for the EXPLAIN view).

## Notes for later

- docs/MAYBE-IDEAS.md collects loose, uncommitted thoughts.
- The remaining ideas (migration-manager growth and platform bets)
  live in docs/PHASE-7.2-IDEAS.md; suggested first bite there is
  drift -> migration.
