//! Command palette in the LazyVim / VS Code spirit: a floating box with a fuzzy
//! filtered list, and small wizards (new agent, new group, open project, API key,
//! open in editor). Pure logic plus grid drawing; no GTK.

mod actions;
mod draw;
mod items;
mod personas;

use orch_core::{model::AppState, orchestrator::Command};
use uuid::Uuid;

use super::{grid::Style, room::View};

const MAX_ROWS: usize = 12;

/// What the room should do after a key in the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Keep,
    Close,
    /// Send and keep the palette open (the wizard goes on).
    Send(Command),
    /// Send and close.
    Done(Command),
    View(View),
    Select(Uuid),
    /// Open the message prompt for the selected agent.
    Message,
    /// Ask for an API key, masked.
    Secret(String),
    Help,
    /// A `:command` typed in the palette.
    Run(String),
    /// Text to add to the message being typed (a mentioned file).
    Insert(String),
    /// Pick a folder with the system's folder chooser.
    Browse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Commands,
    NewAgent(Wizard),
    NewGroup {
        name: Option<String>,
    },
    OpenProject,
    ApiKey,
    OpenIn,
    /// Pick the provider (or `service/model`) an agent continues on.
    SwitchProvider(Uuid),
    /// Pick the model the pet thinks with.
    PetModel,
    /// Pick a Piper voice (installed or to download).
    Voice,
    /// Type who an agent is.
    Persona(Uuid),
    /// `@` in a message: the project's files, to mention one.
    Mention(Vec<String>),
    /// Pick the model an agent works with, among those its CLI offers.
    Model(Uuid),
    /// Pick the colour an agent's lines are painted in.
    Color(Uuid),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Wizard {
    step: Step,
    group: Option<String>,
    /// A group typed in the wizard that still needs its goal.
    new_group: Option<String>,
    name: Option<String>,
    provider: Option<String>,
    task: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Step {
    #[default]
    Group,
    GroupGoal,
    Name,
    Provider,
    Task,
    Persona,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    NewAgent,
    NewGroup,
    OpenProject,
    /// Agent and whether it is running now.
    Toggle(Uuid, bool),
    Message,
    OpenIn,
    OpenProjectIn,
    ApiKey,
    Remove(Uuid),
    View(View),
    GoTo(Uuid),
    Help,
    /// Wizard values.
    Group(String),
    CreateGroup(String),
    Provider(String),
    Service(String),
    Editor(String),
    Run(String),
    SwitchProvider(Uuid),
    UpdateClis,
    PetModel,
    InstallVoice,
    Merge(Uuid),
    Lead(String),
    Language(String),
    ChooseVoice,
    Voice(String),
    OpenEmail,
    OpenTerminal,
    FocusProject(Uuid),
    CloseProject(Uuid),
    Persona(Uuid),
    OpenPath(std::path::PathBuf),
    Browse,
    Insert(String),
    /// A persona, ready-made or typed; empty for none.
    PersonaText(String),
    ChooseModel(Uuid),
    Model(String),
    /// A placeholder while something is on its way.
    Waiting,
    ChooseColor(Uuid),
    /// `None`: picked from the agent's id; `Some("none")`: plain text.
    Color(Option<String>),
}

#[derive(Debug, Clone)]
struct Item {
    label: String,
    detail: String,
    hint: &'static str,
    pick: Pick,
    style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    mode: Mode,
    query: String,
    cursor: usize,
}

impl Palette {
    pub fn commands() -> Self {
        Self {
            mode: Mode::Commands,
            query: String::new(),
            cursor: 0,
        }
    }

    /// Recent projects, repositories found on disk, a typed path or the folder chooser.
    pub fn open_project() -> Self {
        Self {
            mode: Mode::OpenProject,
            query: String::new(),
            cursor: 0,
        }
    }

    /// The models `agent` can switch to.
    pub fn model(agent: Uuid) -> Self {
        Self {
            mode: Mode::Model(agent),
            query: String::new(),
            cursor: 0,
        }
    }

    /// The files of the project, for `@` in a message.
    pub fn mention(files: Vec<String>) -> Self {
        Self {
            mode: Mode::Mention(files),
            query: String::new(),
            cursor: 0,
        }
    }

    pub fn new_agent() -> Self {
        Self {
            mode: Mode::NewAgent(Wizard::default()),
            query: String::new(),
            cursor: 0,
        }
    }

    fn to(&mut self, mode: Mode) {
        self.mode = mode;
        self.query.clear();
        self.cursor = 0;
    }

    pub fn type_char(&mut self, ch: char) {
        self.query.push(ch);
        self.cursor = 0;
    }

    pub fn backspace(&mut self) {
        if self.query.pop().is_none() {
            self.back();
        }
        self.cursor = 0;
    }

    pub fn move_cursor(&mut self, delta: isize, state: &AppState, selected: Option<Uuid>) {
        let len = self.filtered(state, selected).len();
        if len > 0 {
            self.cursor = (self.cursor as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index;
    }

    /// Esc: one step back, or close from the top.
    pub fn escape(&mut self) -> Outcome {
        if self.mode == Mode::Commands {
            return Outcome::Close;
        }
        if !self.back() {
            return Outcome::Close;
        }
        Outcome::Keep
    }

    fn back(&mut self) -> bool {
        let previous = match &mut self.mode {
            Mode::NewAgent(wizard) => {
                let (step, restore) = match wizard.step {
                    Step::Group => return false,
                    Step::GroupGoal => (Step::Group, wizard.new_group.take()),
                    Step::Name => (Step::Group, wizard.group.take()),
                    Step::Provider => (Step::Name, wizard.name.take()),
                    Step::Task => (Step::Provider, wizard.provider.take()),
                    Step::Persona => (Step::Task, wizard.task.take()),
                };
                wizard.step = step;
                self.query = restore.unwrap_or_default();
                self.cursor = 0;
                return true;
            }
            Mode::NewGroup { name } if name.is_some() => {
                self.query = name.take().unwrap_or_default();
                return true;
            }
            Mode::Commands => return false,
            _ => Mode::Commands,
        };
        self.to(previous);
        true
    }

    /// Free-text steps have no list: Enter takes the query.
    fn free_text(&self) -> bool {
        match &self.mode {
            Mode::NewAgent(w) => matches!(w.step, Step::GroupGoal | Step::Name | Step::Task),
            Mode::NewGroup { .. } => true,
            _ => false,
        }
    }

    /// Items matching the query, best first, with the matched character positions.
    fn filtered(&self, state: &AppState, selected: Option<Uuid>) -> Vec<(Item, Vec<usize>)> {
        let items = self.items(state, selected);
        if self.free_text() {
            return Vec::new();
        }
        let query = self.query.trim();
        let mut scored: Vec<(i32, usize, Item, Vec<usize>)> = items
            .into_iter()
            .enumerate()
            .filter_map(|(order, item)| {
                // "+ create group" and ":command" always match what was typed.
                let custom = matches!(&item.pick, Pick::Provider(_) | Pick::Voice(_))
                    && item.label.starts_with("use ");
                let typed_path = matches!(
                    item.pick,
                    Pick::OpenPath(_) | Pick::PersonaText(_) | Pick::Model(_)
                ) && (item.label.starts_with("open ")
                    || item.label.starts_with("use “"));
                if custom || typed_path || matches!(item.pick, Pick::CreateGroup(_) | Pick::Run(_))
                {
                    return Some((-1000, order, item, Vec::new()));
                }
                // Projects match on their folder too: "personal shop".
                let text = if self.mode == Mode::OpenProject {
                    format!("{} {}", item.label, item.detail)
                } else {
                    item.label.clone()
                };
                fuzzy(query, &text).map(|(score, hits)| (score, order, item, hits))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .map(|(_, _, item, hits)| (item, hits))
            .collect()
    }

    pub fn enter(&mut self, state: &AppState, selected: Option<Uuid>) -> Outcome {
        if self.free_text() {
            let text = self.query.trim().to_owned();
            // A group's goal may be left empty.
            let optional = matches!(&self.mode, Mode::NewAgent(w) if w.step == Step::GroupGoal)
                || matches!(self.mode, Mode::NewGroup { name: Some(_) });
            if text.is_empty() && !optional {
                return Outcome::Keep;
            }
            return self.submit_text(text);
        }
        let list = self.filtered(state, selected);
        let Some((item, _)) = list.get(self.cursor.min(list.len().saturating_sub(1))) else {
            return Outcome::Keep;
        };
        self.pick(item.pick.clone(), selected)
    }

    /// Index in the filtered list of the `n`th visible row.
    pub fn row_index(&self, row: usize) -> usize {
        self.cursor.saturating_sub(MAX_ROWS - 1) + row
    }
}

/// Subsequence match: every query char in order. Higher is better; consecutive and
/// word-start matches score more.
pub fn fuzzy(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let text: Vec<char> = text.chars().collect();
    let mut hits = Vec::new();
    let mut score = 0;
    let mut from = 0;
    let mut previous: Option<usize> = None;
    for wanted in query.chars().flat_map(char::to_lowercase) {
        let index =
            (from..text.len()).find(|&i| text[i].to_lowercase().eq(std::iter::once(wanted)))?;
        score += 10;
        if previous == Some(index.wrapping_sub(1)) {
            score += 15;
        }
        if index == 0 || !text[index - 1].is_alphanumeric() {
            score += 10;
        }
        score -= index as i32 / 4;
        hits.push(index);
        previous = Some(index);
        from = index + 1;
    }
    Some((score, hits))
}

#[cfg(test)]
mod tests;
