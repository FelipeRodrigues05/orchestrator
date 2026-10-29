//! The permission request modal drawn over the room.

use chrono::{DateTime, Utc};
use orch_core::broker::Answer;

use super::{
    ACCENT, Action, DIM, ERROR, FAINT, FRAME, Room, Spans, TEXT, TITLE, s, title, title_right,
};
use crate::ui::grid::{Grid, Lines, Rect, Weight, spans_width};

impl Room {
    /// The permission popup, drawn in the room's own style (ADR 0014).
    pub(super) fn draw_permission(
        &mut self,
        grid: &mut Grid,
        request: &orch_core::model::PermissionRequest,
        now: DateTime<Utc>,
    ) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let agent = state.agent(request.agent_id);
        let name = agent.map_or("?", |a| a.name.as_str());
        let group = agent
            .and_then(|a| state.group(a.group_id))
            .map_or("?", |g| g.name.as_str());
        let w = 76.min(grid.cols.saturating_sub(8));
        let inner = w - 6;
        let command: Vec<char> = format!("$ {}", request.input_summary()).chars().collect();
        let mut wrapped: Vec<String> = command
            .chunks(inner - 2)
            .take(6)
            .map(|c| c.iter().collect())
            .collect();
        if command.len() > (inner - 2) * 6
            && let Some(last) = wrapped.last_mut()
        {
            last.pop();
            last.push('…');
        }
        let warning = request.warning.clone();
        let h = 12 + wrapped.len() + usize::from(warning.is_some());
        let rect = Rect::new((grid.cols - w) / 2, (grid.rows.saturating_sub(h)) / 2, w, h);
        grid.fill(rect, ' ', TEXT);

        let code_top = rect.y + 4 + usize::from(warning.is_some());
        let code = Rect::new(rect.x + 2, code_top, w - 4, wrapped.len() + 2);
        let mut frame = Lines::new(grid.cols, grid.rows);
        frame.rect(rect, Weight::Double);
        frame.hline(rect.bottom() - 3, rect.x, rect.right() - 1, Weight::Double);
        frame.draw(grid, ERROR);
        let mut code_frame = Lines::new(grid.cols, grid.rows);
        code_frame.rect(code, Weight::Single);
        code_frame.draw(grid, FRAME);

        let total = state.permissions.len();
        let position = state
            .permissions
            .iter()
            .position(|p| p.id == request.id)
            .unwrap_or(0)
            + 1;
        let blink = if self.blink() {
            ERROR.bold().inverse()
        } else {
            ERROR.bold()
        };
        title(grid, rect.x, rect.y, &[s("◆ PERMISSION REQUEST", blink)]);
        title_right(
            grid,
            rect.right() - 2,
            rect.y,
            &[s(format!("{position} of {total}"), DIM)],
        );
        grid.spans(
            rect.x + 3,
            rect.y + 2,
            inner,
            &[
                s(name.to_uppercase(), TITLE),
                s(format!(" · {}", group.to_uppercase()), DIM),
                s("  wants to use ", TEXT),
                s(&request.tool, ACCENT.bold()),
            ],
        );
        if let Some(warning) = &warning {
            grid.text(
                rect.x + 3,
                rect.y + 3,
                inner,
                &format!("▲ {warning}: rules do not apply"),
                ERROR.bold(),
            );
        }
        for (row, line) in wrapped.iter().enumerate() {
            grid.text(code.x + 2, code.y + 1 + row, inner - 2, line, TEXT.bold());
        }
        let left = (request.expires_at() - now).num_seconds().max(0);
        let expiry = if left < 60 { ERROR.bold() } else { DIM };
        grid.spans(
            rect.x + 3,
            code.bottom() + 1,
            inner,
            &[
                s(
                    format!("expires in {:02}:{:02}", left / 60, left % 60),
                    expiry,
                ),
                s("  ·  say \"permitir\", \"sempre\" or \"negar\"", FAINT),
            ],
        );

        let y = rect.bottom() - 2;
        let buttons: [(Spans, Action); 5] = [
            (
                vec![s("[", DIM), s("A", ACCENT.bold()), s("]llow", TEXT.bold())],
                Action::Answer(request.id, Answer::Allow),
            ),
            (
                vec![s("A[", DIM), s("l", ACCENT.bold()), s("]ways", TEXT)],
                Action::Answer(request.id, Answer::AlwaysAllow),
            ),
            (
                vec![s("[", DIM), s("G", ACCENT.bold()), s("]roup", TEXT)],
                Action::Answer(request.id, Answer::AlwaysAllowGroup),
            ),
            (
                vec![s("[", DIM), s("D", ERROR.bold()), s("]eny", ERROR)],
                Action::Answer(request.id, Answer::Deny),
            ),
            (
                vec![s("[", DIM), s("Esc", DIM.bold()), s("] later", DIM)],
                Action::Defer(request.id),
            ),
        ];
        let slot = (w - 2) / buttons.len();
        for (index, (spans, action)) in buttons.into_iter().enumerate() {
            let width = spans_width(&spans);
            let x = rect.x + 1 + index * slot + slot.saturating_sub(width) / 2;
            grid.spans(x, y, width, &spans);
            self.hits
                .push((Rect::new(rect.x + 1 + index * slot, y, slot, 1), action));
        }
    }
}
