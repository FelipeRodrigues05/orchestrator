//! Provider-agnostic agent runners (ADR 0004). Nothing outside this module matches
//! on the provider type or parses CLI/API output.

pub mod acp;
pub mod api;
pub mod codex;
#[cfg(test)]
pub mod fake;
pub mod llm;
pub mod mcp_cli;
pub mod tools;
pub mod update;

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    broker::{AskContext, BrokerHandle},
    model::{Agent, Decision, PermissionRequest, Provider},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Started,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// A chunk of the agent's reply.
    Text(String),
    /// A chunk of reasoning, shown dimmed.
    Thought(String),
    Tool {
        title: String,
        status: ToolStatus,
    },
    /// The agent finished answering the last message.
    TurnEnd {
        stop_reason: String,
    },
    /// Tokens spent by the last model call or turn.
    Usage(crate::model::Usage),
    /// How full the context window is (`size` 0: unknown).
    Context {
        used: u64,
        size: u64,
    },
    /// What the current run has cost so far, in millionths of a dollar.
    Cost(u64),
    /// The models the agent can use and the one it uses now.
    Models {
        current: Option<String>,
        available: Vec<String>,
    },
    /// Something the operator should know that is not an error of the agent.
    Notice(String),
    Error(String),
    /// The process or task is gone.
    Exited,
}

pub type RunnerResult<T> = Result<T, String>;

#[async_trait]
pub trait AgentRunner: Send {
    async fn start(&mut self, agent: &Agent, mcp_url: &str) -> RunnerResult<()>;
    async fn send(&mut self, text: String) -> RunnerResult<()>;
    /// Taken once, right after `start`.
    fn events(&mut self) -> mpsc::Receiver<AgentEvent>;
    async fn stop(&mut self) -> RunnerResult<()>;
    /// Switches the model of the running session, from its next message on.
    async fn set_model(&mut self, _model: &str) -> RunnerResult<()> {
        Err("this agent cannot switch models while it runs".into())
    }
}

/// What a runner needs from the rest of the app.
#[derive(Clone)]
pub struct RunnerContext {
    pub broker: BrokerHandle,
    pub group_id: Uuid,
    pub group_name: String,
    /// Run agent processes and commands inside bubblewrap.
    pub sandbox: bool,
    /// Permission endpoint for CLIs that ask through MCP.
    pub mcp: Option<crate::mcp::McpHandle>,
    /// `list_agents` / `ask_agent`: the agent's teammates.
    pub team: Option<crate::team::TeamHandle>,
    /// The agent's places in the group's other projects (ADR 0024).
    pub extra_dirs: Vec<PathBuf>,
}

pub fn for_provider(provider: &Provider, context: RunnerContext) -> RunnerResult<Box<dyn AgentRunner>> {
    match provider {
        Provider::CliAcp { .. } => Ok(Box::new(acp::AcpRunner::new(context))),
        Provider::Api { .. } | Provider::Local { .. } => Ok(Box::new(api::ApiRunner::new(context))),
        Provider::CliMcp { .. } => Ok(Box::new(mcp_cli::McpCliRunner::new(context))),
        Provider::CliCodex { .. } => Ok(Box::new(codex::CodexRunner::new(context))),
    }
}

/// The agent fields a runner keeps after `start`.
#[derive(Debug, Clone)]
pub(crate) struct AgentInfo {
    pub id: Uuid,
    pub name: String,
    pub worktree: PathBuf,
    /// The model the operator chose, if any.
    pub model: Option<String>,
}

impl AgentInfo {
    pub fn of(agent: &Agent) -> Self {
        Self { id: agent.id, name: agent.name.clone(), worktree: agent.worktree.clone(), model: agent.model.clone() }
    }
}

/// Builds the `PermissionRequest` and waits for the broker (ADR 0005).
pub(crate) async fn ask_permission(
    context: &RunnerContext,
    agent: &AgentInfo,
    tool: &str,
    input: Value,
    paths: Vec<PathBuf>,
) -> Decision {
    let request = PermissionRequest {
        id: Uuid::new_v4(),
        agent_id: agent.id,
        tool: tool.to_owned(),
        spoken_summary: String::new(),
        input,
        created_at: Utc::now(),
        timeout_secs: crate::model::PERMISSION_TIMEOUT_SECS,
        warning: None,
    };
    let what = request.input_summary();
    let request =
        PermissionRequest { spoken_summary: format!("{} wants to use {tool}: {what}", agent.name), ..request };
    let ask = AskContext {
        group_id: context.group_id,
        agent_name: agent.name.clone(),
        group_name: context.group_name.clone(),
        worktree: agent.worktree.clone(),
        extra: context.extra_dirs.clone(),
        paths,
    };
    context.broker.ask(request, ask).await
}
