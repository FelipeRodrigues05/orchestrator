# Architecture Decision Records

Each ADR records one decision: the context, what was decided and what it costs.
`SPEC.md` is still the source of truth for the product; ADRs explain *why* the
architecture looks the way it does.

## Process

- Copy [`template.md`](template.md) to `NNNN-short-title.md` with the next number.
- Statuses: `Proposed`, `Accepted`, `Superseded by NNNN`, `Deprecated`.
- Never rewrite an accepted ADR's decision. Write a new ADR that supersedes it and
  update the old one's status.
- If a decision diverges from `SPEC.md`, also update the spec's **Decisões** section.

## Index

| # | Title | Status |
| --- | --- | --- |
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-gtk-free-core-crate.md) | GTK-free `core` crate in a two-crate workspace | Accepted |
| [0003](0003-actors-and-channels.md) | Actors and channels, tokio on its own thread | Accepted |
| [0004](0004-provider-agnostic-agent-runner.md) | Provider-agnostic `AgentRunner`, ACP first | Accepted |
| [0005](0005-central-permission-broker.md) | Every sensitive action goes through a central permission broker | Accepted |
| [0006](0006-one-worktree-per-agent.md) | One git worktree and branch per agent | Accepted |
| [0007](0007-secrets-in-secret-service.md) | API keys live only in the Secret Service | Accepted |
| [0008](0008-linux-wayland-only.md) | Linux + Wayland only | Accepted |
| [0009](0009-gtk4-libadwaita-libpanel.md) | GTK4 + libadwaita UI with libpanel docking | Superseded by 0014 (docking) |
| [0010](0010-sqlite-persistence.md) | SQLite for persistence | Accepted |
| [0011](0011-local-voice-pipeline.md) | Local voice: push-to-talk, whisper-rs, Piper | Accepted |
| [0012](0012-pet-as-host-agent.md) | The pet is a host agent, floating only where layer-shell exists | Accepted (drawing amended by 0015) |
| [0013](0013-english-codebase.md) | English for all code | Accepted |
| [0014](0014-ascii-control-room-ui.md) | ASCII control room instead of libpanel docking | Accepted |
| [0015](0015-ascii-pet-and-llm-host.md) | ASCII pet per user, host with fixed phrases first and an LLM after | Accepted |
| [0016](0016-minimal-mcp-permission-server.md) | A minimal MCP permission server instead of rmcp | Accepted (extended by 0018) |
| [0017](0017-several-open-projects.md) | Several projects open at once | Accepted |
| [0018](0018-personas-and-team-tools.md) | Agent personas and agents that call each other | Accepted |
| [0019](0019-desktop-install-scope-and-tray.md) | Desktop install, own systemd scope and a tray icon | Accepted |
| [0020](0020-model-router-on-usage-limits.md) | Model router: fall back to the closest model on usage limits | Accepted |
| [0021](0021-colour-themes.md) | Colour themes | Accepted |
| [0022](0022-start-up-animation.md) | Start-up animation | Accepted |
| [0023](0023-agent-library-and-memory.md) | Agent library (global agents) and memory | Accepted |
| [0024](0024-groups-spanning-projects.md) | Groups that span several projects | Accepted |
| [0025](0025-codex-app-server.md) | Codex through its own app-server protocol | Accepted |
