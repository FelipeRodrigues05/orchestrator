//! Start-up animation (ADR 0022): the screen opens like a CRT, a system check lists
//! what the runtime really found, and the pet wakes up. Any key or click skips it.

use std::time::Instant;

use orch_core::model::AppState;

use super::{ACCENT, DIM, FAINT, OK, Room, TEXT, TITLE, s, title};
use crate::ui::{
    grid::{Grid, Lines, Rect, Weight},
    pet::Mood,
};

/// The screen opens from a line in the middle until here (ms).
const OPENED: u128 = 450;
/// One check line every so often after that.
const CHECK_EVERY: u128 = 140;
/// The pet wakes here, after the checks.
const WAKE: u128 = 1_700;
/// And the room is all there.
const DONE: u128 = 2_400;

pub(super) struct Boot {
    started: Instant,
}

impl Boot {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    /// An intro already `ms` in, to draw its later frames in tests.
    #[cfg(test)]
    pub(super) fn at(ms: u64) -> Self {
        Self {
            started: Instant::now() - std::time::Duration::from_millis(ms),
        }
    }
}

impl Room {
    /// The animation is over (or never ran): no more fast redraws needed.
    pub fn intro_done(&self) -> bool {
        self.intro_over
    }

    /// Ends the animation; true when there was one to end (the key is consumed).
    pub(super) fn skip_intro(&mut self) -> bool {
        self.intro_over = true;
        self.boot.take().is_some()
    }

    /// Draws the animation over the composed room, while it lasts.
    pub(super) fn draw_boot(&mut self, grid: &mut Grid) {
        let Some(boot) = &self.boot else { return };
        let elapsed = boot.started.elapsed().as_millis();
        if elapsed >= DONE {
            self.skip_intro();
            return;
        }
        let Some(state) = self.state.clone() else {
            return;
        };
        // A CRT turning on: a line in the middle that opens up and down.
        let opened = (elapsed.min(OPENED) as f64 / OPENED as f64).sqrt();
        let middle = grid.rows / 2;
        let half = ((grid.rows as f64 / 2.0) * opened).ceil() as usize;
        for y in (0..grid.rows).filter(|y| y + half < middle || *y > middle + half) {
            grid.fill(Rect::new(0, y, grid.cols, 1), ' ', TEXT);
        }
        if half < middle {
            let edge = Rect::new(0, middle.saturating_sub(half), grid.cols, 1);
            grid.fill(edge, '─', ACCENT);
        }
        if elapsed < OPENED {
            return;
        }
        let checks = checks(&state);
        let width = 50.min(grid.cols.saturating_sub(4));
        let height = checks.len() + 4;
        let rect = Rect::new(
            (grid.cols - width) / 2,
            (grid.rows.saturating_sub(height)) / 2,
            width,
            height,
        );
        grid.fill(rect, ' ', TEXT);
        let mut lines = Lines::new(grid.cols, grid.rows);
        lines.rect(rect, Weight::Double);
        lines.draw(grid, ACCENT);
        let inner = rect.w.saturating_sub(4);
        if elapsed < WAKE {
            title(
                grid,
                rect.x,
                rect.y,
                &[s("ORCHESTRATOR // SYSTEM CHECK", TITLE)],
            );
            let shown = ((elapsed - OPENED) / CHECK_EVERY) as usize + 1;
            for (row, (label, result)) in checks.iter().take(shown).enumerate() {
                let (mark, style) = match result {
                    Some(detail) => (format!("OK  {detail}").trim_end().to_owned(), OK.bold()),
                    None => ("OFF".to_owned(), DIM),
                };
                let dots = inner.saturating_sub(label.chars().count() + mark.chars().count() + 1);
                grid.spans(
                    rect.x + 2,
                    rect.y + 2 + row,
                    inner,
                    &[
                        s(*label, TEXT),
                        s(format!(" {}", ".".repeat(dots)), FAINT),
                        s(mark, style),
                    ],
                );
            }
            return;
        }
        // The pet wakes: eyes closed, then open.
        title(grid, rect.x, rect.y, &[s("ORCHESTRATOR // ONLINE", TITLE)]);
        let awake = elapsed > WAKE + 300;
        let body = self.pet.body(Mood::Idle, if awake { 1 } else { 0 });
        let top = rect.y + (rect.h.saturating_sub(body.len() + 2)) / 2;
        for (row, line) in body.iter().enumerate() {
            let x = rect.x + (rect.w.saturating_sub(line.chars().count())) / 2;
            grid.text(x, top + row, line.chars().count(), line, ACCENT.bold());
        }
        let greeting = if awake {
            format!("{} is awake", self.pet.name)
        } else {
            "…".to_owned()
        };
        let x = rect.x + (rect.w.saturating_sub(greeting.chars().count())) / 2;
        grid.text(
            x,
            top + body.len() + 1,
            greeting.chars().count(),
            &greeting,
            DIM,
        );
    }
}

/// What the runtime reported, as `(check, Some(detail) when on)`.
fn checks(state: &AppState) -> Vec<(&'static str, Option<String>)> {
    let ready = state.providers.iter().filter(|p| p.ready).count();
    let on = |flag: bool, detail: &str| flag.then(|| detail.to_owned());
    vec![
        ("runtime", Some(String::new())),
        ("permission broker", Some(String::new())),
        (
            "agent providers",
            (ready > 0).then(|| format!("{ready} ready")),
        ),
        ("sandbox (bubblewrap)", on(state.sandbox, "")),
        ("speech (piper)", on(state.voice.tts, "")),
        ("listening (whisper)", on(state.voice.stt, "")),
        ("pet model", state.host.provider.clone()),
        ("projects", Some(format!("{} open", state.projects.len()))),
    ]
}
