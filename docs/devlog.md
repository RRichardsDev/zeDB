# Devlog

Findings worth remembering, especially GPUI gaps and gotchas that could
become upstream contributions. Newest at the bottom.

## M0 (2026-08-05)

- gpui 0.2.2 is on crates.io and genuinely usable; no git dependency on
  the zed repo needed. The `Application::new().run` / `open_window` /
  `Render` API worked first try.
- Building gpui on macOS requires Apple's Metal toolchain, which no longer
  ships with the Command Line Tools:
  `xcodebuild -downloadComponent MetalToolchain` (688MB, one-time). The
  build error ("cannot execute tool 'metal'") does not hint at the fix.
  Candidate for a CONTRIBUTING.md note and maybe a friendlier build-script
  message upstream.
- Dev-profile gpui is unusably slow without optimization; workspace sets
  `[profile.dev.package."*"] opt-level = 2` so only our crates compile
  unoptimized.

## M1 (2026-08-05)

- Dropped the `clickhouse` crate in favor of a hand-rolled
  `RowBinaryWithNamesAndTypes` decoder (see SPEC amendment): the crate
  wants compile-time row structs; an explorer has runtime-discovered
  schemas.
- The ephemeral-server test pattern from analytics-clickhouse-ddl ports
  beautifully to Rust: temp dir + minimal config.xml/users.xml + random
  ports; full start-query-teardown in under a second. This will carry the
  Phase 1 replay engine too.
- RowBinary notes: UUIDs arrive as two little-endian u64 halves (reverse
  each 8-byte half for RFC order); LowCardinality is transparent on the
  wire (values serialize as the inner type); Nullable is a 0x00/0x01
  prefix byte per value.
- Type strings are parsed strictly; anything unknown (e.g.
  AggregateFunction states) fails loudly as UnsupportedType rather than
  guessing. The explorer will need a graceful UI story for those columns
  eventually.

## M2: grid spike (2026-08-05)

Verdict: uniform_list carries the grid. 1M rows x 50 cols scrolls smoothly
with ~126MB flat memory (cells generated on demand from (row, col), only
the visible window rendered).

GPUI findings, in rough order of hours cost:

- **Flex items default to `min-width: auto`, and it bites hard.** The div
  hosting the grid view (`flex_1` in a row) silently expanded to its
  content width (a 6000px header row), which made the whole subtree
  content-sized: every `w_full` inside resolved to content width instead
  of the parent, collapsing the uniform_list (no intrinsic width) to 0px
  wide. Symptom: list renders items (closure called with correct range)
  but paints nothing, because the content mask is 0 wide. Fix: `min_w_0()`
  on the flex item that hosts the view. This is standard CSS flexbox
  behavior, but nothing in the symptom points at it; hours lost.
- **Debug technique that cracked it:** `gpui::canvas(|bounds, _, _|
  eprintln!(...), |_,_,_,_| {})` as an `.absolute().size_full()` child is
  a perfect bounds probe for any element. Also useful:
  `UniformListScrollHandle.0.borrow().last_item_size` exposes the list's
  laid-out viewport vs content size. Wishlist: a layout inspector.
- **Two-axis scrolling is built into uniform_list** via
  `.with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)`.
  Do NOT wrap the list in an `overflow_x_scroll` container: the list
  consumes wheel events and the wrapper never scrolls. Discoverability of
  this option is poor (found by reading the crate source).
- Scroll styling methods (`overflow_x_scroll` etc.) exist only on
  stateful elements, i.e. after `.id(...)`. The compile error does not
  hint at that.
- A fixed header outside the list can mirror the list's horizontal offset:
  read `scroll.0.borrow().base_handle.offset().x`, apply as `ml()` on the
  header's inner row, and `cx.notify()` from an `on_scroll_wheel` listener
  so it repaints. Works, feels instant.
- Grid cells need `.overflow_hidden().whitespace_nowrap()` or long values
  wrap into vertically-squeezed lines inside the fixed row height.
- Process discipline note: `cargo clippy` does not produce the binary;
  after edits, `cargo build` before relaunching, or you debug a stale app.

## M3: connections (2026-08-05)

Status: complete.

- GPUI 0.2.2 has no built-in text input control. The connection form uses
  a small single-line input adapted from GPUI's `examples/input.rs`,
  including IME ranges, Unicode grapheme navigation, selection, and
  clipboard actions. This should become a shared primitive before M6's
  multiline query editor.
- ClickHouse read-only mode is best enforced by the server. Sending
  `readonly=2` on every request rejects writes and DDL while avoiding
  brittle client-side SQL classification.
- Connection metadata is JSON in the platform config directory. Passwords
  are keyed by connection name in macOS Keychain and never enter the
  serializable connection type.
- GPUI's executor is not a Tokio reactor. Network work runs on one shared
  Tokio runtime, then a GPUI task awaits the executor-agnostic join handle
  and updates the view.
- A saved connection is a logical cluster, not a single URL. The config
  stores an endpoint list and transparently reads the original singular
  `url` shape. The primary form action tests every node and persists only
  when at least one accepts the credentials; saving an untested/offline
  cluster remains an explicit secondary action.
- The M2 synthetic grid remains useful as a performance harness, but it is
  now an explicitly labeled opt-in view. Showing it as the default content
  after a live connection made synthetic rows look like server data.
- Connection deletion requires confirmation, removes the Keychain password,
  disconnects the deleted active cluster, and preserves a valid sidebar
  selection. Config and Keychain updates use rollback paths so a partial
  rename or delete does not silently leave the app in an inconsistent state.
- Local development runs can use `scripts/run-signed-macos.sh` to build a
  minimal `zeDB.app` bundle with the stable `dev.zedb.app` identifier. The
  script selects the valid Apple Development certificate with the latest
  expiry, signs and verifies the bundle, then launches it.
- Bare GPUI renders SVG assets but does not include Zed's icon library. zeDB
  embeds its own monochrome utility icons and uses compact footer toolbars.
  Environment identity stays next to the connection name, using muted tinted
  pills instead of saturated status colors. The durable rules live in
  `docs/UI-DESIGN.md`.

## M4: schema tree (2026-08-05)

Status: implementation complete, awaiting UI acceptance.

- Schema discovery uses `system.databases`, `system.tables`, and
  `system.columns`. The client exposes typed database, object, and column
  metadata instead of leaking query-result indexing into the UI.
- Database names load on connection. Object lists load only when their
  database is expanded, and columns load only when an object is selected.
  Network work stays on the shared Tokio runtime, so expanding the tree never
  blocks GPUI.
- Filtering is local and immediate over the metadata already loaded into the
  tree. It does not issue a query per keystroke.
- GPUI has the cursor and mouse primitives needed for pane resizing, but no
  ready-made Zed splitter. The connection and schema sidebar uses a 1-pixel
  divider with an 8-pixel centered drag target and a clamped width. This is now
  the contract for future structural panes in `docs/UI-DESIGN.md`.
- The M4 object view deliberately stops at engine, row/size metadata, columns,
  and types. DDL, keys, and richer summaries remain M5 work.
- The original macOS credential backend used `keyring`, whose Apple backend
  reads legacy generic-password items and gives poor control over authorization
  prompts. zeDB now uses Security framework `SecItem` operations directly and
  caches an unlocked credential for the workspace session. Signed development
  builds embed the `dev.zedb.app` Mac App Development profile and use its
  matching application, team, and Keychain access-group entitlements. Passwords
  live in the data-protection Keychain with user-presence access control. Raw
  `cargo run` builds must not be used for credential testing.

## M5: object inspector (2026-08-05)

Status: implementation complete, awaiting UI acceptance.

- Selecting a schema object loads its columns and `system.tables` details in
  parallel without blocking GPUI.
- The inspector keeps the compact object identity header visible and separates
  metadata into Overview, Columns, and DDL tabs.
- Overview exposes the full engine definition, partition key, order-by key,
  primary key, row count, and stored size. Empty keys are shown explicitly.
- Columns retain a fixed header and independent scrolling. DDL is read-only,
  line-numbered, scrollable, and can be copied. SQL highlighting remains M8
  work as planned.
- M8 must use one Tree-sitter SQL rendering path for the query editor, full DDL
  documents, and engine-definition fragments. The engine block wraps in M5 so
  its content never depends on a hidden horizontal scrollbar.

## M6: query editor v1 (2026-08-05)

Status: complete.

- Full Vim support is a product requirement, enabled through a persistent
  Preferences toggle. M6 therefore starts with a command-driven multiline
  editor core instead of extending the connection form's single-line input.
- M9 is the explicit Vim-mode milestone, including modal editing, operators,
  motions, text objects, registers, search, repeat, counts, macros, and relevant
  command-line actions. Daily-driver polish moves to M10.
- Query tabs use `gpui-component`'s multiline code editor. `Cmd+Enter` runs the
  selection when present and otherwise runs the full buffer. Tabs can be added
  and closed, while the final tab and an actively running tab are protected.
- Query execution stays on the shared Tokio runtime. Cancellation aborts the
  task and drops the in-flight HTTP request. Success, failure, and cancellation
  retain a per-tab elapsed time with sub-millisecond precision where useful.
- The SQL Tree-sitter bundle landed early for the query editor because the
  editor dependency already supplies it. It uses a general SQL grammar, so
  ClickHouse-specific constructs may be highlighted incompletely. M8 still
  owns the shared highlighted DDL and engine-definition renderer.
- The editor uses the zeDB palette, a compact gutter, and a contained error
  surface. Connections and Schema now share a vertically draggable divider,
  using the same 1-pixel line and forgiving 8-pixel hit target as the main
  sidebar splitter.

## M7: streaming results (2026-08-05)

- Added incremental `RowBinaryWithNamesAndTypes` decoding that retains partial
  headers and rows across arbitrary HTTP chunk boundaries.
- Query tabs stream result batches into their own virtualized grids, preserve
  cancellation, and report fetched rows and elapsed time while running.
- Streaming queries use unique query IDs and poll `system.processes` while
  waiting for both response headers and body data. The status strip reports
  rows read, estimated total rows, bytes read, and live elapsed time. Received
  HTTP bytes remain available as a permission-safe fallback.
- Decoded rows are coalesced into batches of up to 512 before crossing into
  the UI, and progress updates are throttled to 100 ms to avoid unnecessary
  renders on large results.
- Each tab owns its maximum-row setting. The picker defaults to 100k and offers
  1k, 10k, 50k, 100k, 1m, and explicit Unlimited modes.
- The editor, results grid, and query status strip have independent vertical
  resize handles with narrow dividers and forgiving drag targets.
- Component popovers use the same Menlo font, muted surfaces, borders, and
  hover contrast as the rest of zeDB.
- The real ClickHouse integration test covers incremental streaming, server
  progress, the row cap, typed columns, and final byte accounting.

## M8: SQL highlighting (2026-08-05)

Status: implementation complete, awaiting UI acceptance.

- Query buffers, full DDL documents, and engine-definition fragments now use
  the same `gpui-component` code-editor and Tree-sitter highlighting path.
- The bundled grammar is `tree-sitter-sequel` 0.3.11, a permissive general SQL
  grammar. It handles standard SQL structure, functions, arrays, tuples,
  lambdas, window expressions, and many ClickHouse expressions usefully.
- ClickHouse-only clauses are not fully modeled. In particular, `PREWHERE`,
  `ARRAY JOIN`, `LIMIT BY`, table `TTL`, codecs, engine parameters, and some
  `SETTINGS` forms may parse or color as generic identifiers rather than their
  precise semantic role. This is a highlighting limitation only and does not
  affect query execution.
- The DDL viewer remains read-only, line-numbered, scrollable in both axes, and
  copyable. Engine definitions are formatted one top-level clause per line and
  use a compact highlighted block without line numbers.
- The initial M8 acceptance buffer contains safe ClickHouse-flavored SQL that
  makes supported syntax and grammar gaps visible immediately. New query tabs
  continue to start with `SELECT 1;`.

## M9: Vim visual selection rendering (2026-08-05)

- modalkit owns visual-mode state; `EditBuffer::get_leader_selection` returns a
  sorted `(start, end, shape)` triple, which the snapshot converts into a
  single character range (char-wise ranges include the cursor character,
  line-wise ranges run from column 0 through the start of the line after the
  selection).
- `gpui-component` 0.5.1 `InputState` has no public way to set an arbitrary
  selection range: `selected_range` and `select_to` are crate-private, and the
  public surface only offers `set_cursor_position` plus fixed-motion select
  actions. Upstream contribution candidate: a `set_selected_range` method.
- Workaround: the public `EntityInputHandler` impl can position a selection.
  `replace_and_mark_text_in_range(Some(range), same_text, Some(0..0))` sets
  `selected_range` to exactly `range` without changing the buffer, and an
  immediate `unmark_text` clears the IME marked range before it renders. The
  no-op replace does push an entry onto the input's own undo history, which is
  harmless here because undo/redo in Vim mode goes through modalkit.
- Documented limitations: block-wise (Ctrl-V) selections cannot be expressed
  as the editor's single linear range, so they get no highlight; the status
  bar reports V-BLOCK (and V-LINE) so the active shape is always explicit.
  For selections extended backwards, the highlight is correct but the cursor
  renders at the selection end, since `selection_reversed` is crate-private.

## M9: Vim key routing and the modalkit desync crash (2026-08-05)

- GPUI dispatches key events to matching key BINDINGS before any key-down
  listener, and a handled binding ends dispatch entirely. In Vim mode this let
  the editor's built-in Enter/Backspace/arrow bindings edit the buffer while
  modalkit never saw the key; feeding the editor's moved cursor back into
  modalkit then indexed past its rope and aborted the app (ropey
  "Line index out of bounds", non-unwinding panic inside the AppKit key
  handler). Element-level `on_key_down`/`capture_key_down` cannot fix this
  because listeners run after binding dispatch.
- The correct hook is `App::intercept_keystrokes`, which fires before action
  dispatch; calling `stop_propagation` there suppresses both the editor's
  bindings and the macOS IME insertText path (gpui only forwards printable
  keys to the IME when the GPUI dispatch did not handle them).
- `VimController::set_cursor` now clamps line and column against the modalkit
  buffer; modalkit's `set_leader` stores positions unchecked and later edits
  index the rope with them.
- The editor cursor is only fed back into modalkit outside visual mode; in
  visual mode the editor cursor tracks the selection end, not the Vim head.

## M2 spike surface removed (2026-08-05)

- The synthetic 1M-row demo (launcher button, banner panel, generated cells)
  is gone; the virtualized grid itself lives on as the per-tab results grid,
  now holding plain columns/rows with an empty initial state.

## M9: search, macros, and the command line (2026-08-05)

- The command bar is now real: `/`, `?`, and `:` focus a dedicated modalkit
  EditBuffer rendered in the editor status strip, so typed patterns no longer
  leak into the query buffer. Submit stores the pattern via
  `set_last_search` and runs the deferred action; `n`/`N` work through the
  buffer's `Searchable` impl.
- Macro recording/replay (`q`, `@`, counts) mirrors modalkit's `KeyManager`
  key-replay loop inline in `VimController`; the wrapper itself is unused
  because boxing the machine hides `ModalMachine::mode()`. A "recording @x"
  indicator shows in the status strip.
- Ex commands parse through `VimCommandMachine`. modalkit's built-in set is
  mostly window/tab management (surfaced via the unsupported-action notice)
  and `:substitute` is not yet implemented upstream; both cases surface an
  explicit message instead of silently doing nothing.

## Phase 1 M0-M3: format, repo model, pin, regen (2026-08-05)

- Format v1 decided in FORMAT.md; repo model and zedb CLI (init, new, ls,
  show) landed with fixture-backed tests; pinned binary management caches
  per version under the platform cache dir and downloads from the official
  GitHub release assets (both channels tried; the app binary was renamed
  zedb-app to give the zedb name to the CLI).
- The replay module ports canonical.py: statement splitting, declustering,
  access-control stripping, multi-side isolated clickhouse-local replays,
  canonical dumps, sentinel round-tripping. Verified against the real
  pinned 26.3.12.3 binary.
- regen ports regen.py onto format v1: text tracking (CREATE/DROP/GRANT/
  REVOKE/RENAME with attached comments), generalized scopes (hardcoded
  global/org became config-driven), shared databases discovered from the
  chain instead of hardcoded, the dependency bump pass, and the
  three-replay canonical phase with self-verifying synthesis. ON CLUSTER
  now only travels where a file declared it, so unclustered repos never
  gain the clause. Demonstrated live: ALTER rewrites exactly one file
  canonically, data-only migrations cause zero churn, reruns are
  byte-stable, and regen --check catches hand edits.

## Phase 1 M4-M5: checks and the live runner (2026-08-05)

- zedb check sql (real-parser validation with file:line:col errors) and
  check equivalence (current-state vs chain canonical diff) landed; the
  lifecycle check waits on the runner it drives.
- The runner ports runner.py: versioned tracking bootstrap (zedb_meta +
  zedb_migrations with a params map), argMax-latest applied-set queries,
  upgrade with --to ceiling, peel-from-top rollback plus the --to walk with
  refuse-before-touching gates, stamp, targeted apply with allow-list
  enforcement, and status. Mutations demand --write; structural rollbacks
  warn, irreversible ones need --irreversible, removing a customisation
  needs --targeted; every run appends to a local audit log with password
  values redacted. Deferred from the ancestor, recorded here: admin
  credential routing and live parameter inheritance (rendered params are
  recorded in the tracking table instead).
- EphemeralServer runs the pinned binary as a throwaway single-node server
  for tests. Two gotchas: ClickHouse forks a watchdog parent unless
  CLICKHOUSE_WATCHDOG_ENABLE=0, so killing the spawned pid orphans the
  real server (this had leaked a dozen servers from the ancestor's checks
  on this machine), and /ping responses are chunked, so readiness must
  check the status line, not the body.

## Phase 1 M6-M7: fleet targeting and verify (2026-08-05)

- Target resolution reports what --all skipped (database + owning group)
  instead of only printing, so both CLI and tests see it; --group and
  --db reach excluded databases deliberately.
- zedb verify replays exactly the migrations each database's tracking
  rows say ran and diffs the canonical result against system.tables under
  shared normalization. Databases behind head verify clean at their own
  position, which the fleet view will lean on.

## Phase 1 M8: the importer, and the acceptance gate (2026-08-05)

- zedb import converts an analytics-clickhouse-ddl checkout: migrations
  copy verbatim, exceptions.toml becomes exclusions.toml, and zedb.toml is
  synthesized with the ancestor's built-ins made explicit: declared
  global/org scopes, shared bootstrap databases under [replay], the pinned
  version parsed from pin.py, and the analytics parameters declared with
  the ancestor's dummy and sentinel values (new ParamConfig fields; the
  offset expressions cannot use generated numeric sentinels).
- The acceptance gate passes on the real repo: import without hand
  editing, regen byte-stable, sql and equivalence checks green. Beyond
  the semantic bar, the regenerated current-state is byte-for-byte
  identical to the ancestor's committed tree, including the Replicated
  engine transplant with a custom zk path on the ALTERed ConversionFacts
  and the rv_ naming, generalized to shared-database initials.
- import-tracking copies default.schema_migrations into zedb_migrations
  server-side, preserving recorded_at so argMax-latest state (including
  rollback history) carries over; a second run refuses. Remaining
  acceptance: zedb status against staging agreeing with ddl status, which
  needs staging credentials.

## Phase 1 M9 (buildable half) and the first cluster deploy (2026-08-05)

- status, verify, and check gained --json; CI now lints and tests
  zedb-cli and seeds the zedb binary cache from the installed clickhouse
  so the replay-backed tests run instead of skipping. A ready-made
  workflow for migration repos lives in docs/ci-migration-repo.example.yml.
- First real clustered deploy: zedb_demo onto the two-node docker cluster
  (zedb_cluster, keeper-backed). ON CLUSTER DDL fanned out through the
  distributed queue, ReplicatedMergeTree replicated inserted rows to the
  second node, the tracking tables themselves replicated (first exercise
  of the clustered ensure_tracking path), and zedb verify reports clean
  at 00100. Remaining for M9: the staging status comparison and one real
  migration end to end, both needing fleet credentials.

## Phase 1 M4 closed: the lifecycle check, and admin routing (2026-08-05)

- zedb check lifecycle ports the ancestor's deepest gate: a throwaway
  single-node clustered server (embedded Keeper, real distributed DDL),
  a migrator user with a real provisioning user's restricted grants,
  baseline provisioning as written (ON CLUSTER, Replicated engines),
  stamp, upgrade, targeted smoke tests, the full rollback walk, and a
  final schema diff against replayed current-state.
- Running it against the real imported repo immediately proved the M5
  admin-routing deferral wrong: the chain's ALTER ADD COLUMN is refused
  to the migration user. Admin routing is now ported: statements matching
  the ancestor's classifier (OPTIMIZE, TRUNCATE, ALTER TABLE, functions,
  SYSTEM, definers) execute over --admin-user credentials, and the
  lifecycle check first proves the run fails without them. Deferred
  still: per-replica fan-out of SYSTEM statements on multi-replica
  clusters (single-replica servers behave identically without it).
- The real repo now passes the whole check: 29 vs 29 objects, zero
  differences. One debugging note for posterity: a Python heredoc turned
  regex \b into literal backspace bytes, which display as nothing and
  match nothing.

## SYSTEM statement fan-out (2026-08-05)

- SYSTEM statements (START/REFRESH VIEW, ...) take no ON CLUSTER and act
  on the connected node only, but every replica keeps its own refresh
  scheduler, so the runner now fans them out: replicas are discovered
  from system.clusters on the connected node and reached on the same
  HTTP port and credentials (the ancestor's assumption; port-mapped
  docker topologies where each node maps to a different host port cannot
  satisfy it, which is why the fan-out test bed is the single-replica
  lifecycle server short-circuiting to the connected executor). Failures
  on any replica fail the migration loudly.

## Phase 2 M0-M1: fleet view first light (2026-08-05)

- The app opens a migration repo (path input with ~ expansion, remembered
  in preferences) and renders the databases x migrations matrix: applied,
  pending, failed, customised (targeted columns marked *), and excluded
  databases parked in-line with their group named. Filter-by-typing
  narrows rows. Status fetch runs on the shared tokio runtime through the
  same Runner the CLI uses, with a generation counter so stale responses
  cannot clobber newer ones.
- Verified live against the seeded demo fleet: twelve databases showing
  every state at once, filter narrowing instantly. Remaining for M1: the
  several-hundred-database scale check. M2 (drift) and M3 (dry-run) next.

## Phase 2 M2-M3: drift and dry-run in the detail panel (2026-08-05)

- Selecting a matrix row opens a detail panel: state summary, an
  on-demand Verify (the real replay-backed verifier on the tokio runtime,
  never blocking the matrix) rendering clean-with-age or the full
  expected/live drift findings, and the dry-run section showing every
  pending migration's SQL rendered with that database's parameters,
  unresolved placeholders (like ${cluster}) left visible rather than
  guessed.
- Verified in the GUI against the demo fleet: kappa's sneaky column
  surfaced as a drift finding, eta's pending 00100 rendered per-database.
  Scale: a 400-database synthetic registry statuses in 0.7s and rows ride
  the virtualized list proven at 1M rows in the grid spike.
- Driving gpui with synthetic CGEvents needs a preceding mouse-move and
  mouseEventClickState=1, or clicks are silently dropped.

## Phase 2 M4: applies behind the safety ladder (2026-08-05)

- Mutation reached the GUI, gated: a per-session "Writes locked/unlocked"
  toggle that disarms on every connection change, a cluster input
  (remembered in preferences; blank means declustered), and per-database
  Upgrade / Rollback-of-head / targeted apply-remove buttons plus a
  fleet-wide Upgrade all, every one of them funneled through a single
  confirmation modal. The modal carries the environment tier as its
  header color and label, the rendered dry-run of exactly what would
  execute (unresolved placeholders visible), a click-to-acknowledge gate
  for structural rollbacks, an IRREVERSIBLE warning plus typed
  "irreversible" for irreversible ones, and a typed database-name (or
  "all") confirmation on production tiers. Execution runs the same
  Runner calls as the CLI (write consent, tracking rows, audit log,
  redaction all identical) and refreshes the matrix on completion.
- Not GUI-automatable here: connects now require Touch ID for the saved
  credential, so the live ladder walk is a human step. Deferred: admin
  credentials from the GUI, per-statement live progress (the modal shows
  per-run progress and the runner's error text).

## Phase 3 M0: fleet git awareness (2026-08-06)

- On the phase-3 branch. zedb-core gains a git module: shell out to the
  user's git (status --porcelain=v2 --branch), parse branch, dirty entry
  count, and ahead/behind the local remote-tracking ref. Not a checkout
  means None, not an error; zeDB never fetches, so ahead/behind is as
  fresh as the user's last fetch, and the doc comment says so.
- Fleet view shows the summary (branch, dirty star, +ahead -behind) next
  to the repo chip in the bottom strip, amber when stale; re-read on
  every refresh. The action modal leads with the specifics (uncommitted
  changes, behind upstream, detached HEAD) before the consent controls,
  meeting M0's warn-before-the-ladder condition.

## Phase 3 M1: authoring in the app (2026-08-06)

- New migration drafts live entirely in memory: a fleet-toolbar "New
  migration" button opens an overlay with upgrade.sql and rollback.sql
  in the code editor surface, a rollback-class picker (clean /
  structural / irreversible / no rollback) that rewrites the marker
  line, and a chain-vs-targeted toggle. Check runs check_sql_text (new
  text variant extracted from check_sql_file, which now delegates)
  against the pinned binary via ensure_binary; errors render
  file:line:col exactly as the CLI prints them.
- Save is only enabled while the last passing check still matches the
  editor text byte-for-byte; it then scaffolds the next chain number,
  writes the draft over the templates, reopens the repo, and refreshes
  the matrix. So a deliberate SQL error surfaces before anything is
  written to the chain, which was the milestone's done-condition.
- Verified headlessly: a simulated app save (scaffold + draft overwrite
  on a demo-fleet copy) passes `zedb check sql` 10/10 and lists in the
  chain with class and headline intact. Deferred: Vim mode in the
  authoring editors (query tabs only for now), editing an existing tip
  migration, check-as-you-type debounce (IDEAS.md has the live-check
  entry).

## Phase 3 M2: codegen in the app (2026-08-06)

- Two fleet-toolbar buttons. Regen replays the chain through the pinned
  binary in the background and shows diff_tree's churn lines (stale /
  missing / unexpected per generated file) in a modal; writing
  current-state is a separate explicit button, absent entirely when the
  tree is in sync. Check chain runs sql, equivalence, and lifecycle
  concurrently with per-check progress lines, passing summaries phrased
  like the CLI's and failures rendered as the reports' difference lines.
- Same functions as the CLI end to end (Regenerator, diff_tree,
  write_tree, check_sql, check_equivalence, check_lifecycle), so the
  M2 exactly-the-tree-zedb-regen-produces condition holds by
  construction; verified on a demo-fleet copy where an app-style
  authored migration produced the expected single stale line, the
  written tree carried the new column, and equivalence passed.

## Phase 3 M3: commit and push in the app (2026-08-06)

- zedb-core git grows the mutation half: changed_paths (porcelain v2,
  handles renames and unmerged entries), commit_paths (stages and
  commits exactly the named pathspecs, so anything else already staged
  stays out by construction), and push (git's own words verbatim on
  failure). Tested: an unrelated dirty file survives a migrations-only
  commit untouched, and push without a remote fails readably.
- The fleet toolbar shows Commit only while the checkout is dirty. The
  modal partitions dirty paths into repo-owned (migrations,
  current-state, zedb.toml, exclusions.toml), which are listed and
  committed, versus everything else, which is shown and left strictly
  alone. The message is templated from the tip migration's headline and
  editable in a multi-line input. Push is a separate button with its
  own progress and verbatim git errors; the git chip refreshes after
  both steps. No forge, no conflict resolution, no history rewriting.

## Decluster missed quoted cluster names (2026-08-06)

- First real M4-loop walk found it: an authored migration using
  ON CLUSTER '${cluster}' (quoted, which ClickHouse accepts) survived
  decluster because the regex only matched bare identifiers, so the
  replay hit QUERY_IS_PROHIBITED on clickhouse-local. The regex now
  accepts bare, single-quoted, double-quoted, and backticked names,
  with unit tests for each and for ON CLUSTER inside a string literal
  staying untouched. CRITICAL blast radius per impact analysis (regen,
  verify, runner, CLI and app), but strictly widening.
- With that fixed, the same walk produced the next, correct error:
  the draft altered a table the chain never creates, which the replay
  catches and syntax checks cannot. The modal wording is doing its job.

## Migration view/edit from the matrix header (2026-08-06)

- Clicking a migration number in the matrix header opens it in the
  authoring overlay. Editability follows the only rule that matters:
  a migration is a draft until it has been applied (or
  customised-applied for targeted ones) on ANY database, judged from
  the fleet status rows (head passed it and not pending); after that
  it opens read-only with the applied count in the header, since
  history that ran somewhere is immutable. Not git state: an
  uncommitted-but-applied migration is just as frozen.
- Edit mode saves in place (upgrade.sql, rollback.sql presence
  following the class picker, targeted.toml following the toggle)
  behind the same check-then-save gate as new drafts. With no fleet
  status loaded, editing stays possible but carries an explicit
  warning to confirm the migration never ran anywhere.

## Lifecycle check vs targeted allow lists (2026-08-06)

- The in-app Chain checks run surfaced it: the lifecycle smoke test
  applied targeted migrations through the same allow-list enforcement
  as real applies, and the ephemeral lifecycle_db can never be on an
  allow list, so any repo pinning a targeted migration to named
  databases (demo-fleet's 00200 -> zedb_theta) could never pass check
  lifecycle or check all. Policy gating a throwaway database verified
  nothing.
- The check now bypasses allow lists via a crate-only
  apply_targeted_for_check; the public apply_targeted keeps its exact
  signature and enforcement, so no real apply path can reach the
  bypass. The step narration says the bypass happened. check all on
  demo-fleet is green across sql, equivalence, and lifecycle.

## Tracking gets a home and an identity (2026-08-06)

- Surfaced by pointing a second, empty repo at the demo cluster: it
  read demo-fleet's tracking rows, because both repos used
  default.zedb_migrations and the rows carry no notion of whose they
  are. Two changes. Tracking now defaults to its own zedb_config
  database (one per cluster is the expected shape), and
  zedb_migrations gains a repo identity column (ORDER BY (repo, db,
  migration)), filtered in every read and written by every insert,
  with [tracking].repo overriding the default of the repo directory
  name.
- The tracking database is never a migration target, however the
  registry is written; the default registry also excludes `default`.
  The demo cluster's 93 tracking rows moved to
  zedb_config.zedb_migrations with repo='demo-fleet', verified by
  status parity before dropping the old default.* tables; check all
  stays green with the new schema.

## Phase 3.1 M0: the ACP round trip (2026-08-06)

- New zedb-acp crate: a headless Agent Client Protocol client. JSON-RPC
  2.0, one JSON object per line over stdio; three pumps (writer,
  reader, stderr) around a spawned agent process with kill_on_drop;
  responses routed to pending requests, session/update notifications
  decoded into AgentEvents, agent-initiated permission requests carried
  as events with a oneshot responder (an unanswered responder counts as
  cancelled so the agent never hangs on us), unknown methods refused
  politely, and unknown update kinds carried as Other rather than
  dropped.
- Proven by four lifecycle tests against a scripted fake agent (happy
  stream, permission round trip, cancel mid-stream with a cancelled
  stop reason, and abrupt death failing pending requests and emitting
  Closed), plus a smoke example against the real Claude Code adapter
  (npx @agentclientprotocol/claude-agent-acp): initialize, session,
  streamed pong, end_turn, with the adapter's newer update kinds
  (usage_update and friends) flowing through as Other exactly as the
  lenient decoding intended.

## Phase 3.1 M1: the thread pane (2026-08-06)

- The agent pane: a resizable right-hand column with a Zed-shaped
  header (thread title, a + dropdown listing External Agents plus a
  disabled Add More Agents until M2, close), transcript, and composer.
  Threads spawn the built-in agents (Claude Code and Codex adapters
  via npx) with sessions rooted in the open migration repo's checkout,
  falling back to home. Streamed replies render as markdown through
  gpui-component's TextView (inline code and fences for free), tool
  calls show as compact status lines that update in place, permission
  requests render as inline option cards wired to the responder, and
  Stop cancels the turn.
- Two launch lessons. AgentConnection spawns tokio tasks and a tokio
  child process, which abort the whole app when called outside the
  runtime (the lifecycle tests never saw it: tokio::test provides the
  context); the pane now enters the shared runtime before spawning.
  And codex-acp dumps entire ANSI-colored model configs as single
  stderr lines, so the pre-session status line now strips escapes and
  clamps to one truncated line. Verified live: a Claude Code thread
  answers with correct markdown in the pane under the user's existing
  auth; Codex boots but showers first-run keychain prompts for its own
  credential items (its design, not ours; per-item Always Allow is
  durable).

## Phase 3.1 M2: agent discovery and settings (2026-08-06)

- zedb-acp gains a discovery module: known agents (Claude Code, Codex)
  resolved against the machine with commands as absolute paths, because
  GUI apps launch with a skinny PATH; lookup covers PATH plus homebrew,
  /usr/local, ~/.local/bin, ~/bin, and nvm's per-version bins.
  Availability is Ready / NeedsLogin / Missing with human-actionable
  hints, keyed off binaries and each agent's auth markers (~/.claude*,
  ~/.codex/auth.json); NeedsLogin stays launchable since the heuristic
  can be stale.
- The + menu is built from discovery each open: ready agents plain,
  needs-login with a dim hint line, missing entries disabled with the
  install hint. Add More Agents now works: a name plus a command line
  saved to preferences (custom_agents), listed with remove buttons,
  re-resolved on every registry refresh, wearing the sparkle mark.
- Verified live: both built-ins Ready with no configuration, and a
  hand-added custom agent (the test crate's fake-agent binary) appears
  in the menu, threads, streams a reply, and shows its tool call.

## Phase 3.1 M3: fleet context via MCP (2026-08-06)

- zedb mcp: a read-only MCP server over stdio in zedb-ch, exposed as a
  CLI subcommand and as a hidden serve mode in the app binary (the
  bundle ships no CLI; the pane spawns its own executable). Fleet
  tools ride the same runner/verifier as everything else; ClickHouse
  tools go through a new query_guarded client path with server-side
  readonly plus execution-time, row, and byte caps. Proven live:
  guarded queries and fleet status against the demo cluster, an
  INSERT refused READONLY, and a terminal-run Claude Code answering
  fleet questions through the CLI form of the server.
- Pane sessions register the server automatically: connection
  credentials travel via a 0600 file the server deletes on read,
  never argv or env. Sends carry ambient screen context (screen,
  connection with tier and posture, repo with git summary, selected
  fleet row with status and fetched drift, open modal or authoring
  overlay), attached visibly as an entry in the transcript and
  toggleable per thread. Remaining for live sign-off: the deictic
  what's-wrong-with-this-database walk, which needs a Touch ID
  connect.

## Phase 3.1 M3 sign-off (2026-08-06)

- The deictic drift walk, from the agent log: asked what's wrong with
  zedb_kappa, the agent went straight to mcp__zedb__fleet_status and
  mcp__zedb__drift (ignoring the user's unrelated ClickHouse MCP
  servers, so the steering line works), the permission cards answered
  allow_always, and the diagnosis separated clean migration state from
  schema drift, named both rogue columns, noted the read-only dev
  posture, and offered to draft the reconciling migration. M3 done;
  that offer is M4's cue.

## Phase 3.1 M4: the authoring and editor bridges (2026-08-06)

- App-hosted tools reach pane agents through a unix-socket bridge: the
  MCP serve subprocess forwards propose_migration, propose_query, and
  navigate to the running app, whose window-needing effects (editor
  creation) queue and apply at render. propose_migration fills the
  authoring overlay (class, targeted, marker-line handling), surfaces
  the fleet view, and refuses honestly when no repo is open or a draft
  is already up; propose_query lands SQL in a fresh query tab;
  navigate switches views, selects fleet rows, and auto-opens the
  repo. Every app-tool use is narrated in the transcript.
- Assistant replies containing fenced SQL grow insert-into-editor
  buttons per block, and a repo watcher polls the open checkout's file
  signature every two seconds while a thread lives, reopening the
  chain and refreshing the git chip when agents edit files with their
  own tools (narrated as repo files changed on disk).
- Verified live through the real permission flow: an agent navigated
  the app to the fleet view (repo auto-opened) and proposed a draft;
  both tool calls approved via the pane's cards.

## Phase 3.1 M5: permissions and daily-driver polish (2026-08-06)

- Transcript text is selectable and copyable everywhere it matters
  (assistant markdown, user bubbles, approval notices) via selectable
  TextViews, and the transcript sticks to the bottom while streaming,
  unsticking when the user scrolls up and re-sticking near the end.
- Each session's first send carries the AGENT_PRIMER (orientation on
  zeDB, the tool menu, draft-not-deed, templating), slimming the
  per-send ambient context to just the screen snapshot. Cmd-i and
  thread start focus the composer.
- Permissions finished: requests queue rather than superseding, cards
  preview the tool's raw input, and Always Allow choices persist in
  preferences per agent-and-tool, auto-approving across sessions with
  a narrated line. Pane width persists. Transcripts persist per turn
  and reopen read-only from the empty pane; long threads trim past
  600 entries; the empty-pane hint finally renders.

## Phase 3.2: schema intelligence (2026-08-06)

- Added a per-connection schema cache in zedb-ch. Readers load immutable
  snapshots through ArcSwap, while tables refresh fleet-wide in one
  system.tables sweep and columns load only for touched databases. Snapshots
  persist atomically under the app cache directory, survive relaunch, and
  retain at most 64 warmed column sets while preserving every database and
  table entry.
- The first performance test caught a linear column lookup. Cached columns
  now use hash maps, and 100,000 lookups against a synthetic 500-database,
  10,000-table fleet stay inside the debug-build budget. Missing column data
  remains distinct from a known-empty column set, so partial or stale caches
  never create false diagnostics.
- Query editors now get conservative schema diagnostics after a 180 ms
  off-thread debounce, in-process table and alias-qualified column
  completions, and Markdown hover details for known tables and columns. CTEs,
  unqualified ambiguity, missing default databases, and unwarmed columns all
  stay neutral. Nothing in these paths calls the network.
- Connection startup opens the persisted snapshot in the background, shows
  its databases immediately, refreshes tables after connect and on the
  existing five-minute health cadence, and prioritizes columns when a
  database or object is touched. Successful DDL invalidates and refreshes the
  affected database in the background. The schema pane quietly reports
  warmed databases without adding a new control pattern.
- Verification: cargo check --workspace and cargo test --workspace pass,
  including 99 unit, integration, live ClickHouse, replay, import, runner,
  drift, and repository-format tests.
