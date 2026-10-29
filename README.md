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
  (Ollama). You can switch an agent's provider in the middle of a task.
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

## First steps

1. `:open ~/code/my-repo` opens a git repository. It needs at least one commit.
2. Press `n` to create an agent: group › name › provider › task › persona.
3. Select it with `j`/`k` and press Enter to talk to it, or hold Ctrl+Space.
4. When it asks for permission: `a` allows, `l` always allows, `g` always allows
   for the group, `d` denies.
5. `F5` shows what this machine offers: providers, keys, editors, sandbox, pet
   and voice. Run `:voice install yes` to download the voice models.

To get push-to-talk outside the window on Hyprland, add:

```
bind = CTRL, SPACE, global, dev.feliperodrigues.Orchestrator:push-to-talk
```

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

Read [SPEC.md](SPEC.md) and the [ADRs](docs/adr/README.md) before changing the
architecture. [docs/HANDOFF.md](docs/HANDOFF.md) has the current state and
[docs/REFACTORING.md](docs/REFACTORING.md) what is left to clean up. Code,
comments and docs are in English.
