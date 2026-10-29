//! Keyboard and mouse: keys, clicks, the palette's outcomes and moving the selection
//! between agents and projects.

use orch_core::{
    broker::Answer,
    model::{AgentState, Secret},
    orchestrator::Command,
};
use uuid::Uuid;

use super::format::display_order;
use super::{Action, DIM, Key, Prompt, Room, View};
use crate::ui::palette::{Outcome, Palette};

impl Room {
    pub fn key(&mut self, key: Key) {
        if let Some(palette) = self.palette.take() {
            self.palette_key(palette, key);
            return;
        }
        if let Some(prompt) = self.prompt.as_mut() {
            let buffer = match prompt {
                Prompt::Text(text) => text,
                Prompt::Secret { key, .. } => key,
            };
            match key {
                Key::Char(ch) => buffer.push(ch),
                Key::Backspace => {
                    buffer.pop();
                }
                Key::Escape => self.prompt = None,
                Key::Enter => match self.prompt.take() {
                    Some(Prompt::Text(text)) => self.submit(&text),
                    Some(Prompt::Secret { service, key }) if !key.trim().is_empty() => {
                        self.outbox.push(Command::SetApiKey {
                            service,
                            key: Secret(key.trim().to_owned()),
                        });
                    }
                    _ => {}
                },
                _ => {}
            }
            return;
        }

        if self.help {
            if matches!(key, Key::Escape | Key::Char('?' | 'q')) {
                self.help = false;
            }
            return;
        }

        match key {
            Key::Function(n @ 1..=5) => self.set_view(View::ALL[usize::from(n) - 1]),
            Key::Char(ch @ '1'..='5') => self.set_view(View::ALL[ch as usize - '1' as usize]),
            Key::Char('/' | 'i') | Key::Enter => self.prompt = Some(Prompt::Text(String::new())),
            Key::Char('j') | Key::Down => self.step(1),
            Key::Char('k') | Key::Up => self.step(-1),
            Key::Escape if self.modal().is_some() => {
                if let Some(id) = self.modal().map(|r| r.id) {
                    self.deferred.insert(id);
                }
            }
            Key::Palette | Key::Char(':' | ' ') => self.palette = Some(Palette::commands()),
            Key::Char('n') => self.palette = Some(Palette::new_agent()),
            Key::Char('a') => self.answer_first(Answer::Allow),
            Key::Char('l') => self.answer_first(Answer::AlwaysAllow),
            Key::Char('g') => self.answer_first(Answer::AlwaysAllowGroup),
            Key::Char('d') => self.answer_first(Answer::Deny),
            Key::Char('c') => self.ack(),
            Key::Char('s') => self.toggle_selected(),
            Key::Char('e') => self.outbox.push(Command::OpenIn {
                agent: self.selected,
                editor: None,
            }),
            Key::Char('?') => self.help = true,
            Key::Char('[') => self.cycle_project(-1),
            Key::Char(']') => self.cycle_project(1),
            Key::Project(n) => {
                let id = self
                    .state
                    .as_ref()
                    .and_then(|s| s.projects.get(usize::from(n).saturating_sub(1)))
                    .map(|p| p.id);
                if let Some(id) = id {
                    self.focus_project(id);
                }
            }
            Key::Escape => self.flash = None,
            _ => {}
        }
    }

    pub fn click(&mut self, col: usize, row: usize) {
        if self.help {
            self.help = false;
            return;
        }
        let action = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(col, row))
            .map(|(_, action)| *action);
        match action {
            Some(Action::View(view)) => self.set_view(view),
            Some(Action::Select(id)) => self.select(Some(id)),
            Some(Action::FocusProject(id)) => self.focus_project(id),
            Some(Action::Answer(id, answer)) => self.answer(id, answer),
            Some(Action::Ack) => self.ack(),
            Some(Action::Prompt) => {
                self.prompt
                    .get_or_insert_with(|| Prompt::Text(String::new()));
            }
            Some(Action::Defer(id)) => {
                self.deferred.insert(id);
            }
            Some(Action::PaletteRow(row)) => {
                if let Some(mut palette) = self.palette.take() {
                    palette.set_cursor(palette.row_index(row));
                    let state = self.state.clone().unwrap_or_default();
                    let outcome = palette.enter(&state, self.selected);
                    self.apply(palette, outcome);
                }
            }
            Some(Action::ClosePalette) => self.palette = None,
            None => {}
        }
    }

    pub(super) fn palette_key(&mut self, mut palette: Palette, key: Key) {
        let state = self.state.clone().unwrap_or_default();
        let outcome = match key {
            Key::Char(ch) => {
                palette.type_char(ch);
                Outcome::Keep
            }
            Key::Backspace => {
                palette.backspace();
                Outcome::Keep
            }
            Key::Up => {
                palette.move_cursor(-1, &state, self.selected);
                Outcome::Keep
            }
            Key::Down => {
                palette.move_cursor(1, &state, self.selected);
                Outcome::Keep
            }
            Key::Escape => palette.escape(),
            Key::Enter => palette.enter(&state, self.selected),
            Key::Palette => Outcome::Close,
            Key::Function(_) | Key::Project(_) => Outcome::Keep,
        };
        self.apply(palette, outcome);
    }

    pub(super) fn apply(&mut self, palette: Palette, outcome: Outcome) {
        match outcome {
            Outcome::Keep => self.palette = Some(palette),
            Outcome::Close => {}
            Outcome::Send(command) => {
                self.outbox.push(command);
                self.palette = Some(palette);
            }
            Outcome::Done(command) => self.outbox.push(command),
            Outcome::View(view) => self.set_view(view),
            Outcome::Select(id) => self.select(Some(id)),
            Outcome::Message => self.prompt = Some(Prompt::Text(String::new())),
            Outcome::Secret(service) => {
                self.prompt = Some(Prompt::Secret {
                    service,
                    key: String::new(),
                });
            }
            Outcome::Help => self.help = true,
            Outcome::Run(command) => self.run(&command),
        }
    }

    /// Plain text goes to agents (parsed like speech in the runtime); `:` starts a command.
    pub(super) fn submit(&mut self, text: &str) {
        let text = text.trim();
        match text.strip_prefix(':') {
            Some(command) => self.run(command.trim()),
            None if text.is_empty() => {}
            None => self.outbox.push(Command::Say {
                text: text.to_owned(),
                selected: self.selected,
            }),
        }
    }

    pub(super) fn step(&mut self, delta: isize) {
        let Some(state) = &self.state else { return };
        let order = display_order(state);
        if order.is_empty() {
            return;
        }
        let current = order.iter().position(|a| Some(a.id) == self.selected);
        let next = match current {
            Some(index) => (index as isize + delta).rem_euclid(order.len() as isize) as usize,
            None => 0,
        };
        let id = order[next].id;
        self.select(Some(id));
    }

    pub(super) fn toggle_selected(&mut self) {
        let Some(agent) = self.selected.and_then(|id| self.state.as_ref()?.agent(id)) else {
            return;
        };
        let command = match agent.state {
            AgentState::Stopped | AgentState::Error => Command::StartAgent(agent.id),
            _ => Command::StopAgent(agent.id),
        };
        self.outbox.push(command);
    }

    pub(super) fn answer_first(&mut self, answer: Answer) {
        let first = self.modal().map(|p| p.id).or_else(|| {
            self.state
                .as_ref()
                .and_then(|s| s.permissions.first())
                .map(|p| p.id)
        });
        match first {
            Some(id) => self.answer(id, answer),
            None => self.say("no pending requests", DIM),
        }
    }

    pub(super) fn ack(&mut self) {
        if let Some(state) = &self.state {
            self.acked.extend(state.permissions.iter().map(|p| p.id));
        }
    }

    pub(super) fn focus_project(&mut self, id: Uuid) {
        if self.state.as_ref().is_some_and(|s| s.focus != Some(id)) {
            self.outbox.push(Command::FocusProject(id));
        }
    }

    /// `Alt+n` picks the nth open project; `delta` steps through them (`[` `]`).
    pub(super) fn cycle_project(&mut self, delta: isize) {
        let Some(state) = &self.state else { return };
        let len = state.projects.len();
        if len < 2 {
            return;
        }
        let current = state
            .projects
            .iter()
            .position(|p| Some(p.id) == state.focus)
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(len as isize) as usize;
        let id = state.projects[next].id;
        self.focus_project(id);
    }
}
