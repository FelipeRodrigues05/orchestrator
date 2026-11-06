//! `CodexRunner`: Codex through its own app-server protocol (ADR 0025), JSON-RPC
//! over stdio (`codex app-server`). It runs the installed Codex itself, so new
//! models work as soon as the CLI knows them; the ACP adapter bundles an older Codex.
//! Commands and edits wait for the broker: with its sandbox read-only, Codex asks
//! the client before each one.
//! Protocol parsing is Codex-specific and stays here (ADR 0004).

use std::{collections::HashMap, path::Path, process::Stdio};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdout, Command},
    sync::mpsc,
    task::JoinHandle,
};
use uuid::Uuid;

use super::{AgentEvent, AgentInfo, AgentRunner, RunnerContext, RunnerResult, ToolStatus, ask_permission};
use crate::model::{Agent, Decision, Provider, Usage};

/// Request ids of the start-up calls; turns use ids from `FIRST_TURN` on.
const INITIALIZE: u64 = 1;
const THREAD: u64 = 2;
const MODELS: u64 = 3;
const FIRST_TURN: u64 = 100;

pub struct CodexRunner {
    context: RunnerContext,
    events_tx: mpsc::Sender<AgentEvent>,
    events_rx: Option<mpsc::Receiver<AgentEvent>>,
    input: Option<mpsc::UnboundedSender<Input>>,
    task: Option<JoinHandle<()>>,
    pid: Option<u32>,
    token: String,
}

enum Input {
    Prompt(String),
    Model(String),
}

impl CodexRunner {
    pub fn new(context: RunnerContext) -> Self {
        let (events_tx, events_rx) = mpsc::channel(256);
        Self {
            context,
            events_tx,
            events_rx: Some(events_rx),
            input: None,
            task: None,
            pid: None,
            token: Uuid::new_v4().simple().to_string(),
        }
    }
}

/// `codex` → `codex app-server`, in its own process group; inside the app's
/// sandbox when `sandbox` holds the folders it may write.
fn spawn(command: &str, cwd: &Path, sandbox: Option<&[std::path::PathBuf]>) -> RunnerResult<tokio::process::Child> {
    let mut words = shell_words::split(command).map_err(|e| e.to_string())?;
    words.push("app-server".into());
    let (program, args) = words.split_first().ok_or("empty command")?;
    let (program, args) = match sandbox {
        Some(extra) if crate::sandbox::available() => crate::sandbox::wrap(program, args, cwd, extra),
        _ => (program.clone(), args.to_vec()),
    };
    Command::new(&program)
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))
}

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": INITIALIZE, "method": "initialize", "params": {
        "clientInfo": { "name": "orchestrator", "title": "Orchestrator", "version": env!("CARGO_PKG_VERSION") },
        "capabilities": null
    } })
}

/// The models Codex offers (hidden ones left out) and its default.
fn models_of(result: &Value) -> (Option<String>, Vec<String>) {
    let data = result["data"].as_array().into_iter().flatten();
    let visible: Vec<&Value> = data.filter(|m| !m["hidden"].as_bool().unwrap_or(false)).collect();
    let default = visible.iter().find(|m| m["isDefault"] == true).and_then(|m| m["id"].as_str()).map(str::to_owned);
    (default, visible.iter().filter_map(|m| m["id"].as_str()).map(str::to_owned).collect())
}

/// Asks Codex which models it offers, without starting a session.
pub async fn probe_models(command: &str, cwd: &Path) -> Result<(Option<String>, Vec<String>), String> {
    let mut child = spawn(command, cwd, None)?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let list = json!({ "jsonrpc": "2.0", "id": MODELS, "method": "model/list", "params": {} });
    let lines = format!("{}\n{}\n{list}\n", initialize(), json!({ "jsonrpc": "2.0", "method": "initialized" }));
    stdin.write_all(lines.as_bytes()).await.map_err(|e| e.to_string())?;
    let read = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            if message["id"] == MODELS {
                return match message.get("error") {
                    Some(error) => Err(error["message"].as_str().unwrap_or("model/list failed").to_owned()),
                    None => Ok(models_of(&message["result"])),
                };
            }
        }
        Err("codex closed before listing its models".to_owned())
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), read)
        .await
        .unwrap_or_else(|_| Err("codex did not list its models in 30 s".into()));
    let _ = child.kill().await;
    result
}

#[async_trait]
impl AgentRunner for CodexRunner {
    async fn start(&mut self, agent: &Agent, _mcp_url: &str) -> RunnerResult<()> {
        let Provider::CliCodex { command } = &agent.provider else {
            return Err("CodexRunner needs a CliCodex provider".into());
        };
        let sandbox = self.context.sandbox.then_some(self.context.extra_dirs.as_slice());
        let mut child = spawn(command, &agent.worktree, sandbox)?;
        self.pid = child.id();
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;

        // One writer: replies to approvals come from their own tasks.
        let (out, mut lines) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            while let Some(message) = lines.recv().await {
                if stdin.write_all(format!("{message}\n").as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        let info = AgentInfo::of(agent);
        let team_url = match (&self.context.mcp, &self.context.team) {
            (Some(mcp), Some(_)) => Some(mcp.register(&self.token, self.context.clone(), info.clone())),
            _ => None,
        };
        let _ = out.send(initialize());
        let (input, input_rx) = mpsc::unbounded_channel();
        self.input = Some(input);
        let session = Session {
            context: self.context.clone(),
            info,
            out,
            events: self.events_tx.clone(),
            thread: None,
            thread_params: thread_params(agent, team_url.as_deref()),
            model: agent.model.clone(),
            switch_to: None,
            models: Vec::new(),
            queued: Vec::new(),
            turn_active: false,
            turn_failed: false,
            next_id: FIRST_TURN,
            items: HashMap::new(),
        };
        let events = self.events_tx.clone();
        self.task = Some(tokio::spawn(async move {
            session.run(stdout, input_rx).await;
            let _ = child.kill().await;
            let _ = events.send(AgentEvent::Exited).await;
        }));
        Ok(())
    }

    async fn send(&mut self, text: String) -> RunnerResult<()> {
        let input = self.input.as_ref().ok_or("agent not started")?;
        input.send(Input::Prompt(text)).map_err(|_| "agent is not running".to_owned())
    }

    async fn set_model(&mut self, model: &str) -> RunnerResult<()> {
        let input = self.input.as_ref().ok_or("agent not started")?;
        input.send(Input::Model(model.to_owned())).map_err(|_| "agent is not running".to_owned())
    }

    fn events(&mut self) -> mpsc::Receiver<AgentEvent> {
        self.events_rx.take().unwrap_or_else(|| mpsc::channel(1).1)
    }

    async fn stop(&mut self) -> RunnerResult<()> {
        self.input = None;
        if let Some(mcp) = &self.context.mcp {
            mcp.unregister(&self.token);
        }
        if let Some(pid) = self.pid.take() {
            let _ = Command::new("kill").arg("--").arg(format!("-{pid}")).status().await;
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        Ok(())
    }
}

/// `thread/start`: the worktree, and the team tools as an MCP server. Codex's own
/// sandbox is read-only so every write, edit or command asks first (ADR 0005); what
/// the operator allows runs inside the app's sandbox, if it is on.
fn thread_params(agent: &Agent, team_url: Option<&str>) -> Value {
    let mut params = json!({
        "cwd": agent.worktree,
        "approvalPolicy": "untrusted",
        "sandbox": "read-only",
    });
    if let Some(url) = team_url {
        params["config"] = json!({ "mcp_servers": { crate::mcp::SERVER: { "url": url } } });
    }
    if let Some(model) = &agent.model {
        params["model"] = json!(model);
    }
    params
}

/// A command, file change or tool call between its start and end.
struct Item {
    title: String,
    paths: Vec<std::path::PathBuf>,
}

struct Session {
    context: RunnerContext,
    info: AgentInfo,
    out: mpsc::UnboundedSender<Value>,
    events: mpsc::Sender<AgentEvent>,
    thread: Option<String>,
    thread_params: Value,
    /// The model in use, and one chosen for the next turn.
    model: Option<String>,
    switch_to: Option<String>,
    models: Vec<String>,
    /// Messages that arrived before the thread or during a turn.
    queued: Vec<String>,
    turn_active: bool,
    /// An error was already reported for this turn.
    turn_failed: bool,
    next_id: u64,
    items: HashMap<String, Item>,
}

impl Session {
    async fn run(mut self, stdout: ChildStdout, mut input: mpsc::UnboundedReceiver<Input>) {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Ok(message) = serde_json::from_str::<Value>(&line) {
                            self.handle(message).await;
                        }
                    }
                    _ => break,
                },
                command = input.recv() => match command {
                    Some(Input::Prompt(text)) => {
                        self.queued.push(text);
                        self.next_turn();
                    }
                    Some(Input::Model(model)) => {
                        self.switch_to = Some(model.clone());
                        self.model = Some(model);
                        self.emit(AgentEvent::Models { current: self.model.clone(), available: self.models.clone() })
                            .await;
                    }
                    None => break,
                },
            }
        }
    }

    async fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event).await;
    }

    fn request(&mut self, id: u64, method: &str, params: Value) {
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        message["params"] = params;
        let _ = self.out.send(message);
    }

    /// Sends the queued messages as one turn when the thread is free.
    fn next_turn(&mut self) {
        let Some(thread) = self.thread.clone() else { return };
        if self.turn_active || self.queued.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.queued).join("\n\n");
        let mut params =
            json!({ "threadId": thread, "input": [{ "type": "text", "text": text, "text_elements": [] }] });
        if let Some(model) = self.switch_to.take() {
            params["model"] = json!(model);
        }
        self.turn_active = true;
        self.turn_failed = false;
        let id = self.next_id;
        self.next_id += 1;
        self.request(id, "turn/start", params);
    }

    async fn handle(&mut self, message: Value) {
        let method = message["method"].as_str().map(str::to_owned);
        match (message.get("id"), method) {
            // A request from Codex: approvals.
            (Some(id), Some(method)) => self.answer(id.clone(), &method, &message["params"]),
            (Some(id), None) => self.response(id.as_u64().unwrap_or(0), &message).await,
            (None, Some(method)) => self.notification(&method, &message["params"]).await,
            (None, None) => {}
        }
    }

    async fn response(&mut self, id: u64, message: &Value) {
        if let Some(error) = message.get("error") {
            let text = error["message"].as_str().unwrap_or("codex refused the request").to_owned();
            if id >= FIRST_TURN {
                self.turn_active = false;
                self.emit(AgentEvent::Error(text)).await;
                self.emit(AgentEvent::TurnEnd { stop_reason: "error".into() }).await;
            } else if id != MODELS {
                self.emit(AgentEvent::Error(format!("codex: {text}"))).await;
            }
            return;
        }
        let result = &message["result"];
        match id {
            INITIALIZE => {
                let _ = self.out.send(json!({ "jsonrpc": "2.0", "method": "initialized" }));
                let params = self.thread_params.take();
                self.request(THREAD, "thread/start", params);
                self.request(MODELS, "model/list", json!({}));
            }
            THREAD => {
                self.thread = result["thread"]["id"].as_str().map(str::to_owned);
                self.model = result["model"].as_str().map(str::to_owned).or(self.model.take());
                if !self.models.is_empty() {
                    self.emit(AgentEvent::Models { current: self.model.clone(), available: self.models.clone() }).await;
                }
                self.next_turn();
            }
            MODELS => {
                self.models = models_of(result).1;
                if self.thread.is_some() {
                    self.emit(AgentEvent::Models { current: self.model.clone(), available: self.models.clone() }).await;
                }
            }
            _ => {}
        }
    }

    async fn notification(&mut self, method: &str, params: &Value) {
        match method {
            "item/started" => {
                let item = &params["item"];
                if let Some((id, title, paths)) = describe_item(item) {
                    self.items.insert(id, Item { title: title.clone(), paths });
                    self.emit(AgentEvent::Tool { title, status: ToolStatus::Started }).await;
                }
            }
            "item/completed" => {
                let item = &params["item"];
                for event in completed(item, &mut self.items) {
                    self.emit(event).await;
                }
            }
            "thread/tokenUsage/updated" => {
                let usage = &params["tokenUsage"];
                let last = &usage["last"];
                let input = last["inputTokens"].as_u64().unwrap_or(0);
                self.emit(AgentEvent::Usage(Usage {
                    input_tokens: input,
                    output_tokens: last["outputTokens"].as_u64().unwrap_or(0),
                }))
                .await;
                let size = usage["modelContextWindow"].as_u64().unwrap_or(0);
                self.emit(AgentEvent::Context { used: input, size }).await;
            }
            "error" if !params["willRetry"].as_bool().unwrap_or(false) => {
                self.turn_failed = true;
                self.emit(AgentEvent::Error(error_text(&params["error"]))).await;
            }
            "turn/completed" => {
                let turn = &params["turn"];
                if !self.turn_failed && turn["error"].is_object() {
                    self.emit(AgentEvent::Error(error_text(&turn["error"]))).await;
                }
                let stop_reason = turn["status"].as_str().unwrap_or("completed").to_owned();
                self.turn_active = false;
                self.emit(AgentEvent::TurnEnd { stop_reason }).await;
                self.next_turn();
            }
            "model/rerouted" => {
                let to = params["toModel"].as_str().or_else(|| params["model"].as_str()).unwrap_or("another model");
                self.emit(AgentEvent::Notice(format!("codex moved this turn to {to}"))).await;
            }
            _ => {}
        }
    }

    /// Approval requests go to the broker on their own task, so the session keeps
    /// reading; anything else Codex asks is declined.
    fn answer(&mut self, id: Value, method: &str, params: &Value) {
        let (tool, input, paths) = match method {
            "item/commandExecution/requestApproval" => command_request(params),
            "item/fileChange/requestApproval" => {
                let item = params["itemId"].as_str().and_then(|i| self.items.get(i));
                let paths = item.map(|i| i.paths.clone()).unwrap_or_default();
                let shown: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                ("edit".to_owned(), json!({ "path": shown.join(", "), "reason": params["reason"] }), paths)
            }
            _ => {
                tracing::info!("codex asked {method}: declined");
                let _ = self.out.send(json!({ "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": "the orchestrator does not support this request" } }));
                return;
            }
        };
        let (context, info, out) = (self.context.clone(), self.info.clone(), self.out.clone());
        tokio::spawn(async move {
            let decision = ask_permission(&context, &info, &tool, input, paths).await;
            let answer = match decision {
                Decision::Allow | Decision::AlwaysAllow(_) => "accept",
                Decision::Deny { .. } => "decline",
            };
            let _ = out.send(json!({ "jsonrpc": "2.0", "id": id, "result": { "decision": answer } }));
        });
    }
}

/// A command as the broker sees it: reads, listings and searches count as `read`.
fn command_request(params: &Value) -> (String, Value, Vec<std::path::PathBuf>) {
    let actions = params["commandActions"].as_array().cloned().unwrap_or_default();
    let shown = shown_command(params["command"].as_str().unwrap_or_default(), &actions);
    let reads = !actions.is_empty()
        && actions.iter().all(|a| matches!(a["type"].as_str(), Some("read" | "listFiles" | "search")));
    let paths = actions.iter().filter_map(|a| a["path"].as_str()).map(std::path::PathBuf::from).collect();
    let tool = if reads { "read" } else { "bash" };
    (tool.to_owned(), json!({ "command": shown, "reason": params["reason"] }), paths)
}

/// `/usr/bin/bash -lc 'ls -la'` → `ls -la`, from Codex's parsed actions.
fn shown_command(command: &str, actions: &[Value]) -> String {
    let parts: Vec<&str> = actions.iter().filter_map(|a| a["command"].as_str()).collect();
    if parts.is_empty() { command.to_owned() } else { parts.join(" && ") }
}

/// The tool line for an item that starts: `(id, title, paths)`.
fn describe_item(item: &Value) -> Option<(String, String, Vec<std::path::PathBuf>)> {
    let id = item["id"].as_str()?.to_owned();
    match item["type"].as_str()? {
        "commandExecution" => {
            let actions = item["commandActions"].as_array().cloned().unwrap_or_default();
            let command = shown_command(item["command"].as_str().unwrap_or_default(), &actions);
            Some((id, format!("bash {command}"), Vec::new()))
        }
        "fileChange" => {
            let paths: Vec<std::path::PathBuf> = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["path"].as_str())
                .map(std::path::PathBuf::from)
                .collect();
            let shown: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            Some((id, format!("edit {}", shown.join(", ")), paths))
        }
        "mcpToolCall" => Some((id, item["tool"].as_str().unwrap_or("tool").to_owned(), Vec::new())),
        "webSearch" => Some((id, "web search".to_owned(), Vec::new())),
        _ => None,
    }
}

/// Events for an item that ended: the agent's text, its reasoning, or a tool's end.
fn completed(item: &Value, items: &mut HashMap<String, Item>) -> Vec<AgentEvent> {
    match item["type"].as_str() {
        Some("agentMessage") => {
            let text = item["text"].as_str().unwrap_or_default().trim();
            if text.is_empty() { Vec::new() } else { vec![AgentEvent::Text(format!("{text}\n"))] }
        }
        Some("reasoning") => {
            let summary: Vec<&str> =
                item["summary"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if summary.is_empty() { Vec::new() } else { vec![AgentEvent::Thought(summary.join("\n"))] }
        }
        _ => {
            let Some(started) = item["id"].as_str().and_then(|id| items.remove(id)) else { return Vec::new() };
            let ok = matches!(item["status"].as_str(), Some("completed" | "success")) || item["status"].is_null();
            vec![AgentEvent::Tool {
                title: started.title,
                status: if ok { ToolStatus::Done } else { ToolStatus::Failed },
            }]
        }
    }
}

/// Codex's error, worded so the model router recognises limits and unknown models.
fn error_text(error: &Value) -> String {
    let message = error["message"].as_str().unwrap_or("the turn failed");
    let details = error["additionalDetails"].as_str().filter(|d| !d.is_empty());
    let text = details.map_or_else(|| message.to_owned(), |d| format!("{message} ({d})"));
    match error["codexErrorInfo"].as_str() {
        Some("usageLimitExceeded" | "rateLimitExceeded" | "sessionBudgetExceeded") => format!("usage limit: {text}"),
        Some("modelNotFound") => format!("unknown model: {text}"),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_become_tool_lines_and_text() {
        let mut items = HashMap::new();
        let command = json!({ "type": "commandExecution", "id": "c1", "command": "/usr/bin/bash -lc 'ls -la'",
            "commandActions": [{ "type": "listFiles", "command": "ls -la", "path": null }], "status": "inProgress" });
        let (id, title, _) = describe_item(&command).unwrap();
        assert_eq!(title, "bash ls -la");
        items.insert(id, Item { title, paths: Vec::new() });
        let done = json!({ "type": "commandExecution", "id": "c1", "status": "completed", "exitCode": 0 });
        assert_eq!(
            completed(&done, &mut items),
            vec![AgentEvent::Tool { title: "bash ls -la".into(), status: ToolStatus::Done }]
        );
        let reply = json!({ "type": "agentMessage", "id": "m1", "text": "Done. " });
        assert_eq!(completed(&reply, &mut items), vec![AgentEvent::Text("Done.\n".into())]);
    }

    #[test]
    fn reads_are_reads_and_writes_are_shell() {
        let read = json!({ "command": "bash -lc 'cat a'", "commandActions": [{ "type": "read", "command": "cat a", "path": "a" }] });
        assert_eq!(command_request(&read).0, "read");
        let write =
            json!({ "command": "bash -lc 'rm a'", "commandActions": [{ "type": "unknown", "command": "rm a" }] });
        let (tool, input, _) = command_request(&write);
        assert_eq!((tool.as_str(), input["command"].as_str()), ("bash", Some("rm a")));
    }

    #[test]
    fn limits_and_unknown_models_reach_the_router() {
        let limit = json!({ "message": "You've hit your limit", "codexErrorInfo": "usageLimitExceeded" });
        assert!(crate::router::is_limit(&error_text(&limit)));
        let model = json!({ "message": "The 'x' model is gone", "codexErrorInfo": "modelNotFound" });
        assert!(crate::router::is_unsupported_model(&error_text(&model)));
    }

    #[test]
    fn hidden_models_stay_hidden() {
        let list = json!({ "data": [
            { "id": "gpt-6.1-sol", "hidden": false, "isDefault": true },
            { "id": "internal", "hidden": true, "isDefault": false },
            { "id": "gpt-6-astra", "hidden": false, "isDefault": false }
        ] });
        assert_eq!(models_of(&list), (Some("gpt-6.1-sol".into()), vec!["gpt-6.1-sol".into(), "gpt-6-astra".into()]));
    }
}

#[cfg(test)]
mod smoke {
    use std::time::Duration;

    use super::*;
    use crate::{broker, model::AgentState};

    /// The installed Codex, asking the broker before it writes:
    /// `cargo test -p core codex_smoke -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs Codex and an account"]
    async fn codex_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut broker_events) = mpsc::unbounded_channel();
        let broker = broker::spawn(Duration::from_secs(120), tx, None);
        let approver = broker.clone();
        tokio::spawn(async move {
            while let Some(event) = broker_events.recv().await {
                if let broker::BrokerEvent::Queued(request) = event {
                    approver.answer(Some(request.id), broker::Answer::Allow, broker::DecisionSource::Click);
                }
            }
        });
        let context = RunnerContext {
            broker,
            group_id: Uuid::new_v4(),
            group_name: "smoke".into(),
            sandbox: true,
            mcp: None,
            team: None,
            extra_dirs: Vec::new(),
        };
        let agent = Agent {
            id: Uuid::new_v4(),
            group_id: Uuid::new_v4(),
            name: "Smoke".into(),
            task: String::new(),
            persona: String::new(),
            model: None,
            models: Vec::new(),
            color: None,
            context: None,
            cost_micros: 0,
            profile_id: None,
            provider: Provider::CliCodex { command: "codex".into() },
            worktree: dir.path().into(),
            state: AgentState::Stopped,
            rules: Vec::new(),
            started_at: None,
            usage: Usage::default(),
        };
        let mut runner = CodexRunner::new(context);
        runner.start(&agent, "").await.unwrap();
        let mut events = runner.events();
        runner.send("Create hello.txt containing hi, then say done.".into()).await.unwrap();
        let mut seen = Vec::new();
        while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(180), events.recv()).await {
            println!("{event:?}");
            let end = matches!(event, AgentEvent::TurnEnd { .. });
            seen.push(event);
            if end {
                break;
            }
        }
        runner.stop().await.unwrap();
        assert!(seen.iter().any(|e| matches!(e, AgentEvent::Models { .. })), "{seen:?}");
        assert_eq!(std::fs::read_to_string(dir.path().join("hello.txt")).unwrap().trim(), "hi");
    }
}
