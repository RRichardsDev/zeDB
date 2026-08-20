# UI design contract

zeDB is a dense, keyboard-friendly desktop tool. Its interaction language is
closer to Zed than to a web dashboard or mobile application.

## Principles

- Prefer compact, information-dense controls with clear hierarchy.
- Use reusable primitives instead of styling each screen independently.
- Keep common utility actions visually quiet until hover or focus.
- Use muted semantic colors on dark tinted surfaces. Reserve high saturation
  for transient focus or critical states, not persistent badges.
- Put identity metadata next to the identity it describes. For example, the
  environment tier pill sits beside the connection name.
- Keep safety state visible. Production identity and writable connections must
  never rely on subtle copy alone.
- Avoid explanatory milestone or implementation copy in finished product UI.

## Toolbars and actions

- Familiar utility actions use 24-pixel icon buttons inside a 32-pixel toolbar.
- Toolbars use a subtle divider and align actions to the relevant content edge.
- Use embedded monochrome SVG icons so color follows interaction state.
- Do not use large, full-width action bars for edit, delete, or similar utility
  operations.
- Destructive actions are neutral at rest, destructive on hover, and explicit
  after confirmation.
- Primary workflow actions may use filled buttons. Secondary actions use quiet
  or outlined styling.

## Forms

- Labels are compact and secondary to their values.
- Group related identity fields visually. Environment tier belongs next to the
  connection name, while read-only state belongs with connection safety
  settings.
- Preserve consistent control heights, gaps, borders, and hover states.
- Keep optional and destructive paths subordinate to the primary workflow.

## Structural panes

- Structural panes are resizable unless a fixed size is essential to their
  purpose.
- Show a crisp 1-pixel divider, but center a wider invisible drag target on it.
  Use an 8-pixel target by default so the divider is easy to acquire without
  adding visual weight.
- Use the platform column or row resize cursor while the pointer is over a
  splitter.
- Clamp pane sizes to keep both sides useful. Preserve the chosen size in view
  state for the lifetime of the workspace.
- Reuse the shared splitter pattern for future sidebars, inspectors, consoles,
  and other structural panes.

## Preferences and editor modes

- Editor behavior that persists across launches belongs in Preferences, not in
  an ad hoc toolbar toggle.
- Vim mode is optional and disabled by default. Its preference is global and
  applies consistently to every editable SQL buffer.
- Editor commands are the stable interaction layer. Default shortcuts, Vim
  mappings, menus, and future command-palette actions invoke the same commands.
- Mode state must be visible when Vim mode is active without consuming a large
  permanent toolbar.

## Review checklist

Before accepting UI work, check that it:

- Looks at home beside existing zeDB and Zed-style desktop controls.
- Does not introduce a one-off spacing or button pattern without reason.
- Makes the primary action obvious without making every action prominent.
- Shows destructive or production risk clearly at the moment it matters.
- Remains legible and useful at dense desktop-window sizes.

## State-facing UI: the review bar

Anything that shows or changes the state of something outside the app
(a connection, a Cloud service, a running job) is held to these as
well. Each was learned the expensive way; the parenthetical is the
incident, kept so the rule does not drift back into a platitude.

- **No label may lie or mislead.** Words carry exact truth ("not
  tested" became "not connected": the old word implied it had never
  worked). A state shown optimistically, before the server confirms,
  must either say so or revert visibly when the server refuses.
- **One state, every surface.** A transition started anywhere shows
  everywhere that state appears, immediately, and keeps updating
  until it settles (dashboard wake buttons went stale while the
  connect flow was waking the same service).
- **Transitions are watched, bounded, and abandoned.** Anything
  in flight polls until it settles, gives up after a stated bound
  with a message, and stops when a newer action supersedes it.
- **Confirms must be unmissable.** An armed destructive action says
  what the next click will do, in plain text where the eye already
  is, not in a tooltip or a color alone, and disarms itself when
  abandoned (a red icon and a changed tooltip read as a stuck
  button).
- **Disabled means explained.** A control that cannot work (no
  credential, an upstream rule) renders disabled with the reason in
  its tooltip; it never lets a click bounce off a 4xx.
- **Test upstream rules before encoding them.** Vendor docs and
  plausible theories both got the ClickHouse Cloud primary-stop rule
  wrong until curl against the live control plane settled it. A rule
  shipped in the UI cites the live test, not the documentation.
- **Icons follow the app's grammar.** Quiet utility icons, color set
  on the svg element and recolored with `group_hover` (parents do not
  inherit into svgs), hover color announcing intent (green wakes, red
  stops), and primary actions that cost money keeping their words.
