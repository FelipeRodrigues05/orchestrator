# 0023. Agent library (global agents) and memory

- **Status:** Accepted
- **Date:** 2026-11-04

## Context

An agent belonged to one project: its persona, provider, model and learnt
permissions stayed there. The operator wants the same agent (a marketing
specialist, a reviewer) in any project, and wants it to keep what it learnt about
the operator and their conventions instead of starting from zero each time.

## Decision

- **Library.** A global agent is a `Profile` (name, persona, provider, model,
  colour, permission rules), stored in its own SQLite table `profiles`. Agents keep
  a nullable `profile_id`.
- **Saving.** `:save [agent]` or the palette's "Save … to the library" turns an
  existing agent into a profile and links it. `:forget <name>` removes a profile;
  agents already made from it keep working, unlinked.
- **Using.** The new-agent wizard asks, after the group, for "+ new agent" or one
  from the library (the step is skipped while the library is empty). The palette's
  "Agent library" (`:library`) picks one first and then asks only for the group
  and the task. `Command::CreateFromProfile` creates the agent in the focused
  project, opening the personal workspace if no project is open.
- **Kept in step.** Changing persona, provider, model or colour of a linked agent
  updates the profile and its other agents. A permission rule learnt by one of them
  holds for all of them.
- **Memory.** One Markdown file per memory key under `<data dir>/memory/<key>.md`:
  the key is the profile id for global agents, the agent id otherwise
  (`core/src/memory.rs`). Each note is one dated line. Agents add notes with the
  `remember` team tool; the operator with `:remember [agent]: <note>`. The newest
  notes, up to 3000 characters, go into the agent's briefing. Saving an agent to
  the library moves its memory to the profile.
- Files, not SQLite, so the operator can read and edit memory with any editor.

## Consequences

- Memory grows without bound on disk; only its tail is sent, so token cost stays
  capped. Cleaning it is a text edit.
- The same agent working in two projects at once shares one memory file; appends
  are line-sized, so interleaving is harmless.
- A profile's persona is shared: tuning it for one project changes it everywhere.
