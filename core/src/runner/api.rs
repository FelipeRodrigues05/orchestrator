//! `ApiRunner`: HTTP providers (Anthropic, OpenAI-compatible, Ollama) with the
//! app's own tool loop. Every tool call goes through the broker before it runs.

use async_trait::async_trait;
use tokio::{sync::mpsc, task::JoinHandle};

use super::{
    AgentEvent, AgentInfo, AgentRunner, RunnerContext, RunnerResult, ToolStatus, ask_permission,
    llm::{self, Turn, Wire},
    tools::{self, ToolDef},
};
use crate::model::{Agent, Decision};

/// Upper bound of model round trips for one message, against tool loops.
const MAX_STEPS: usize = 40;

pub struct ApiRunner {
    context: RunnerContext,
    events_tx: mpsc::Sender<AgentEvent>,
    events_rx: Option<mpsc::Receiver<AgentEvent>>,
    prompts: Option<mpsc::UnboundedSender<String>>,
    task: Option<JoinHandle<()>>,
}

impl ApiRunner {
    pub fn new(context: RunnerContext) -> Self {
        let (events_tx, events_rx) = mpsc::channel(256);
        Self { context, events_tx, events_rx: Some(events_rx), prompts: None, task: None }
    }
}

#[async_trait]
impl AgentRunner for ApiRunner {
    async fn start(&mut self, agent: &Agent, _mcp_url: &str) -> RunnerResult<()> {
        let wire = llm::wire_for(&agent.provider).await?;
        let client = llm::client()?;
        let (prompts, mut prompt_rx) = mpsc::unbounded_channel::<String>();
        self.prompts = Some(prompts);
        let events = self.events_tx.clone();
        let context = self.context.clone();
        let info = AgentInfo::of(agent);
        let system = format!(
            "You are {name}, a coding agent in the group \"{group}\". Your working directory is a git \
             worktree of the project; only touch files inside it. Use the tools to read, search, edit \
             files and run commands. Be concise; report what you changed.",
            name = agent.name,
            group = context.group_name,
        );
        self.task = Some(tokio::spawn(async move {
            let mut definitions = tools::definitions();
            if context.team.is_some() {
                definitions.extend(crate::team::tools().into_iter().map(|(name, description, schema)| ToolDef {
                    name,
                    description,
                    schema,
                }));
            }
            let mut history = Vec::new();
            while let Some(text) = prompt_rx.recv().await {
                history.push(Turn::User(text));
                let stop =
                    run_turn(&client, &wire, &system, &definitions, &mut history, &events, &context, &info).await;
                let _ = events.send(AgentEvent::TurnEnd { stop_reason: stop }).await;
            }
            let _ = events.send(AgentEvent::Exited).await;
        }));
        Ok(())
    }

    async fn send(&mut self, text: String) -> RunnerResult<()> {
        let prompts = self.prompts.as_ref().ok_or("agent not started")?;
        prompts.send(text).map_err(|_| "agent is not running".to_owned())
    }

    fn events(&mut self) -> mpsc::Receiver<AgentEvent> {
        self.events_rx.take().unwrap_or_else(|| mpsc::channel(1).1)
    }

    async fn stop(&mut self) -> RunnerResult<()> {
        self.prompts = None;
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = self.events_tx.send(AgentEvent::Exited).await;
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    client: &reqwest::Client,
    wire: &Wire,
    system: &str,
    definitions: &[ToolDef],
    history: &mut Vec<Turn>,
    events: &mpsc::Sender<AgentEvent>,
    context: &RunnerContext,
    info: &AgentInfo,
) -> String {
    for _ in 0..MAX_STEPS {
        let reply = match llm::complete(client, wire, system, definitions, history).await {
            Ok(reply) => reply,
            Err(error) => {
                let _ = events.send(AgentEvent::Error(error)).await;
                return "error".into();
            }
        };
        let _ = events.send(AgentEvent::Usage(reply.usage)).await;
        if !reply.text.trim().is_empty() {
            let _ = events.send(AgentEvent::Text(format!("{}\n", reply.text.trim()))).await;
        }
        history.push(Turn::Assistant { text: reply.text, calls: reply.calls.clone() });
        if reply.calls.is_empty() {
            return "end_turn".into();
        }
        let mut results = Vec::new();
        for call in reply.calls {
            if let Some(team) = &context.team
                && crate::team::is_team_tool(&call.name)
            {
                let title = format!("{} {}", call.name, crate::model::input_summary(&call.input));
                let _ = events.send(AgentEvent::Tool { title: title.clone(), status: ToolStatus::Started }).await;
                let result = team.call(info.id, &call.name, &call.input).await.unwrap_or(Err("unknown tool".into()));
                let failed = result.is_err();
                let status = if failed { ToolStatus::Failed } else { ToolStatus::Done };
                let _ = events.send(AgentEvent::Tool { title, status }).await;
                results.push((call.id, result.unwrap_or_else(|error| error), failed));
                continue;
            }
            let (tool, input, paths) = tools::permission_of(&call);
            let title = format!("{tool} {}", crate::model::input_summary(&input));
            let decision = ask_permission(context, info, &tool, input, paths).await;
            let (output, failed) = if let Decision::Deny { reason } = decision {
                (format!("permission denied: {reason}"), true)
            } else {
                let _ = events.send(AgentEvent::Tool { title: title.clone(), status: ToolStatus::Started }).await;
                match tools::run(&call, &info.worktree, &context.extra_dirs, context.sandbox).await {
                    Ok(output) => (output, false),
                    Err(error) => (error, true),
                }
            };
            let status = if failed { ToolStatus::Failed } else { ToolStatus::Done };
            let _ = events.send(AgentEvent::Tool { title, status }).await;
            results.push((call.id, output, failed));
        }
        history.push(Turn::Results(results));
    }
    "max_steps".into()
}

#[cfg(test)]
mod smoke {
    use std::time::Duration;

    use super::*;
    use crate::{
        broker::{self, BrokerEvent, DecisionSource},
        model::{AgentState, Provider},
    };

    /// Edits a file through a local Ollama model, approving through the broker:
    /// `ORCH_OLLAMA_MODEL=qwen3:8b cargo test -p core api_smoke -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a local Ollama"]
    async fn api_smoke() {
        let model = std::env::var("ORCH_OLLAMA_MODEL").unwrap_or_else(|_| "qwen3:8b".into());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut broker_events) = mpsc::unbounded_channel();
        let broker = broker::spawn(Duration::from_secs(60), tx, None);
        let approver = broker.clone();
        let asked = tokio::spawn(async move {
            let mut asked = Vec::new();
            while let Some(event) = broker_events.recv().await {
                if let BrokerEvent::Queued(request) = event {
                    asked.push(request.tool.clone());
                    approver.answer(Some(request.id), broker::Answer::Allow, DecisionSource::Click);
                }
            }
            asked
        });
        let context = RunnerContext {
            broker,
            group_id: uuid::Uuid::new_v4(),
            group_name: "Smoke".into(),
            sandbox: crate::sandbox::available(),
            mcp: None,
            team: None,
            extra_dirs: Vec::new(),
        };
        let agent = Agent {
            id: uuid::Uuid::new_v4(),
            group_id: context.group_id,
            name: "Smoke".into(),
            task: String::new(),
            provider: Provider::Local { url: crate::discovery::OLLAMA_URL.into(), model },
            worktree: dir.path().into(),
            state: AgentState::Stopped,
            rules: Vec::new(),
            started_at: None,
            usage: Default::default(),
            persona: String::new(),
            model: None,
            models: Vec::new(),
            color: None,
            context: None,
            cost_micros: 0,
            profile_id: None,
        };
        let mut runner = ApiRunner::new(context);
        runner.start(&agent, "").await.unwrap();
        let mut events = runner.events();
        runner
            .send(
                "Use the edit_file tool to create hello.txt containing exactly: hi from the orchestrator /no_think"
                    .into(),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(300), async {
            while let Some(event) = events.recv().await {
                eprintln!("{event:?}");
                if matches!(event, AgentEvent::TurnEnd { .. } | AgentEvent::Error(_)) {
                    break;
                }
            }
        })
        .await
        .expect("model answered in time");
        runner.stop().await.unwrap();
        let written = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
        assert!(written.contains("hi from the orchestrator"));
        drop(runner);
        asked.abort();
    }
}
