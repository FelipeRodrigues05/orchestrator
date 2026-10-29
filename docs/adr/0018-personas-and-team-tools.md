# 0018. Agent personas and agents that call each other

- **Status:** Accepted
- **Date:** 2026-10-29

## Context

Agents of a project work on parts of one whole: a backend agent, a front-end agent,
an integration agent. Each one only knew its own task, so the operator had to carry
questions between them. The operator wants agents to know their teammates and call
them when they need something ("the backend asks the integration agent whether the
front end works against its API").

## Decision

- An agent has a **persona** (`Agent.persona`): who it is, its focus and what it
  leaves to others. It is optional, saved in SQLite, set in the new-agent wizard,
  with `:new … | <persona>`, `:persona [agent]: <text>`, the palette, or by the pet
  (`create_agent` takes a persona). A change applies from the agent's next start.
- When an agent starts, its briefing says who it is, its group's goal, its task and
  its teammates (the other agents of the project with persona or task and state).
- Every agent gets two tools (`core::team`):
  - `list_agents`: the project's other agents, live.
  - `ask_agent { agent, message }`: delivers the message to that teammate (starting
    it if it is stopped, queued after its current turn if it is busy) and returns the
    text of the turn that answers it. It times out after 20 minutes.
- Each message to an agent is one turn and turns run in order, so the orchestrator
  numbers the turns it sends and the turn ends it sees; the asker waits for the
  turn number its message got. If the teammate stops or fails, the asker gets an
  error instead of waiting.
- Asking is limited to the asker's project. Asking oneself, an unknown name, or a
  teammate that is (directly or through others) waiting for the asker is refused
  with a message the model can act on.
- Transport per runner (ADR 0004):
  - API and local models: the tools join the app's tool loop.
  - Claude Code (`McpCliRunner`) and ACP CLIs: the app's MCP server (ADR 0016) also
    serves `list_agents` and `ask_agent` under the agent's token; ACP agents get it as
    an HTTP MCP server in `session/new` when they declare `mcpCapabilities.http`
    (opencode does). Without it the agent has no team tools.
- The broker allows the team tools by default: talking touches no files, and what
  the teammate does is asked for on its own.
- The log shows `→ asks <agent>: …` on the asker and `← <agent> asks: …` on the
  teammate.

## Consequences

- A team can split work by persona and coordinate without the operator relaying.
- One question can start a stopped agent and spend its tokens; the group limit
  still applies.
- Long chains (A asks B, B asks C) hold A's turn until C answers; the timeout and the
  cycle check keep them from hanging forever.
- Verified for real with opencode inside the sandbox calling `ask_agent` over MCP.
