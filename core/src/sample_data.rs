//! Sample room used by UI tests and screenshots; the app itself loads SQLite.

use std::path::PathBuf;

use chrono::{Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::model::{Agent, AgentState, AppState, Group, LogEntry, LogLevel, PermissionRequest, Project, Provider};

pub fn sample_state() -> AppState {
    let now = Utc::now();
    let root = PathBuf::from("/tmp/shop");
    let project = Project { id: Uuid::new_v4(), name: "Shop".into(), root: root.clone() };

    let group = |name: &str, goal: &str| Group {
        id: Uuid::new_v4(),
        project_id: project.id,
        name: name.into(),
        goal: goal.into(),
        rules: Vec::new(),
        max_active: None,
    };
    let api = group("API", "Pix payments in the backend");
    let front = group("Front", "Checkout screen");

    let agent = |group: &Group, name: &str, task: &str, command: &str, state, running_secs: Option<i64>| Agent {
        id: Uuid::new_v4(),
        group_id: group.id,
        name: name.into(),
        task: task.into(),
        provider: Provider::CliAcp { command: command.into() },
        worktree: root.join(".worktrees").join(name.to_lowercase()),
        state,
        rules: Vec::new(),
        started_at: running_secs.map(|secs| now - Duration::seconds(secs)),
        usage: Default::default(),
        persona: String::new(),
        model: None,
        models: Vec::new(),
        color: None,
    };
    let backend =
        agent(&api, "Backend", "implement the /pix handler", "claude-code-acp", AgentState::Running, Some(7 * 60 + 14));
    let tests = agent(
        &api,
        "Tests",
        "write integration tests for the /pix handler",
        "gemini --experimental-acp",
        AgentState::AwaitingPermission,
        Some(3 * 60 + 41),
    );
    let checkout = agent(&front, "Checkout", "build the checkout screen", "codex-acp", AgentState::Stopped, None);

    let permissions = vec![PermissionRequest {
        id: Uuid::new_v4(),
        agent_id: tests.id,
        tool: "Bash".into(),
        input: json!({ "command": "cargo test" }),
        spoken_summary: "Tests wants to run cargo test".into(),
        created_at: now - Duration::seconds(48),
        timeout_secs: crate::model::PERMISSION_TIMEOUT_SECS,
        warning: None,
    }];

    let line = |secs_ago: i64, agent: Option<&Agent>, level, text: &str| LogEntry {
        at: now - Duration::seconds(secs_ago),
        agent_id: agent.map(|a| a.id),
        level,
        text: text.into(),
    };
    let log = vec![
        line(434, None, LogLevel::Info, "runtime online"),
        line(434, Some(&backend), LogLevel::Info, "session started"),
        line(410, Some(&backend), LogLevel::Info, "reading src/payments/mod.rs"),
        line(352, Some(&backend), LogLevel::Info, "planning /pix handler"),
        line(301, Some(&backend), LogLevel::Info, "edit src/payments/pix.rs"),
        line(221, Some(&tests), LogLevel::Info, "session started"),
        line(214, Some(&backend), LogLevel::Info, "edit src/routes.rs"),
        line(190, Some(&tests), LogLevel::Info, "reading src/payments/pix.rs"),
        line(160, Some(&backend), LogLevel::Warn, "clippy: needless clone in pix.rs"),
        line(131, Some(&tests), LogLevel::Info, "writing tests/pix_handler.rs"),
        line(122, Some(&backend), LogLevel::Info, "edit src/payments/pix.rs"),
        line(95, Some(&backend), LogLevel::Info, "edit ok"),
        line(70, Some(&tests), LogLevel::Info, "writing tests/pix_refund.rs"),
        line(48, Some(&tests), LogLevel::Warn, "permission: Bash cargo test"),
        line(30, Some(&backend), LogLevel::Info, "reading src/payments/qr.rs"),
        line(12, Some(&backend), LogLevel::Info, "edit src/payments/qr.rs"),
    ];

    AppState {
        focus: Some(project.id),
        projects: vec![project],
        groups: vec![api, front],
        agents: vec![backend, tests, checkout],
        permissions,
        log,
        ..AppState::default()
    }
}
