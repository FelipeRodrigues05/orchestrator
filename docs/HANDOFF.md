# Handoff — state of the orchestrator (2026-10-29)

What was built, how to run and verify it, what is left. `SPEC.md` and `docs/adr/`
stay the source of truth; this file is a snapshot for whoever picks the work up.

> Nothing is committed yet: the repository has no commits. Commit before changing
> anything else.

## Run it

```sh
cargo run -p app                    # data in ~/.local/share/orchestrator/orchestrator.db
ORCH_DB=/tmp/demo.db cargo run -p app   # throwaway database
cargo run -p app --features gpu-vulkan  # whisper on the GPU (or gpu-cuda)
scripts/install.sh                  # release build → ~/.local/bin/orchestrator,
                                    # icon + "Orchestrator" in the app menu
scripts/install.sh --uninstall
```

The binary re-runs itself once inside a systemd scope `app-dev.feliperodrigues.Orchestrator-*`
(skipped when `ORCH_SCOPED` is set) so the desktop portal files the global
shortcut under the app's own id, wherever it was started from. It also appends
`~/.local/bin`, `~/.cargo/bin`, `~/.opencode/bin` and similar to `PATH` when they
are missing, because launchers do not read shell profiles.

First steps inside the app: `:open ~/path/to/git-repo` (needs at least one commit),
then `n` for the new-agent wizard, then Enter to talk to the selected agent.
`?` lists every key and command; `F5` shows providers, editors, sandbox, pet and voice.

Environment overrides: `ORCH_DB`, `ORCH_WHISPER_MODEL`, `ORCH_PIPER_VOICE`,
`ORCH_SANDBOX_BIND` (extra writable paths for the sandbox, colon separated).

## Verify it

```sh
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                      # 85 tests
cargo tree -p core | grep -E 'gtk|adw|glib' # must print nothing
# Real agents (ignored by default):
cargo test -p core acp_ -- --ignored        # opencode over ACP, inside the sandbox
cargo test -p core api_smoke -- --ignored   # Ollama qwen3:8b edits a file (~3 min)
cargo test -p core host_smoke -- --ignored  # the pet creates a group via Ollama (~2.5 min)
cargo test -p core claude_mcp_smoke -- --ignored  # Claude Code via MCP: Write goes through the broker
cargo test -p core wake_hears_a_real_voice -- --ignored  # Piper says "Ei Kai, resumo"; VAD + whisper hear the name
cargo test -p core acp_team_tools -- --ignored  # opencode calls ask_agent over the app's MCP server
```

All of the above passed at the time of writing; the app starts cleanly.

## Code map

```
core/src/
  model.rs          domain types; PermissionRule patterns (Bash(cargo test:*)); Usage
  orchestrator.rs   central actor: commands in, state snapshots out; agent actors,
                    routing (text/voice/pet), auto-update, provider switch, group limits
  broker.rs         permissions: agent rules → group rules → safe defaults → queue;
                    timeout, cancellation, risky-Bash guard, always-allow (agent/group)
  host.rs           the pet as host agent: LLM + app tools → HostAction messages
  mcp.rs            minimal MCP server (one URL token per agent): permission + team tools
  team.rs           list_agents / ask_agent: agents of a project call each other
  runner/           AgentRunner trait; acp.rs (ACP CLIs), mcp_cli.rs (Claude Code -p),
                    api.rs (API/Ollama tool loop),
                    llm.rs (Anthropic + OpenAI-compatible wire), tools.rs (read/edit/
                    search/run), update.rs (CLI self-update), fake.rs (tests)
  voice/            actor + reload; stt.rs (cpal + whisper-rs), tts.rs (Piper + rodio),
                    commands.rs (fixed phrases), shortcut.rs (portal), install.rs,
                    wake.rs (VAD + pet-name matching)
  persistence.rs    SQLite on its own thread (projects, groups, agents, rules,
                    layout, decision history, settings)
  sandbox.rs        bubblewrap wrapper: system read-only, worktree writable
  discovery.rs      ACP CLIs on PATH, Ollama, editors, terminal, "open in"
  secrets.rs        API keys in the Secret Service (oo7)
  worktree.rs       one git worktree + branch per agent under <root>/.worktrees; merge
app/src/ui/
  grid.rs           character grid + box-drawing junctions
  room.rs           control-room layout, input, commands, permission modal
  palette.rs        command palette (fuzzy) and wizards
  render.rs         Cairo/Pango painter (box glyphs are stroked)
  pet.rs            per-user ASCII pet
  popup.rs          gio::Notification when unfocused
  float.rs          floating pet on a layer-shell surface
  tray.rs           tray icon (ksni): show/hide the room, wake word, quit
  mod.rs            window, key/mouse wiring, runtime bridge, .desktop editors
```

## Done

Roadmap steps 1–8 are implemented. Evidence per area:

| Area | Status | How it was checked |
| --- | --- | --- |
| Control room UI (ADR 0014) | done | text-dump tests; screenshots in the real window |
| ACP agents (step 2) | done | real conversation with `opencode acp` in the UI and in tests |
| Broker + popup (steps 3, 8) | done | broker tests (rules, group rules, queue order, timeout, cancel, always-allow, risky Bash); modal rendered in tests |
| Pet visuals (step 4) | done | grid tests; moods follow state |
| Groups, worktrees, SQLite (step 5) | done | worktree + persistence tests; layout (view + selection) restored |
| API provider (step 6) | done | Ollama tool loop edits a file through the broker (real) |
| Voice (step 7) | code done | parser table tests; **STT/TTS never ran** (no models/Piper here) |
| Pet as host LLM (ADR 0012/0015) | done | host unit tests; real Ollama run created a group |
| Permission timeout, group always-allow, risky-Bash guard | done | broker tests |
| Token usage, group active limits | done | orchestrator test (queue frees a slot); usage shown in header/F2/F5 |
| Auto-update of agent CLIs, provider/model switch mid-task | done | update rules tests; hand-off test |
| `.desktop` editors, `:voice install`, GPU features | done | builds; download not exercised |
| Sandbox (step 9, F1) | done | real test: writes outside the worktree do not land; opencode and Claude Code run inside |
| Claude Code via MCP (F2, ADR 0016) | done | MCP server end-to-end test; real `claude -p` run where `Write` reached the broker |
| Merge and group lead (F3) | done | merge test incl. conflict abort; `:lead` reuses the pet's tools (pet verified on Ollama) |
| Floating pet (F4) | done | real run on Hyprland: layer surface walks along the bottom, readable on any wallpaper |
| Wake word (F5) | done | Piper-synthesised "Ei Kai, resumo" → VAD → whisper small → name matched (real); on by default |
| Language and voice | done | `:lang pt|en`, `:voice <id>` (downloads Piper voices); phrase tables tested |
| Pet opens things | done | tools: editor, terminal, email (`:email <url>`), http/https/mailto links only |
| Long lines | done | log and console wrap under the text column |
| Several open projects (ADR 0017) | done | orchestrator test (side by side, focus, close); room test (PROJECTS column, Alt+n) |
| Personas, agents calling each other (ADR 0018) | done | orchestrator tests (ask starts a stopped teammate, answer returns; briefing); MCP test; real opencode run calling ask_agent |
| Agents without a project, ready-made personas | done | orchestrator test (personal workspace opened, agent runs in its worktree); palette tests (preset and typed personas) |
| `@` file mentions, open project list | done | palette and room tests (mention inserts the file, `:open <name>`, Browse…, recent/found repositories) |
| Desktop install, tray (ADR 0019) | done | `scripts/install.sh`; launched via `gtk-launch` from Hyprland; tray registered with the SNI watcher, close hides, tray click shows |

## Not verified on real hardware/accounts

- Speech with the operator's own voice and microphone (Piper and models are
  installed; the chain was verified with a synthetic voice). Default mic is the
  system default source ("Raptor Lake-P/U/H cAVS Digital Microphone").
- Paid APIs (Anthropic, OpenAI, OpenRouter, Gemini) and writing a key to the Secret
  Service: only the request/response conversion is tested.
- The CLI auto-updater against a real outdated CLI.
- Wake word with a real voice, and Claude Code via MCP in the UI (only the runner
  was run for real).
- Global push-to-talk on Hyprland needs a bind: `bind = CTRL, SPACE, global,
  dev.feliperodrigues.Orchestrator:push-to-talk`.

## Remaining work

All roadmap steps (1–9) and the approved plan (phases A–F) are implemented. What is
left is verification on real hardware and accounts (above) plus these follow-ups:

- Run `:voice install yes` once more to fetch `ggml-tiny.bin` (the wake word then
  uses it instead of the slower small model).
- Microphone choice inside the app (today it follows the system default source).
- Lead agent is the pet acting on a group (`:lead`); a dedicated long-running lead
  per group is not built.
- Merge targets the branch checked out in the project root; there is no review UI
  for the diff before merging.

## Known limitations and gotchas

- `codex-acp` embeds a Codex older than the newest models (`gpt-5.6-sol` in
  `~/.codex/config.toml`). The router treats "requires a newer version" as a model
  the CLI cannot run and switches the agent to another model of the same CLI
  (ADR 0020 amendment); `:model` picks one by hand.
- opencode allows edits/shell by default; the ACP runner forces "ask" through
  `OPENCODE_CONFIG_CONTENT` so the broker sees them.
- The risky-Bash guard is a heuristic; the sandbox is the real enforcement.
- The permission modal is drawn on the canvas (not a screen-reader widget); the
  system notification and the spoken announcement are the non-visual paths.
- Models: ACP CLIs that list a `model` config option (opencode does) switch in the
  live session; Claude Code restarts with `--model` and the conversation handed
  over; CLIs that list no models get the id passed through and say if it fails.
- Group limits apply to starting agents; a follow-up message to an idle (Done)
  agent is not queued.
- Claude Code runs with `--permission-mode manual` so every action not covered by
  a rule reaches the broker.
- The wake word keeps the microphone open while on and runs whisper `tiny` on
  each utterance: it costs CPU; it is off by default.
- With a tray host, closing the room hides it; quit from the tray menu. The tray
  registers under the connection's unique name because the quickshell host kept
  items registered under `org.kde.StatusNotifierItem-<pid>-1` after exit.
- The local `qwen3:8b` takes minutes per answer on this CPU; use a fast API model
  for the pet (`:pet anthropic/claude-haiku-5-5`).

## Decisions taken in this round

- ADR 0014 (+ amendment): ASCII control room instead of libpanel; in-grid
  permission modal; command palette.
- ADR 0015: per-user ASCII pet; fixed phrases first, LLM host after.
- ADR 0016: a minimal MCP permission server instead of `rmcp`.
- ADR 0017: several projects open at once, PROJECTS column left of the sector map.
- ADR 0018: agent personas; `list_agents` / `ask_agent` between agents of a project.
- ADR 0019: desktop install, own systemd scope, tray icon.
- SPEC "Decisões" updated for permissions, provider switching, auto-update,
  limits and usage. Persona per agent was discussed and deferred by the user.
