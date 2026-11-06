//! Drawing the floating palette box on the grid.

use orch_core::model::AppState;
use uuid::Uuid;

use super::{MAX_ROWS, Mode, Palette, Step};
use crate::ui::{
    grid::{Grid, Lines, Rect, Style, Weight, fit},
    room::{ACCENT, DIM, FAINT, TEXT, TITLE, s, title, title_right},
};

impl Palette {
    pub(super) fn breadcrumb(&self) -> Vec<(String, Style)> {
        let sep = || s(" › ", DIM);
        match &self.mode {
            Mode::Commands => vec![s("⌘ COMMANDS", TITLE)],
            Mode::NewAgent(w) => {
                let mut spans = vec![s("NEW AGENT", TITLE)];
                for value in [
                    w.group.as_ref().or(w.new_group.as_ref()),
                    w.name.as_ref(),
                    w.provider.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    spans.push(sep());
                    spans.push(s(value.clone(), ACCENT.bold()));
                }
                spans
            }
            Mode::NewGroup { name } => {
                let mut spans = vec![s("NEW GROUP", TITLE)];
                if let Some(name) = name {
                    spans.push(sep());
                    spans.push(s(name.clone(), ACCENT.bold()));
                }
                spans
            }
            Mode::OpenProject => vec![s("OPEN PROJECT", TITLE)],
            Mode::Mention(_) => vec![s("MENTION A FILE", TITLE)],
            Mode::Model(_) => vec![s("MODEL", TITLE)],
            Mode::Color(_) => vec![s("COLOR", TITLE)],
            Mode::DeleteKey => vec![s("DELETE API KEY", TITLE)],
            Mode::Theme => vec![s("THEME", TITLE)],
            Mode::Library => vec![s("AGENT LIBRARY", TITLE)],
            Mode::ApiKey => vec![s("API KEY", TITLE)],
            Mode::OpenIn => vec![s("OPEN IN EDITOR", TITLE)],
            Mode::SwitchProvider(_) => vec![s("SWITCH PROVIDER", TITLE)],
            Mode::PetModel => vec![s("PET MODEL", TITLE)],
            Mode::Voice => vec![s("PET VOICE", TITLE)],
            Mode::Persona(_) => vec![s("PERSONA", TITLE)],
        }
    }

    /// The field being filled and what to type there.
    pub(super) fn field(&self) -> (&'static str, &'static str) {
        match &self.mode {
            Mode::Commands => (
                "",
                "search actions, or type a command: open ~/repo, new …, key …",
            ),
            Mode::NewAgent(w) => match w.step {
                Step::Group => ("group", "pick a group, or type a new name"),
                Step::Source => (
                    "agent",
                    "a new one, or one from your library (it brings its persona and memory)",
                ),
                Step::GroupGoal => (
                    "goal",
                    "optional: context every agent of the group gets · Enter skips",
                ),
                Step::Name => (
                    "name",
                    "short and speakable · Enter on empty: the app picks one",
                ),
                Step::Provider => ("provider", "who runs it · F5 lists them"),
                Step::Task => ("task", "what this agent should do"),
                Step::Persona => (
                    "persona",
                    "pick one (marketing, backend…) or describe your own · Enter creates it",
                ),
            },
            Mode::NewGroup { name: None } => ("name", "the group's name"),
            Mode::NewGroup { name: Some(_) } => (
                "goal",
                "optional: context every agent of the group gets · Enter skips",
            ),
            Mode::OpenProject => (
                "project",
                "type its name, or a path · Browse… picks a folder",
            ),
            Mode::Mention(_) => ("file", "type part of its name or path · Esc keeps the @"),
            Mode::Theme => ("theme", "the room's colours · red stays for what is urgent"),
            Mode::Library => (
                "agent",
                "pick one to put to work here · :forget <name> removes one",
            ),
            Mode::DeleteKey => (
                "service",
                "its key leaves the Secret Service; :key <service> stores a new one",
            ),
            Mode::Color(_) => (
                "color",
                "its lines in the log and its name everywhere · red stays for what is urgent",
            ),
            Mode::Model(_) => (
                "model",
                "what the agent's CLI offers, or type an id · same provider, conversation kept",
            ),
            Mode::ApiKey => ("service", "the key is typed next, masked"),
            Mode::OpenIn => ("editor", "opens the selected agent's worktree"),
            Mode::SwitchProvider(_) => (
                "provider",
                "or service/model · it continues with the conversation",
            ),
            Mode::Persona(_) => (
                "persona",
                "pick one (marketing, backend…) or describe your own",
            ),
            Mode::Voice => (
                "voice",
                "installed or downloaded from Piper (lang_REGION-name-quality)",
            ),
            Mode::PetModel => (
                "model",
                "an API or local model; fast ones feel better (anthropic/claude-haiku-5-5)",
            ),
        }
    }

    /// Draws the floating box; returns the rows of the list for click handling.
    pub fn draw(
        &self,
        grid: &mut Grid,
        state: &AppState,
        selected: Option<Uuid>,
        blink: bool,
    ) -> Vec<Rect> {
        let list = self.filtered(state, selected);
        let (field, help) = self.field();
        let free = self.free_text();
        let rows = if free {
            3
        } else {
            list.len().clamp(1, MAX_ROWS)
        };
        let w = 86.min(grid.cols.saturating_sub(8)).max(40);
        let h = rows + 4;
        let rect = Rect::new((grid.cols - w) / 2, (grid.rows / 6).max(3), w, h);
        grid.fill(rect, ' ', TEXT);
        let mut lines = Lines::new(grid.cols, grid.rows);
        lines.rect(rect, Weight::Double);
        lines.hline(rect.y + 2, rect.x, rect.right() - 1, Weight::Double);
        lines.draw(grid, ACCENT);
        title(grid, rect.x, rect.y, &self.breadcrumb());

        // Input line.
        let inner = rect.w - 4;
        let x = rect.x + 2;
        let label = if field.is_empty() {
            "› ".to_owned()
        } else {
            format!("{field} › ")
        };
        let room = inner.saturating_sub(label.chars().count() + 12);
        let shown: String = {
            let skip = self.query.chars().count().saturating_sub(room);
            self.query.chars().skip(skip).collect()
        };
        let cursor = if blink { "█" } else { " " };
        let used = grid.spans(
            x,
            rect.y + 1,
            inner,
            &[
                s(label, ACCENT.bold()),
                s(shown, TEXT.bold()),
                s(cursor, ACCENT),
            ],
        );
        if self.query.is_empty() {
            grid.text(
                x + used,
                rect.y + 1,
                inner.saturating_sub(used),
                help,
                FAINT,
            );
        }
        if !free {
            let count = format!(
                "{}/{}",
                if list.is_empty() { 0 } else { self.cursor + 1 },
                list.len()
            );
            grid.text(
                rect.right() - 2 - count.len(),
                rect.y + 1,
                count.len(),
                &count,
                DIM,
            );
        }

        let mut hits = Vec::new();
        let top = rect.y + 3;
        if free {
            // Preview of long text (tasks, goals), wrapped.
            let preview = if self.query.is_empty() {
                help.to_owned()
            } else {
                self.query.clone()
            };
            let chars: Vec<char> = preview.chars().collect();
            for (row, chunk) in chars.chunks(inner).take(rows).enumerate() {
                let line: String = chunk.iter().collect();
                grid.text(
                    x,
                    top + row,
                    inner,
                    &line,
                    if self.query.is_empty() { FAINT } else { DIM },
                );
            }
        } else if list.is_empty() {
            grid.text(x, top, inner, "no match", FAINT);
        } else {
            let start = self.cursor.saturating_sub(rows - 1);
            for (row, (entry, matched)) in list.iter().enumerate().skip(start).take(rows) {
                let y = top + row - start;
                let active = row == self.cursor;
                grid.text(x - 1, y, 1, if active { "▌" } else { " " }, ACCENT.bold());
                let label_w = 34.min(inner / 2);
                for (i, ch) in fit(&entry.label, label_w).chars().enumerate() {
                    let style = if matched.contains(&i) {
                        ACCENT.bold()
                    } else {
                        entry.style
                    };
                    grid.put(x + 1 + i, y, ch, style);
                }
                let detail_w = inner.saturating_sub(label_w + 10);
                grid.text(
                    x + 2 + label_w,
                    y,
                    detail_w,
                    &fit(&entry.detail, detail_w),
                    DIM,
                );
                if !entry.hint.is_empty() {
                    let hint = format!("[{}]", entry.hint);
                    grid.text(rect.right() - 2 - hint.len(), y, hint.len(), &hint, FAINT);
                }
                if active {
                    grid.restyle(x - 1, y, inner + 1, TEXT.bold().inverse());
                }
                hits.push(Rect::new(rect.x + 1, y, rect.w - 2, 1));
            }
        }
        let keys = if free {
            "Enter confirm · Backspace on empty goes back · Esc back"
        } else {
            "↑↓ ^j ^k move · Enter pick · Esc back"
        };
        title_right(grid, rect.right() - 2, rect.bottom() - 1, &[s(keys, FAINT)]);
        hits
    }
}
