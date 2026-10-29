# 0009. GTK4 + libadwaita UI with libpanel docking

- **Status:** Superseded by 0014 (docking only; GTK4 + libadwaita stay)
- **Date:** 2026-10-28

## Context

The UI needs native GNOME-style widgets (alert dialogs, toasts) and panels for
project, groups and agents that can be dragged, docked to edges, stacked as tabs or
detached.

## Decision

- UI toolkit: `gtk4` + `libadwaita` (gtk-rs). `adw::AlertDialog` for the permission
  popup.
- Docking: `libpanel` (GNOME Builder's dock library) through the community Rust
  bindings, using `panel::DocumentWorkspace` and `panel::Widget`.
- Plan B if the bindings get in the way: our own docking with `gtk::Paned` +
  `gtk::DragSource`/`gtk::DropTarget`.
- Crate versions must move together: `gtk4` 0.11, `libadwaita` 0.9, `libpanel` 0.7
  (all on glib 0.22).

## Consequences

- Building `app` needs the system `libpanel` (Arch: `pacman -S libpanel`).
- The libpanel bindings are community-maintained and may lag behind gtk-rs releases.
- Layout save and restore per project (step 5) has to serialize libpanel positions.
