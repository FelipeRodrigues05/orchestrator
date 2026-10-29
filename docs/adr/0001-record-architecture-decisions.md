# 0001. Record architecture decisions

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

The project is built step by step (see the roadmap in `SPEC.md`), often with an AI
coding agent. Without a written record, later steps tend to re-litigate or silently
undo earlier choices (threading model, permission flow, naming).

## Decision

We will keep Architecture Decision Records in `docs/adr/`, one file per decision,
in the Nygard format (Context / Decision / Consequences). `SPEC.md` keeps the
product description and a short **Decisões** list; ADRs hold the reasoning.

## Consequences

- Any change that breaks an invariant needs a new ADR that supersedes the old one.
- Contributors (human or agent) can check `docs/adr/README.md` before designing.
- Small overhead per decision.
