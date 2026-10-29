//! Panels around the map: permission queue, agent table, console, voice strip,
//! command line and the event log.

use chrono::{DateTime, Utc};
use orch_core::{
    broker::Answer,
    model::{AppState, LogEntry, LogLevel},
};

use super::format::{
    clock, display_order, format_tokens, lamp_for, level_style, sparkline, state_label, uptime,
};
use super::{
    ACCENT, Action, DIM, ERROR, FAINT, FRAME, OK, Prompt, Room, SPARK_WIDTH, Spans, TEXT, TITLE,
    WARN, s, title, title_right,
};
use crate::ui::grid::{Grid, Rect, fit};

impl Room {
    pub(super) fn draw_queue(
        &mut self,
        grid: &mut Grid,
        area: Rect,
        state: &AppState,
        now: DateTime<Utc>,
        detailed: bool,
    ) {
        let x = area.x + 1;
        let width = area.w.saturating_sub(2);
        if state.permissions.is_empty() {
            grid.text(x, area.y, width, "no pending requests", DIM);
            grid.text(x, area.y + 1, width, "every agent is clear", FAINT);
            return;
        }

        let mut y = area.y;
        for (index, request) in state.permissions.iter().enumerate() {
            let focused = index == 0;
            let with_buttons = focused || detailed;
            let height = 3 + usize::from(with_buttons) + if detailed { 2 } else { 0 };
            if y + height > area.bottom() {
                let left = state.permissions.len() - index;
                grid.text(
                    x,
                    area.bottom() - 1,
                    width,
                    &format!("+{left} more · F3"),
                    DIM,
                );
                break;
            }
            let agent = state.agent(request.agent_id);
            let name = agent.map_or("?", |a| a.name.as_str());
            let group = agent
                .and_then(|a| state.group(a.group_id))
                .map_or("?", |g| g.name.as_str());
            let lamp = if focused && !self.acked.contains(&request.id) && !self.blink() {
                " "
            } else {
                "◆"
            };
            let head_style = if focused { WARN.bold() } else { WARN };
            grid.spans(
                x,
                y,
                width,
                &[
                    s(lamp, head_style),
                    s(format!(" {name}"), head_style),
                    s(format!(" · {group}"), DIM),
                ],
            );
            y += 1;
            grid.spans(
                x,
                y,
                width,
                &[
                    s("  ", TEXT),
                    s(&request.tool, ACCENT.bold()),
                    s(": ", DIM),
                    s(
                        fit(
                            &request.input_summary(),
                            width.saturating_sub(4 + request.tool.len()),
                        ),
                        TEXT,
                    ),
                ],
            );
            y += 1;
            if detailed {
                grid.spans(
                    x,
                    y,
                    width,
                    &[s("  says> ", DIM), s(&request.spoken_summary, TEXT)],
                );
                y += 1;
                grid.spans(
                    x,
                    y,
                    width,
                    &[s("  input> ", DIM), s(request.input.to_string(), FAINT)],
                );
                y += 1;
            }
            if with_buttons {
                let mut bx = x + 2;
                for (answer, spans) in [
                    (
                        Answer::Allow,
                        vec![s("[", DIM), s("A", ACCENT.bold()), s("]llow", TEXT)],
                    ),
                    (
                        Answer::AlwaysAllow,
                        vec![s("A[", DIM), s("l", ACCENT.bold()), s("]ways", TEXT)],
                    ),
                    (
                        Answer::Deny,
                        vec![s("[", DIM), s("D", ERROR.bold()), s("]eny", TEXT)],
                    ),
                ] {
                    let used = grid.spans(bx, y, area.right().saturating_sub(bx + 1), &spans);
                    self.hits.push((
                        Rect::new(bx, y, used, 1),
                        Action::Answer(request.id, answer),
                    ));
                    bx += used + 2;
                }
                y += 1;
            }
            let left = (request.expires_at() - now).num_seconds().max(0);
            let style = if left < 60 { WARN.bold() } else { DIM };
            let expires = format!("  expires in {:02}:{:02}", left / 60, left % 60);
            grid.text(x, y, width, &expires, style);
            y += 2;
        }
    }

    pub(super) fn draw_agent_table(
        &mut self,
        grid: &mut Grid,
        area: Rect,
        state: &AppState,
        now: DateTime<Utc>,
    ) {
        let x = area.x + 1;
        let width = area.w.saturating_sub(2);
        let header = format!(
            "{}{}{}{}{}{}{}{}TASK",
            fit("ST", 3),
            fit("NAME", 14),
            fit("GROUP", 11),
            fit("PROVIDER", 11),
            fit("STATE", 21),
            fit("UPTIME", 8),
            fit("TOKENS", 8),
            fit("ACTIVITY", SPARK_WIDTH + 2),
        );
        grid.text(x, area.y, width, &header, DIM.bold());
        for (row, agent) in display_order(state).into_iter().enumerate() {
            let y = area.y + 1 + row;
            if y >= area.bottom() {
                break;
            }
            let (glyph, lamp) = lamp_for(agent.state);
            let selected = self.selected == Some(agent.id);
            let group = state.group(agent.group_id).map_or("?", |g| g.name.as_str());
            let spans = vec![
                s(format!("{glyph}  "), lamp),
                s(
                    fit(&agent.name, 13),
                    if selected {
                        ACCENT.bold().inverse()
                    } else {
                        TEXT.bold()
                    },
                ),
                s(" ", TEXT),
                s(fit(group, 11), ACCENT),
                s(fit(agent.provider.short_name(), 11), DIM),
                s(fit(state_label(agent.state), 21), lamp),
                s(fit(&uptime(agent, now), 8), DIM),
                s(fit(&format_tokens(agent.usage.total()), 8), DIM),
                s(sparkline(state, agent.id, now), FAINT),
                s("  ", TEXT),
                s(&agent.task, TEXT),
            ];
            grid.spans(x, y, width, &spans);
            self.hits
                .push((Rect::new(area.x, y, area.w, 1), Action::Select(agent.id)));
        }
    }

    pub(super) fn draw_console(&self, grid: &mut Grid, area: Rect, sep: usize, state: &AppState) {
        let x = area.x + 1;
        let width = area.w.saturating_sub(2);
        let Some(agent) = self.selected.and_then(|id| state.agent(id)) else {
            title(grid, 0, sep, &[s("AGENT", TITLE)]);
            grid.text(
                x,
                area.y,
                width,
                "no agent selected · j/k or click an agent",
                DIM,
            );
            return;
        };
        let group = state.group(agent.group_id).map_or("?", |g| g.name.as_str());
        title(
            grid,
            0,
            sep,
            &[
                s("AGENT // ", TITLE),
                s(agent.name.to_uppercase(), ACCENT.bold()),
                s(format!(" · {group}"), DIM),
            ],
        );
        let (glyph, lamp) = lamp_for(agent.state);
        title_right(
            grid,
            grid.cols - 2,
            sep,
            &[
                s(agent.provider.short_name(), DIM),
                s(" ═ ", FRAME),
                s(format!("{glyph} {}", state_label(agent.state)), lamp.bold()),
            ],
        );

        grid.spans(
            x,
            area.y,
            width,
            &[
                s("task> ", DIM),
                s(&agent.task, TEXT),
                s(
                    if agent.persona.is_empty() {
                        ""
                    } else {
                        "   who> "
                    },
                    DIM,
                ),
                s(&agent.persona, ACCENT),
                s("   worktree> ", DIM),
                s(agent.worktree.display().to_string(), FAINT),
            ],
        );

        let request = state.permissions.iter().find(|p| p.agent_id == agent.id);
        let room = area.h.saturating_sub(1 + usize::from(request.is_some()));
        let lines: Vec<&LogEntry> = state
            .log
            .iter()
            .filter(|e| e.agent_id == Some(agent.id))
            .collect();
        let output = Rect::new(area.x, area.y + 1, area.w, room);
        draw_wrapped(grid, output, lines.iter().copied(), 11, |entry| {
            vec![s(clock(entry.at), DIM), s(" > ", ACCENT)]
        });
        if lines.is_empty() && room > 0 {
            grid.text(x, area.y + 1, width, "no output yet", FAINT);
        }
        if let Some(request) = request {
            let style = if self.blink() { WARN.bold() } else { WARN };
            let text = format!(
                "◆ waiting for permission: {} {} · a allow · l always · d deny",
                request.tool,
                request.input_summary()
            );
            grid.text(x, area.bottom() - 1, width, &text, style);
        }
    }

    pub(super) fn draw_voice(&self, grid: &mut Grid, area: Rect, sep: usize) {
        let voice = self
            .state
            .as_ref()
            .map(|s| s.voice.clone())
            .unwrap_or_default();
        title(grid, 0, sep, &[s("VOICE // PUSH-TO-TALK", TITLE)]);
        let lamp = |on: bool, name: &str| {
            if on {
                s(format!("{name} ● ONLINE"), OK)
            } else {
                s(format!("{name} ○ OFFLINE"), DIM)
            }
        };
        title_right(
            grid,
            grid.cols - 2,
            sep,
            &[
                lamp(voice.stt, "STT"),
                s(" ═ ", FRAME),
                lamp(voice.tts, "TTS"),
            ],
        );

        let x = area.x + 1;
        let width = area.w.saturating_sub(2);
        let listening = self.talking || voice.listening;
        let meter = 32.min(width.saturating_sub(70)).max(8);
        let (mic, wave) = if listening {
            let mic = if self.blink() {
                s("● REC     ", ERROR.bold())
            } else {
                s("  REC     ", ERROR.bold())
            };
            let head = self.tick as usize % meter;
            let wave: String = (0..meter)
                .map(|i| match head.abs_diff(i) {
                    0 => '▇',
                    1 => '▅',
                    2 => '▃',
                    3 => '▂',
                    _ => '▁',
                })
                .collect();
            (mic, s(wave, ERROR))
        } else if voice.transcribing {
            (s("◐ HEARING ", ACCENT.bold()), s("▁".repeat(meter), ACCENT))
        } else if voice.stt {
            (s("○ READY   ", DIM), s("▁".repeat(meter), FAINT))
        } else {
            (s("✖ OFFLINE ", DIM), s("▁".repeat(meter), FAINT))
        };
        let speech = if voice.speaking {
            s("   ▶ SPEAKING", ACCENT.bold())
        } else if voice.armed {
            s(format!("   ◉ {} IS LISTENING", self.pet.name), ERROR.bold())
        } else if voice.wake {
            s(format!("   wake: \"ei {}\"", self.pet.name), DIM)
        } else {
            s("", TEXT)
        };
        grid.spans(
            x,
            area.y,
            width,
            &[
                s("MIC ", DIM),
                mic,
                s(" ▕", FAINT),
                wave,
                s("▏", FAINT),
                s("   [", DIM),
                s("CTRL+SPACE", ACCENT.bold()),
                s("] hold to talk", DIM),
                speech,
            ],
        );

        let heard = if listening {
            s("listening… release to send", ERROR)
        } else if voice.transcribing {
            s("transcribing…", ACCENT)
        } else if let Some(text) = voice.heard.as_ref().filter(|t| !t.is_empty()) {
            s(text.clone(), TEXT)
        } else if let Some(detail) = &voice.detail {
            s(format!("{detail} · F5 for setup"), FAINT)
        } else {
            s(
                "say \"permitir\", \"negar\", \"resumo\" or \"grupo API, agente Tests: …\"",
                FAINT,
            )
        };
        grid.spans(x, area.y + 1, width, &[s("heard> ", DIM), heard]);
    }

    pub(super) fn draw_command_line(&mut self, grid: &mut Grid) {
        let y = grid.rows - 1;
        let hints: Spans = vec![
            s("[", DIM),
            s("Enter", ACCENT.bold()),
            s("]talk [", DIM),
            s(": ^P", ACCENT.bold()),
            s("]palette [", DIM),
            s("j/k", ACCENT.bold()),
            s("]nav [", DIM),
            s("a/l/d", ACCENT.bold()),
            s("]answer [", DIM),
            s("?", ACCENT.bold()),
            s("]help", DIM),
        ];
        let hints_x = title_right(grid, grid.cols - 2, y, &hints);
        let x = 2;
        let width = hints_x.saturating_sub(x + 2);
        let cursor = if self.blink() { "█" } else { " " };
        let target = self
            .selected
            .and_then(|id| self.state.as_ref()?.agent(id))
            .map_or_else(|| "no agent".to_owned(), |a| a.name.clone());
        let mut spans = vec![s(" > ", ACCENT.bold())];
        let shown = |text: &str, room: usize| -> String {
            let skip = text.chars().count().saturating_sub(room);
            text.chars().skip(skip).collect()
        };
        match (&self.prompt, &self.flash) {
            (Some(Prompt::Text(text)), _) if text.starts_with(':') => {
                spans.push(s(shown(text, width.saturating_sub(6)), ACCENT.bold()));
                spans.push(s(cursor, ACCENT));
            }
            (Some(Prompt::Text(text)), _) => {
                let pet = text.starts_with('@') || self.selected.is_none();
                let label = if pet {
                    format!("to {} (pet) › ", self.pet.name)
                } else {
                    format!("to {target} › ")
                };
                let room = width.saturating_sub(label.chars().count() + 6);
                spans.push(s(label, DIM));
                spans.push(s(shown(text, room), TEXT.bold()));
                spans.push(s(cursor, ACCENT));
            }
            (Some(Prompt::Secret { service, key }), _) => {
                spans.push(s(format!("{service} API key › "), WARN));
                spans.push(s("•".repeat(key.chars().count().min(40)), TEXT));
                spans.push(s(cursor, ACCENT));
            }
            (None, Some((text, style, _))) => spans.push(s(text.as_str(), *style)),
            (None, None) => spans.push(s(
                format!("Enter to talk to {target} · : for the palette"),
                FAINT,
            )),
        }
        spans.push(s(" ", TEXT));
        let used = grid.spans(x, y, width, &spans);
        if used < width && self.prompt.is_some() {
            grid.text(x + used, y, width - used, &" ".repeat(width - used), TEXT);
        }
        self.hits.push((Rect::new(x, y, width, 1), Action::Prompt));
    }
}

/// Splits `text` into rows of at most `width` characters, breaking at spaces when
/// it can.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut rows = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > width {
        let cut = rest[..=width]
            .iter()
            .rposition(|c| *c == ' ')
            .filter(|i| *i > width / 3)
            .unwrap_or(width);
        rows.push(rest[..cut].iter().collect::<String>().trim_end().to_owned());
        rest.drain(..cut);
        while rest.first() == Some(&' ') {
            rest.remove(0);
        }
    }
    rows.push(rest.into_iter().collect());
    rows
}

/// Draws entries newest at the bottom, each wrapped under its text column after a
/// `prefix` of fixed width.
pub(super) fn draw_wrapped<'a>(
    grid: &mut Grid,
    area: Rect,
    entries: impl DoubleEndedIterator<Item = &'a LogEntry>,
    prefix_width: usize,
    prefix: impl Fn(&LogEntry) -> Spans,
) {
    let width = area.w.saturating_sub(2);
    let text_width = width.saturating_sub(prefix_width);
    let mut bottom = area.bottom();
    for entry in entries.rev() {
        if bottom <= area.y {
            break;
        }
        let rows = wrap(&entry.text, text_width);
        let first = bottom.saturating_sub(rows.len()).max(area.y);
        let skip = rows.len().saturating_sub(bottom - first);
        for (index, row) in rows.iter().enumerate().skip(skip) {
            let y = first + index - skip;
            let x = area.x + 1;
            if index == 0 {
                let used = grid.spans(x, y, width, &prefix(entry));
                grid.text(x + used, y, text_width, row, level_style(entry.level));
            } else {
                grid.text(
                    x + prefix_width,
                    y,
                    text_width,
                    row,
                    level_style(entry.level),
                );
            }
        }
        bottom = first;
    }
}

pub(super) fn draw_log(grid: &mut Grid, area: Rect, state: &AppState, pet: &str) {
    draw_wrapped(grid, area, state.log.iter(), 20, |entry| {
        let fallback = if entry.level == LogLevel::Pet {
            pet
        } else {
            "runtime"
        };
        let source = entry
            .agent_id
            .and_then(|id| state.agent(id))
            .map_or(fallback, |a| a.name.as_str());
        vec![
            s(clock(entry.at), DIM),
            s(" ", TEXT),
            s(fit(source, 9), ACCENT),
            s(" ", TEXT),
        ]
    });
}
