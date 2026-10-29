//! The pet as host agent (ADR 0012): an LLM with the app's own tools. It hears what
//! the parser does not handle, answers out loud and acts through the orchestrator;
//! permission answers still go through the broker.

use serde_json::{Value, json};
use std::fmt::Write as _;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    broker::Answer,
    model::{AppState, Provider, Usage},
    runner::{
        llm::{self, Turn, Wire},
        tools::{ToolCall, ToolDef},
    },
};

/// Model round trips for one request.
const MAX_STEPS: usize = 8;
/// History kept between requests, in turns.
const MAX_HISTORY: usize = 24;

/// Something the pet asked the orchestrator to do. Agents and groups are named the
/// way the operator said them; the orchestrator resolves the names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAction {
    Message {
        group: Option<String>,
        agent: String,
        text: String,
    },
    CreateGroup {
        name: String,
        goal: String,
    },
    CreateAgent {
        group: String,
        name: String,
        provider: Option<String>,
        task: String,
        persona: String,
    },
    Start {
        agent: String,
    },
    Stop {
        agent: String,
    },
    Answer {
        agent: Option<String>,
        answer: Answer,
    },
    SetProvider {
        agent: String,
        provider: String,
    },
    /// An agent's worktree (or the project when `agent` is `None`) in an editor.
    OpenEditor {
        agent: Option<String>,
        editor: Option<String>,
    },
    /// A terminal in an agent's worktree or the project root.
    OpenTerminal {
        agent: Option<String>,
    },
    /// The operator's mail (webmail URL from `:email`, else the mail client).
    OpenEmail,
    /// A web or mail link.
    OpenUrl(String),
}

#[derive(Debug, Clone)]
pub enum HostEvent {
    Thinking(bool),
    /// What the pet says back (spoken).
    Reply(String),
    Action(HostAction),
    Usage(Usage),
    Error(String),
}

enum Msg {
    Ask { text: String, state: Box<AppState>, selected: Option<Uuid> },
    SetProvider(Option<Provider>),
}

#[derive(Clone)]
pub struct HostHandle {
    tx: mpsc::UnboundedSender<Msg>,
}

impl HostHandle {
    pub fn ask(&self, text: String, state: AppState, selected: Option<Uuid>) {
        let _ = self.tx.send(Msg::Ask { text, state: Box::new(state), selected });
    }

    pub fn set_provider(&self, provider: Option<Provider>) {
        let _ = self.tx.send(Msg::SetProvider(provider));
    }
}

pub fn spawn(events: mpsc::UnboundedSender<HostEvent>) -> HostHandle {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut provider: Option<Provider> = None;
        let mut wire: Option<Wire> = None;
        let mut history: Vec<Turn> = Vec::new();
        let client = match llm::client() {
            Ok(client) => client,
            Err(error) => {
                let _ = events.send(HostEvent::Error(error));
                return;
            }
        };
        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::SetProvider(next) => {
                    provider = next;
                    wire = None;
                    history.clear();
                }
                Msg::Ask { text, state, selected } => {
                    let Some(chosen) = &provider else {
                        let _ = events.send(HostEvent::Error("the pet has no provider: :pet <provider>".into()));
                        continue;
                    };
                    if wire.is_none() {
                        match llm::wire_for(chosen).await {
                            Ok(resolved) => wire = Some(resolved),
                            Err(error) => {
                                let _ = events.send(HostEvent::Error(error));
                                continue;
                            }
                        }
                    }
                    let Some(wire) = &wire else { continue };
                    let _ = events.send(HostEvent::Thinking(true));
                    history.push(Turn::User(text));
                    let system = system_prompt(&state, selected);
                    if let Err(error) = respond(&client, wire, &system, &state, &mut history, &events).await {
                        let _ = events.send(HostEvent::Error(error));
                    }
                    trim(&mut history);
                    let _ = events.send(HostEvent::Thinking(false));
                }
            }
        }
    });
    HostHandle { tx }
}

async fn respond(
    client: &reqwest::Client,
    wire: &Wire,
    system: &str,
    state: &AppState,
    history: &mut Vec<Turn>,
    events: &mpsc::UnboundedSender<HostEvent>,
) -> Result<(), String> {
    let definitions = tools();
    for _ in 0..MAX_STEPS {
        let reply = llm::complete(client, wire, system, &definitions, history).await?;
        let _ = events.send(HostEvent::Usage(reply.usage));
        history.push(Turn::Assistant { text: reply.text.clone(), calls: reply.calls.clone() });
        if reply.calls.is_empty() {
            let text = reply.text.trim();
            if !text.is_empty() {
                let _ = events.send(HostEvent::Reply(text.to_owned()));
            }
            return Ok(());
        }
        let results = reply
            .calls
            .iter()
            .map(|call| {
                let (output, failed) = match run_tool(call, state) {
                    Ok(Some(action)) => {
                        let _ = events.send(HostEvent::Action(action));
                        ("done".to_owned(), false)
                    }
                    Ok(None) => (describe(state), false),
                    Err(error) => (error, true),
                };
                (call.id.clone(), output, failed)
            })
            .collect();
        history.push(Turn::Results(results));
    }
    Ok(())
}

/// Drops old turns, never splitting a tool call from its results.
fn trim(history: &mut Vec<Turn>) {
    while history.len() > MAX_HISTORY {
        history.remove(0);
        while history.first().is_some_and(|turn| !matches!(turn, Turn::User(_))) {
            history.remove(0);
        }
    }
}

fn system_prompt(state: &AppState, selected: Option<Uuid>) -> String {
    let selected = selected.and_then(|id| state.agent(id)).map_or("none".to_owned(), |a| a.name.clone());
    let language = state.voice.language.name();
    format!(
        "You are the pet: the host of a control room where coding agents work on a project. \
         The operator talks to you by voice or text. Act with the tools, then answer in {language} \
         with one or two short sentences, no markdown and no lists: your answer is read aloud. When the operator gives an instruction without naming an agent, send it to the \
         selected agent ({selected}). Only answer permission requests when the operator clearly says \
         so. Never invent agents or groups that are not in the state below.\n\nCurrent state:\n{}",
        describe(state)
    )
}

/// The room in a few lines, for the model.
pub fn describe(state: &AppState) -> String {
    let mut out = String::new();
    if state.projects.is_empty() {
        out.push_str("project: none open\n");
    }
    for project in &state.projects {
        let focus = if state.focus == Some(project.id) { " [in focus: new groups go here]" } else { "" };
        let _ = writeln!(out, "project {} ({}){focus}", project.name, project.root.display());
        for group in state.groups_of(Some(project.id)) {
            let _ = writeln!(out, "  group {}: goal \"{}\"", group.name, group.goal);
            for agent in state.agents.iter().filter(|a| a.group_id == group.id) {
                let _ = writeln!(
                    out,
                    "    agent {}: {:?}, provider {}, task \"{}\"",
                    agent.name,
                    agent.state,
                    agent.provider.short_name(),
                    agent.task
                );
            }
        }
    }
    for request in &state.permissions {
        let agent = state.agent(request.agent_id).map_or("?", |a| a.name.as_str());
        let _ = writeln!(out, "pending permission: {agent} wants {} {}", request.tool, request.input_summary());
    }
    let providers: Vec<&str> = state.providers.iter().filter(|p| p.ready).map(|p| p.label.as_str()).collect();
    let _ = writeln!(out, "providers: {}", providers.join(", "));
    let editors: Vec<&str> = state.editors.iter().map(|e| e.name.as_str()).collect();
    let _ = writeln!(out, "editors: {}", editors.join(", "));
    out
}

fn text(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

fn optional(input: &Value, key: &str) -> Option<String> {
    text(input, key).ok()
}

/// `Ok(None)` is a read-only tool answered from the state.
fn run_tool(call: &ToolCall, state: &AppState) -> Result<Option<HostAction>, String> {
    let input = &call.input;
    let action = match call.name.as_str() {
        "list_agents" => return Ok(None),
        "message_agent" => HostAction::Message {
            group: optional(input, "group"),
            agent: text(input, "agent")?,
            text: text(input, "text")?,
        },
        "create_group" => HostAction::CreateGroup { name: text(input, "name")?, goal: text(input, "goal")? },
        "create_agent" => HostAction::CreateAgent {
            group: text(input, "group")?,
            name: text(input, "name")?,
            provider: optional(input, "provider"),
            task: text(input, "task")?,
            persona: optional(input, "persona").unwrap_or_default(),
        },
        "start_agent" => HostAction::Start { agent: text(input, "agent")? },
        "stop_agent" => HostAction::Stop { agent: text(input, "agent")? },
        "answer_permission" => {
            if state.permissions.is_empty() {
                return Err("there is no pending permission request".into());
            }
            let answer = match text(input, "answer")?.as_str() {
                "allow" => Answer::Allow,
                "always" => Answer::AlwaysAllow,
                "deny" => Answer::Deny,
                other => return Err(format!("answer must be allow, always or deny, not {other}")),
            };
            HostAction::Answer { agent: optional(input, "agent"), answer }
        }
        "set_provider" => HostAction::SetProvider { agent: text(input, "agent")?, provider: text(input, "provider")? },
        "open_editor" => HostAction::OpenEditor { agent: optional(input, "agent"), editor: optional(input, "editor") },
        "open_terminal" => HostAction::OpenTerminal { agent: optional(input, "agent") },
        "open_email" => HostAction::OpenEmail,
        "open_url" => {
            let url = text(input, "url")?;
            if !["http://", "https://", "mailto:"].iter().any(|scheme| url.starts_with(scheme)) {
                return Err("only http, https and mailto links".into());
            }
            HostAction::OpenUrl(url)
        }
        other => return Err(format!("unknown tool {other}")),
    };
    Ok(Some(action))
}

fn tools() -> Vec<ToolDef> {
    let string = |description: &str| json!({ "type": "string", "description": description });
    vec![
        ToolDef {
            name: "list_agents",
            description: "Current project, groups, agents, their state and pending permissions.",
            schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "message_agent",
            description: "Send an instruction to an agent. It starts the agent when stopped.",
            schema: json!({ "type": "object", "properties": {
                "agent": string("agent name"), "group": string("group name, optional"), "text": string("the instruction")
            }, "required": ["agent", "text"] }),
        },
        ToolDef {
            name: "create_group",
            description: "Create a group of agents with a shared goal.",
            schema: json!({ "type": "object", "properties": {
                "name": string("short name"), "goal": string("what the group works towards")
            }, "required": ["name", "goal"] }),
        },
        ToolDef {
            name: "create_agent",
            description: "Create an agent in a group with a task. Provider is a label from the state, optional. \
                          The persona says who the agent is (its focus and what it stays away from); agents of a \
                          project can ask each other for help, so distinct personas make a team.",
            schema: json!({ "type": "object", "properties": {
                "group": string("group name"), "name": string("short speakable name"),
                "provider": string("provider label"), "task": string("what the agent should do"),
                "persona": string("e.g. backend specialist: server code only, asks the front-end agent about the UI")
            }, "required": ["group", "name", "task"] }),
        },
        ToolDef {
            name: "start_agent",
            description: "Start an agent on its task.",
            schema: json!({ "type": "object", "properties": { "agent": string("agent name") }, "required": ["agent"] }),
        },
        ToolDef {
            name: "stop_agent",
            description: "Stop an agent.",
            schema: json!({ "type": "object", "properties": { "agent": string("agent name") }, "required": ["agent"] }),
        },
        ToolDef {
            name: "answer_permission",
            description: "Answer a pending permission request: allow, always (allow and remember) or deny.",
            schema: json!({ "type": "object", "properties": {
                "answer": { "type": "string", "enum": ["allow", "always", "deny"] },
                "agent": string("whose request; the oldest one when omitted")
            }, "required": ["answer"] }),
        },
        ToolDef {
            name: "open_editor",
            description: "Open an agent's worktree, or the whole project when no agent is given, in a code editor.",
            schema: json!({ "type": "object", "properties": {
                "agent": string("agent name; omit for the project"),
                "editor": string("editor name from the state, optional")
            } }),
        },
        ToolDef {
            name: "open_terminal",
            description: "Open a terminal in an agent's worktree, or in the project when no agent is given.",
            schema: json!({ "type": "object", "properties": { "agent": string("agent name, optional") } }),
        },
        ToolDef {
            name: "open_email",
            description: "Open the operator's email.",
            schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "open_url",
            description: "Open a web page (http/https) or a mailto link in the default browser or mail app.",
            schema: json!({ "type": "object", "properties": { "url": string("the link") }, "required": ["url"] }),
        },
        ToolDef {
            name: "set_provider",
            description: "Move an agent to another provider (label or service/model); it keeps the conversation.",
            schema: json!({ "type": "object", "properties": {
                "agent": string("agent name"), "provider": string("provider label or service/model")
            }, "required": ["agent", "provider"] }),
        },
    ]
}

/// The provider the pet uses when the operator picked none: a fast API model when a
/// key exists, else a local model.
pub fn default_provider(state: &AppState) -> Option<Provider> {
    let anthropic = state.providers.iter().find(|p| p.ready && p.label == "anthropic");
    if let Some(Provider::Api { service, key_ref, .. }) = anthropic.map(|p| &p.provider) {
        return Some(Provider::Api {
            service: service.clone(),
            model: "claude-haiku-5-5".into(),
            key_ref: key_ref.clone(),
        });
    }
    state
        .providers
        .iter()
        .find(|p| p.ready && matches!(p.provider, Provider::Api { .. } | Provider::Local { .. }))
        .map(|p| p.provider.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_data::sample_state;

    fn call(name: &str, input: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), input }
    }

    #[test]
    fn tools_become_actions_for_the_orchestrator() {
        let state = sample_state();
        assert_eq!(
            run_tool(&call("message_agent", json!({ "agent": "Tests", "text": "rode a suíte" })), &state),
            Ok(Some(HostAction::Message { group: None, agent: "Tests".into(), text: "rode a suíte".into() }))
        );
        assert_eq!(
            run_tool(&call("answer_permission", json!({ "answer": "always" })), &state),
            Ok(Some(HostAction::Answer { agent: None, answer: Answer::AlwaysAllow }))
        );
        assert_eq!(run_tool(&call("list_agents", json!({})), &state), Ok(None));
        assert_eq!(
            run_tool(&call("open_editor", json!({ "editor": "zed" })), &state),
            Ok(Some(HostAction::OpenEditor { agent: None, editor: Some("zed".into()) }))
        );
        assert!(run_tool(&call("open_url", json!({ "url": "file:///etc/passwd" })), &state).is_err());
        assert!(run_tool(&call("message_agent", json!({ "agent": "Tests" })), &state).is_err());
    }

    #[test]
    fn answering_without_pending_requests_is_refused() {
        let mut state = sample_state();
        state.permissions.clear();
        assert!(run_tool(&call("answer_permission", json!({ "answer": "allow" })), &state).is_err());
    }

    #[test]
    fn describe_lists_groups_agents_and_requests() {
        let text = describe(&sample_state());
        assert!(text.contains("group API") && text.contains("agent Tests") && text.contains("pending permission"));
    }

    #[test]
    fn history_trim_keeps_tool_calls_with_their_results() {
        let mut history = Vec::new();
        for i in 0..20 {
            history.push(Turn::User(format!("q{i}")));
            history.push(Turn::Assistant { text: String::new(), calls: vec![call("list_agents", json!({}))] });
            history.push(Turn::Results(vec![("1".into(), "x".into(), false)]));
        }
        trim(&mut history);
        assert!(history.len() <= MAX_HISTORY);
        assert!(matches!(history.first(), Some(Turn::User(_))));
    }
}

#[cfg(test)]
mod smoke {
    use std::time::Duration;

    use super::*;

    /// `ORCH_OLLAMA_MODEL=qwen3:8b cargo test -p core host_smoke -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs a local Ollama"]
    async fn host_smoke() {
        let model = std::env::var("ORCH_OLLAMA_MODEL").unwrap_or_else(|_| "qwen3:8b".into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let host = spawn(tx);
        host.set_provider(Some(Provider::Local { url: crate::discovery::OLLAMA_URL.into(), model }));
        host.ask(
            "Crie um grupo chamado Docs com o objetivo de escrever a documentação. /no_think".into(),
            crate::sample_data::sample_state(),
            None,
        );
        let mut actions = Vec::new();
        let done = tokio::time::timeout(Duration::from_secs(400), async {
            while let Some(event) = rx.recv().await {
                eprintln!("{event:?}");
                match event {
                    HostEvent::Action(action) => actions.push(action),
                    HostEvent::Thinking(false) => break,
                    _ => {}
                }
            }
        })
        .await;
        assert!(done.is_ok());
        assert!(
            actions.iter().any(|a| matches!(a, HostAction::CreateGroup { name, .. } if name == "Docs")),
            "{actions:?}"
        );
    }
}
