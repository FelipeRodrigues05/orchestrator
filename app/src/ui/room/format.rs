//! Small formatting helpers shared by the panels.

use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use orch_core::model::{Agent, AgentState, AppState, LogLevel};
use uuid::Uuid;

use super::{ACCENT, DIM, ERROR, OK, SPARK_WIDTH, STOPPED, TEXT, WARN};
use crate::ui::grid::{Style, Tone};

/// Agents in the order the room shows them: by group, then the ungrouped ones.
/// Agents of the focused project, group by group.
pub(super) fn display_order(state: &AppState) -> Vec<&Agent> {
    state
        .focused_groups()
        .flat_map(|g| state.agents.iter().filter(move |a| a.group_id == g.id))
        .collect()
}

pub(super) fn lamp_for(state: AgentState) -> (char, Style) {
    match state {
        AgentState::Running => ('●', OK),
        AgentState::AwaitingPermission => ('◆', WARN.bold()),
        AgentState::Stopped => ('■', STOPPED),
        AgentState::Done => ('✓', ACCENT),
        AgentState::Queued => ('○', DIM),
        AgentState::Error => ('✖', ERROR.bold()),
    }
}

pub(super) fn state_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Running => "RUNNING",
        AgentState::AwaitingPermission => "AWAITING PERMISSION",
        AgentState::Stopped => "STOPPED",
        AgentState::Done => "DONE",
        AgentState::Queued => "QUEUED · WAITING SLOT",
        AgentState::Error => "ERROR",
    }
}

pub(super) fn level_style(level: LogLevel) -> Style {
    match level {
        LogLevel::Info => TEXT,
        LogLevel::Warn => WARN,
        LogLevel::Error => ERROR.bold(),
        LogLevel::Operator => ACCENT.bold(),
        LogLevel::Pet => ACCENT,
    }
}

/// Who is at the console; picks the pet.
pub(super) fn user_name() -> String {
    ["USER", "LOGNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .unwrap_or_else(|| "operator".into())
}

pub(super) fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M:%S").to_string()
}

pub(super) fn format_tokens(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1e3),
        _ => format!("{:.1}M", tokens as f64 / 1e6),
    }
}

pub(super) fn format_rtt(rtt: Duration) -> String {
    if rtt < Duration::from_millis(1) {
        format!("{}µs", rtt.as_micros())
    } else {
        format!("{}ms", rtt.as_millis())
    }
}

pub(super) fn uptime(agent: &Agent, now: DateTime<Utc>) -> String {
    let active = matches!(
        agent.state,
        AgentState::Running | AgentState::AwaitingPermission
    );
    match agent.started_at {
        Some(started) if active => {
            let secs = (now - started).num_seconds().max(0);
            if secs < 3600 {
                format!("{:02}:{:02}", secs / 60, secs % 60)
            } else {
                format!("{:02}h{:02}", secs / 3600, secs % 3600 / 60)
            }
        }
        _ => "--:--".into(),
    }
}

/// Log lines per minute over the last `SPARK_WIDTH` minutes, oldest on the left.
pub(super) fn sparkline(state: &AppState, agent: Uuid, now: DateTime<Utc>) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut buckets = [0_usize; SPARK_WIDTH];
    for entry in state.log.iter().filter(|e| e.agent_id == Some(agent)) {
        let minutes = (now - entry.at).num_seconds().max(0) as usize / 60;
        if minutes < SPARK_WIDTH {
            buckets[SPARK_WIDTH - 1 - minutes] += 1;
        }
    }
    let max = buckets.iter().copied().max().unwrap_or(0).max(1);
    buckets
        .iter()
        .map(|&n| BARS[n * (BARS.len() - 1) / max])
        .collect()
}

/// Agents' colours: `(name, Portuguese name)`, in the order of `render`'s palette.
pub(in crate::ui) const AGENT_COLORS: [(&str, &str); 10] = [
    ("cyan", "ciano"),
    ("green", "verde"),
    ("yellow", "amarelo"),
    ("magenta", "magenta"),
    ("blue", "azul"),
    ("orange", "laranja"),
    ("teal", "turquesa"),
    ("pink", "rosa"),
    ("purple", "roxo"),
    ("lime", "lima"),
];

/// The style an agent's lines are painted in; `None` when it asked for plain text.
pub(super) fn agent_style(agent: &Agent) -> Option<Style> {
    let index = match agent.color.as_deref() {
        Some("none") => return None,
        Some(name) => AGENT_COLORS.iter().position(|(en, _)| *en == name)?,
        // Picked from its id: stable, and different agents mostly differ.
        None => (agent.id.as_u128() % AGENT_COLORS.len() as u128) as usize,
    };
    Some(Style::new(Tone::Agent(index as u8)))
}

/// `verde`, `green` → `Some(Some("green"))`; `auto` → `Some(None)`; `none` → plain.
pub(super) fn parse_color(name: &str) -> Option<Option<String>> {
    let name = name.trim().to_lowercase();
    match name.as_str() {
        "auto" => Some(None),
        "none" | "off" | "plain" | "nenhuma" => Some(Some("none".into())),
        _ => AGENT_COLORS
            .iter()
            .find(|(en, pt)| *en == name || *pt == name)
            .map(|(en, _)| Some((*en).to_owned())),
    }
}

/// `$0.42`, `$12.30`; empty when nothing was reported.
pub(super) fn format_cost(micros: u64) -> String {
    if micros == 0 {
        return String::new();
    }
    let cents = micros.div_ceil(10_000);
    format!("${}.{:02}", cents / 100, cents % 100)
}
