# 0003. Actors and channels, tokio on its own thread

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

Several agents, the permission broker, the voice pipeline and the UI all run at
the same time. GTK must only be touched from the main thread, and mixing the GLib
main loop with tokio is a known way to freeze the UI. Shared state behind mutexes
across these components is hard to reason about.

## Decision

- Agents, broker, voice and orchestrator are actors: `tokio::spawn` + `mpsc` for
  messages, `oneshot` for replies.
- We avoid shared `Mutex` state between actors when a message will do.
- The tokio runtime runs on its own OS thread (`core::runtime::Runtime`); GTK owns
  the main thread.
- The UI ↔ runtime bridge is `async-channel`, which works on both tokio and the
  GLib loop. The UI sends `Command` with non-blocking sends and reads `Event` with
  `glib::spawn_future_local`.
- The GTK thread never calls `block_on` or does I/O. CPU-heavy work (whisper)
  runs in `spawn_blocking`.

## Consequences

- Each actor can be tested by feeding it messages.
- Data crossing to the UI must be `Send` snapshots or events, not references.
- One more hop for every UI action; acceptable for a desktop app.
