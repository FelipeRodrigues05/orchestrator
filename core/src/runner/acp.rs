//! `AcpRunner`: drives a CLI that speaks the Agent Client Protocol over stdio.
//! Permission requests (`session/request_permission`) go through the broker.

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex, PoisonError},
};

use agent_client_protocol::{
    Agent as AcpAgent, ByteStreams, Client, ConnectionTo,
    schema::{
        ProtocolVersion,
        v1::{
            ContentBlock, InitializeRequest, McpServer, McpServerHttp, NewSessionRequest, PermissionOptionKind,
            PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
            SelectedPermissionOutcome, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, TextContent,
            ToolCallStatus, ToolCallUpdate, ToolKind,
        },
    },
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{io::AsyncBufReadExt, process::Command, sync::mpsc, task::JoinHandle};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::{AgentEvent, AgentInfo, AgentRunner, RunnerContext, RunnerResult, ToolStatus, ask_permission};
use crate::model::{Agent, Decision, Provider};

/// What the session task gets: a message to answer, or a model to switch to.
enum Input {
    Prompt(String),
    Model(String),
}

pub struct AcpRunner {
    context: RunnerContext,
    events_tx: mpsc::Sender<AgentEvent>,
    events_rx: Option<mpsc::Receiver<AgentEvent>>,
    prompts: Option<mpsc::UnboundedSender<Input>>,
    task: Option<JoinHandle<()>>,
    pid: Option<u32>,
    /// This agent's path on the app's MCP server (team tools).
    token: String,
}

impl AcpRunner {
    pub fn new(context: RunnerContext) -> Self {
        let (events_tx, events_rx) = mpsc::channel(256);
        let token = uuid::Uuid::new_v4().simple().to_string();
        Self { context, events_tx, events_rx: Some(events_rx), prompts: None, task: None, pid: None, token }
    }
}

#[async_trait]
impl AgentRunner for AcpRunner {
    async fn start(&mut self, agent: &Agent, _mcp_url: &str) -> RunnerResult<()> {
        let Provider::CliAcp { command } = &agent.provider else {
            return Err("AcpRunner needs a CliAcp provider".into());
        };
        let command = super::update::latest_command(command);
        let words = shell_words::split(&command).map_err(|e| e.to_string())?;
        let (program, args) = words.split_first().ok_or("empty command")?;
        let (program, args) = if self.context.sandbox && crate::sandbox::available() {
            crate::sandbox::wrap(program, args, &agent.worktree, &self.context.extra_dirs)
        } else {
            (program.clone(), args.to_vec())
        };
        let mut command = Command::new(&program);
        // CLI-specific setup stays here (ADR 0004): opencode allows edits and shell
        // by default, which would bypass the broker (ADR 0005).
        if words[0].rsplit('/').next() == Some("opencode") {
            command.env("OPENCODE_CONFIG_CONTENT", r#"{"permission":{"edit":"ask","bash":"ask","webfetch":"ask"}}"#);
        }
        let mut child = command
            .args(&args)
            .current_dir(&agent.worktree)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Own process group, so `stop` also ends what wrappers like npx spawn.
            .process_group(0)
            .spawn()
            .map_err(|e| format!("could not start {program}: {e}"))?;
        self.pid = child.id();

        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            let name = agent.name.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(agent = %name, "stderr: {line}");
                }
            });
        }

        let (prompts, prompt_rx) = mpsc::unbounded_channel();
        self.prompts = Some(prompts);
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        let events = self.events_tx.clone();
        let context = self.context.clone();
        let info = AgentInfo::of(agent);
        // The team tools reach the agent as an MCP server of its session.
        let team_url = match (&self.context.mcp, &self.context.team) {
            (Some(mcp), Some(_)) => Some(mcp.register(&self.token, self.context.clone(), info.clone())),
            _ => None,
        };
        self.task = Some(tokio::spawn(async move {
            let result = connect(transport, prompt_rx, events.clone(), context, info, team_url).await;
            if let Err(error) = result {
                let _ = events.send(AgentEvent::Error(error)).await;
            }
            let _ = child.kill().await;
            let _ = events.send(AgentEvent::Exited).await;
        }));
        Ok(())
    }

    async fn send(&mut self, text: String) -> RunnerResult<()> {
        let prompts = self.prompts.as_ref().ok_or("agent not started")?;
        prompts.send(Input::Prompt(text)).map_err(|_| "agent is not running".to_owned())
    }

    async fn set_model(&mut self, model: &str) -> RunnerResult<()> {
        let prompts = self.prompts.as_ref().ok_or("agent not started")?;
        prompts.send(Input::Model(model.to_owned())).map_err(|_| "agent is not running".to_owned())
    }

    fn events(&mut self) -> mpsc::Receiver<AgentEvent> {
        self.events_rx.take().unwrap_or_else(|| mpsc::channel(1).1)
    }

    async fn stop(&mut self) -> RunnerResult<()> {
        self.prompts = None;
        if let Some(mcp) = &self.context.mcp {
            mcp.unregister(&self.token);
        }
        if let Some(pid) = self.pid.take() {
            // Negative pid: the whole process group.
            let _ = Command::new("kill").arg("--").arg(format!("-{pid}")).status().await;
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        Ok(())
    }
}

async fn connect<T>(
    transport: T,
    mut prompts: mpsc::UnboundedReceiver<Input>,
    events: mpsc::Sender<AgentEvent>,
    context: RunnerContext,
    info: AgentInfo,
    team_url: Option<String>,
) -> Result<(), String>
where
    T: agent_client_protocol::ConnectTo<Client> + 'static,
{
    let titles = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let updates = events.clone();
    let worktree = info.worktree.clone();
    let extra_dirs = context.extra_dirs.clone();
    let wanted_model = info.model.clone();
    Client
        .builder()
        .name("orchestrator")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                for event in translate(notification.update, &titles) {
                    let _ = updates.send(event).await;
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, cx: ConnectionTo<AcpAgent>| {
                let context = context.clone();
                let info = info.clone();
                // Waiting for the operator must not hold the dispatch loop.
                cx.spawn(async move {
                    let (tool, input, paths) = describe(&request.tool_call);
                    let decision = ask_permission(&context, &info, &tool, input, paths).await;
                    responder.respond(RequestPermissionResponse::new(outcome(&request, &decision)))
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<AcpAgent>| {
            let init = cx.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
            let mut session = NewSessionRequest::new(worktree);
            // The group's other projects (ADR 0024), for CLIs that take more roots.
            if !extra_dirs.is_empty() {
                if init.agent_capabilities.session_capabilities.additional_directories.is_some() {
                    session = session.additional_directories(extra_dirs);
                } else {
                    tracing::info!("the agent takes no additional directories: its briefing lists them");
                }
            }
            match team_url {
                Some(url) if init.agent_capabilities.mcp_capabilities.http => {
                    session = session.mcp_servers(vec![McpServer::Http(McpServerHttp::new(crate::mcp::SERVER, url))]);
                }
                Some(_) => tracing::info!("the agent takes no HTTP MCP servers: no team tools"),
                None => {}
            }
            let session = cx.send_request(session).block_task().await?;
            let mut models = ModelOption::of(&session.config_options);
            if let Some(models) = &mut models {
                if let Some(wanted) = wanted_model.as_deref().filter(|w| Some(*w) != models.current.as_deref()) {
                    if models.available.iter().any(|m| m == wanted) {
                        let request =
                            SetSessionConfigOptionRequest::new(session.session_id.clone(), models.id.clone(), wanted);
                        match cx.send_request(request).block_task().await {
                            Ok(_) => models.current = Some(wanted.to_owned()),
                            Err(error) => {
                                let _ =
                                    events.send(AgentEvent::Notice(format!("could not use {wanted}: {error}"))).await;
                            }
                        }
                    } else {
                        let current = models.current.clone().unwrap_or_default();
                        let text = format!("{wanted} is not offered by this CLI; it stays on {current}");
                        let _ = events.send(AgentEvent::Notice(text)).await;
                    }
                }
                let _ = events.send(models.event()).await;
            }
            while let Some(input) = prompts.recv().await {
                let text = match input {
                    Input::Prompt(text) => text,
                    Input::Model(wanted) => {
                        let Some(models) = &mut models else {
                            let _ = events
                                .send(AgentEvent::Notice("this CLI does not let the app pick a model".into()))
                                .await;
                            continue;
                        };
                        let request = SetSessionConfigOptionRequest::new(
                            session.session_id.clone(),
                            models.id.clone(),
                            wanted.as_str(),
                        );
                        match cx.send_request(request).block_task().await {
                            Ok(_) => {
                                models.current = Some(wanted);
                                let _ = events.send(models.event()).await;
                            }
                            Err(error) => {
                                let _ = events
                                    .send(AgentEvent::Notice(format!("could not switch to {wanted}: {error}")))
                                    .await;
                            }
                        }
                        continue;
                    }
                };
                let prompt =
                    PromptRequest::new(session.session_id.clone(), vec![ContentBlock::Text(TextContent::new(text))]);
                let response = cx.send_request(prompt).block_task().await?;
                if let Some(usage) = &response.usage {
                    let usage =
                        crate::model::Usage { input_tokens: usage.input_tokens, output_tokens: usage.output_tokens };
                    let _ = events.send(AgentEvent::Usage(usage)).await;
                }
                let stop_reason = format!("{:?}", response.stop_reason);
                let _ = events.send(AgentEvent::TurnEnd { stop_reason }).await;
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())
}

/// Asks an ACP CLI which models it offers, without a task: `initialize` and
/// `session/new` in `cwd`, read the model option, close. Returns the CLI's default
/// and the list.
pub async fn probe_models(command: &str, cwd: &std::path::Path) -> Result<(Option<String>, Vec<String>), String> {
    let command = super::update::latest_command(command);
    let words = shell_words::split(&command).map_err(|e| e.to_string())?;
    let (program, args) = words.split_first().ok_or("empty command")?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let cwd = cwd.to_path_buf();
    let probe =
        Client.builder().name("orchestrator").connect_with(transport, async move |cx: ConnectionTo<AcpAgent>| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
            let session = cx.send_request(NewSessionRequest::new(cwd)).block_task().await?;
            Ok(ModelOption::of(&session.config_options))
        });
    let found = tokio::time::timeout(std::time::Duration::from_secs(60), probe)
        .await
        .map_err(|_| "the CLI did not answer in a minute".to_owned())?
        .map_err(|e| e.to_string());
    if let Some(pid) = child.id() {
        let _ = Command::new("kill").arg("--").arg(format!("-{pid}")).status().await;
    }
    let models = found?.ok_or("this CLI does not list its models")?;
    Ok((models.current, models.available))
}

/// The session's model option, as the agent offers it (`session/new` config options).
struct ModelOption {
    id: String,
    current: Option<String>,
    available: Vec<String>,
}

impl ModelOption {
    /// The first select option in the `model` category. Read through JSON: the
    /// option lists come grouped or flat, and only the values matter here.
    fn of<T: serde::Serialize>(options: &T) -> Option<Self> {
        let options = serde_json::to_value(options).ok()?;
        let option = options.as_array()?.iter().find(|o| o["category"] == "model")?;
        let mut available = Vec::new();
        collect_values(&option["options"], &mut available);
        Some(Self {
            id: option["id"].as_str()?.to_owned(),
            current: option["currentValue"].as_str().map(str::to_owned),
            available,
        })
    }

    fn event(&self) -> AgentEvent {
        AgentEvent::Models { current: self.current.clone(), available: self.available.clone() }
    }
}

/// Every `value` in a flat or grouped option list.
fn collect_values(options: &Value, out: &mut Vec<String>) {
    for option in options.as_array().into_iter().flatten() {
        match option["value"].as_str() {
            Some(value) => out.push(value.to_owned()),
            None => collect_values(&option["options"], out),
        }
    }
}

fn translate(update: SessionUpdate, titles: &Mutex<HashMap<String, String>>) -> Vec<AgentEvent> {
    let text_of = |block: ContentBlock| match block {
        ContentBlock::Text(text) => Some(text.text),
        _ => None,
    };
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => text_of(chunk.content).map(AgentEvent::Text).into_iter().collect(),
        SessionUpdate::AgentThoughtChunk(chunk) => {
            text_of(chunk.content).map(AgentEvent::Thought).into_iter().collect()
        }
        SessionUpdate::UsageUpdate(usage) => {
            let mut events = vec![AgentEvent::Context { used: usage.used, size: usage.size }];
            if let Some(cost) = usage.cost.filter(|c| c.currency.eq_ignore_ascii_case("usd")) {
                events.push(AgentEvent::Cost(super::mcp_cli::dollars_to_micros(cost.amount)));
            }
            events
        }
        SessionUpdate::ToolCall(call) => {
            titles
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(call.tool_call_id.to_string(), call.title.clone());
            vec![AgentEvent::Tool { title: call.title, status: ToolStatus::Started }]
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let status = match update.fields.status {
                Some(ToolCallStatus::Completed) => ToolStatus::Done,
                Some(ToolCallStatus::Failed) => ToolStatus::Failed,
                _ => return Vec::new(),
            };
            let id = update.tool_call_id.to_string();
            let title = update
                .fields
                .title
                .or_else(|| titles.lock().unwrap_or_else(PoisonError::into_inner).remove(&id))
                .unwrap_or_else(|| "tool".into());
            vec![AgentEvent::Tool { title, status }]
        }
        _ => Vec::new(),
    }
}

/// Tool name, input and touched paths of an ACP tool call, in broker terms.
fn describe(call: &ToolCallUpdate) -> (String, Value, Vec<PathBuf>) {
    let fields = &call.fields;
    let tool = match fields.kind {
        Some(ToolKind::Read) => "Read",
        Some(ToolKind::Edit) => "Edit",
        Some(ToolKind::Delete) => "Delete",
        Some(ToolKind::Move) => "Move",
        Some(ToolKind::Search) => "Search",
        Some(ToolKind::Execute) => "Bash",
        Some(ToolKind::Think) => "Think",
        Some(ToolKind::Fetch) => "WebFetch",
        _ => "Tool",
    };
    let mut input = fields.raw_input.clone().unwrap_or_else(|| json!({}));
    let known = ["command", "file_path", "path", "url"].iter().any(|k| input.get(*k).is_some());
    if let (false, Some(title), Some(object)) = (known, &fields.title, input.as_object_mut()) {
        object.insert("title".into(), json!(title));
    }
    let mut paths: Vec<PathBuf> = fields.locations.iter().flatten().map(|l| l.path.clone()).collect();
    for key in ["file_path", "path", "filePath"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            paths.push(PathBuf::from(path));
        }
    }
    (tool.to_owned(), input, paths)
}

/// The broker keeps "always" rules itself, so agents only ever get one-time
/// answers and keep asking (ADR 0005).
fn outcome(request: &RequestPermissionRequest, decision: &Decision) -> RequestPermissionOutcome {
    let wanted: &[PermissionOptionKind] = if decision.allows() {
        &[PermissionOptionKind::AllowOnce, PermissionOptionKind::AllowAlways]
    } else {
        &[PermissionOptionKind::RejectOnce, PermissionOptionKind::RejectAlways]
    };
    wanted
        .iter()
        .find_map(|kind| request.options.iter().find(|o| o.kind == *kind))
        .map_or(RequestPermissionOutcome::Cancelled, |option| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option.option_id.clone()))
        })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{broker, model::AgentState};

    #[test]
    fn reads_the_model_option_flat_or_grouped() {
        let flat = json!([
            { "id": "mode", "category": "mode", "currentValue": "build", "options": [{ "value": "build" }] },
            { "id": "model", "category": "model", "currentValue": "opencode/big-pickle",
              "options": [{ "value": "opencode/big-pickle" }, { "value": "opencode/beam-free" }] }
        ]);
        let models = ModelOption::of(&flat).unwrap();
        assert_eq!(models.id, "model");
        assert_eq!(models.current.as_deref(), Some("opencode/big-pickle"));
        assert_eq!(models.available, ["opencode/big-pickle", "opencode/beam-free"]);

        let grouped = json!([{ "id": "m", "category": "model", "currentValue": "gpt-6",
            "options": [{ "group": "openai", "options": [{ "value": "gpt-6" }, { "value": "gpt-6-astra" }] }] }]);
        assert_eq!(ModelOption::of(&grouped).unwrap().available, ["gpt-6", "gpt-6-astra"]);
        assert!(ModelOption::of(&json!([])).is_none());
    }

    /// Every edit goes through the broker, even with CLIs that allow edits by default.
    #[tokio::test]
    #[ignore = "needs a real ACP CLI"]
    async fn acp_edit_asks_the_broker() {
        let command = std::env::var("ORCH_ACP_COMMAND").unwrap_or_else(|_| "opencode acp".into());
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut broker_events) = mpsc::unbounded_channel();
        let broker = broker::spawn(Duration::from_secs(60), tx, None);
        let approver = broker.clone();
        let asked = tokio::spawn(async move {
            while let Some(event) = broker_events.recv().await {
                if let broker::BrokerEvent::Queued(request) = event {
                    approver.answer(Some(request.id), broker::Answer::Allow, broker::DecisionSource::Click);
                    return Some(request.tool);
                }
            }
            None
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
            provider: Provider::CliAcp { command },
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
        let mut runner = AcpRunner::new(context);
        runner.start(&agent, "").await.unwrap();
        let mut events = runner.events();
        runner.send("Create the file note.txt containing the word ok. Nothing else.".into()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(180), async {
            while let Some(event) = events.recv().await {
                if matches!(event, AgentEvent::TurnEnd { .. } | AgentEvent::Error(_) | AgentEvent::Exited) {
                    break;
                }
            }
        })
        .await
        .expect("turn ended");
        runner.stop().await.unwrap();
        let tool =
            tokio::time::timeout(Duration::from_secs(1), asked).await.ok().and_then(std::result::Result::ok).flatten();
        assert_eq!(tool.as_deref(), Some("Edit"), "the edit never reached the broker");
        assert!(dir.path().join("note.txt").exists());
    }

    /// Talks to a real ACP CLI: `ORCH_ACP_COMMAND="opencode acp" cargo test -p core acp_smoke -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a real ACP CLI"]
    async fn acp_smoke() {
        let command = std::env::var("ORCH_ACP_COMMAND").unwrap_or_else(|_| "opencode acp".into());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let context = RunnerContext {
            broker: broker::spawn(Duration::from_secs(5), tx, None),
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
            provider: Provider::CliAcp { command },
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
        let mut runner = AcpRunner::new(context);
        runner.start(&agent, "").await.unwrap();
        let mut events = runner.events();
        runner.send("Reply with exactly the word PONG and nothing else. Do not use tools.".into()).await.unwrap();
        let mut text = String::new();
        let outcome = tokio::time::timeout(Duration::from_secs(120), async {
            while let Some(event) = events.recv().await {
                eprintln!("{event:?}");
                match event {
                    AgentEvent::Text(chunk) => text.push_str(&chunk),
                    AgentEvent::TurnEnd { .. } | AgentEvent::Error(_) | AgentEvent::Exited => break,
                    _ => {}
                }
            }
        })
        .await;
        runner.stop().await.unwrap();
        assert!(outcome.is_ok(), "no answer in time");
        assert!(text.to_uppercase().contains("PONG"), "got {text:?}");
    }

    /// The team tools reach a real CLI through the app's MCP server:
    /// `cargo test -p core acp_team_tools -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs a real ACP CLI with HTTP MCP support"]
    async fn acp_team_tools() {
        let command = std::env::var("ORCH_ACP_COMMAND").unwrap_or_else(|_| "opencode acp".into());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (team_tx, mut team_rx) = mpsc::unbounded_channel();
        let asked = tokio::spawn(async move {
            while let Some(request) = team_rx.recv().await {
                match request {
                    crate::team::TeamRequest::Roster { reply, .. } => {
                        let _ = reply.send("- Backend (group API, Running): owns the /pix endpoint".into());
                    }
                    crate::team::TeamRequest::Remember { reply, .. } => {
                        let _ = reply.send(Ok("noted".into()));
                    }
                    crate::team::TeamRequest::Ask { to, message, reply, .. } => {
                        let _ = reply.send(Ok("POST /pix answers 201 with the charge id ZX-4471".into()));
                        return Some((to, message));
                    }
                }
            }
            None
        });
        let context = RunnerContext {
            broker: broker::spawn(Duration::from_secs(60), tx, None),
            group_id: uuid::Uuid::new_v4(),
            group_name: "Smoke".into(),
            sandbox: crate::sandbox::available(),
            mcp: Some(crate::mcp::spawn().await.unwrap()),
            team: Some(crate::team::TeamHandle::new(team_tx)),
            extra_dirs: Vec::new(),
        };
        let agent = Agent {
            id: uuid::Uuid::new_v4(),
            group_id: context.group_id,
            name: "Front".into(),
            task: String::new(),
            provider: Provider::CliAcp { command },
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
        let mut runner = AcpRunner::new(context);
        runner.start(&agent, "").await.unwrap();
        let mut events = runner.events();
        let prompt = "Use the ask_agent tool to ask the agent named Backend what POST /pix returns. \
                      Then reply with the charge id it gave you and nothing else.";
        runner.send(prompt.into()).await.unwrap();
        let mut text = String::new();
        let outcome = tokio::time::timeout(Duration::from_secs(240), async {
            while let Some(event) = events.recv().await {
                eprintln!("{event:?}");
                match event {
                    AgentEvent::Text(chunk) => text.push_str(&chunk),
                    AgentEvent::TurnEnd { .. } | AgentEvent::Error(_) | AgentEvent::Exited => break,
                    _ => {}
                }
            }
        })
        .await;
        runner.stop().await.unwrap();
        assert!(outcome.is_ok(), "no answer in time");
        let asked =
            tokio::time::timeout(Duration::from_secs(1), asked).await.ok().and_then(std::result::Result::ok).flatten();
        let (to, _) = asked.expect("the agent never called ask_agent");
        assert!(to.eq_ignore_ascii_case("backend"), "asked {to}");
        assert!(text.contains("ZX-4471"), "got {text:?}");
    }

    /// The chosen model reaches a real CLI's session, and it can switch again:
    /// `cargo test -p core acp_model -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs a real ACP CLI that offers models"]
    async fn acp_model_is_chosen_and_switched() {
        let command = std::env::var("ORCH_ACP_COMMAND").unwrap_or_else(|_| "opencode acp".into());
        let wanted = std::env::var("ORCH_ACP_MODEL").unwrap_or_else(|_| "opencode/beam-free".into());
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let context = RunnerContext {
            broker: broker::spawn(Duration::from_secs(5), tx, None),
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
            provider: Provider::CliAcp { command },
            worktree: dir.path().into(),
            state: AgentState::Stopped,
            rules: Vec::new(),
            started_at: None,
            usage: Default::default(),
            persona: String::new(),
            model: Some(wanted.clone()),
            models: Vec::new(),
            color: None,
            context: None,
            cost_micros: 0,
            profile_id: None,
        };
        let mut runner = AcpRunner::new(context);
        runner.start(&agent, "").await.unwrap();
        let mut events = runner.events();
        let (current, available) =
            tokio::time::timeout(Duration::from_secs(60), next_models(&mut events)).await.unwrap();
        eprintln!("offers {available:?}");
        assert_eq!(current.as_deref(), Some(wanted.as_str()));
        let other = available.iter().find(|m| **m != wanted).cloned().expect("a second model");
        runner.set_model(&other).await.unwrap();
        let (current, _) = tokio::time::timeout(Duration::from_secs(30), next_models(&mut events)).await.unwrap();
        runner.stop().await.unwrap();
        assert_eq!(current.as_deref(), Some(other.as_str()));
    }

    /// `cargo test -p core acp_probe -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs a real ACP CLI that offers models"]
    async fn acp_probe_lists_models_without_a_task() {
        let command = std::env::var("ORCH_ACP_COMMAND").unwrap_or_else(|_| "opencode acp".into());
        let dir = tempfile::tempdir().unwrap();
        let (current, available) = probe_models(&command, dir.path()).await.unwrap();
        eprintln!("default {current:?}, offers {available:?}");
        assert!(!available.is_empty());
    }

    async fn next_models(events: &mut mpsc::Receiver<AgentEvent>) -> (Option<String>, Vec<String>) {
        loop {
            match events.recv().await {
                Some(AgentEvent::Models { current, available }) => return (current, available),
                Some(other) => eprintln!("{other:?}"),
                None => panic!("the session ended"),
            }
        }
    }
}
