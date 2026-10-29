# 0002. GTK-free `core` crate in a two-crate workspace

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

Most of the app's behaviour (agent lifecycle, permission rules, timeouts, voice
command parsing, persistence) is logic that must be tested reliably. Tests that
need a GTK window are slow, flaky and hard to run in CI.

## Decision

We will split the Cargo workspace into two crates:

- `core`: all logic (model, broker, runners, orchestrator, voice actors,
  persistence, discovery). It must not depend on `gtk4`, `libadwaita`, `glib`,
  `gio` or any UI crate.
- `app`: GTK4/libadwaita UI. It only draws state and sends commands.

The package is named `core` so `cargo test -p core` works. Its library target is
named `orch_core`, because a crate imported as `core` would clash with Rust's
built-in `core` library.

## Consequences

- `cargo test -p core` covers the logic without a display.
- Pieces that need `gio` (e.g. `gio::AppInfo` for editor discovery,
  `gio::Notification`) live in `app`; lists of known binaries can live in `core`.
- Enforced by checking `cargo tree -p core | grep -E 'gtk|adw|glib'` is empty.
