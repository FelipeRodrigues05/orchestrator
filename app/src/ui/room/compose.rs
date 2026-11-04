//! Screen composition: the frame, the masthead, the view tabs and where each
//! panel goes.

use chrono::{DateTime, Utc};
use orch_core::model::AgentState;

use super::format::{clock, format_cost, format_rtt, format_tokens, lamp_for};
use super::pages::{draw_help, draw_setup};
use super::panels::draw_log;
use super::{
    ACCENT, Action, Alarm, DIM, ERROR, FAINT, FLASH_FOR, FRAME, MIN_COLS, MIN_ROWS, OK, Pane, Room,
    Spans, TEXT, TITLE, View, WARN, s, title, title_right,
};
use crate::ui::grid::{Grid, Lines, Rect, Weight, fit, spans_width};

impl Room {
    /// Lays the whole room out on a `cols` × `rows` grid and records click targets.
    pub fn compose(&mut self, cols: usize, rows: usize) -> Grid {
        self.hits.clear();
        self.scroll_areas.clear();
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > FLASH_FOR)
        {
            self.flash = None;
        }

        let mut grid = Grid::new(cols, rows);
        if cols < MIN_COLS || rows < MIN_ROWS {
            too_small(&mut grid);
            return grid;
        }

        let now = Utc::now();
        let layout = Layout::new(cols, rows, self.view == View::Overview);
        let mut lines = Lines::new(cols, rows);
        lines.rect(Rect::new(0, 0, cols, rows), Weight::Double);
        for y in [2, layout.console_sep, layout.voice_sep] {
            lines.hline(y, 0, cols - 1, Weight::Double);
        }
        if let Some(projects) = layout.projects {
            lines.vline(projects.right(), 2, layout.console_sep, Weight::Double);
        }
        if let (Some(queue), Some(log)) = (layout.queue, layout.log) {
            lines.vline(queue.x - 1, 2, layout.console_sep, Weight::Double);
            lines.hline(log.y - 1, queue.x - 1, cols - 1, Weight::Double);
        }
        lines.draw(&mut grid, FRAME);

        self.draw_masthead(&mut grid, now);
        self.draw_tabs(&mut grid);

        if let Some(state) = self.state.clone() {
            let scoped = state.scoped();
            match self.view {
                View::Overview => {
                    if let Some(projects) = layout.projects {
                        title(
                            &mut grid,
                            0,
                            2,
                            &[s(format!("PROJECTS ({})", state.projects.len()), TITLE)],
                        );
                        title(&mut grid, projects.right(), 2, &[s("SECTOR MAP", TITLE)]);
                        self.draw_projects(&mut grid, projects, &state);
                    } else {
                        title(&mut grid, 0, 2, &[s("SECTOR MAP", TITLE)]);
                    }
                    self.draw_sector(&mut grid, layout.main, &scoped, now);
                    if let (Some(queue), Some(log)) = (layout.queue, layout.log) {
                        let count = state.permissions.len();
                        let tone = if count > 0 { WARN.bold() } else { TITLE };
                        title(
                            &mut grid,
                            queue.x - 1,
                            2,
                            &[s(format!("PERMISSION QUEUE ({count})"), tone)],
                        );
                        self.draw_queue(&mut grid, queue, &state, now, false);
                        title(&mut grid, log.x - 1, log.y - 1, &[s("EVENT LOG", TITLE)]);
                        draw_log(&mut grid, log, &state, self.pet.name, &mut self.log_scroll);
                        self.scroll_areas.push((log, Pane::Log));
                    }
                }
                View::Agents => {
                    title(
                        &mut grid,
                        0,
                        2,
                        &[s(format!("AGENTS ({})", scoped.agents.len()), TITLE)],
                    );
                    self.draw_agent_table(&mut grid, layout.main, &scoped, now);
                }
                View::Permissions => {
                    let count = state.permissions.len();
                    title(
                        &mut grid,
                        0,
                        2,
                        &[s(format!("PERMISSION QUEUE ({count})"), TITLE)],
                    );
                    self.draw_queue(&mut grid, layout.main, &state, now, true);
                }
                View::Log => {
                    title(
                        &mut grid,
                        0,
                        2,
                        &[s(format!("EVENT LOG ({})", state.log.len()), TITLE)],
                    );
                    draw_log(
                        &mut grid,
                        layout.main,
                        &state,
                        self.pet.name,
                        &mut self.log_scroll,
                    );
                    self.scroll_areas.push((layout.main, Pane::Log));
                }
                View::Providers => {
                    title(
                        &mut grid,
                        0,
                        2,
                        &[s("SETUP // PROVIDERS · KEYS · EDITORS · VOICE", TITLE)],
                    );
                    draw_setup(&mut grid, layout.main, &state);
                }
                View::Reader => self.draw_reader(&mut grid, layout.main, &state),
            }
            self.draw_console(&mut grid, layout.console, layout.console_sep, &state);
        } else {
            title(&mut grid, 0, 2, &[s("SECTOR MAP", TITLE)]);
            let main = layout.main;
            let text = "waiting for runtime…";
            let x = main.x + main.w.saturating_sub(text.len()) / 2;
            grid.text(x, main.y + main.h / 2, main.w, text, DIM);
            title(&mut grid, 0, layout.console_sep, &[s("AGENT", TITLE)]);
        }

        self.draw_voice(&mut grid, layout.voice, layout.voice_sep);
        self.draw_command_line(&mut grid);
        if self.help {
            draw_help(&mut grid);
        }
        if let Some(palette) = &self.palette {
            let state = self.state.clone().unwrap_or_default();
            let list = palette.draw(&mut grid, &state, self.selected, self.blink());
            self.hits
                .push((Rect::new(0, 0, cols, rows), Action::ClosePalette));
            for (index, rect) in list.into_iter().enumerate() {
                self.hits.push((rect, Action::PaletteRow(index)));
            }
        }
        if let Some(request) = self.modal().cloned() {
            self.draw_permission(&mut grid, &request, now);
        }
        grid
    }

    pub(super) fn draw_masthead(&mut self, grid: &mut Grid, now: DateTime<Utc>) {
        let project = self
            .state
            .as_ref()
            .and_then(|s| s.project())
            .map_or("NO PROJECT".to_owned(), |p| p.name.to_uppercase());
        title(
            grid,
            0,
            0,
            &[s("ORCH", ACCENT.bold()), s("//", DIM), s(project, TITLE)],
        );

        let (lamp, rtt) = match (self.link_ok(), self.link.rtt) {
            (true, Some(rtt)) => (s("●", OK), s(format!(" {}", format_rtt(rtt)), TEXT)),
            (false, _) if self.link.up && self.link.last_pong.is_none() => {
                (s("○", DIM), s(" --", DIM))
            }
            _ => (s("●", ERROR.bold()), s(" LOST", ERROR.bold())),
        };
        let mut spans: Spans = vec![
            s(clock(now), TEXT.bold()),
            s(" ═ ", FRAME),
            s("RT ", DIM),
            lamp,
            rtt,
            s(" ═ ", FRAME),
        ];
        if let Some(state) = &self.state {
            let tokens: u64 = state.agents.iter().map(|a| a.usage.total()).sum::<u64>()
                + state.host.usage.total();
            spans.push(if state.sandbox {
                s("BOX ● ", OK)
            } else {
                s("BOX ○ ", WARN)
            });
            spans.push(s("═ ", FRAME));
            spans.push(s("TOK ", DIM));
            spans.push(s(format_tokens(tokens), TEXT));
            let cost: u64 = state.agents.iter().map(|a| a.cost_micros).sum();
            if cost > 0 {
                spans.push(s(format!(" {}", format_cost(cost)), TEXT));
            }
            spans.push(s(" ═ ", FRAME));
        }
        spans.push(s("AGENTS", DIM));
        if let Some(state) = &self.state {
            let count =
                |wanted: AgentState| state.agents.iter().filter(|a| a.state == wanted).count();
            for (wanted, always) in [
                (AgentState::Running, true),
                (AgentState::AwaitingPermission, true),
                (AgentState::Stopped, true),
                (AgentState::Done, false),
                (AgentState::Error, false),
            ] {
                let n = count(wanted);
                if always || n > 0 {
                    let (glyph, style) = lamp_for(wanted);
                    let style = if n == 0 { FAINT } else { style };
                    spans.push(s(format!(" {n}{glyph}"), style));
                }
            }
        }
        title_right(grid, grid.cols - 2, 0, &spans);
    }

    pub(super) fn draw_tabs(&mut self, grid: &mut Grid) {
        let pending = self.state.as_ref().map_or(0, |s| s.permissions.len());
        let mut x = 2;
        for (index, view) in View::ALL.into_iter().enumerate() {
            let active = view == self.view;
            let base = if active { ACCENT.bold().inverse() } else { DIM };
            let mut spans = vec![s(format!("[F{} {}", index + 1, view.label()), base)];
            if view == View::Permissions && pending > 0 {
                let style = if active {
                    WARN.bold().inverse()
                } else {
                    WARN.bold()
                };
                spans.push(s(format!(" ◆{pending}"), style));
            }
            spans.push(s("]", base));
            let width = grid.spans(x, 1, grid.cols - 2 - x, &spans);
            self.hits
                .push((Rect::new(x, 1, width, 1), Action::View(view)));
            x += width + 1;
        }

        let caution: Spans = match self.alarm() {
            Alarm::Active if self.blink() => vec![s(" ▲ CAUTION ▲ ", WARN.bold().inverse())],
            Alarm::Active => vec![s(" ▲ CAUTION ▲ ", WARN.bold())],
            Alarm::Acked => vec![s(" ▲ CAUTION ", WARN), s("ACK ", DIM)],
            Alarm::None => vec![s(" ● ", OK), s("NOMINAL ", DIM)],
        };
        let width = spans_width(&caution);
        let caution_x = grid.cols - 2 - width;
        grid.spans(caution_x, 1, width, &caution);
        self.hits
            .push((Rect::new(caution_x, 1, width, 1), Action::Ack));

        if let Some(project) = self.state.as_ref().and_then(|s| s.project()) {
            let root = format!("root> {}", project.root.display());
            let room = caution_x.saturating_sub(x + 3);
            if room > 12 {
                grid.text(x + 2, 1, room, &fit(&root, room), FAINT);
            }
        }
    }
}

pub(super) fn too_small(grid: &mut Grid) {
    let lines = [
        "ORCH // CONTROL ROOM".to_owned(),
        format!(
            "enlarge the window: need {MIN_COLS}×{MIN_ROWS}, have {}×{}",
            grid.cols, grid.rows
        ),
    ];
    for (row, line) in lines.iter().enumerate() {
        let x = grid.cols.saturating_sub(line.chars().count()) / 2;
        let y = (grid.rows / 2 + row).saturating_sub(1);
        grid.text(x, y, grid.cols, line, if row == 0 { TITLE } else { DIM });
    }
}

pub(super) struct Layout {
    /// Open projects, left of the sector map (overview with room to spare).
    pub(super) projects: Option<Rect>,
    pub(super) main: Rect,
    pub(super) queue: Option<Rect>,
    pub(super) log: Option<Rect>,
    pub(super) console: Rect,
    pub(super) voice: Rect,
    pub(super) console_sep: usize,
    pub(super) voice_sep: usize,
}

impl Layout {
    pub(super) fn new(cols: usize, rows: usize, overview: bool) -> Self {
        let voice_sep = rows - 4;
        let console_h = (rows / 6).clamp(3, 10);
        let console_sep = voice_sep - console_h - 1;
        let body = Rect::new(1, 3, cols - 2, console_sep - 3);
        let console = Rect::new(1, console_sep + 1, cols - 2, console_h);
        let voice = Rect::new(1, voice_sep + 1, cols - 2, 2);
        if !overview {
            return Self {
                projects: None,
                main: body,
                queue: None,
                log: None,
                console,
                voice,
                console_sep,
                voice_sep,
            };
        }
        let side_w = (cols * 3 / 10).clamp(34, 56);
        let sep_x = cols - 2 - side_w;
        let queue_h = (body.h * 9 / 20).max(6).min(body.h - 3);
        let queue = Rect::new(sep_x + 1, body.y, side_w, queue_h);
        let log_sep = body.y + queue_h;
        let log = Rect::new(sep_x + 1, log_sep + 1, side_w, body.bottom() - log_sep - 1);
        let main = Rect::new(1, body.y, sep_x - 1, body.h);
        // The projects column takes room from the map only where the map stays wide.
        let projects_w = (cols / 8).clamp(16, 26);
        let (projects, main) = if main.w >= projects_w + 1 + 46 {
            (
                Some(Rect::new(main.x, main.y, projects_w, main.h)),
                Rect::new(
                    main.x + projects_w + 1,
                    main.y,
                    main.w - projects_w - 1,
                    main.h,
                ),
            )
        } else {
            (None, main)
        };
        Self {
            projects,
            main,
            queue: Some(queue),
            log: Some(log),
            console,
            voice,
            console_sep,
            voice_sep,
        }
    }
}
