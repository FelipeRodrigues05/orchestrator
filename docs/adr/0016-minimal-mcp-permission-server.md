# 0016. A minimal MCP permission server instead of rmcp

- **Status:** Accepted (extended by 0018: the server also serves the team tools)
- **Date:** 2026-10-29

## Context

CLIs without ACP (Claude Code in print mode) ask permission through an MCP tool
(`--permission-prompt-tool`). The SPEC names `rmcp` for this. `rmcp` 3.x exposes
the streamable HTTP server as a `tower` service, which needs an HTTP stack (axum or
hyper) on top, for a server that only ever offers one tool to localhost.

## Decision

- `core::mcp` implements the slice of MCP the permission flow needs: JSON-RPC over
  HTTP on `127.0.0.1:<random port>/mcp/<token>`, answering `initialize`, `ping`,
  notifications (202), `tools/list` and `tools/call` for one `permission` tool. No
  server-initiated stream (GET answers 405).
- Each agent gets its own token; the handler knows who asks and calls the broker
  (ADR 0005). Unknown tokens get 404.
- `McpCliRunner` starts `claude -p` with stream-json in and out,
  `--permission-mode manual`, `--permission-prompts host`,
  `--permission-prompt-tool mcp__orch__permission` and `--strict-mcp-config`, so every
  action not covered by a rule reaches the broker. It runs inside the sandbox.

## Consequences

- No extra HTTP framework; the protocol subset is covered by an end-to-end test and
  a real run with Claude Code.
- If a CLI needs more of MCP (resources, SSE), move to `rmcp` then.
