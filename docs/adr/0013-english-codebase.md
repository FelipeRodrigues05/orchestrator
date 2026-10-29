# 0013. English for all code

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

`SPEC.md` is written in Portuguese and originally used Portuguese domain names
(`Agente`, `PedidoPermissao`, `iniciar`…). The first step was implemented that way.
The author does not want Portuguese in the codebase.

## Decision

Everything in the code is in English: identifiers, module names, comments, log
messages and UI text. The Portuguese names in `SPEC.md` are descriptive only and
map to code through the glossary in the spec's **Decisões** section
(e.g. `PedidoPermissao` → `PermissionRequest`, `Agente` → `Agent`,
`AgentRunner::iniciar` → `start`).

## Consequences

- The spec and the code use different names; the glossary must be kept up to date
  when new domain terms appear.
- Content addressed to the user's ears or speech stays in Portuguese: spoken TTS
  phrases and the voice command phrases ("permitir", "negar", "sempre").
  See [0011](0011-local-voice-pipeline.md).

## Amendment (2026-10-29)

Spoken phrases and the pet's answers follow the language the operator picks
(`:lang pt|en`, default Portuguese). They are rendered from `voice::phrases`, so the
code stays English and new languages only add a table there.
