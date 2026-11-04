# 0022. Start-up animation

- **Status:** Accepted
- **Date:** 2026-11-04

## Context

The room appeared at once, already full. A short start-up says what the runtime
found (providers, sandbox, voice) before the operator relies on it, and gives the
control room the feel of a machine powering on.

## Decision

- When the first state arrives from the runtime, a ~2.4 s animation plays over the
  room (`app/src/ui/room/boot.rs`):
  1. the screen opens from a line in the middle, like a CRT;
  2. a **system check** lists runtime, broker, agent providers, sandbox, speech,
     listening, the pet's model and open projects, each with the real result (OK
     with a detail, or OFF);
  3. the pet wakes up (eyes closed, then open) and says it is awake.
- It starts on the first state, not on the window, because only then are the
  results and the operator's choice known.
- Any key or click skips it, and that key does nothing else. `:intro off` turns it
  off (saved in SQLite as `intro`); `:intro on` brings it back.
- While it plays the window redraws every 40 ms; after it, only the room's usual
  250 ms tick remains.

## Consequences

- A slower start by about two seconds unless skipped or turned off.
- OFF is shown dim, not red: a missing optional part is not urgent.
