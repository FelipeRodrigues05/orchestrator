//! Central runtime actor: receives commands from the UI, owns the app state, runs
//! one actor per started agent and publishes state snapshots back to the UI.

mod agents;
mod commands;
mod events;
mod operator;
mod projects;
mod settings;
mod team;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    broker::{self, Answer, BrokerEvent, BrokerHandle},
    discovery,
    host::{self, HostEvent, HostHandle},
    model::{
        Agent, AppState, Language, LogEntry, LogLevel, PERMISSION_TIMEOUT_SECS, PermissionRule, Provider,
        ProviderOption, Secret,
    },
    persistence::Store,
    runner::{self, AgentEvent, AgentRunner, RunnerContext, RunnerResult},
    secrets,
    team::{TeamHandle, TeamRequest},
    voice::{self, Priority, VoiceEvent, VoiceHandle, phrases::Phrase},
};

const MAX_LOG: usize = 2000;
const PUBLISH_EVERY: Duration = Duration::from_millis(100);

/// UI → runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Round trip through the runtime, to check the bridge is alive.
    Ping,
    Shutdown,
    /// Opens the project next to the ones already open (or focuses it) and focuses it.
    OpenProject {
        root: PathBuf,
    },
    /// Shows this open project in the room; new groups go there.
    FocusProject(Uuid),
    /// Stops the project's agents and takes it out of the room (nothing is deleted).
    CloseProject(Uuid),
    CreateGroup {
        name: String,
        goal: String,
    },
    CreateAgent {
        group: String,
        name: String,
        provider: Option<String>,
        task: String,
        /// Who the agent is; empty for none.
        persona: String,
    },
    /// Changes who the agent is; it takes effect on its next start.
    SetPersona {
        agent: Uuid,
        persona: String,
    },
    StartAgent(Uuid),
    StopAgent(Uuid),
    RemoveAgent(Uuid),
    /// Text for one agent; starts it when it is not running.
    Message {
        agent: Uuid,
        text: String,
    },
    /// Free text from the command line, parsed like speech: answers, routing to
    /// "grupo X, agente Y: …", or a message to `selected`.
    Say {
        text: String,
        selected: Option<Uuid>,
    },
    /// A click on a permission request.
    Answer {
        request: Uuid,
        answer: Answer,
    },
    /// Selection in the UI; spoken messages without names go there.
    Select(Option<Uuid>),
    /// Opens the agent's worktree (or the project root) in an editor.
    OpenIn {
        agent: Option<Uuid>,
        editor: Option<String>,
    },
    SetApiKey {
        service: String,
        key: Secret,
    },
    /// Switches an agent to another provider (or `service/model`) mid-task; a running
    /// agent restarts on it with a hand-off of the conversation so far.
    SetProvider {
        agent: Uuid,
        provider: String,
    },
    /// The model the host pet thinks with (an API or local provider label).
    SetHostProvider(String),
    /// Updates every installed agent CLI.
    UpdateClis,
    /// Minutes a permission request waits before it becomes a denial.
    SetPermissionTimeout(u64),
    /// At most `limit` agents of the group working at once; `None` lifts it.
    SetGroupLimit {
        group: String,
        limit: Option<usize>,
    },
    /// Downloads the whisper model and the Piper voice (~550 MB).
    InstallVoice,
    /// Runs agents inside bubblewrap (applies to agents started from now on).
    SetSandbox(bool),
    /// Commits the agent's worktree and merges its branch into the project.
    Merge(Uuid),
    /// Listen for the pet's name (`Some(name)`) or stop (`None`).
    SetWake(Option<String>),
    /// Opens the operator's email (`:email` URL, else the mail client).
    OpenEmail,
    /// A terminal in the agent's worktree, or the project root.
    OpenTerminal(Option<Uuid>),
    /// Webmail address for "open my email", e.g. `https://outlook.live.com/mail`.
    SetEmailUrl(String),
    /// The pet's name, sent by the UI at start; the wake word listens for it unless
    /// the operator turned it off.
    PetName(String),
    /// Language for speech-to-text, the pet's answers and announcements (`pt`, `en`).
    SetLanguage(String),
    /// Piper voice by id (`en_US-lessac-medium`); downloaded when missing.
    SetVoice(String),
    /// The pet plans a group: splits its goal into tasks, one agent each, and starts them.
    Lead {
        group: String,
        agents: usize,
    },
    /// Editors the UI found through `.desktop` files, merged with the ones on PATH.
    AddEditors(Vec<crate::model::Editor>),
    AddRule {
        agent: Uuid,
        rule: PermissionRule,
    },
    VoicePress,
    VoiceRelease,
    SaveLayout(String),
}

/// Runtime → UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    State(Box<AppState>),
    Pong {
        at: DateTime<Utc>,
    },
    /// Feedback for the operator's last command.
    Notice {
        text: String,
        level: LogLevel,
    },
}

pub type RunnerFactory = Arc<dyn Fn(&Provider, RunnerContext) -> RunnerResult<Box<dyn AgentRunner>> + Send + Sync>;

pub struct Config {
    /// `None`: nothing is persisted.
    pub store: Option<PathBuf>,
    pub voice: bool,
    pub discover: bool,
    pub permission_timeout: Duration,
    pub runners: RunnerFactory,
    /// Extra provider options, on top of the discovered ones.
    pub providers: Vec<ProviderOption>,
    /// Sandbox agents with bubblewrap when it is installed.
    pub sandbox: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            store: Some(Store::default_path()),
            voice: true,
            discover: true,
            permission_timeout: Duration::from_secs(PERMISSION_TIMEOUT_SECS as u64),
            runners: Arc::new(runner::for_provider),
            providers: Vec::new(),
            sandbox: true,
        }
    }
}

enum Internal {
    Agent {
        id: Uuid,
        event: AgentEvent,
    },
    Started {
        id: Uuid,
        result: Result<(), String>,
    },
    Broker(BrokerEvent),
    Voice(VoiceEvent),
    Providers(Vec<ProviderOption>),
    Host(HostEvent),
    Notice(String, LogLevel),
    VoiceInstalled(Result<(), String>),
    Merged {
        agent: Uuid,
        result: Result<String, String>,
    },
    VoiceDownloaded {
        voice: String,
        result: Result<(), String>,
    },
    /// A command queued from inside the actor (async ones from the pet).
    Run(Command),
    /// An agent called `list_agents` or `ask_agent`.
    Team(TeamRequest),
    /// An update finished; `agent` is set when it was triggered by that agent's error.
    Updated {
        agent: Option<Uuid>,
        label: String,
        result: Result<String, String>,
    },
}

enum AgentCmd {
    Send(String),
    Stop,
}

struct Orchestrator {
    state: AppState,
    events: async_channel::Sender<Event>,
    inbox: mpsc::UnboundedSender<Internal>,
    broker: BrokerHandle,
    voice: Option<VoiceHandle>,
    store: Option<Store>,
    runners: RunnerFactory,
    running: HashMap<Uuid, mpsc::UnboundedSender<AgentCmd>>,
    /// Agents whose last log line is still receiving streamed text.
    open_lines: HashSet<Uuid>,
    selected: Option<Uuid>,
    dirty: bool,
    /// Last prompt sent to each agent, resent after an update.
    last_prompt: HashMap<Uuid, String>,
    /// Agents that already got an automatic update this session (no loops).
    auto_updated: HashSet<Uuid>,
    /// Prompt to start with once the agent's current run has exited.
    pending_restart: HashMap<Uuid, String>,
    host: HostHandle,
    /// Provider label the operator chose for the pet; `None`: pick a default.
    host_choice: Option<String>,
    /// Agents waiting for a slot in their group, oldest first, with their prompt.
    waiting: Vec<(Uuid, String)>,
    /// Permission endpoint for MCP-based CLIs (Claude Code in print mode).
    mcp: Option<crate::mcp::McpHandle>,
    /// Saved UI layout of each open project.
    layouts: HashMap<Uuid, Option<String>>,
    /// Handed to runners: `list_agents` and `ask_agent`.
    team: TeamHandle,
    /// Turns sent to and finished by each agent, and who waits for which turn.
    turns: HashMap<Uuid, TurnBook>,
    /// Asker → the teammate it waits for (to refuse cycles).
    asking: HashMap<Uuid, Uuid>,
}

/// Each message to an agent is one turn, answered in order; numbering them tells
/// which turn end answers a teammate's question.
#[derive(Default)]
struct TurnBook {
    sent: u64,
    ended: u64,
    /// Reply text of the turn in progress.
    text: String,
    /// `(turn, asker, reply)`.
    waiters: Vec<(u64, Uuid, Reply)>,
}

/// Where a teammate's answer (or why there is none) goes.
type Reply = oneshot::Sender<Result<String, String>>;

/// Runs until it gets `Shutdown` or the UI drops its end of the channels.
pub async fn run(commands: async_channel::Receiver<Command>, events: async_channel::Sender<Event>) {
    run_with(Config::default(), commands, events).await;
}

pub async fn run_with(
    config: Config,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<Event>,
) {
    let (inbox, internal) = mpsc::unbounded_channel();
    let voice = config.voice.then(|| {
        let handle = voice::spawn(forward(&inbox, Internal::Voice));
        voice::shortcut::spawn(handle.clone());
        handle
    });
    let broker = broker::spawn(config.permission_timeout, forward(&inbox, Internal::Broker), voice.clone());
    let host = host::spawn(forward(&inbox, Internal::Host));
    let team = TeamHandle::new(forward(&inbox, Internal::Team));
    let store = config.store.as_deref().and_then(|path| match Store::open(path) {
        Ok(store) => Some(store),
        Err(error) => {
            tracing::error!(%error, "could not open the database; nothing will be saved");
            None
        }
    });
    let mcp = match crate::mcp::spawn().await {
        Ok(mcp) => Some(mcp),
        Err(error) => {
            tracing::warn!(%error, "MCP server unavailable: no Claude Code agents, no team tools for CLIs");
            None
        }
    };

    let mut orchestrator = Orchestrator {
        state: AppState::default(),
        events,
        inbox,
        broker,
        voice,
        store,
        runners: config.runners,
        running: HashMap::new(),
        open_lines: HashSet::new(),
        selected: None,
        dirty: true,
        last_prompt: HashMap::new(),
        auto_updated: HashSet::new(),
        pending_restart: HashMap::new(),
        host,
        host_choice: None,
        waiting: Vec::new(),
        layouts: HashMap::new(),
        team,
        turns: HashMap::new(),
        asking: HashMap::new(),
        mcp,
    };
    orchestrator.state.sandbox = config.sandbox && crate::sandbox::available();
    orchestrator.restore_settings().await;
    orchestrator.apply_host_provider();
    orchestrator.state.editors = discovery::editors();
    orchestrator.state.providers = config.providers;
    orchestrator.log(None, LogLevel::Info, "runtime online");
    if config.discover {
        orchestrator.state.providers.extend(discovery::acp_clis());
        orchestrator.refresh_providers();
    }
    orchestrator.reopen_projects().await;
    if orchestrator.publish().await {
        orchestrator.serve(commands, internal).await;
    }
    for (_, agent) in orchestrator.running.drain() {
        let _ = agent.send(AgentCmd::Stop);
    }
    tracing::info!("orchestrator stopped");
}

/// A channel whose messages reach the orchestrator's inbox wrapped by `wrap`.
fn forward<T: Send + 'static>(
    inbox: &mpsc::UnboundedSender<Internal>,
    wrap: fn(T) -> Internal,
) -> mpsc::UnboundedSender<T> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let inbox = inbox.clone();
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if inbox.send(wrap(message)).is_err() {
                break;
            }
        }
    });
    tx
}

impl Orchestrator {
    /// Settings saved by earlier runs: sandbox, the pet's model, language, voice, timeout.
    async fn restore_settings(&mut self) {
        let Some(store) = &self.store else { return };
        if store.setting("sandbox").await.as_deref() == Some("off") {
            self.state.sandbox = false;
        }
        self.host_choice = store.setting("host_provider").await;
        if let (Some(language), Some(voice)) =
            (store.setting("language").await.and_then(|code| Language::parse(&code)), &self.voice)
        {
            voice.set_language(language);
            self.state.voice.language = language;
        }
        if let (Some(id), Some(voice)) = (store.setting("voice").await, &self.voice) {
            voice.set_voice(Some(id));
        }
        let minutes = store.setting("permission_timeout_minutes").await.and_then(|m| m.parse::<u64>().ok());
        if let Some(minutes) = minutes {
            self.broker.set_timeout(Duration::from_secs(minutes * 60));
        }
    }

    /// The projects open when the app last ran, and the one in focus.
    async fn reopen_projects(&mut self) {
        let Some(store) = self.store.clone() else { return };
        // `last_project` is what older versions kept: one project.
        let open = match store.setting("open_projects").await {
            Some(roots) => roots,
            None => store.setting("last_project").await.unwrap_or_default(),
        };
        for root in open.lines().filter(|line| !line.trim().is_empty()) {
            self.open_project(PathBuf::from(root)).await;
        }
        if let Some(focus) = store.setting("focus_project").await
            && let Some(id) = self.state.projects.iter().find(|p| p.root == Path::new(&focus)).map(|p| p.id)
        {
            self.focus_project(id);
        }
    }

    /// The actor loop: commands from the UI, internal messages, and a publish tick.
    async fn serve(
        &mut self,
        commands: async_channel::Receiver<Command>,
        mut internal: mpsc::UnboundedReceiver<Internal>,
    ) {
        let mut tick = tokio::time::interval(PUBLISH_EVERY);
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Ok(command) = command else { break };
                    tracing::debug!(?command, "command received");
                    match command {
                        Command::Shutdown => break,
                        Command::Ping => {
                            if self.events.send(Event::Pong { at: Utc::now() }).await.is_err() {
                                break;
                            }
                        }
                        command => self.command(command).await,
                    }
                }
                Some(message) = internal.recv() => match message {
                    Internal::Run(command) => self.command(command).await,
                    message => self.internal(message),
                },
                _ = tick.tick() => {
                    if self.dirty && !self.publish().await {
                        break;
                    }
                }
            }
        }
    }

    async fn publish(&mut self) -> bool {
        self.dirty = false;
        self.events.send(Event::State(Box::new(self.state.clone()))).await.is_ok()
    }

    fn notice(&mut self, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        self.log(None, level, &text);
        let _ = self.events.try_send(Event::Notice { text, level });
    }

    fn log(&mut self, agent: Option<Uuid>, level: LogLevel, text: &str) {
        if let Some(id) = agent {
            self.open_lines.remove(&id);
        }
        // One entry per line: errors often carry pretty-printed JSON.
        let at = Utc::now();
        let lines = text.lines().map(str::trim_end).filter(|line| !line.trim().is_empty());
        for line in lines {
            self.state.log.push(LogEntry { at, agent_id: agent, level, text: line.to_owned() });
        }
        let excess = self.state.log.len().saturating_sub(MAX_LOG);
        self.state.log.drain(..excess);
        self.dirty = true;
    }

    /// Streams agent text into the log, one entry per line.
    fn output(&mut self, agent: Uuid, chunk: &str) {
        let mut parts = chunk.split('\n');
        if let Some(first) = parts.next() {
            let open = self
                .open_lines
                .contains(&agent)
                .then(|| self.state.log.iter().rposition(|e| e.agent_id == Some(agent)))
                .flatten();
            match open {
                Some(index) => self.state.log[index].text.push_str(first),
                None if !first.is_empty() => {
                    self.log(Some(agent), LogLevel::Info, first);
                    self.open_lines.insert(agent);
                }
                None => {}
            }
        }
        for part in parts {
            self.open_lines.remove(&agent);
            if !part.is_empty() {
                self.log(Some(agent), LogLevel::Info, part);
                self.open_lines.insert(agent);
            }
        }
        self.dirty = true;
    }

    fn agent_mut(&mut self, id: Uuid) -> Option<&mut Agent> {
        self.state.agents.iter_mut().find(|a| a.id == id)
    }

    fn names(&self, id: Uuid) -> (String, String) {
        let agent = self.state.agent(id);
        let group = agent.and_then(|a| self.state.group(a.group_id));
        (agent.map_or_else(|| "?".into(), |a| a.name.clone()), group.map_or_else(|| "?".into(), |g| g.name.clone()))
    }

    fn speak(&self, priority: Priority, text: String) {
        if let Some(voice) = &self.voice {
            voice.speak(priority, text);
        }
    }

    fn announce(&self, priority: Priority, phrase: Phrase) {
        if let Some(voice) = &self.voice {
            voice.announce(priority, phrase);
        }
    }

    fn refresh_providers(&self) {
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            let mut keys = HashSet::new();
            for (service, _) in discovery::API_SERVICES {
                if secrets::has(service).await {
                    keys.insert(service);
                }
            }
            let mut providers = discovery::acp_clis();
            providers.extend(discovery::ollama_models(discovery::OLLAMA_URL).await);
            providers.extend(discovery::api_options(|service| keys.contains(service)));
            let _ = inbox.send(Internal::Providers(providers));
        });
    }
}

#[cfg(test)]
mod tests;
