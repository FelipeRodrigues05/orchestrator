# 0004. Provider-agnostic `AgentRunner`, ACP first

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

Users already have agent CLIs installed (Gemini CLI, Claude Code, Codex) and may
also want models by API key (Anthropic, OpenAI, Gemini, OpenRouter) or local
models (Ollama). Each CLI has its own output format, which changes between
versions.

## Decision

Every agent is driven through one trait:

```rust
#[async_trait]
trait AgentRunner: Send {
    async fn start(&mut self, agent: &Agent, mcp_url: &str) -> Result<()>;
    async fn send(&mut self, text: String) -> Result<()>;
    fn events(&mut self) -> mpsc::Receiver<AgentEvent>;
    async fn stop(&mut self) -> Result<()>;
}
```

Implementations:

- `AcpRunner`: CLIs that speak the Agent Client Protocol (JSON-RPC over stdio).
  This is the main path; one runner covers several CLIs and brings a standard
  permission request (`session/request_permission`).
- `McpCliRunner`: CLIs without ACP, using an app MCP permission tool
  (e.g. `--permission-prompt-tool`). Post-MVP fallback.
- `ApiRunner`: HTTP providers and Ollama, with the app's own minimal tool set
  (read, edit, search, run command).

Nothing outside a runner may `match` on the provider type or parse CLI output.

## Consequences

- A group can mix providers.
- CLI-specific parsing is isolated and versioned inside the runner.
- A `FakeRunner` with scripted events makes the broker and orchestrator testable.
- The app has to maintain its own tool set for API providers.
