//! Domain types: project, group, agent and permissions (SPEC "Modelo de dados").

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: Uuid,
    pub project_id: Uuid,
    /// "Payments API"
    pub name: String,
    /// Shared context passed to every agent in the group.
    pub goal: String,
    /// Checked by the broker after the agent's own rules.
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    /// At most this many agents working at once; `None`: no limit.
    #[serde(default)]
    pub max_active: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub group_id: Uuid,
    /// Name spoken by the TTS: "Tests".
    pub name: String,
    /// "write integration tests for the /pix handler"
    pub task: String,
    /// Who the agent is: its focus, its taste, what it stays away from ("backend
    /// only; never touches the front end"). Empty: no persona.
    #[serde(default)]
    pub persona: String,
    pub provider: Provider,
    /// The model the agent's CLI should use (`gpt-6-astra`, `opus`…); `None`: the
    /// CLI's own default. API agents keep theirs in `provider`.
    #[serde(default)]
    pub model: Option<String>,
    /// Models the running CLI offers, as it reported them; empty when unknown.
    #[serde(default)]
    pub models: Vec<String>,
    /// The colour its lines are painted in: a name from the room's palette,
    /// `none` for plain text, `None` for one picked from its id.
    #[serde(default)]
    pub color: Option<String>,
    /// How full its context window is, when the CLI says.
    #[serde(default)]
    pub context: Option<ContextUse>,
    /// What it has cost since the app started, in millionths of a dollar, when
    /// the provider says.
    #[serde(default)]
    pub cost_micros: u64,
    pub worktree: PathBuf,
    pub state: AgentState,
    pub rules: Vec<PermissionRule>,
    /// When the current run started; `None` while the agent is not running.
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// Tokens spent since the app started.
    #[serde(default)]
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    CliAcp {
        command: String,
    },
    CliMcp {
        command: String,
    },
    /// `key_ref` points into the Secret Service; the key itself never lives here.
    Api {
        service: String,
        model: String,
        key_ref: String,
    },
    Local {
        url: String,
        model: String,
    },
}

impl Provider {
    /// Short label for tight UI columns: `claude-code-acp` → `claude`.
    pub fn short_name(&self) -> &str {
        match self {
            Provider::CliAcp { command } | Provider::CliMcp { command } => {
                let program = command.split_whitespace().next().unwrap_or(command);
                let program = program.rsplit('/').next().unwrap_or(program);
                if program == "npx" {
                    // `npx -y @zed-industries/claude-code-acp` → `claude`
                    let package = command.split_whitespace().last().unwrap_or(program);
                    let package = package.rsplit('/').next().unwrap_or(package);
                    return package.split('-').next().unwrap_or(package);
                }
                program.split('-').next().unwrap_or(program)
            }
            Provider::Api { service, .. } => service,
            Provider::Local { model, .. } => model,
        }
    }
}

/// Tokens in an agent's context window, out of its size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUse {
    pub used: u64,
    pub size: u64,
}

impl ContextUse {
    /// Percent of the window in use; `None` when the size is unknown.
    pub fn percent(&self) -> Option<u64> {
        (self.size > 0).then(|| self.used.saturating_mul(100) / self.size)
    }
}

/// Tokens spent, summed over a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    Stopped,
    /// Waiting for a free slot in its group (`Group::max_active`).
    Queued,
    Running,
    AwaitingPermission,
    Done,
    Error,
}

/// How long a request may wait before it becomes `Deny { reason: "no response" }`.
pub const PERMISSION_TIMEOUT_SECS: i64 = 10 * 60;

/// Lives only in memory while pending; the decision goes to the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: Uuid,
    pub agent_id: Uuid,
    /// "Bash", "Edit", "WebFetch"
    pub tool: String,
    /// E.g. `{ "command": "cargo test" }`. May contain secrets: never log it.
    pub input: serde_json::Value,
    /// "Tests wants to run cargo test"
    pub spoken_summary: String,
    pub created_at: DateTime<Utc>,
    /// Set by the broker: how long it waits before denying.
    #[serde(default = "default_timeout")]
    pub timeout_secs: i64,
    /// Set by the broker for risky requests, e.g. writes outside the worktree.
    #[serde(default)]
    pub warning: Option<String>,
}

fn default_timeout() -> i64 {
    PERMISSION_TIMEOUT_SECS
}

impl PermissionRequest {
    /// One line for the UI: the command, path or URL the tool wants to use.
    pub fn input_summary(&self) -> String {
        input_summary(&self.input)
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.created_at + chrono::Duration::seconds(self.timeout_secs)
    }
}

/// The command, path or URL a tool input is about.
pub fn input_summary(input: &serde_json::Value) -> String {
    ["command", "file_path", "path", "url", "title"]
        .iter()
        .find_map(|key| input.get(*key).and_then(|value| value.as_str()))
        .map_or_else(|| input.to_string(), str::to_owned)
}

/// A secret typed by the operator; never printed by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    AlwaysAllow(PermissionRule),
    Deny { reason: String },
}

impl Decision {
    pub fn allows(&self) -> bool {
        !matches!(self, Decision::Deny { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    pub tool: String,
    /// E.g. `cargo test:*` in `Bash(cargo test:*)`.
    pub pattern: Option<String>,
    pub effect: Effect,
}

impl PermissionRule {
    /// Whether this rule covers `tool` used with `summary` (command, path…).
    ///
    /// Patterns ending in `:*` match by prefix (`cargo test:*` covers
    /// `cargo test --lib`); any other pattern must match exactly.
    pub fn matches(&self, tool: &str, summary: &str) -> bool {
        if !(self.tool == "*" || self.tool.eq_ignore_ascii_case(tool)) {
            return false;
        }
        match &self.pattern {
            None => true,
            Some(pattern) => match pattern.strip_suffix(":*") {
                Some(prefix) => {
                    summary == prefix || summary.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(' '))
                }
                None => summary == pattern,
            },
        }
    }

    /// The narrow rule "always allow" proposes for `request`: for commands, the
    /// program plus its first word (`Bash(cargo test:*)`), never a bare `Bash`.
    pub fn suggest(request: &PermissionRequest) -> PermissionRule {
        let summary = request.input_summary();
        let pattern = if request.tool.eq_ignore_ascii_case("bash") {
            let words: Vec<&str> = summary.split_whitespace().take(2).collect();
            Some(format!("{}:*", words.join(" ")))
        } else {
            None
        };
        PermissionRule { tool: request.tool.clone(), pattern, effect: Effect::Allow }
    }

    /// Parses `Bash(cargo test:*)` or `Edit`.
    pub fn parse(text: &str, effect: Effect) -> Option<PermissionRule> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        match text.split_once('(') {
            Some((tool, rest)) => {
                let pattern = rest.strip_suffix(')')?.trim();
                Some(PermissionRule {
                    tool: tool.trim().to_owned(),
                    pattern: (!pattern.is_empty()).then(|| pattern.to_owned()),
                    effect,
                })
            }
            None => Some(PermissionRule { tool: text.to_owned(), pattern: None, effect }),
        }
    }
}

impl fmt::Display for PermissionRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.pattern {
            Some(pattern) => write!(f, "{}({pattern})", self.tool),
            None => write!(f, "{}", self.tool),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    /// What the operator said or typed to an agent.
    Operator,
    /// What the host pet answered.
    Pet,
}

/// One line of the event log. `agent_id: None` means the runtime itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub at: DateTime<Utc>,
    pub agent_id: Option<Uuid>,
    pub level: LogLevel,
    pub text: String,
}

/// An agent provider found on this machine, ready to pick when creating an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOption {
    /// What the operator types: `opencode`, `ollama/qwen3:8b`, `anthropic`.
    pub label: String,
    pub provider: Provider,
    /// False while an API provider has no key in the Secret Service.
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Editor {
    pub name: String,
    pub command: String,
    /// Opens inside the system terminal (`nvim`, `hx`).
    pub terminal: bool,
}

/// The language the operator speaks and hears (STT, the pet's answers, announcements).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// Brazilian Portuguese.
    #[default]
    Pt,
    English,
}

impl Language {
    pub fn parse(text: &str) -> Option<Language> {
        match text.trim().to_lowercase().as_str() {
            "pt" | "pt-br" | "pt_br" | "portugues" | "português" | "portuguese" => Some(Language::Pt),
            "en" | "en-us" | "en_us" | "english" | "ingles" | "inglês" => Some(Language::English),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Language::Pt => "pt",
            Language::English => "en",
        }
    }

    /// For the pet's instructions: "answer in …".
    pub fn name(self) -> &'static str {
        match self {
            Language::Pt => "Brazilian Portuguese",
            Language::English => "English",
        }
    }

    /// Piper voice used when the operator picked none.
    pub fn default_voice(self) -> &'static str {
        match self {
            Language::Pt => "pt_BR-faber-medium",
            Language::English => "en_US-lessac-medium",
        }
    }

    /// Locale prefix of Piper voices in this language.
    pub fn voice_prefix(self) -> &'static str {
        match self {
            Language::Pt => "pt_",
            Language::English => "en_",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceStatus {
    /// Speech-to-text model and microphone are available.
    pub stt: bool,
    /// Piper and its voice are available.
    pub tts: bool,
    pub listening: bool,
    pub transcribing: bool,
    pub speaking: bool,
    /// The wake word is on (`:wake on`).
    pub wake: bool,
    /// The pet heard its name and waits for the command.
    pub armed: bool,
    pub language: Language,
    /// Piper voice in use (file stem, e.g. `pt_BR-faber-medium`).
    pub voice: Option<String>,
    /// Piper voices installed in the data folder.
    pub voices: Vec<String>,
    /// Last transcript.
    pub heard: Option<String>,
    /// Why STT/TTS are offline, for the voice strip.
    pub detail: Option<String>,
}

/// The host pet's model and whether it is working on an answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    /// `None`: no model; the pet only understands the fixed phrases.
    pub provider: Option<String>,
    pub thinking: bool,
    pub usage: Usage,
}

/// Snapshot of what the UI needs to draw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    /// Every open project, in the order they were opened.
    pub projects: Vec<Project>,
    /// The project the room shows; new groups go there. `None` with nothing open.
    pub focus: Option<Uuid>,
    /// Groups and agents of all open projects.
    pub groups: Vec<Group>,
    pub agents: Vec<Agent>,
    /// Pending requests, oldest first.
    pub permissions: Vec<PermissionRequest>,
    /// Most recent events, oldest first.
    pub log: Vec<LogEntry>,
    pub providers: Vec<ProviderOption>,
    pub editors: Vec<Editor>,
    pub voice: VoiceStatus,
    pub host: HostStatus,
    /// Agents run inside bubblewrap.
    pub sandbox: bool,
    /// Opaque UI layout saved with the focused project.
    pub layout: Option<String>,
    /// Projects opened before, newest first, to reopen without typing a path.
    pub recent_projects: Vec<PathBuf>,
    /// Git repositories found under the usual project folders.
    pub repositories: Vec<PathBuf>,
}

impl AppState {
    /// The focused project.
    pub fn project(&self) -> Option<&Project> {
        self.focus.and_then(|id| self.projects.iter().find(|p| p.id == id))
    }

    pub fn project_of_group(&self, group: Uuid) -> Option<&Project> {
        let project = self.group(group)?.project_id;
        self.projects.iter().find(|p| p.id == project)
    }

    pub fn project_of_agent(&self, agent: Uuid) -> Option<&Project> {
        self.project_of_group(self.agent(agent)?.group_id)
    }

    /// Groups of the focused project.
    pub fn focused_groups(&self) -> impl Iterator<Item = &Group> {
        self.groups_of(self.focus)
    }

    /// Agents of the focused project.
    pub fn focused_agents(&self) -> impl Iterator<Item = &Agent> {
        self.agents_of(self.focus)
    }

    /// A copy with only the focused project's groups and agents, for views that
    /// show one project (sector map, agent table).
    pub fn scoped(&self) -> AppState {
        let mut scoped = self.clone();
        scoped.groups.retain(|g| Some(g.project_id) == self.focus);
        let groups: Vec<Uuid> = scoped.groups.iter().map(|g| g.id).collect();
        scoped.agents.retain(|a| groups.contains(&a.group_id));
        scoped
    }

    pub fn groups_of(&self, project: Option<Uuid>) -> impl Iterator<Item = &Group> {
        self.groups.iter().filter(move |g| Some(g.project_id) == project)
    }

    pub fn agents_of(&self, project: Option<Uuid>) -> impl Iterator<Item = &Agent> {
        self.agents.iter().filter(move |a| self.group(a.group_id).is_some_and(|g| Some(g.project_id) == project))
    }

    pub fn group(&self, id: Uuid) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn agent(&self, id: Uuid) -> Option<&Agent> {
        self.agents.iter().find(|a| a.id == id)
    }
}

/// Lowercase without accents or surrounding punctuation, for matching spoken or
/// typed names: "Testes." → "testes".
pub fn normalize(text: &str) -> String {
    let folded: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| match ch {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect();
    folded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c.is_ascii_punctuation())
        .trim()
        .to_owned()
}

/// Resolves `path` against `base` without touching the disk (`..` is folded).
pub fn resolve(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(tool: &str, input: serde_json::Value) -> PermissionRequest {
        PermissionRequest {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            tool: tool.into(),
            input,
            spoken_summary: String::new(),
            created_at: Utc::now(),
            timeout_secs: crate::model::PERMISSION_TIMEOUT_SECS,
            warning: None,
        }
    }

    #[test]
    fn input_summary_prefers_known_fields() {
        assert_eq!(request("Bash", json!({ "command": "cargo test" })).input_summary(), "cargo test");
        assert_eq!(request("Edit", json!({ "file_path": "src/lib.rs" })).input_summary(), "src/lib.rs");
        assert_eq!(request("X", json!({ "depth": 2 })).input_summary(), r#"{"depth":2}"#);
    }

    #[test]
    fn provider_short_names() {
        let acp = |command: &str| Provider::CliAcp { command: command.into() };
        assert_eq!(acp("claude-code-acp").short_name(), "claude");
        assert_eq!(acp("gemini --experimental-acp").short_name(), "gemini");
        assert_eq!(acp("npx -y @zed-industries/codex-acp").short_name(), "codex");
        assert_eq!(acp("opencode acp").short_name(), "opencode");
        let local = Provider::Local { url: "http://localhost:11434".into(), model: "qwen3".into() };
        assert_eq!(local.short_name(), "qwen3");
    }

    #[test]
    fn requests_expire_after_the_timeout() {
        let request = request("Bash", json!({}));
        let waited = request.expires_at() - request.created_at;
        assert_eq!(waited.num_seconds(), PERMISSION_TIMEOUT_SECS);
    }

    #[test]
    fn rule_patterns_match_by_prefix() {
        let rule = PermissionRule::parse("Bash(cargo test:*)", Effect::Allow).unwrap();
        assert_eq!(rule.to_string(), "Bash(cargo test:*)");
        assert!(rule.matches("Bash", "cargo test"));
        assert!(rule.matches("bash", "cargo test --lib"));
        assert!(!rule.matches("Bash", "cargo testing"));
        assert!(!rule.matches("Bash", "rm -rf /"));
        assert!(!rule.matches("Edit", "cargo test"));
        let edit = PermissionRule::parse("Edit", Effect::Allow).unwrap();
        assert!(edit.matches("Edit", "src/main.rs"));
    }

    #[test]
    fn always_allow_suggests_narrow_command_rules() {
        let rule = PermissionRule::suggest(&request("Bash", json!({ "command": "cargo test --lib" })));
        assert_eq!(rule.to_string(), "Bash(cargo test:*)");
        let rule = PermissionRule::suggest(&request("Edit", json!({ "file_path": "a.rs" })));
        assert_eq!(rule.to_string(), "Edit");
    }

    #[test]
    fn normalize_folds_case_accents_and_punctuation() {
        assert_eq!(normalize("  Permitir. "), "permitir");
        assert_eq!(normalize("Grupo  Ação"), "grupo acao");
    }

    #[test]
    fn resolve_folds_parent_dirs() {
        let base = Path::new("/tmp/shop/.worktrees/tests");
        assert_eq!(resolve(base, Path::new("src/a.rs")), Path::new("/tmp/shop/.worktrees/tests/src/a.rs"));
        assert_eq!(resolve(base, Path::new("../../x")), Path::new("/tmp/shop/x"));
    }
}
