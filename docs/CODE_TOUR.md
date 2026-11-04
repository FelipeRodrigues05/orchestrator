# Code tour

How the app starts, which code runs next, and what every file does. Read it top to
bottom the first time; after that, use the file map at the end to find things.

The workspace has two crates:

- **`core`** (library `orch_core`): all the logic. No GTK; it can be tested
  without a window.
- **`app`** (binary `orchestrator`): the GTK shell. It draws the state that `core`
  publishes and sends `core` the operator's commands.

## 1. Start-up, in order

Everything starts in `app/src/main.rs`, `fn main()`:

| Step | Code | What happens |
| --- | --- | --- |
| 1 | `enter_own_scope()` | Re-runs the binary once inside a systemd scope named after the app, so the desktop portal knows the app's id (needed for the global push-to-talk shortcut). |
| 2 | `init_logging()` | `tracing` to the terminal, or to `~/.local/state/orchestrator/orchestrator.log` when started from the menu. |
| 3 | `extend_path()` | Adds `~/.local/bin`, `~/.cargo/bin` and similar to `PATH`, because app launchers do not read shell profiles. |
| 4 | `Runtime::start()` | **The whole backend starts here.** `core/src/runtime.rs` builds a tokio runtime on its own thread (`orch-runtime`) and runs `orchestrator::run_with` on it. It returns two channels: `commands` (UI → core) and `events` (core → UI). |
| 5 | `adw::Application` + `quit_on_signals` | The GTK application. `kill`, Ctrl+C and a closed terminal quit cleanly. |
| 6 | `connect_startup` | Registers the app icon. |
| 7 | `connect_activate` → `ui::build_window` | Builds the window (see §3). A second launch only brings the existing window back. |
| 8 | `app.run()` | The GTK main loop, until quit. |
| 9 | `runtime.shutdown()` | Sends `Command::Shutdown`; the orchestrator stops every agent and the runtime thread ends. |

From step 4 on there are two worlds that only talk through the two channels:

```
GTK main thread (app)                         tokio thread (core)
───────────────────────                       ─────────────────────────────
ui::build_window                              orchestrator::run_with
  Room (state of the screen)   ── Command ──▶   Orchestrator (state of the app)
  render / tray / popups       ◀── Event ────     ├─ broker      (permissions)
                                                  ├─ voice actor (STT / TTS)
                                                  ├─ host        (the pet's LLM)
                                                  ├─ MCP server  (CLI tools)
                                                  └─ one actor per running agent
                                                         └─ runner (ACP / Claude / API)
```

GTK never waits on core, and core never touches GTK (ADR 0002, ADR 0003).

## 2. The backend: `orchestrator::run_with`

`core/src/orchestrator/mod.rs`, `run_with`:

1. **Spawns the actors.** Each is a task with a channel; `forward()` turns what
   each one sends into an `Internal` message for the orchestrator's inbox:
   - the **voice actor** (`voice::spawn`, `voice/actor.rs`) and the global shortcut
     (`voice/shortcut.rs`);
   - the **broker** (`broker::spawn`), which decides permissions;
   - the **host** (`host::spawn`), the pet's LLM;
   - the **team** handle (`team.rs`), for agents that ask each other;
   - the **MCP server** (`mcp::spawn`), a small HTTP server on localhost.
2. **Opens SQLite** (`persistence.rs`) on its own thread.
3. **Builds the `Orchestrator`** struct: it owns the `AppState` (projects, groups,
   agents, permission queue, log, voice and pet status).
4. **Restores**: settings (`restore_settings`), the pet's model
   (`apply_host_provider`), editors and agent CLIs found on the machine
   (`discovery.rs`), and the projects that were open (`reopen_projects`).
5. **Looks for git repositories** in the background (`find_repositories`) for the
   open-project list.
6. **Serves** (`serve`): a `select!` loop over three things:
   - a `Command` from the UI → `command()` (`orchestrator/commands.rs`);
   - an `Internal` message from an actor → `internal()` (`orchestrator/events.rs`);
   - a 100 ms tick → `publish()` sends `Event::State(AppState)` to the UI when
     something changed.

The orchestrator is split by subject, all `impl Orchestrator`:

| File | Subject |
| --- | --- |
| `commands.rs` | `command()`: one line per `Command`, plus agent commands (stop, remove, merge, lead…) |
| `settings.rs` | settings: API keys, pet model, voice, language, wake word, sandbox, timeout, email, fallback order, CLI updates |
| `projects.rs` | opening, focusing and closing projects; groups; creating agents; the personal workspace |
| `agents.rs` | starting agents, group limits, provider and model switches, the fallback on usage limits, runner events, and `agent_actor` (one task per running agent) |
| `team.rs` | the briefing an agent starts with, the roster, `ask_agent` delivery and turn bookkeeping |
| `operator.rs` | what the operator types or says (`say`), the pet's actions, the spoken summary, opening editor/terminal/email |
| `events.rs` | `Internal` messages and the broker's events |
| `tests.rs` | end-to-end tests with the fake runner |

## 3. The window: `ui::build_window`

`app/src/ui/mod.rs`, `build_window`:

1. Creates the **`Room`** (`ui/room/mod.rs`): the screen's state (view, selection,
   prompt, palette, scroll positions). It has no GTK in it.
2. `new_screen`: a `gtk::DrawingArea` whose draw function calls
   `Room::compose(cols, rows)` → a `Grid` of characters, which `render.rs` paints
   with Cairo/Pango.
3. `flush`: sends the commands the room queued (`Room::drain`) to core, and opens
   the folder chooser when the room asked for it.
4. Input: `key_controller` (keys → `map_key` → `Room::key`), `click_controller`
   (`Room::click`), `wheel_controllers` (`Room::scroll`).
5. `start_ticking`: every 250 ms `Room::tick` (animations, pings, blinking).
6. `tray::install`: the tray icon.
7. `bridge_events`: receives `Event`s from core. `Event::State` goes to the system
   notifications (`popup.rs`), the tray, and `Room::set_state`; then the screen is
   redrawn.
8. Window content, close behaviour (hide with a tray, quit without), and the
   floating pet (`float.rs`).

## 4. Following one message

You type "rode os testes" and press Enter with agent Tests selected:

1. GTK key → `map_key` → `Room::key(Key::Enter)` (`room/input.rs`).
2. `prompt_key` takes the `Editor` text → `Room::submit` → queues
   `Command::Say { text, selected }`.
3. `flush` sends it; core's `serve` loop → `command()` → `say()` (`operator.rs`).
4. `say` parses it (`voice/commands.rs`): not an answer, not "grupo X, agente Y" →
   `message(Tests, text)` (`agents.rs`).
5. Tests is not running → `start` → `briefing` (who it is, group goal, task,
   teammates) + your text → `launch`: picks the runner for its provider
   (`runner::for_provider`) and spawns `agent_actor`.
6. `agent_actor` creates the worktree (`worktree.rs`), starts the runner (for
   opencode: `runner/acp.rs`, the CLI inside bubblewrap via `sandbox.rs`) and sends
   it the prompt.
7. The runner turns the CLI's output into `AgentEvent`s (text, tools, usage,
   turn end) → `Internal::Agent` → `agent_event` updates the agent and the log.
8. The next tick publishes `Event::State`; `Room::set_state` stores it; the console
   panel (`room/panels.rs`) shows the new lines.

## 5. Following one permission request

The agent wants to run `cargo test`:

1. The CLI asks: ACP `session/request_permission` (`runner/acp.rs`), or Claude
   Code calls the MCP `permission` tool (`mcp.rs`), or the API tool loop asks
   before running a tool (`runner/api.rs`). All call `runner::ask_permission`.
2. `ask_permission` sends a `PermissionRequest` to the **broker** (`broker.rs`) and
   waits.
3. The broker checks the agent's rules, the group's rules, then safe defaults.
   If none decides, the request is queued: `BrokerEvent::Queued` → the
   orchestrator adds it to `AppState.permissions`, the voice announces it.
4. The UI shows the modal (`room/modal.rs`), a system notification if the window
   is not focused (`popup.rs`), the red CAUTION light and the tray's attention icon.
5. You press `a` → `Command::Answer` → `broker.answer` → the waiting runner gets
   the decision and the agent goes on. "Always" answers become rules saved in
   SQLite.

## 6. File map

### `app/src` — the GTK shell

| File | What it does |
| --- | --- |
| `main.rs` | Entry point (§1): scope, logging, PATH, runtime, GTK application, signals |
| `ui/mod.rs` | `build_window` (§3), key mapping, `.desktop` editors, styles |
| `ui/grid.rs` | The character grid and box-drawing line junctions |
| `ui/render.rs` | Paints a grid with Cairo/Pango; box lines are stroked, not glyphs |
| `ui/pet.rs` | The ASCII pet, different for each user, and its moods |
| `ui/float.rs` | The pet walking on the desktop (layer-shell) |
| `ui/popup.rs` | System notifications with Allow/Deny when unfocused |
| `ui/tray.rs` | Tray icon: show/hide, wake word, quit, attention state |
| `ui/room/mod.rs` | `Room`: screen state, keys enum, styles, state updates, selection |
| `ui/room/input.rs` | Keys, clicks, the message editor's keys, palette outcomes |
| `ui/room/commands.rs` | Every `:command` |
| `ui/room/editor.rs` | The message editor: cursor, wrapping, movement |
| `ui/room/markdown.rs` | Agents' Markdown as styled rows (titles, lists, code boxes, tables), for the console and the reader view (F6) |
| `ui/room/compose.rs` | Frame, header, tabs, and where each panel goes (`Layout`) |
| `ui/room/sector.rs` | PROJECTS column and the sector map (groups, agents, links, pet) |
| `ui/room/panels.rs` | Permission queue, agent table, console, voice strip, command line, log, scrolling |
| `ui/room/modal.rs` | The permission modal |
| `ui/room/pages.rs` | Help, getting-started guide, setup view (F5) |
| `ui/room/format.rs` | Clock, tokens, uptime, sparklines, lamps, agents' colours |
| `ui/palette/mod.rs` | The command palette: modes, fuzzy filter, Enter |
| `ui/palette/items.rs` | What each palette mode lists |
| `ui/palette/actions.rs` | What picking an item does |
| `ui/palette/draw.rs` | Drawing the palette box |
| `ui/palette/personas.rs` | Ready-made personas (marketing, backend…) |

### `core/src` — the logic

| File | What it does |
| --- | --- |
| `lib.rs` | Module list |
| `runtime.rs` | Tokio on its own thread, the two channels to the UI |
| `model.rs` | Domain types: `Project`, `Group`, `Agent`, `Provider`, `PermissionRequest`, rules, `AppState` |
| `orchestrator/` | The central actor (§2) |
| `broker.rs` | Permission decisions, queue, timeout, risky-command guard |
| `router.rs` | Model router: usage-limit detection, model tiers, closest fallback |
| `team.rs` | `list_agents` / `ask_agent` tool definitions and handle |
| `host.rs` | The pet as an LLM with app tools (create agents, answer permissions, open editor…) |
| `mcp.rs` | Minimal MCP server: permission tool for Claude Code, team tools for CLIs |
| `runner/mod.rs` | `AgentRunner` trait, `AgentEvent`, `ask_permission` |
| `runner/acp.rs` | Agents over ACP (opencode, Gemini, Codex adapter…); models via session config |
| `runner/mcp_cli.rs` | Claude Code in print mode, stream-json in and out |
| `runner/api.rs` | API and local models with the app's tool loop |
| `runner/llm.rs` | HTTP calls: Anthropic and OpenAI-compatible |
| `runner/tools.rs` | The API agents' tools: read, edit, search, run |
| `runner/update.rs` | Updating outdated CLIs |
| `runner/fake.rs` | Scripted runner for tests |
| `persistence.rs` | SQLite on its own thread: projects, groups, agents, rules, layout, history, settings |
| `worktree.rs` | One git worktree per agent; merge; repository set-up |
| `sandbox.rs` | Bubblewrap wrapper |
| `discovery.rs` | Agent CLIs, Ollama, editors, terminal, repositories, project files, opening URLs |
| `secrets.rs` | API keys in the Secret Service |
| `sample_data.rs` | A sample room for UI tests |
| `voice/mod.rs` | `VoiceHandle`, model discovery, background transcription |
| `voice/actor.rs` | The voice actor: speech, push-to-talk, wake word |
| `voice/stt.rs` | Microphone capture and whisper transcription |
| `voice/tts.rs` | Piper speech with a priority queue |
| `voice/wake.rs` | Voice activity detection and matching the pet's name |
| `voice/commands.rs` | Parser for fixed phrases ("permitir", "grupo X, agente Y: …") |
| `voice/phrases.rs` | What the app says, in Portuguese or English |
| `voice/install.rs` | Downloading whisper models and Piper voices |
| `voice/shortcut.rs` | Global push-to-talk through the desktop portal |

## 7. Where to read next

- `SPEC.md`: what the app is meant to do and the decisions taken.
- `docs/adr/`: one file per architecture decision, with its reasons.
- `docs/HANDOFF.md`: current state, how to run and verify.
- `docs/REFACTORING.md`: what is left to clean up.
