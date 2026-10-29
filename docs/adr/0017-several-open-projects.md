# 0017. Several projects open at once

- **Status:** Accepted
- **Date:** 2026-10-29

## Context

The SPEC opens one project at a time: opening another stopped every agent of the
first. Operators work on several repositories in parallel (an API and its front end,
a library and the app that uses it) and want their agents to keep running while
they look at another project.

## Decision

- `AppState` holds every open project (`projects`) and the one in focus (`focus`).
  `groups` and `agents` hold those of all open projects; a group's `project_id` says
  where it belongs. `AppState::scoped()` gives the focused project's slice.
- `OpenProject` adds a project next to the open ones (or focuses it if it is already
  open) and never stops agents. `FocusProject` changes what the room shows;
  `CloseProject` stops that project's agents and takes it out of the room (nothing is
  deleted). Selecting an agent of another project focuses its project.
- Group names are unique per project. Names typed or spoken resolve in the focused
  project first, then in the others.
- The room shows a **PROJECTS** column left of the sector map: one card per project
  with its agents at work and a blinking red badge for requests waiting there.
  `Alt+1…9` or a click focuses a project, `[` `]` step through them, `:project <name>`
  and `:close` work from the command line and the palette.
- The sector map and the agent table show the focused project. The permission queue,
  the CAUTION light, the event log and the header counts stay global: nothing urgent
  hides behind another project.
- The pet sees every open project; new groups go to the focused one.
- SQLite keeps `open_projects` (their roots) and `focus_project`; the layout stays
  per project. `last_project` from older versions is read once.

## Consequences

- Agents of every open project share the broker, the voice and the token counters.
- More agents may run at once; group limits still apply per group.
- Commands that take a group or agent name may need the project in focus to pick
  between equal names.
