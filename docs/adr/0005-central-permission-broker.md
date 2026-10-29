# 0005. Every sensitive action goes through a central permission broker

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

Permission requests come from different places (ACP messages, an MCP tool, the
API runner's tool loop) and can be answered by different means (popup, system
notification, voice). If each path handled permissions itself, rules would drift
and some path would end up skipping the user.

## Decision

- Every permission request, from any provider, becomes a `PermissionRequest` with a
  `oneshot::Sender<Decision>` and is sent to the broker actor. The agent waits on
  the receiver.
- The broker checks agent and group rules first; a matching rule answers
  immediately.
- Otherwise the request is queued and turned into events: one dialog at a time
  with a counter, a `gio::Notification` when unfocused, and a spoken announcement
  that jumps the speech queue.
- Only the broker opens the popup and tells the TTS to speak. Answers given by the
  pet also go through the broker.
- No answer within a configurable timeout (e.g. 10 min) becomes
  `Deny { reason: "no response" }`.
- "Always allow" stores narrow patterns (e.g. `Bash(cargo test:*)`), not blanket
  tool rules.

## Consequences

- One place to test permission behaviour, with fake runners.
- Agents never block forever.
- The broker is a single point of failure: if it dies, every agent is stuck, so it
  must be supervised.
