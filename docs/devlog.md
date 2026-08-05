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
