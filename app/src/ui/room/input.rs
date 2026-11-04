//! Keyboard and mouse: keys, clicks, the palette's outcomes and moving the selection
//! between agents and projects.

use orch_core::{
    broker::Answer,
    model::{AgentState, Secret},
    orchestrator::Command,
};
use uuid::Uuid;

use super::format::display_order;
use super::{Action, DIM, Key, Prompt, Room, Scroll, View, editor::Editor};
use crate::ui::palette::{Outcome, Palette};

impl Room {
    pub fn key(&mut self, key: Key) {
        // A key during the intro only skips it.
        if self.skip_intro() {
            return;
        }
        if let Some(palette) = self.palette.take() {
            self.palette_key(palette, key);
            return;
        }
        if self.prompt.is_some() {
            self.prompt_key(key);
            return;
        }

        if self.help {
            if matches!(key, Key::Escape | Key::Char('?' | 'q')) {
                self.help = false;
            }
            return;
        }

        match key {
            Key::Function(n @ 1..=6) => self.set_view(View::ALL[usize::from(n) - 1]),
            Key::Char(ch @ '1'..='6') => self.set_view(View::ALL[ch as usize - '1' as usize]),
            Key::Char('r') => self.set_view(View::Reader),
            Key::Char('/' | 'i') | Key::Enter => {
                self.prompt = Some(Prompt::Text(Editor::default()));
            }
            Key::Char('j') | Key::Down => self.step(1),
            Key::Char('k') | Key::Up => self.step(-1),
            Key::Escape if self.modal().is_some() => {
                if let Some(id) = self.modal().map(|r| r.id) {
                    self.deferred.insert(id);
                }
            }
            Key::Palette | Key::Char(':' | ' ') => self.palette = Some(Palette::commands()),
            Key::Char('n') => self.palette = Some(Palette::new_agent()),
            Key::Char('o') => self.palette = Some(Palette::open_project()),
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
            Key::Page { up, log } => {
                // A page is most of the console (or log) height.
                let rows = if log { 12 } else { 6 };
                let scroll = if log {
                    &mut self.log_scroll
                } else {
                    &mut self.console_scroll
                };
                scroll.by(if up { rows } else { -rows });
            }
            Key::End => {
                self.log_scroll = Scroll::default();
                self.console_scroll = Scroll::default();
            }
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

    /// Text from the clipboard, where the operator is typing: the palette, the API
    /// key (trimmed: keys are often copied with a line break) or the message.
    pub fn paste(&mut self, text: &str) {
        if let Some(palette) = &mut self.palette {
            palette.type_str(text.trim());
            return;
        }
        match &mut self.prompt {
            Some(Prompt::Secret { key, .. }) => key.push_str(text.trim()),
            Some(Prompt::Text(editor)) => editor.insert_str(&text.replace("\r\n", "\n")),
            // Nothing open: the pasted text starts a message.
            None => {
                let mut editor = Editor::default();
                editor.insert_str(&text.replace("\r\n", "\n"));
                self.prompt = Some(Prompt::Text(editor));
            }
        }
    }

    /// A key while the command line is open: edit the message, or type the API key.
    fn prompt_key(&mut self, key: Key) {
        let width = self.editor_width;
        match (self.prompt.as_mut(), key) {
            (_, Key::Escape) => self.prompt = None,
            (Some(Prompt::Secret { .. }), Key::Enter) => {
                if let Some(Prompt::Secret { service, key }) = self.prompt.take()
                    && !key.trim().is_empty()
                {
                    self.outbox.push(Command::SetApiKey {
                        service,
                        key: Secret(key.trim().to_owned()),
                    });
                }
            }
            (Some(Prompt::Secret { key, .. }), Key::Char(ch)) => key.push(ch),
            (Some(Prompt::Secret { key, .. }), Key::Backspace) => {
                key.pop();
            }
            (Some(Prompt::Text(_)), Key::Enter) => {
                if let Some(Prompt::Text(editor)) = self.prompt.take() {
                    self.submit(editor.text());
                }
            }
            // `@` starting a word mentions a file of the project.
            (Some(Prompt::Text(editor)), Key::Char('@'))
                if editor.before_cursor().is_none_or(char::is_whitespace) =>
            {
                editor.insert('@');
                let files = self
                    .mention_root()
                    .map(|root| orch_core::discovery::project_files(&root, MENTION_FILES))
                    .unwrap_or_default();
                self.palette = Some(Palette::mention(files));
            }
            (Some(Prompt::Text(editor)), key) => match key {
                Key::Char(ch) => editor.insert(ch),
                Key::NewLine => editor.insert('\n'),
                Key::Backspace => editor.backspace(),
                Key::Delete => editor.delete(),
                Key::Left => editor.left(),
                Key::Right => editor.right(),
                Key::Home => editor.home(width),
                Key::End => editor.end(width),
                Key::Up => editor.vertical(-1, width),
                Key::Down => editor.vertical(1, width),
                _ => {}
            },
            _ => {}
        }
    }

    pub fn click(&mut self, col: usize, row: usize) {
        if self.skip_intro() {
            return;
        }
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
                    .get_or_insert_with(|| Prompt::Text(Editor::default()));
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
            Key::Function(_)
            | Key::Project(_)
            | Key::NewLine
            | Key::Delete
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End
            | Key::Page { .. } => Outcome::Keep,
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
            Outcome::Message => self.prompt = Some(Prompt::Text(Editor::default())),
            Outcome::Secret(service) => {
                self.prompt = Some(Prompt::Secret {
                    service,
                    key: String::new(),
                });
            }
            Outcome::Help => self.help = true,
            Outcome::Run(command) => self.run(&command),
            Outcome::Insert(text) => {
                if let Some(Prompt::Text(editor)) = &mut self.prompt {
                    editor.insert_str(&text);
                }
            }
            Outcome::Browse => self.browse = true,
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

/// Files offered after `@`; enough for any repository worth mentioning by name.
const MENTION_FILES: usize = 20_000;
