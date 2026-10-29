# 0015. ASCII pet per user, host with fixed phrases first and an LLM after

- **Status:** Accepted
- **Date:** 2026-10-28
- **Amends:** [0012](0012-pet-as-host-agent.md) (drawing), [0011](0011-local-voice-pipeline.md) ("LLM-based routing comes later")

## Context

ADR 0012 drew the pet as a sprite sheet and made it a host agent with app tools.
The control room (ADR 0014) is an ASCII grid, so a sprite would be the only bitmap
on screen. ADR 0011 left LLM routing for later, but step 7 of the roadmap asks for
the host pet now. A local model on CPU takes minutes per answer, too slow for
"permitir" or "negar".

## Decision

- The pet is ASCII, drawn in the sector map. Its look (ears, frame, eyes, mouth,
  name tag) is picked from the operator's user name with a stable hash, so each
  person gets their own pet. States: idle, watch, listen, think, speak, alert,
  offline. It floats a row up and down with a shadow.
- Spoken and typed text first goes through the fixed-phrase parser
  (`voice::commands`): answers, "resumo" and "grupo X, agente Y: …" are handled at
  once, with no model.
- Anything else said by voice, typed as `@pet …`, or typed with no agent selected
  goes to the host actor (`core::host`): an LLM with app tools (list, message,
  create group/agent, start/stop, answer permission, switch provider). Its actions
  return to the orchestrator as messages; permission answers go through the broker.
  Its reply is logged and spoken.
- The host model is configurable (`:pet`). Without a choice it uses a fast API
  model when a key exists (Anthropic Haiku), otherwise a local model.

## Consequences

- The fixed phrases keep working with no model and no network.
- Without a fast model the pet answers slowly; the UI shows it thinking.
- The host sees a text snapshot of the room per request and keeps a short history;
  it cannot act on agents it does not see in that snapshot.
