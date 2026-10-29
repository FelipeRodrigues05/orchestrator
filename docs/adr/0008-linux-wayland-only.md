# 0008. Linux + Wayland only

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

Supporting several platforms would multiply work on audio, notifications, global
shortcuts, secret storage and window behaviour. The author uses Linux on Wayland.

## Decision

We target Linux with Wayland only, for now. We rely on Wayland-era desktop APIs:

- `xdg-desktop-portal` via `ashpd` (e.g. `GlobalShortcuts`).
- `gio::Notification` with action buttons.
- PipeWire for audio through `cpal`.
- `gtk4-layer-shell` where the compositor supports it.

## Consequences

- The app cannot stay always on top or steal focus. Permission requests use a
  system notification with buttons plus a voice announcement; the full dialog opens
  when the window gets focus again.
- Global shortcuts depend on the compositor (KDE, GNOME 48+); otherwise shortcuts
  only work while the window is focused.
- Porting later would need abstractions we are deliberately not building now.
