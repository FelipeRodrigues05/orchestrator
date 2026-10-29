# 0019. Desktop install, own systemd scope and a tray icon

- **Status:** Accepted
- **Date:** 2026-10-29

## Context

The app ran only from a terminal or the editor (`cargo run`). Started that way, the
desktop portal filed its global push-to-talk shortcut under the parent process (VS
Code, a terminal), launchers did not have the PATH entries a shell profile adds,
and closing the window while the floating pet existed left an invisible process.

## Decision

- `scripts/install.sh` builds in release mode and installs, per user, the
  `orchestrator` binary in `~/.local/bin`, the icon (and its attention variant) in
  the hicolor theme and `dev.feliperodrigues.Orchestrator.desktop` in the app menu;
  `--uninstall` removes them.
- The binary re-runs itself once inside a systemd user scope named
  `app-dev.feliperodrigues.Orchestrator-<pid>.scope` (skipped with `ORCH_SCOPED`) so
  the portal knows the app by its own id wherever it was started.
- It appends `~/.local/bin`, `~/.cargo/bin`, `~/.opencode/bin` and similar to `PATH`
  when missing, and logs to `~/.local/state/orchestrator/orchestrator.log` when it
  has no terminal.
- A StatusNotifierItem tray icon (`ksni`) shows or hides the room on click, toggles
  the wake word and quits; it turns to its attention icon while a request waits, and
  its tooltip lists the open projects. The icon is sent as pixels, and the item
  registers under the connection's unique bus name, because hosts with their own
  icon index (quickshell) did not find themed icons and kept items registered under
  `org.kde.StatusNotifierItem-<pid>-1` after the app quit.
- With a tray, closing the room hides it; without one, closing quits. Activating the
  app again presents the room, never the floating pet's window.

## Consequences

- The app starts from the menu like any desktop app and keeps working in the tray.
- Quitting is an explicit tray action when a tray host is present.
