//! Scripted runner for tests: echoes messages, `perm:<command>` asks the broker to
//! run `<command>` and reports the decision, `ask:<agent>:<message>` asks a teammate,
//! `limit:` fails like a provider out of quota, `full:` fills 90% of the context.
//! Turns run one after another, like a real agent's.

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use super::{AgentEvent, AgentInfo, AgentRunner, RunnerContext, RunnerResult, ask_permission};
use crate::model::Agent;

pub struct FakeRunner {
    context: RunnerContext,
    events_tx: mpsc::Sender<AgentEvent>,
    events_rx: Option<mpsc::Receiver<AgentEvent>>,
    prompts: Option<mpsc::UnboundedSender<String>>,
}

impl FakeRunner {
    pub fn new(context: RunnerContext) -> Self {
        let (events_tx, events_rx) = mpsc::channel(64);
        Self { context, events_tx, events_rx: Some(events_rx), prompts: None }
    }
}

#[async_trait]
impl AgentRunner for FakeRunner {
    async fn start(&mut self, agent: &Agent, _mcp_url: &str) -> RunnerResult<()> {
        let info = AgentInfo::of(agent);
        let current = agent.model.clone().or_else(|| Some("fake-1".into()));
        let available = vec!["fake-1".into(), "fake-2".into()];
        let _ = self.events_tx.send(AgentEvent::Models { current, available }).await;
        let events = self.events_tx.clone();
        let context = self.context.clone();
        let (prompts, mut queue) = mpsc::unbounded_channel::<String>();
        self.prompts = Some(prompts);
        tokio::spawn(async move {
            while let Some(text) = queue.recv().await {
                // The orchestrator wraps first prompts in a briefing; the script is the last line.
                let last = text.lines().last().unwrap_or_default().trim();
                let script = last.strip_prefix("Operator: ").unwrap_or(last).to_owned();
                if script == "nomodel:" {
                    let error = r#"Internal error: {"message": "{\"error\":{\"message\":\"The 'fake-1' model requires a newer version of Codex.\"}}"}"#;
                    let _ = events.send(AgentEvent::Error(error.into())).await;
                    continue;
                }
                if script == "full:" {
                    let _ = events.send(AgentEvent::Context { used: 180_000, size: 200_000 }).await;
                    let _ = events.send(AgentEvent::Cost(250_000)).await;
                    let _ = events.send(AgentEvent::Text("done with a big file\n".into())).await;
                    let _ = events.send(AgentEvent::TurnEnd { stop_reason: "EndTurn".into() }).await;
                    continue;
                }
                if script == "limit:" {
                    let _ = events.send(AgentEvent::Error("Claude AI usage limit reached|1793300000".into())).await;
                    continue;
                }
                let reply = if let Some(command) = script.strip_prefix("perm:") {
                    let decision =
                        ask_permission(&context, &info, "Bash", json!({ "command": command }), Vec::new()).await;
                    format!("{command}: {}\n", if decision.allows() { "allowed" } else { "denied" })
                } else if let Some(note) = script.strip_prefix("note:") {
                    let team = context.team.as_ref().expect("team tools");
                    let input = serde_json::json!({ "note": note });
                    match team.call(info.id, crate::team::REMEMBER, &input).await {
                        Some(Ok(_)) => "noted\n".to_owned(),
                        other => format!("remember failed: {other:?}\n"),
                    }
                } else if let Some((agent, message)) = script.strip_prefix("ask:").and_then(|r| r.split_once(':')) {
                    let team = context.team.as_ref().expect("team tools");
                    match team.ask(info.id, agent, message).await {
                        Ok(answer) => format!("{agent} said: {answer}\n"),
                        Err(error) => format!("ask failed: {error}\n"),
                    }
                } else {
                    format!("echo: {text}\n")
                };
                let _ = events.send(AgentEvent::Text(reply)).await;
                let _ = events.send(AgentEvent::TurnEnd { stop_reason: "EndTurn".into() }).await;
            }
        });
        Ok(())
    }

    async fn send(&mut self, text: String) -> RunnerResult<()> {
        let prompts = self.prompts.as_ref().ok_or("not started")?;
        prompts.send(text).map_err(|_| "stopped".to_owned())
    }

    fn events(&mut self) -> mpsc::Receiver<AgentEvent> {
        self.events_rx.take().unwrap_or_else(|| mpsc::channel(1).1)
    }

    async fn set_model(&mut self, model: &str) -> RunnerResult<()> {
        let available = vec!["fake-1".into(), "fake-2".into()];
        let _ = self.events_tx.send(AgentEvent::Models { current: Some(model.into()), available }).await;
        Ok(())
    }

    async fn stop(&mut self) -> RunnerResult<()> {
        let _ = self.events_tx.send(AgentEvent::Exited).await;
        Ok(())
    }
}
