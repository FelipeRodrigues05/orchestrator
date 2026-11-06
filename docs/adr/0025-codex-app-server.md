# 0025. Codex through its own app-server protocol

- **Status:** Accepted
- **Date:** 2026-11-06

## Context

Codex ran through `@zed-industries/codex-acp`, an ACP adapter that bundles its own
copy of Codex. The adapter lags behind the CLI: at 0.16.0 (the latest on npm) it
cannot run `gpt-5.6-sol`, which the installed Codex 0.167 runs fine ("requires a
newer version of Codex"). Updating the adapter does not help, and the router could
only move the agent to another CLI. Codex has no ACP mode of its own, but
`codex app-server` speaks a JSON-RPC protocol over stdio: threads, turns, a model
list, and approval requests sent to the client.

## Decision

- New provider kind `CliCodex { command }`, served by `runner/codex.rs`, which runs
  `<command> app-server` with the installed Codex. Discovery offers it as `codex`
  whenever `codex` is on the `PATH`, ahead of any ACP adapter.
- Agents and library profiles saved with the `codex-acp` adapter move to it when
  they load.
- Session: `initialize` → `thread/start` in the agent's worktree → `turn/start`
  per message. Messages that arrive during a turn wait and go together as the next
  turn. `model/list` (hidden models left out) fills the model picker, and a chosen
  model is sent with the next turn.
- Permissions (ADR 0005): `approvalPolicy: untrusted` with Codex's sandbox
  `read-only`, so every command, write and edit becomes an approval request.
  - Commands whose parsed actions are only reads, listings or searches reach the
    broker as `read`, so they are allowed on their own like other agents' reads.
  - Everything else reaches the broker as `bash` or `edit` with its paths.
  - Other kinds of request (extra permissions, user input) are declined.
- With `:sandbox on`, the app's bubblewrap wraps `codex app-server` like any other
  CLI (Codex works nested), so what the operator allows can still write only the
  agent's folders.
- The team tools reach Codex as the MCP server `orch` in the thread's `config`.
- Codex's `codexErrorInfo` is put into words the model router knows: "usage limit"
  and "unknown model".
- Token usage and the context window come from `thread/tokenUsage/updated`.

## Consequences

- New Codex models work as soon as `codex update` brings them.
- The app-server protocol is marked experimental by OpenAI. The runner reads its
  messages as loose JSON, so new fields do not break it. A renamed method would
  need a change here. `cargo test -p core codex_smoke -- --ignored` checks it
  against the real CLI.
- No Codex cost in dollars: the protocol reports tokens, not prices.
