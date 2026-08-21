# Phase 15: the remaining test seams

Status: PLANNED (2026-08-21). The gpui window-test framework covers
state, keyboard, action dispatch, mouse (via `debug_selector` bounds),
render invariants, and an end-to-end tier on a real ephemeral
ClickHouse (`zedb-ch/test-support`). Four seams remain untestable;
this phase is the backlog for them, in priority order. None blocks
writing suites for the surfaces the tooling already reaches.

## 1. Cloud and GitHub HTTP fakes

Cloud linking, password provisioning, usage/cost dashboards, GitHub
device-flow sign-in, and the update check all talk to real APIs, so
their suites currently stop at the request boundary. zedb-ch already
has the pattern to copy: a loopback fake HTTP server the test controls
(see its transport-deadline tests). The work is an app-level fake with
per-test routes, plus threading base URLs through the three or four
places that hardcode api endpoints (updates, clickhouse_cloud,
cloud_oauth, github). Worth doing first: the Cloud flows carry real
user risk (password rotation) and have zero window coverage.

## 2. A Keychain seam

`zedb_core::secrets` hits the real macOS Keychain with no override, so
every flow that stores, fetches, renames, or deletes a secret is
untested (and untestable) at the window level: connection saves with
passwords, the rollback path in `persist_draft`, git elevated tokens,
Cloud API keys. Options, roughly in order of honesty: an env-gated
file-backed store like `ZEDB_CONFIG_DIR` (simple, but changes prod
code paths), or a `Secrets` trait injected at Workspace build (wider
churn, cleaner seam). Decide when the first test actually needs it.

## 3. Simulated time

gpui's test clock (`advance_clock`) should make debounces and timers
deterministic: the notice flash (1 s), focus-recheck debounce (30 s),
health/update polls (2-5 min), control-highlight decay. Unexplored;
likely just works for pure gpui `Timer` waits. Flows that hop through
the real tokio runtime stay wall-clock (`wait_for`), by design.

## 4. Pixel output

The test platform does not rasterize, so "does it look right" is out
of scope for window tests. If rendering regressions ever bite, the
plan from the original discussion stands: a thin screenshot harness on
the signed build (`screencapture -l` + perceptual diff) over a handful
of screens, kept separate from this framework. Do not build it
speculatively.
