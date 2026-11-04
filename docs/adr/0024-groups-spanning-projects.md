# 0024. Groups that span several projects

- **Status:** Accepted
- **Date:** 2026-11-04

## Context

Work often crosses repositories: an API and the site that calls it, a library and
the app that uses it. A group belonged to one project, so its agents could only
see that project's folder. The operator wants one group whose agents see every
project involved.

## Decision

- A group keeps its home project and gains `attached`: other project folders it
  spans (a JSON list in the `groups.attached` column). `:attach <group>: <folder or
  recent project name>` adds one; `:detach <group>: <name>` removes it. The sector
  map shows the count next to the group's name (`API +1`).
- Each agent of the group works in every attached project the same way as in its
  home project: its own worktree and branch (`<root>/.worktrees/<name>-<id>`) in a
  git repository, the folder itself otherwise. The worktrees are made when the
  agent starts; running agents see a new attachment after a restart.
- The home worktree stays the working directory. The extra folders reach each
  runner as it can take them:
  - ACP CLIs: `additionalDirectories` on the new session, when the agent
    advertises `sessionCapabilities.additionalDirectories`;
  - Claude Code: `--add-dir` for each folder;
  - API agents: the file tools accept absolute paths inside them;
  - the sandbox binds them (and their repositories' `.git`) writable;
  - the broker treats them as inside the agent's worktree.
- The briefing lists each attached project with the agent's folder there, so CLIs
  without extra roots still know where to work.
- `:merge` merges the home worktree, then every attached worktree into its own
  project, and reports one line per project.

## Consequences

- An agent of a spanning group holds a branch in every attached repository.
  `:detach` leaves those worktrees on disk, like `:remove`.
- Teammates are still those of the home project; agents in an attached project's
  own groups are not on the roster.
- Without git in an attached folder, the group's agents share it, as in ADR 0006's
  no-git case.
