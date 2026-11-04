# Orchestrator

A desktop control room for coding agents. Open your projects, put agents to work
in groups, each in its own git worktree, and supervise them from one ASCII screen.
Every sensitive action an agent takes waits for your answer, by click or by voice.

```
╔═ ORCH//SHOP ══════════════════════════ 15:18:42 ═ RT ● 3ms ═ BOX ● ═ TOK 12.4k ═╗
║ [F1 OVERVIEW] [F2 AGENTS] [F3 PERMS ◆1] [F4 LOG] [F5 SETUP]       ▲ CAUTION ▲  ║
╠═ PROJECTS (2) ═╦═ SECTOR MAP ════════════════════╦═ PERMISSION QUEUE (1) ══════╣
║ 1 shop      ◆1 ║   ┌─ API ──────────┐            ║ ◆ Tests · API               ║
║   2● of 3      ║   │● Backend  07:14│─┐   _||_   ║   Bash: cargo test          ║
║ 2 storefront   ║   │◆ Tests    03:41│═╬═[ @  @ ] ║   [A]llow A[l]ways [D]eny   ║
║   1● of 1      ║   └────────────────┘ │ [ KAI  ] ║                             ║
```

Linux and Wayland only. Built with Rust, GTK4 and libadwaita.

## What it does

- **Agents from any provider.** CLIs that speak ACP (opencode, Gemini CLI, Codex
  and Claude Code through adapters), Claude Code directly through an MCP permission
  tool, API models (Anthropic, OpenAI, Gemini, OpenRouter) and local models
  (Ollama). Switch an agent's provider or model in the middle of a task; when a
  model hits its usage limit the agent goes on with the closest one available.
- **One worktree per agent.** Agents never edit the same checkout; `:merge` brings
  an agent's branch back into the project.
- **A single permission broker.** Rules per agent and per group (`Bash(cargo test:*)`),
  safe defaults, a queue with a timeout, and a sandbox (bubblewrap) where only the
  worktree is writable.
- **Several projects at once,** each with its groups and agents.
- **Personas and teamwork.** Give an agent a persona; agents of a project know
  each other and can ask a teammate for help (`ask_agent`).
- **Voice.** Push-to-talk (Ctrl+Space) or call the pet by name; whisper.cpp
  transcribes locally, Piper reads permission requests aloud.
- **The pet.** An ASCII host, different for each user, that can answer permissions,
  route instructions, summarise, create agents and open your editor, a terminal or
  your email. It can walk on your desktop where layer-shell exists.
- **Readable output.** Agents' Markdown is rendered (titles, lists, code boxes,
  tables), each agent has its own colour, and a reader view shows one
  conversation full screen. Context use, tokens and cost are shown per agent.
- **Keyboard first.** A command palette (`:`, `Ctrl+P`, `Space`), a new-agent
  wizard (`n`), `?` for every key and command. A tray icon keeps it one click away.

## Requirements

- Rust 1.88 or newer (edition 2024)
- GTK 4.12+, libadwaita 1.6+, gtk4-layer-shell
- `git`
- Optional:
  - `bubblewrap` (`bwrap`) for the sandbox
  - `piper` (for example `piper-tts-bin` on the AUR) for spoken announcements
  - a microphone, and the whisper models (downloaded from inside the app)
  - at least one agent: an ACP CLI on your `PATH`, `claude`, an API key, or Ollama

On Arch Linux:

```sh
sudo pacman -S rust gtk4 libadwaita gtk4-layer-shell git bubblewrap
```

## Install

```sh
scripts/install.sh                         # release build, per user
scripts/install.sh --features gpu-vulkan   # whisper on the GPU (or gpu-cuda)
scripts/install.sh --uninstall
```

This puts `orchestrator` in `~/.local/bin`, the icon in your icon theme and
**Orchestrator** in your app menu.

Or run it from the checkout:

```sh
cargo run -p app
ORCH_DB=/tmp/demo.db cargo run -p app      # with a throwaway database
```

## Using the app

### The ideas

- **Project**: a git repository you open. Several can be open at once; one is in
  focus. With none open, agents go to a personal workspace of their own.
- **Group**: agents that work towards one goal ("Payments API"). The goal is
  optional context every agent of the group receives.
- **Agent**: one AI worker with a name, a task, a provider (who runs it), a model,
  an optional persona (who it is) and a colour. Each agent works in its own git
  worktree and branch, so agents never edit the same files.
- **Permission**: whenever an agent wants to run a command or change a file, it
  asks. You answer, or a rule you made answers for you.
- **The pet** (KAI, or the name it shows): your host. It hears you, reads
  requests aloud, summarises, and can create agents or open your editor for you.

### The screen

```
╔═ header: clock · runtime · sandbox · tokens and cost · agents by state ════╗
║ tabs F1-F6                                               ▲ CAUTION light ║
╠═ PROJECTS ═╦═ SECTOR MAP (groups and agents) ═╦═ PERMISSION QUEUE ═══════╣
║            ║                                   ╠═ EVENT LOG ══════════════╣
╠═ AGENT // NAME (the selected agent's console) ═ ctx 45% · 120k tok · $0.42 ╣
╠═ VOICE strip ═══════════════════════════════════════════════════════════╣
╚═ command line ═══════════════════════════════════════════════════════════╝
```

| Key | View |
| --- | --- |
| `F1` `1` | Overview: projects, sector map, permission queue, event log |
| `F2` `2` | Agents table: state, uptime, tokens, context, cost, task |
| `F3` `3` | Permission queue |
| `F4` `4` | The whole event log |
| `F5` `5` | Setup: providers, keys, editors, sandbox, pet, voice |
| `F6` `6` `r` | Read the selected agent's conversation full screen |

Red means "needs you": a pending request, an error, a full context. Everything
else stays in greys and each agent's own colour.

### 1. Open a project

- `o` (or `:open`) lists recent projects, git repositories found under `~/www`,
  `~/code`, `~/projects`… and **Browse…**, the system folder chooser.
- `:open shop` opens by folder name; `:open ~/code/shop` by path.
- The repository needs at least one commit.
- `Alt+1…9` or `[` `]` switch between open projects; `:close` closes the focused one.

### 2. Create an agent

Press `n`. The wizard asks, one step at a time:

1. **Group**: pick one, or type a new name (then an optional goal). With no project
   open, pick **Personal** for an agent of your own (marketing, writing, planning…).
2. **Name**: short and speakable; the voice calls it by this name.
3. **Provider**: an ACP CLI, `claude`, an API, or a local model. `F5` lists them.
4. **Task**: what it should do.
5. **Persona** (optional): a ready-made one (Marketing specialist, Backend engineer,
   Code reviewer…) or your own description.

One line does the same: `:new API Tests opencode: write the /pix tests | tests only`.

### 3. Talk to agents

- Select an agent with `j`/`k` (or click it) and press `Enter` to type. `Enter`
  sends, `Shift+Enter` breaks the line, `Esc` cancels. Long messages open an
  editor panel; `←→↑↓`, `Home`, `End` move in it.
- `Ctrl+V` pastes. `@` at the start of a word lists the project's files and
  inserts `@path/to/file`.
- `@pet …` talks to the pet instead. "grupo API, agente Tests: …" reaches an agent
  by name.
- Hold `Ctrl+Space` to talk, or say the pet's name ("ei KAI…") when the wake word
  is on. Fixed phrases work too: "permitir", "sempre", "negar", "resumo".
- `s` starts or stops the selected agent; `e` opens its worktree in your editor.

### 4. Answer permissions

A request shows a red modal with the agent, the tool and the command, and the pet
says it out loud.

| Key | Answer |
| --- | --- |
| `a` | Allow this once |
| `l` | Always allow (a rule for this agent) |
| `g` | Always allow for the whole group |
| `d` | Deny |
| `Esc` | Later: hide the modal, the request stays in the queue |

Rules can also be written by hand: `:allow Bash(cargo test:*)`, `:deny Bash(rm:*)`.
Unanswered requests become a denial after the timeout (`:timeout <minutes>`).
With the sandbox on (`:sandbox on`, the default when `bwrap` exists) agents can
only write inside their worktree.

### 5. Read what agents do

- The console under the map shows the selected agent; the event log shows
  everyone. What you typed reads `you›Agent`, in white; agents write in their colour.
- Scroll with the mouse wheel, `PageUp`/`PageDown` (the console) or
  `Shift+PageUp`/`Shift+PageDown` (the log). `End` jumps back to the newest line.
- `r` (F6) opens the reader: the conversation in a comfortable column, Markdown
  rendered.
- `:color green` (or `:color Kio rosa`, or "Color of …" in the palette) picks an
  agent's colour; `auto` and `none` too.

### 6. Models, limits and cost

- `:model` lists the models the agent's provider offers; `:model <id>` switches.
  ACP CLIs switch in the live session; Claude Code restarts with the conversation
  handed over.
- `:provider <label>` moves the agent to another provider, conversation included.
- When a model runs out of tokens or quota, the agent goes on with the closest
  available model (Opus → gpt-6-astra → Sonnet…). `:fallback a, b/c` fixes the
  order, `:fallback reset` frees every model again.
- The console title shows `ctx 45% · 120k tok · $0.42`. Past 85% of the context
  (`:compact <n|off>`), the agent goes on with a summary of the conversation.

### 7. Teams and personas

- Agents of a project see each other. One can ask another with the `ask_agent`
  tool: the backend agent asks the integration agent to check the front end.
- `:persona Back: backend only; asks Integration about the UI` sets who an agent is.
- `:lead API 3` asks the pet to split the group's goal into three agents.

### 8. Finish the work

- `:merge` commits what the selected agent left in its worktree and merges its
  branch into the project's current branch. On conflicts the merge is aborted and
  the files are listed.
- `:remove <agent>` forgets an agent; its worktree stays on disk.

### 9. Set up the machine

| Command | What it does |
| --- | --- |
| `:key anthropic` | Store an API key in the Secret Service (paste it with `Ctrl+V`) |
| `:key anthropic delete` | Remove it |
| `:pet <model>` | The model the pet thinks with (`anthropic/claude-haiku-5-5`, a local one…) |
| `:voice install yes` | Download the whisper models and a Piper voice |
| `:lang pt` / `:lang en` | Language of speech, the pet and announcements |
| `:voice <id>` | Another Piper voice (downloaded if missing) |
| `:wake on` / `off` | Call the pet by name instead of holding Ctrl+Space |
| `:update` | Update the agent CLIs |
| `:email <url>` | Your webmail, for the pet's "open my email" |
| `:theme <name>` | Colours: `mono`, `amber`, `green`, `nord`, `light` |

`?` shows every key and command; `:` opens the palette, which finds any action by
typing part of its name.

To get push-to-talk outside the window on Hyprland, add:

```
bind = CTRL, SPACE, global, dev.feliperodrigues.Orchestrator:push-to-talk
```

The tray icon shows or hides the window and quits the app. With a tray, closing
the window only hides it.

## Project layout

```
core/   the app's logic, no GTK: orchestrator, broker, runners, MCP server,
        voice, persistence, sandbox, worktrees
app/    the GTK shell: control room, palette, pet, tray
docs/   architecture decisions (adr/), handoff notes, refactoring notes
SPEC.md the architecture spec (Portuguese)
```

## Development

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p core acp_ -- --ignored        # against a real ACP CLI
```

Start with [docs/CODE_TOUR.md](docs/CODE_TOUR.md): how the app boots and what
each file does. Read [SPEC.md](SPEC.md) and the [ADRs](docs/adr/README.md) before
changing the architecture. [docs/HANDOFF.md](docs/HANDOFF.md) has the current state and
[docs/REFACTORING.md](docs/REFACTORING.md) what is left to clean up. Code,
comments and docs are in English.
