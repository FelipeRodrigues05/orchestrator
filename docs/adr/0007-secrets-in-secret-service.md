# 0007. API keys live only in the Secret Service

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

API providers need keys. Storing them in SQLite or config files, or leaking them
into logs, would expose them to anything that can read the user's home directory
or log output.

## Decision

- Keys are stored only in the desktop Secret Service (GNOME Keyring / KWallet) via
  `oo7` or `keyring`.
- SQLite stores only a `key_ref` pointing to the secret.
- Keys and tool `input` values that may contain secrets are never logged; `tracing`
  fields are redacted.

## Consequences

- Requires a running Secret Service; without one, API providers cannot be configured.
- Backups of the database do not carry credentials.
- "Key does not appear in SQLite or logs" is part of step 6's done criteria.
