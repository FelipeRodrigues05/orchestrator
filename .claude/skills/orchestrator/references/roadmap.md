# MVP roadmap and done criteria

Each step ends with something visible working. Implement in order; do not pull work from later steps unless the user asks.

| # | Step | Deliverables | Done when |
| --- | --- | --- | --- |
| 1 | Skeleton | `core` + `app` workspace; libadwaita window with the ASCII control room (sector map, queue, log, console) on sample data; tokio on its own thread | An event sent from the tokio runtime shows up in the UI; `core` builds without GTK |
| 2 | One agent over ACP | CLI discovery on `PATH`; `AcpRunner`; output in the panel; text box sends messages; "Open in" button with detected editors | Real conversation with an ACP CLI from the panel; "Open in" opens the editor in the right directory |
| 3 | Permission popup | `session/request_permission` → broker → permission modal drawn in the room (ADR 0014) + `gio::Notification` with buttons | Agent stops waiting, a click decides, agent continues; broker tests pass |
| 4 | Pet with voice output | Pet in the center with animated states; permission announcement with Piper | A permission request puts the pet on alert and it says "agent, group, what it wants" |
| 5 | Real groups | Several agents per group; worktree/branch per agent; layout and data in SQLite | Closing and reopening the app restores project, groups, agents and layout; two agents never share a worktree |
| 6 | API provider | Providers screen; key in the Secret Service; `ApiRunner` with minimal tools; local Ollama | An API/Ollama agent edits a file in its worktree asking permission through the same broker; the key appears neither in SQLite nor in logs |
| 7 | Pet that listens | Push-to-talk + `whisper-rs`; global shortcut via portal; host pet with app tools | Saying "permitir" answers the open request; "grupo X, agente Y: …" reaches the right agent |
| 8 | Fewer interruptions | "Always allow" with patterns; request queue with counter; timeout | A repeated request covered by a rule opens no popup; an ignored request becomes a "no response" denial |
| 9 | Post-MVP | Floating pet (layer-shell), MCP fallback (`McpCliRunner`), lead agent, wake word, sandbox (bubblewrap) | — |
| 10 | Beyond the roadmap | Several open projects with a PROJECTS column (ADR 0017); agent personas and the `list_agents` / `ask_agent` team tools (ADR 0018); desktop install, own systemd scope, tray icon (ADR 0019) | Two projects run agents side by side; an agent's question reaches its teammate and the answer comes back (fake runner test + real opencode over MCP); the app starts from the menu and lives in the tray |

When a step is finished, tell the user which criteria were verified and how (test, manual run).
