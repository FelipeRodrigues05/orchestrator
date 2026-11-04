//! Agents of one project know each other and can call each other: `list_agents`
//! shows the team with each agent's persona, `ask_agent` sends a message to a
//! teammate and waits for the reply that closes its turn. Every runner offers these
//! two tools: API agents in their tool loop, CLIs through the app's MCP server.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// A teammate that does not answer in this time releases the caller.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(20 * 60);
pub const LIST: &str = "list_agents";
pub const ASK: &str = "ask_agent";
pub const REMEMBER: &str = "remember";

pub(crate) enum TeamRequest {
    Roster { from: Uuid, reply: oneshot::Sender<String> },
    Ask { from: Uuid, to: String, message: String, reply: oneshot::Sender<Result<String, String>> },
    Remember { from: Uuid, note: String, reply: oneshot::Sender<Result<String, String>> },
}

#[derive(Clone)]
pub struct TeamHandle {
    tx: mpsc::UnboundedSender<TeamRequest>,
}

impl TeamHandle {
    pub(crate) fn new(tx: mpsc::UnboundedSender<TeamRequest>) -> Self {
        Self { tx }
    }

    /// The caller's teammates, one per line.
    pub async fn roster(&self, from: Uuid) -> String {
        let (reply, answer) = oneshot::channel();
        let _ = self.tx.send(TeamRequest::Roster { from, reply });
        answer.await.unwrap_or_else(|_| "the orchestrator is not running".into())
    }

    /// Sends `message` to the teammate called `to` and waits for its reply.
    pub async fn ask(&self, from: Uuid, to: &str, message: &str) -> Result<String, String> {
        let (reply, answer) = oneshot::channel();
        let request = TeamRequest::Ask { from, to: to.to_owned(), message: message.to_owned(), reply };
        self.tx.send(request).map_err(|_| "the orchestrator is not running".to_owned())?;
        match tokio::time::timeout(ASK_TIMEOUT, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("{to} stopped before answering")),
            Err(_) => Err(format!("{to} did not answer in {} minutes", ASK_TIMEOUT.as_secs() / 60)),
        }
    }

    /// Runs a team tool; `None` when `name` is not one.
    pub async fn call(&self, from: Uuid, name: &str, input: &Value) -> Option<Result<String, String>> {
        let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_owned();
        match name {
            LIST => Some(Ok(self.roster(from).await)),
            REMEMBER => {
                let note = text("note");
                if note.is_empty() {
                    return Some(Err("remember needs a `note`".into()));
                }
                let (reply, answer) = oneshot::channel();
                let _ = self.tx.send(TeamRequest::Remember { from, note, reply });
                Some(answer.await.unwrap_or_else(|_| Err("the orchestrator is not running".into())))
            }
            ASK => {
                let (to, message) = (text("agent"), text("message"));
                if to.is_empty() || message.is_empty() {
                    return Some(Err("ask_agent needs `agent` and `message`".into()));
                }
                Some(self.ask(from, &to, &message).await)
            }
            _ => None,
        }
    }
}

/// `(name, description, JSON schema)` of the team tools.
pub fn tools() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            REMEMBER,
            "Writes down something worth knowing in later tasks, in any project: the operator's preferences, \
             conventions, a lesson learnt. One short fact per call; your notes come back in your next briefings.",
            json!({ "type": "object", "properties": {
                "note": { "type": "string", "description": "one short fact, e.g. \"the operator prefers Pest over PHPUnit\"" }
            }, "required": ["note"] }),
        ),
        (
            LIST,
            "Lists the other agents of your project: name, group, persona, task and whether they are working now.",
            json!({ "type": "object", "properties": {} }),
        ),
        (
            ASK,
            "Sends a message to another agent of your project and waits for its answer (it may take minutes: it \
             works on it first). Use it when a teammate owns what you need: ask the integration agent to check \
             the front end against your API, the backend agent for an endpoint's contract, and so on.",
            json!({ "type": "object", "properties": {
                "agent": { "type": "string", "description": "the teammate's name, as list_agents shows it" },
                "message": { "type": "string", "description": "what you need, with enough context to act alone" }
            }, "required": ["agent", "message"] }),
        ),
    ]
}

/// Whether a tool name (as a CLI reports it: `mcp__orch__ask_agent`, `orch_ask_agent`…)
/// is one of ours. Talking to a teammate needs no permission.
pub fn is_team_tool(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    tool.ends_with(ASK) || tool.ends_with(LIST) || tool.ends_with(REMEMBER)
}
