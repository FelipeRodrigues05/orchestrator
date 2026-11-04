# 0006. One git worktree and branch per agent

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

Agents in the same group work on the same repository at the same time. Editing the
same checkout leads to conflicts and agents overwriting each other's work. This is
the biggest risk listed in `SPEC.md`.

## Decision

- Each agent runs in its own git worktree on its own branch inside the project.
- Every runner uses the agent's worktree as its working directory.
- Writes outside the agent's worktree are denied by default.
- Merging back is an explicit step, done by the user or (post-MVP) a lead agent.

## Consequences

- Agents cannot conflict while working; conflicts surface at merge time.
- Disk usage grows with the number of agents.
- Worktree creation and cleanup are part of the agent lifecycle in the orchestrator.
- Real isolation of shell commands needs a sandbox (bubblewrap), planned post-MVP.

## Amendment (2026-11-04): folders without git

A project that is not a git repository no longer stops agents. They work in the
project folder itself, sharing it, with a warning when the project opens; `:merge`
says there is nothing to merge. `git init` (and one commit) gives each agent its
own worktree again. The app never initialises git in the operator's folder.
