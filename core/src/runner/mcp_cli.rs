//! `McpCliRunner`: CLIs without ACP that ask permission through an MCP tool. Today
//! that is Claude Code in print mode with stream-json in and out:
//! `claude -p --input-format stream-json --output-format stream-json
//!  --permission-prompt-tool mcp__orch__permission --mcp-config …`.
//! Output parsing is CLI-specific and stays here (ADR 0004).

use std::{collections::HashMap, process::Stdio};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::mpsc,
    task::JoinHandle,
};
use uuid::Uuid;

use super::{AgentEvent, AgentInfo, AgentRunner, RunnerContext, RunnerResult, ToolStatus};
use crate::{
    mcp,
    model::{Agent, Provider, Usage},
};

pub struct McpCliRunner {
    context: RunnerContext,
    events_tx: mpsc::Sender<AgentEvent>,
    events_rx: Option<mpsc::Receiver<AgentEvent>>,
    stdin: Option<ChildStdin>,
    task: Option<JoinHandle<()>>,
    pid: Option<u32>,
    token: String,
}

impl McpCliRunner {
    pub fn new(context: RunnerContext) -> Self {
        let (events_tx, events_rx) = mpsc::channel(256);
        Self {
            context,
            events_tx,
            events_rx: Some(events_rx),
            stdin: None,
            task: None,
            pid: None,
            token: Uuid::new_v4().simple().to_string(),
        }
    }
}

/// The arguments that make Claude Code ask our MCP tool before acting.
/// Aliases Claude Code accepts for `--model`; any full model id works too.
pub const CLAUDE_MODELS: [&str; 3] = ["opus", "sonnet", "haiku"];

pub fn claude_args(mcp_url: &str) -> Vec<String> {
    let config = json!({ "mcpServers": { mcp::SERVER: { "type": "http", "url": mcp_url } } });
    [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        // Ask for everything not covered by a rule; the default may decide on its own.
        "--permission-mode",
        "manual",
        "--permission-prompts",
        "host",
        "--permission-prompt-tool",
        &format!("mcp__{}__{}", mcp::SERVER, mcp::TOOL),
        "--mcp-config",
        &config.to_string(),
        "--strict-mcp-config",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

#[async_trait]
impl AgentRunner for McpCliRunner {
    async fn start(&mut self, agent: &Agent, _mcp_url: &str) -> RunnerResult<()> {
        let Provider::CliMcp { command } = &agent.provider else {
            return Err("McpCliRunner needs a CliMcp provider".into());
        };
        let mcp = self.context.mcp.clone().ok_or("the permission MCP server is not running")?;
        let url = mcp.register(&self.token, self.context.clone(), AgentInfo::of(agent));

        let words = shell_words::split(command).map_err(|e| e.to_string())?;
        let (program, extra) = words.split_first().ok_or("empty command")?;
        let mut args = extra.to_vec();
        args.extend(claude_args(&url));
        if let Some(model) = &agent.model {
            args.extend(["--model".to_owned(), model.clone()]);
        }
        // Claude Code takes model aliases and full ids; these are the aliases.
        let current = agent.model.clone();
        let available = CLAUDE_MODELS.iter().map(|m| (*m).to_owned()).collect();
        let _ = self.events_tx.send(AgentEvent::Models { current, available }).await;
        let (program, args) = if self.context.sandbox && crate::sandbox::available() {
            crate::sandbox::wrap(program, &args, &agent.worktree)
        } else {
            (program.clone(), args)
        };
        let mut child = Command::new(&program)
            .args(&args)
            .current_dir(&agent.worktree)
            // A nested Claude Code would think it runs inside the parent session.
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("could not start {program}: {e}"))?;
        self.pid = child.id();
        self.stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            let name = agent.name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(agent = %name, "stderr: {line}");
                }
            });
        }

        let events = self.events_tx.clone();
        self.task = Some(tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut titles = HashMap::new();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                for event in translate(&message, &mut titles) {
                    let _ = events.send(event).await;
                }
            }
            let _ = child.wait().await;
            let _ = events.send(AgentEvent::Exited).await;
        }));
        Ok(())
    }

    async fn send(&mut self, text: String) -> RunnerResult<()> {
        let stdin = self.stdin.as_mut().ok_or("agent not started")?;
        let line =
            json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } });
        stdin.write_all(format!("{line}\n").as_bytes()).await.map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    fn events(&mut self) -> mpsc::Receiver<AgentEvent> {
        self.events_rx.take().unwrap_or_else(|| mpsc::channel(1).1)
    }

    async fn stop(&mut self) -> RunnerResult<()> {
        self.stdin = None;
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

/// One stream-json line from Claude Code into agent events.
fn translate(message: &Value, titles: &mut HashMap<String, String>) -> Vec<AgentEvent> {
    let blocks = || message["message"]["content"].as_array().into_iter().flatten();
    match message["type"].as_str() {
        Some("assistant") => {
            let mut events: Vec<AgentEvent> = blocks()
                .filter_map(|block| match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or_default().trim();
                        (!text.is_empty()).then(|| AgentEvent::Text(format!("{text}\n")))
                    }
                    Some("thinking") => block["thinking"].as_str().map(|t| AgentEvent::Thought(t.to_owned())),
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("tool");
                        let title = format!("{name} {}", crate::model::input_summary(&block["input"]));
                        titles.insert(block["id"].as_str().unwrap_or_default().to_owned(), title.clone());
                        Some(AgentEvent::Tool { title, status: ToolStatus::Started })
                    }
                    _ => None,
                })
                .collect();
            // What this call sent is what the context holds now.
            let usage = &message["message"]["usage"];
            let used = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
                .iter()
                .filter_map(|key| usage[*key].as_u64())
                .sum::<u64>();
            if used > 0 {
                let model = message["message"]["model"].as_str().unwrap_or_default();
                events.push(AgentEvent::Context { used, size: claude_context_size(model) });
            }
            events
        }
        Some("user") => blocks()
            .filter(|block| block["type"] == "tool_result")
            .map(|block| {
                let id = block["tool_use_id"].as_str().unwrap_or_default();
                let title = titles.remove(id).unwrap_or_else(|| "tool".into());
                let failed = block["is_error"].as_bool().unwrap_or(false);
                AgentEvent::Tool { title, status: if failed { ToolStatus::Failed } else { ToolStatus::Done } }
            })
            .collect(),
        Some("result") => {
            let usage = &message["usage"];
            let mut events = vec![AgentEvent::Usage(Usage {
                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
            })];
            if let Some(cost) = message["total_cost_usd"].as_f64() {
                events.push(AgentEvent::Cost(dollars_to_micros(cost)));
            }
            if message["is_error"].as_bool().unwrap_or(false) {
                let error = message["result"].as_str().unwrap_or("the turn failed").to_owned();
                events.push(AgentEvent::Error(error));
            }
            let stop_reason = message["subtype"].as_str().unwrap_or("done").to_owned();
            events.push(AgentEvent::TurnEnd { stop_reason });
            events
        }
        _ => Vec::new(),
    }
}

/// Claude's context window: 1M for the `[1m]` variants, 200k otherwise.
fn claude_context_size(model: &str) -> u64 {
    if model.contains("[1m]") || model.contains("-1m") { 1_000_000 } else { 200_000 }
}

/// Dollars as millionths, the unit costs are kept in.
pub(crate) fn dollars_to_micros(dollars: f64) -> u64 {
    if dollars.is_finite() && dollars > 0.0 { (dollars * 1_000_000.0).round() as u64 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_json_becomes_agent_events() {
        let mut titles = HashMap::new();
        let assistant = json!({ "type": "assistant", "message": { "content": [
            { "type": "text", "text": "Running the tests." },
            { "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "cargo test" } }
        ] } });
        assert_eq!(
            translate(&assistant, &mut titles),
            vec![
                AgentEvent::Text("Running the tests.\n".into()),
                AgentEvent::Tool { title: "Bash cargo test".into(), status: ToolStatus::Started },
            ]
        );
        let result = json!({ "type": "user", "message": { "content": [
            { "type": "tool_result", "tool_use_id": "t1", "is_error": false }
        ] } });
        assert_eq!(
            translate(&result, &mut titles),
            vec![AgentEvent::Tool { title: "Bash cargo test".into(), status: ToolStatus::Done }]
        );
        let end = json!({ "type": "result", "subtype": "success", "is_error": false,
            "usage": { "input_tokens": 10, "cache_read_input_tokens": 5, "output_tokens": 7 } });
        assert_eq!(
            translate(&end, &mut titles),
            vec![
                AgentEvent::Usage(Usage { input_tokens: 15, output_tokens: 7 }),
                AgentEvent::TurnEnd { stop_reason: "success".into() },
            ]
        );
    }

    #[test]
    fn context_and_cost_come_from_the_stream() {
        let mut titles = HashMap::new();
        let reply = json!({ "type": "assistant", "message": { "model": "claude-opus-5-5", "content": [],
            "usage": { "input_tokens": 1200, "cache_read_input_tokens": 90_000, "cache_creation_input_tokens": 800 } } });
        assert_eq!(translate(&reply, &mut titles), vec![AgentEvent::Context { used: 92_000, size: 200_000 }]);
        let end = json!({ "type": "result", "subtype": "success", "is_error": false, "total_cost_usd": 0.4213,
            "usage": { "input_tokens": 1, "output_tokens": 1 } });
        assert!(translate(&end, &mut titles).contains(&AgentEvent::Cost(421_300)));
    }

    #[test]
    fn claude_is_told_to_ask_our_tool() {
        let args = claude_args("http://127.0.0.1:9/mcp/x");
        let tool = args.iter().position(|a| a == "--permission-prompt-tool").unwrap();
        assert_eq!(args[tool + 1], "mcp__orch__permission");
        assert!(args.iter().any(|a| a.contains("http://127.0.0.1:9/mcp/x")));
    }
}

#[cfg(test)]
mod smoke {
    use std::time::Duration;

    use super::*;
    use crate::{broker, model::AgentState};

    /// Claude Code in print mode, asking through the MCP tool:
    /// `cargo test -p core claude_mcp_smoke -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs Claude Code and an account"]
    async fn claude_mcp_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut broker_events) = mpsc::unbounded_channel();
        let broker = broker::spawn(Duration::from_secs(120), tx, None);
        let approver = broker.clone();
        // Approves the first request and reports which tool it was.
        let asked = tokio::spawn(async move {
            while let Some(event) = broker_events.recv().await {
                if let broker::BrokerEvent::Queued(request) = event {
                    approver.answer(Some(request.id), broker::Answer::Allow, broker::DecisionSource::Click);
                    return vec![request.tool];
                }
            }
            Vec::new()
        });
        let context = RunnerContext {
            broker,
            group_id: Uuid::new_v4(),
            group_name: "Smoke".into(),
            sandbox: crate::sandbox::available(),
            mcp: Some(mcp::spawn().await.unwrap()),
            team: None,
        };
        let agent = Agent {
            id: Uuid::new_v4(),
            group_id: context.group_id,
            name: "Smoke".into(),
            task: String::new(),
            provider: Provider::CliMcp { command: "claude".into() },
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
        let mut runner = McpCliRunner::new(context);
        runner.start(&agent, "").await.unwrap();
        let mut events = runner.events();
        runner.send("Create the file note.txt containing the word ok. Nothing else.".into()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(180), async {
            while let Some(event) = events.recv().await {
                eprintln!("{event:?}");
                if matches!(event, AgentEvent::TurnEnd { .. } | AgentEvent::Exited) {
                    break;
                }
            }
        })
        .await
        .expect("turn ended");
        runner.stop().await.unwrap();
        drop(runner);
        let tools = tokio::time::timeout(Duration::from_secs(1), asked)
            .await
            .ok()
            .and_then(std::result::Result::ok)
            .unwrap_or_default();
        eprintln!("asked: {tools:?}");
        assert!(dir.path().join("note.txt").exists(), "no file; asked {tools:?}");
        assert!(!tools.is_empty(), "the write never reached the broker");
    }
}
