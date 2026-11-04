# 0020. Model router: fall back to the closest model on usage limits

- **Status:** Accepted
- **Date:** 2026-11-03

## Context

Agents run on subscriptions and API keys with usage limits. When Opus runs out of
tokens in the middle of a task the agent stops with an error and waits for the
operator, even when another logged-in CLI (Codex with `gpt-6-astra`, say) could go
on right away.

## Decision

- `core::router` recognises usage-limit errors by their text ("usage limit",
  "rate limit", "quota", "429", "too many requests", "credit balance"…). Other
  errors keep their old handling (outdated CLIs are updated, ADR in SPEC).
- On a limit the agent's provider/model is set aside for an hour and the router
  picks the closest available one:
  - models get a tier from their name: 3 frontier (opus, gpt-6-astra, *-pro),
    2 mid (sonnet, unknown names), 1 small or local (haiku, mini, flash, llama…);
  - the same tier first, on another provider before the same one; then lower
    tiers, then higher;
  - candidates are the ready providers, with the models each CLI reported while
    running (Claude Code's aliases are known in advance).
- The operator may fix the order: `:fallback claude/opus, codex/gpt-6-astra, ollama`
  (saved); `:fallback auto` goes back to closeness; `:fallback reset` makes every
  model available again.
- The switch is the provider/model hand-over that already exists: the agent is
  restarted on the new provider with its briefing and a summary of the
  conversation, in the same worktree. The log says
  `limit reached on claude/opus → going on with codex/gpt-6-astra`, and the voice
  announces it.

## Consequences

- A task survives a limit without the operator; the cost moves to another account.
- Detection is by error text: a CLI that words its limit differently is not
  caught until the pattern is added.
- The hand-over is a summary, not the full context; the new model re-reads the
  worktree.

## Amendment (2026-11-03): models a CLI cannot run

An adapter can lag behind the model the account defaults to (`codex-acp` embeds a
Codex older than `gpt-5.6-sol`). That error is not a limit and updating does not
fix it, so it is recognised on its own ("model … requires a newer version", "not
supported", "unknown model"…): the model is set aside like an exhausted one and
the agent goes on with the closest **other model of the same CLI**, or the closest
model elsewhere when the CLI reported no other. No update is attempted. Errors
wrapped in JSON are logged as their inner message.
