# 0014. ASCII control room instead of libpanel docking

- **Status:** Accepted
- **Date:** 2026-10-28
- **Supersedes:** the docking part of [0009](0009-gtk4-libadwaita-libpanel.md)

## Context

The first UI used libpanel dock panels (Project, Groups, Agents) around a themed
stage. The panels used stock Adwaita lists, which clashed with the stage, started
hidden, and libpanel's frame headers and tab bars could not be made to look like the
rest. The user wants the app to look and behave like a control room drawn in ASCII
(one board with a sector map, permission queue, event log, agent console and a voice
strip, all sharing box-drawing borders), and said panels do not need to float.

## Decision

- We will draw the whole window as one character grid: `app/src/ui/grid.rs` holds
  cells and routes box-drawing lines (junctions such as `╠ ╦ ╪` are computed),
  `room.rs` lays out the board and handles input, `render.rs` paints it with
  Cairo + Pango (`pangocairo`), stroking box-drawing glyphs so borders always meet.
- Layout and input logic stay free of GTK so they are unit tested in `app`.
- The UI is keyboard first (F1–F4 views, `j/k`, `a/l/d`, `/` command line) and also
  answers clicks through hit regions recorded during layout.
- We drop the `libpanel` dependency. GTK4 + libadwaita stay (window, header bar on
  desktops with client-side decorations, `adw::AlertDialog` for the popup in step 3).

## Consequences

- Building `app` no longer needs the system `libpanel` package; it needs `pangocairo`.
- Panels cannot be dragged, stacked or detached. If that is wanted again, it goes in
  the grid layout (movable regions) or back to libpanel/`gtk::Paned`, with a new ADR.
- Step 5 saves the board layout (view, sizes) instead of libpanel positions.
- Text drawn on a canvas is not exposed to screen readers; the permission popup
  (step 3) and notifications stay real widgets.
- Glyphs missing from the monospace font fall back to other fonts; the renderer
  squeezes wider fallbacks into their cell, and the UI avoids emoji-range symbols.

## Amendment (2026-10-28)

- The permission popup is drawn in the grid as a modal (red double frame, the
  command in a box, `[A]llow A[l]ways [D]eny [Esc] later`) instead of an
  `adw::AlertDialog`, so it matches the room. "Later" only hides it; the request
  stays queued. While the window is unfocused a `gio::Notification` with
  Allow/Deny buttons still goes out.
- Agents, groups, projects, API keys and "open in" are also reachable from a
  command palette (Ctrl+P or Space, LazyVim / VS Code style, fuzzy search) with a
  step-by-step "new agent" wizard (`n`): group (pick or create) › name › provider
  › task.
- Consequence: the popup is no longer a separate accessible widget; the system
  notification and the voice announcement remain the non-visual paths.
