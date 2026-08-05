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
