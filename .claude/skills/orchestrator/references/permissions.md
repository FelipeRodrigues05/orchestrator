# Broker and permission flow

The agent blocks on a `oneshot::Receiver<Decision>` until there is a decision (rule, click, voice or timeout).

## Flow

1. The agent wants to run a sensitive tool.
   - ACP: a `session/request_permission` arrives.
   - MCP: the CLI calls the app's `permission` MCP tool (`--permission-prompt-tool`). Each agent gets an MCP server URL with **its own token** → the handler knows which agent is asking.
   - API: the `ApiRunner` builds the request before running the tool.
2. The runner/handler builds a `PermissionRequest` + `oneshot::Sender<Decision>` and sends it to the broker; then awaits the `Receiver`.
3. The broker checks the **agent's** rules, then the **group's**. If one matches with `Allow`/`Deny`, it answers right away.
4. If no rule matches (or the effect is `Ask`), the request is queued and turned into events:
   - UI: the permission modal drawn in the room (ADR 0014) with agent, group, tool and command; **Allow**, **Always** (agent), **Group**, **Deny**, **Esc later**. One at a time, with a counter of the remaining ones.
   - Voice: "Agente Testes, do grupo API, pede permissão para rodar cargo test" (spoken text stays in Portuguese). It jumps the speech queue.
   - System: `gio::Notification` with Allow/Deny when the window is unfocused.
5. The answer by click or voice ("permitir", "sempre", "negar") goes back through the `oneshot`.
6. The runner translates to the protocol:
   - MCP: `{"behavior":"allow","updatedInput":…}` or `{"behavior":"deny","message":…}`
   - ACP: the chosen option in `session/request_permission`
   - API: run the tool or return a tool error to the model

## Broker rules

- Configurable **timeout** (default 10 min) → `Deny { reason: "no response" }`. Use `tokio::time::timeout`.
- If the agent's side goes away (agent stopped), drop the request from the queue and close its popup.
- `AlwaysAllow(rule)` adds the rule to the agent (or to the group, if the user chooses) and persists it in SQLite.
- Rule patterns in the `Bash(cargo test:*)` style: tool + command prefix. Avoid pattern-less rules for `Bash`.
- Safe defaults: reads allowed; writes outside the worktree denied.
- The agent's state goes to `AwaitingPermission` while waiting and back to `Running` after the decision.
- Every decision goes to the history (SQLite): request, decision, source (rule/click/voice/timeout), time.

## Required tests in core

- Agent rule matches → immediate answer, no UI event.
- Group rule matches when the agent has no rule.
- No rule → UI and voice events; the decision goes back to the right agent.
- Two simultaneous requests → queued in order, correct counter.
- Timeout → `Deny` with reason "no response" (use `tokio::time::pause()`).
- `AlwaysAllow` → the next identical request is approved without asking.
