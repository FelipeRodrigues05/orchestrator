//! Control room: UI-side state, keyboard/mouse handling and screen composition.
//!
//! Everything is laid out on a character grid (see `grid`), so this module has no GTK
//! in it; `render` paints the grid and `mod.rs` feeds input in and sends the
//! commands collected here to the runtime.

mod commands;
mod compose;
mod editor;
mod format;
mod input;
mod markdown;
mod modal;
mod pages;
mod panels;
mod sector;

pub(super) use commands::expand_home;
pub(super) use format::AGENT_COLORS;

use editor::Editor;
use panels::Scroll;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use orch_core::{
    broker::Answer,
    model::{AgentState, AppState, LogLevel},
    orchestrator::Command,
};
use uuid::Uuid;

use self::format::{display_order, format_rtt, level_style, user_name};
use super::{
    grid::{Grid, Rect, Style, Tone, spans_width},
    palette::Palette,
    pet::{self, Mood, Pet},
};

pub const MIN_COLS: usize = 90;
pub const MIN_ROWS: usize = 28;
/// Animation tick; blinking toggles every other tick.
pub const TICK: Duration = Duration::from_millis(250);
const PING_EVERY: Duration = Duration::from_secs(2);
const LINK_TIMEOUT: Duration = Duration::from_secs(6);
const FLASH_FOR: Duration = Duration::from_secs(6);
const SPARK_WIDTH: usize = 8;
const BLOCK_WIDTH: usize = 24;

pub(super) const FRAME: Style = Style::new(Tone::Frame);
pub(super) const TITLE: Style = Style::new(Tone::Title).bold();
pub(super) const TEXT: Style = Style::new(Tone::Text);
pub(super) const DIM: Style = Style::new(Tone::Dim);
pub(super) const FAINT: Style = Style::new(Tone::Faint);
pub(super) const ACCENT: Style = Style::new(Tone::Accent);
pub(super) const OK: Style = Style::new(Tone::Ok);
pub(super) const WARN: Style = Style::new(Tone::Warn);
pub(super) const ERROR: Style = Style::new(Tone::Error);
const STOPPED: Style = Style::new(Tone::Stopped);

type Spans = Vec<(String, Style)>;

pub(super) fn s(text: impl Into<String>, style: Style) -> (String, Style) {
    (text.into(), style)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Agents,
    Permissions,
    Log,
    Providers,
    /// The selected agent's conversation, full screen, easy to read.
    Reader,
}

impl View {
    pub(super) const ALL: [View; 6] = [
        View::Overview,
        View::Agents,
        View::Permissions,
        View::Log,
        View::Providers,
        View::Reader,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            View::Overview => "OVERVIEW",
            View::Agents => "AGENTS",
            View::Permissions => "PERMS",
            View::Log => "LOG",
            View::Providers => "SETUP",
            View::Reader => "READ",
        }
    }

    fn name(self) -> &'static str {
        match self {
            View::Overview => "overview",
            View::Agents => "agents",
            View::Permissions => "perms",
            View::Log => "log",
            View::Providers => "setup",
            View::Reader => "read",
        }
    }

    fn from_name(name: &str) -> Option<View> {
        match name {
            "overview" | "f1" | "1" => Some(View::Overview),
            "agents" | "f2" | "2" => Some(View::Agents),
            "perms" | "permissions" | "f3" | "3" => Some(View::Permissions),
            "log" | "f4" | "4" => Some(View::Log),
            "setup" | "providers" | "f5" | "5" => Some(View::Providers),
            "read" | "reader" | "f6" | "6" => Some(View::Reader),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    /// Shift+Enter: a line break in a message.
    NewLine,
    Escape,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Function(u8),
    /// Ctrl+P: the command palette.
    Palette,
    /// PageUp/PageDown: the console; with Shift, the event log.
    Page {
        up: bool,
        log: bool,
    },
    /// Alt+1…9: the nth open project.
    Project(u8),
}

fn answer_label(answer: Answer) -> &'static str {
    match answer {
        Answer::Allow => "allow",
        Answer::AlwaysAllow => "always allow",
        Answer::AlwaysAllowGroup => "always allow for the group",
        Answer::Deny => "deny",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    View(View),
    Select(Uuid),
    Answer(Uuid, Answer),
    Ack,
    Prompt,
    /// "Later": hide the permission modal for this request.
    Defer(Uuid),
    PaletteRow(usize),
    FocusProject(Uuid),
    ClosePalette,
}

#[derive(Default)]
struct Link {
    /// The runtime channels are open.
    up: bool,
    ping_sent: Option<Instant>,
    last_pong: Option<Instant>,
    rtt: Option<Duration>,
    /// The operator typed `:ping` and wants to see the answer.
    ping_requested: bool,
}

/// What the command line is collecting.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Prompt {
    /// A message for agents, or a `:command`.
    Text(Editor),
    /// An API key for `service`, shown masked.
    Secret { service: String, key: String },
}

pub struct Room {
    state: Option<AppState>,
    view: View,
    selected: Option<Uuid>,
    prompt: Option<Prompt>,
    flash: Option<(String, Style, Instant)>,
    help: bool,
    acked: HashSet<Uuid>,
    link: Link,
    /// Push-to-talk key is down (deduplicates key repeat).
    talking: bool,
    project: Option<Uuid>,
    tick: u64,
    hits: Vec<(Rect, Action)>,
    pet: Pet,
    outbox: Vec<Command>,
    palette: Option<Palette>,
    /// Requests the operator put off with "later"; they stay in the queue.
    deferred: HashSet<Uuid>,
    /// The pet floats over the desktop on a layer-shell surface.
    float_pet: bool,
    /// The compositor offers layer-shell (KDE, Hyprland, Sway; not GNOME).
    can_float: bool,
    /// The operator asked for the system's folder chooser; GTK opens it.
    browse: bool,
    /// Columns the message editor was last drawn with (for Home, End, ↑ ↓).
    editor_width: usize,
    /// The event log (overview and F4) and the selected agent's console.
    log_scroll: Scroll,
    console_scroll: Scroll,
    /// Where the mouse wheel scrolls what, from the last frame.
    scroll_areas: Vec<(Rect, Pane)>,
}

/// A pane that scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Log,
    Console,
}

impl Room {
    pub fn new() -> Self {
        Self {
            state: None,
            view: View::Overview,
            selected: None,
            prompt: None,
            flash: None,
            help: false,
            acked: HashSet::new(),
            link: Link::default(),
            talking: false,
            project: None,
            tick: 0,
            hits: Vec::new(),
            pet: Pet::for_user(&user_name()),
            outbox: Vec::new(),
            palette: None,
            deferred: HashSet::new(),
            float_pet: false,
            can_float: false,
            browse: false,
            editor_width: 80,
            log_scroll: Scroll::default(),
            console_scroll: Scroll::default(),
            scroll_areas: Vec::new(),
        }
    }

    /// Commands collected since the last call, for the runtime.
    pub fn drain(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.outbox)
    }

    pub fn set_state(&mut self, state: AppState) {
        let project = state.focus;
        if project != self.project {
            self.project = project;
            // Restore the board saved with this project.
            let saved = state.layout.as_deref().and_then(|layout| {
                View::ALL
                    .into_iter()
                    .find(|v| layout.contains(&format!("\"view\":\"{}\"", v.name())))
            });
            self.view = saved.unwrap_or(View::Overview);
            self.float_pet = state
                .layout
                .as_deref()
                .is_some_and(|layout| layout.contains("\"float\":true"));
            let selected = state.layout.as_deref().and_then(|layout| {
                let start = layout.find("\"selected\":\"")? + "\"selected\":\"".len();
                Uuid::parse_str(layout[start..].split('"').next()?).ok()
            });
            if let Some(id) = selected.filter(|id| state.agent(*id).is_some()) {
                self.selected = Some(id);
                self.outbox.push(Command::Select(Some(id)));
            }
        }
        let still_there = self.selected.is_some_and(|id| state.agent(id).is_some());
        if !still_there {
            let first = state
                .agents
                .iter()
                .find(|a| a.state == AgentState::AwaitingPermission)
                .or_else(|| display_order(&state).into_iter().next())
                .map(|a| a.id);
            self.selected = first;
            self.outbox.push(Command::Select(first));
        }
        self.acked
            .retain(|id| state.permissions.iter().any(|p| p.id == *id));
        self.deferred
            .retain(|id| state.permissions.iter().any(|p| p.id == *id));
        self.link.up = true;
        self.state = Some(state);
    }

    pub fn notice(&mut self, text: &str, level: LogLevel) {
        self.say(text, level_style(level));
    }

    pub fn disconnect(&mut self) {
        self.link.up = false;
        self.say("runtime disconnected", ERROR);
    }

    /// Advances animations and queues the heartbeat ping when due.
    pub fn tick(&mut self) {
        self.tick += 1;
        let due = self
            .link
            .ping_sent
            .is_none_or(|sent| sent.elapsed() >= PING_EVERY);
        if self.link.up && due {
            self.link.ping_sent = Some(Instant::now());
            self.outbox.push(Command::Ping);
        }
    }

    pub fn on_pong(&mut self) {
        if let Some(sent) = self.link.ping_sent {
            self.link.rtt = Some(sent.elapsed());
        }
        self.link.last_pong = Some(Instant::now());
        if std::mem::take(&mut self.link.ping_requested) {
            let rtt = self.link.rtt.map_or_else(|| "?".into(), format_rtt);
            self.say(&format!("pong from runtime in {rtt}"), OK);
        }
    }

    pub fn voice_press(&mut self) {
        if !self.talking {
            self.talking = true;
            self.outbox.push(Command::VoicePress);
        }
    }

    pub fn voice_release(&mut self) {
        if self.talking {
            self.talking = false;
            self.outbox.push(Command::VoiceRelease);
        }
    }

    pub fn is_typing(&self) -> bool {
        self.prompt.is_some() || self.palette.is_some()
    }

    /// The request shown in the permission modal: the oldest one not put off, while
    /// the operator is not typing.
    fn modal(&self) -> Option<&orch_core::model::PermissionRequest> {
        if self.prompt.is_some() || self.palette.is_some() || self.help {
            return None;
        }
        self.state
            .as_ref()?
            .permissions
            .iter()
            .find(|p| !self.deferred.contains(&p.id))
    }

    /// The mouse wheel over a pane: `rows` towards older lines (negative: newer).
    pub fn scroll(&mut self, col: usize, row: usize, rows: isize) {
        let pane = self
            .scroll_areas
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(col, row))
            .map(|(_, pane)| *pane);
        match pane {
            Some(Pane::Log) => self.log_scroll.by(rows),
            Some(Pane::Console) => self.console_scroll.by(rows),
            None => {}
        }
    }

    /// Whether the folder chooser should open (asked once).
    pub fn take_browse(&mut self) -> bool {
        std::mem::take(&mut self.browse)
    }

    /// Where `@` mentions point: the selected agent's worktree, else the project.
    fn mention_root(&self) -> Option<std::path::PathBuf> {
        let state = self.state.as_ref()?;
        let worktree = self
            .selected
            .and_then(|id| state.agent(id))
            .map(|a| a.worktree.clone());
        worktree
            .filter(|w| w.is_dir())
            .or_else(|| state.project().map(|p| p.root.clone()))
    }

    pub fn send(&mut self, command: Command) {
        self.outbox.push(command);
    }

    /// Wake word on (listening for this pet's name) or off.
    pub fn set_wake(&mut self, on: bool) {
        self.outbox
            .push(Command::SetWake(on.then(|| self.pet.name.to_owned())));
    }

    pub fn send_pet_name(&mut self) {
        self.outbox.push(Command::PetName(self.pet.name.to_owned()));
    }

    fn save_layout(&mut self) {
        let selected = self.selected.map(|id| id.to_string()).unwrap_or_default();
        self.outbox.push(Command::SaveLayout(format!(
            "{{\"view\":\"{}\",\"selected\":\"{selected}\",\"float\":{}}}",
            self.view.name(),
            self.float_pet
        )));
    }

    fn set_view(&mut self, view: View) {
        if view != self.view {
            self.view = view;
            self.save_layout();
        }
    }

    fn select(&mut self, agent: Option<Uuid>) {
        if agent != self.selected {
            self.selected = agent;
            self.console_scroll = Scroll::default();
            self.outbox.push(Command::Select(agent));
            self.save_layout();
        }
    }

    pub fn answer(&mut self, request: Uuid, answer: Answer) {
        let agent = self
            .state
            .as_ref()
            .and_then(|s| {
                s.permissions
                    .iter()
                    .find(|p| p.id == request)
                    .and_then(|p| s.agent(p.agent_id))
            })
            .map_or_else(|| "?".to_owned(), |a| a.name.clone());
        self.outbox.push(Command::Answer { request, answer });
        self.say(
            &format!("{} → {agent}", answer_label(answer)),
            if answer == Answer::Deny { WARN } else { OK },
        );
    }

    fn say(&mut self, text: &str, style: Style) {
        self.flash = Some((text.to_owned(), style, Instant::now()));
    }

    fn blink(&self) -> bool {
        (self.tick / 2).is_multiple_of(2)
    }

    fn link_ok(&self) -> bool {
        self.link.up
            && self
                .link
                .last_pong
                .is_some_and(|at| at.elapsed() < LINK_TIMEOUT)
    }

    fn alarm(&self) -> Alarm {
        let Some(state) = &self.state else {
            return Alarm::None;
        };
        let errors = state.agents.iter().any(|a| a.state == AgentState::Error);
        let pending = !state.permissions.is_empty();
        if !errors && !pending {
            Alarm::None
        } else if errors
            || state
                .permissions
                .iter()
                .any(|p| !self.acked.contains(&p.id))
        {
            Alarm::Active
        } else {
            Alarm::Acked
        }
    }

    fn mood(&self) -> Mood {
        let Some(state) = &self.state else {
            return Mood::Offline;
        };
        let voice = &state.voice;
        if !self.link.up {
            Mood::Offline
        } else if self.talking || voice.listening || voice.armed {
            Mood::Listen
        } else if voice.transcribing {
            Mood::Think
        } else if voice.speaking {
            Mood::Speak
        } else if state.host.thinking {
            Mood::Think
        } else if !state.permissions.is_empty() {
            Mood::Alert
        } else if state.agents.iter().any(|a| a.state == AgentState::Running) {
            Mood::Watch
        } else {
            Mood::Idle
        }
    }

    // ---------------------------------------------------------------- compose

    /// The pet alone, for the floating window: `cols` × `rows` cells.
    pub fn compose_pet(&mut self, cols: usize, rows: usize) -> Grid {
        let mut grid = Grid::new(cols, rows);
        let x = cols.saturating_sub(pet::WIDTH) / 2;
        let base_top = 0;
        self.draw_pet(&mut grid, x, base_top + pet::bob(self.tick), base_top);
        grid
    }

    pub fn set_can_float(&mut self, can: bool) {
        self.can_float = can;
    }

    /// Whether the pet should float over the desktop (`:pet float`).
    pub fn floating(&self) -> bool {
        self.float_pet
    }

    /// Whether the pet should stand still (something needs the operator).
    pub fn pet_alert(&self) -> bool {
        matches!(self.mood(), Mood::Alert | Mood::Listen | Mood::Speak)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alarm {
    None,
    Active,
    Acked,
}

/// Writes ` spans ` on a border line, starting two cells after `x` (`╠═ TITLE ═══`).
pub(super) fn title(grid: &mut Grid, x: usize, y: usize, spans: &[(String, Style)]) {
    let max = grid.cols.saturating_sub(x + 4);
    let mut all = vec![s(" ", TEXT)];
    all.extend_from_slice(spans);
    all.push(s(" ", TEXT));
    grid.spans(x + 2, y, max, &all);
}

/// Writes ` spans ` on a border line, ending right before `end`. Returns where it starts.
pub(super) fn title_right(
    grid: &mut Grid,
    end: usize,
    y: usize,
    spans: &[(String, Style)],
) -> usize {
    let width = spans_width(spans) + 2;
    let x = end.saturating_sub(width);
    let mut all = vec![s(" ", TEXT)];
    all.extend_from_slice(spans);
    all.push(s(" ", TEXT));
    grid.spans(x, y, width, &all);
    x
}

#[cfg(test)]
mod tests;
