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

## Review checklist

Before accepting UI work, check that it:

- Looks at home beside existing zeDB and Zed-style desktop controls.
- Does not introduce a one-off spacing or button pattern without reason.
- Makes the primary action obvious without making every action prominent.
- Shows destructive or production risk clearly at the moment it matters.
- Remains legible and useful at dense desktop-window sizes.
