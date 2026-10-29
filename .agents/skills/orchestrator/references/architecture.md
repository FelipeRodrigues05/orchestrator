# Architecture: model, runtime and runners

## Data model (core/src/model.rs)

Project → N Groups → N Agents. Each agent has a task, provider, worktree, state and rules.

```rust
struct Project { id: Uuid, name: String, root: PathBuf }

struct Group {
    id: Uuid, project_id: Uuid,
    name: String,   // "Payments API"
    goal: String,   // shared context passed to every agent in the group
}

struct Agent {
    id: Uuid, group_id: Uuid,
    name: String,   // name spoken by the TTS: "Tests"
    task: String,
    provider: Provider,
    worktree: PathBuf,
    state: AgentState,
    rules: Vec<PermissionRule>,
}

enum Provider {
    CliAcp { command: String },
    CliMcp { command: String },
    Api { service: String, model: String, key_ref: String },
    Local { url: String, model: String },
}

enum AgentState { Stopped, Running, AwaitingPermission, Done, Error }

struct PermissionRequest {
    id: Uuid, agent_id: Uuid,
    tool: String,                // "Bash", "Edit", "WebFetch"
    input: serde_json::Value,    // { "command": "cargo test" }
    spoken_summary: String,      // "Tests wants to run cargo test"
    created_at: DateTime<Utc>,
}

enum Decision { Allow, AlwaysAllow(PermissionRule), Deny { reason: String } }

struct PermissionRule { tool: String, pattern: Option<String>, effect: Effect }
enum Effect { Allow, Deny, Ask }
```

Rules:
- `PermissionRequest` lives **only in memory** while pending. Only the decision goes to the SQLite history.
- `key_ref` is a reference into the Secret Service, never the key.
- Derive `serde::{Serialize, Deserialize}` on persisted types; `Clone, Debug` where it makes sense, but `Debug` must not expose secrets.

## Runtime

```
main thread (GTK/libadwaita)  ── only draws and sends commands
        ▲ async-channel (events)     │ async-channel (commands)
        │                            ▼
tokio thread: orchestrator · broker · internal MCP server · voice/pet
        │ tokio::spawn per agent
        ▼
child processes (CLIs) or HTTP tasks (API), each in its own worktree
```

- Already implemented in step 1: `core::runtime::Runtime::start()` spawns the tokio thread and returns `commands: Sender<Command>` / `events: Receiver<Event>`; `core::orchestrator::run` is the central actor.
- One `tracing` span per agent (`agent_id`, `group`, `name`) to debug several in parallel.
- The broker is the only one that emits popup and permission-speech events.
- In `app`, receive events with `glib::spawn_future_local` reading the `async_channel::Receiver`; send commands with `try_send` on the unbounded channel.

## AgentRunner trait

```rust
#[async_trait]
trait AgentRunner: Send {
    async fn start(&mut self, agent: &Agent, mcp_url: &str) -> Result<()>;
    async fn send(&mut self, text: String) -> Result<()>;
    fn events(&mut self) -> mpsc::Receiver<AgentEvent>; // text, tool_use, cost, end
    async fn stop(&mut self) -> Result<()>;
}
```

Implementations:

| Runner | Providers | How it works |
| --- | --- | --- |
| `AcpRunner` | `CliAcp` (Gemini CLI, Claude Code/Codex via adapter) | spawns the CLI, JSON-RPC over stdio with `agent-client-protocol`; permission arrives as `session/request_permission` |
| `McpCliRunner` | `CliMcp` (e.g. Claude Code directly) | `claude -p --output-format stream-json --permission-prompt-tool mcp__orch__permission`, internal MCP server via `rmcp` |
| `ApiRunner` | `Api`, `Local` (Ollama/llama.cpp) | `reqwest` + own tool loop; builds the `PermissionRequest` before running each sensitive tool |

- All of them run with `cwd` = the agent's worktree.
- `ApiRunner` uses the app's minimal tool set (read, edit, search, run command), shared by every API provider.
- Priority: ACP first (one runner covers several CLIs). MCP is a post-MVP fallback.
- Version the CLI adapters: output formats change between versions; keep parsing inside the runner.
- For tests, write a `FakeRunner` that emits scripted events and permission requests.

## Discovery

- CLIs: look for known binaries on `PATH` (`gemini`, `claude`, `codex`, ACP adapters).
- Local model: probe `localhost:11434` (Ollama).
- Editors: `gio::AppInfo` (`.desktop` files) + `PATH` (`code`, `zed`/`zeditor`, `cursor`, `subl`, `nvim`, JetBrains). The `gio` part lives in `app`; the list of known binaries can live in `core`.
