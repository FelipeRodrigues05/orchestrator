---
name: orchestrator
description: Implementation guide for the agent Orchestrator (Rust + GTK4/libadwaita) following SPEC.md and docs/adr. Use whenever creating, changing or reviewing code in this repository — core/app crates, AgentRunner (ACP, MCP, API), permission broker, voice (whisper-rs, Piper), pet, ASCII control room UI, SQLite, worktrees — or when the user asks for "next step", "implement step N", "follow the roadmap/spec", "does this match the architecture?".
---

# Agent orchestrator

`SPEC.md` at the repository root is the source of truth for the product, and `docs/adr/` records why the architecture is the way it is. This skill sums up the rules that **must not be broken** and how to work on each part. When in doubt or in conflict, read the matching section of `SPEC.md` and the relevant ADR. If the code needs to diverge from them, stop and ask the user first (use `AskUserQuestion`). If they approve, update the code, add a new ADR in `docs/adr/` (superseding the old one if needed) and update the **Decisões** section of `SPEC.md` together.

## Invariants (check on every change)

1. **GTK-free core.** The `core` crate does not depend on `gtk4`, `libadwaita`, `glib`/`gio` or anything UI. All logic (projects, groups, agents, broker, rules, runners, voice as an actor) lives in `core` and is testable with `cargo test -p core`, no window. The `app` crate only draws state and sends commands. (ADR 0002)
2. **Everything talks over channels.** Agents, broker, voice and UI are actors (`tokio::spawn` + `mpsc`/`oneshot`). No `Mutex`-shared state between actors when a message will do. The UI ↔ runtime bridge is `async-channel`. (ADR 0003)
3. **Separate threads.** GTK only on the main thread; the tokio runtime on its own thread. Never call `block_on` or do I/O on the GTK thread. CPU-heavy work (whisper) goes in `spawn_blocking`. (ADR 0003)
4. **No sensitive action without the broker.** Every permission request, from any provider, becomes a `PermissionRequest` with a `oneshot::Sender<Decision>` and goes through the broker. Only the broker opens the popup and tells the TTS to speak. (ADR 0005)
5. **The provider is a detail.** The rest of the app only knows the `AgentRunner` trait and `AgentEvent`. Nothing outside a runner may `match` on the CLI/API type. (ADR 0004)
6. **One worktree per agent.** Each agent runs in its own git worktree/branch. Writes outside the worktree are denied by default. (ADR 0006)
7. **Secrets.** API keys only in the Secret Service (`oo7`/`keyring`). SQLite stores only `key_ref`. Never log a key or an `input` that holds a secret (use `tracing` with redacted fields). (ADR 0007)
8. **No answer = deny.** A pending request expires (configurable, e.g. 10 min) into `Deny { reason: "no response" }`. (ADR 0005)
9. **English code.** Identifiers, modules, comments, logs and UI text are in English. The Portuguese names in `SPEC.md` map to code through the glossary in its **Decisões** section (e.g. `PedidoPermissao` → `PermissionRequest`). Only spoken TTS phrases and voice command phrases stay in Portuguese. (ADR 0013)

## Expected layout

```
Cargo.toml            # workspace: members = ["core", "app"]
core/                 # package `core`, lib `orch_core`
  src/
    model.rs          # Project, Group, Agent, PermissionRequest, Decision, PermissionRule
    runtime.rs        # tokio on its own thread + async-channel bridge
    orchestrator.rs   # central actor: Command in, Event out; agent lifecycle, worktrees
    broker.rs         # permission actor: rules, queue, timeout
    runner/{mod,acp,api,mcp_cli,llm,tools,update,fake}.rs  # AgentRunner trait, ACP, API tool
                                        # loop, Claude Code via MCP, LLM wire, tools, CLI updates
    mcp.rs            # minimal MCP permission server, one token per agent (ADR 0016)
    sandbox.rs        # bubblewrap wrapper: system read-only, worktree writable
                                        # (mcp_cli.rs arrives in step 9)
    voice/{mod,stt,tts,commands,shortcut,install,wake}.rs  # actor, cpal/whisper, Piper+rodio,
                                        # parser, portal, model download, wake word
    host.rs           # the pet as host agent: LLM + app tools (ADR 0015)
    persistence.rs    # SQLite (rusqlite) on its own thread; ORCH_DB overrides the path
    discovery.rs      # CLIs on PATH, editors, terminal, Ollama on localhost:11434
    secrets.rs        # Secret Service (oo7); key_ref = "orchestrator/<service>"
    worktree.rs       # git worktree per agent under <root>/.worktrees
    sample_data.rs    # fixture for UI tests
app/src/
  main.rs             # starts the runtime, adw::Application
  ui/{mod,grid,room,render,pet,palette,popup,float}.rs
docs/adr/             # architecture decision records
```

## How to work

1. Identify the roadmap step the change belongs to — see [references/roadmap.md](references/roadmap.md). Do not implement features from later steps unless the user asks; leave a `// TODO(etapa N)` if you need an extension point.
2. Read the reference for the area:
   - Data model, runtime and `AgentRunner`: [references/architecture.md](references/architecture.md)
   - Broker and permission flow (ACP, MCP, API): [references/permissions.md](references/permissions.md)
   - Voice, pet, control room and "Open in": [references/voice-and-ui.md](references/voice-and-ui.md)
   - The ADRs in `docs/adr/` for the reasoning behind each invariant.
3. Write the logic in `core` first, with tests (broker and rules with fake runners; voice command parser with phrase tables). Then wire the UI in `app`.
4. Verify before saying you are done:
   - `cargo build --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test -p core`
   - `cargo tree -p core | grep -E 'gtk|adw|glib'` returns nothing
   - If you touched the UI, run the app (`cargo run -p app`) and check the step's behaviour.
5. At the end, say which "done" criteria of the step were met and what is still missing.

## Risks to keep in mind

Check the **Riscos** table in `SPEC.md` when designing anything new. The most common ones in code: freezing the UI by mixing GLib and tokio; an agent without a permission timeout; an "always allow" rule that is too broad (prefer patterns like `Bash(cargo test:*)`); coupling the app to one CLI's output format.
