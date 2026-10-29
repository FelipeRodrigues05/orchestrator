# 0010. SQLite for persistence

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

Projects, groups, agents, panel layout, permission rules and decision history must
survive restarts. The app is single-user and local.

## Decision

- Store data in a local SQLite database using `rusqlite` or `sqlx`, with `serde`
  for structured fields.
- Pending `PermissionRequest`s live only in memory; the decision taken is written
  to the history.
- No secrets in the database (see [0007](0007-secrets-in-secret-service.md)).

## Consequences

- No server to run; one file per user.
- Access goes through a persistence actor in `core`, never from the GTK thread.
- Schema migrations need to be handled from the first release that stores data
  (step 5).
