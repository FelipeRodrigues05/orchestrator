//! The overview's left side: the open projects and the sector map (groups, agents,
//! links and the pet).

use chrono::{DateTime, Utc};
use orch_core::model::{Agent, AgentState, AppState};

use super::format::{lamp_for, sparkline, uptime};
use super::pages::draw_guide;
use super::{
    ACCENT, Action, BLOCK_WIDTH, DIM, ERROR, FAINT, FRAME, Room, SPARK_WIDTH, TEXT, WARN, s,
};
use crate::ui::{
    grid::{Grid, Lines, Rect, Weight, fit},
    pet::{self, Mood},
};

/// Shortest link between a block and the bus that still reads as a line.
const MIN_LINK: usize = 3;

impl Room {
    /// The open projects, one card each: name, agents at work, requests waiting.
    pub(super) fn draw_projects(&mut self, grid: &mut Grid, area: Rect, state: &AppState) {
        let x = area.x + 1;
        let width = area.w.saturating_sub(2);
        let mut y = area.y + 1;
        for (index, project) in state.projects.iter().enumerate() {
            if y + 1 >= area.bottom().saturating_sub(2) {
                break;
            }
            let focused = state.focus == Some(project.id);
            let agents: Vec<&Agent> = state.agents_of(Some(project.id)).collect();
            let running = agents
                .iter()
                .filter(|a| {
                    matches!(
                        a.state,
                        AgentState::Running | AgentState::AwaitingPermission
                    )
                })
                .count();
            let waiting = state
                .permissions
                .iter()
                .filter(|p| agents.iter().any(|a| a.id == p.agent_id))
                .count();
            let name_style = if focused {
                ACCENT.bold().inverse()
            } else {
                TEXT
            };
            let key = if index < 9 {
                format!("{} ", index + 1)
            } else {
                "  ".into()
            };
            let mut spans = vec![s(key, if focused { name_style } else { DIM })];
            let badge = (waiting > 0).then(|| format!(" ◆{waiting}"));
            let room_for_name =
                width.saturating_sub(2 + badge.as_ref().map_or(0, |b| b.chars().count()));
            let name = fit(&project.name, room_for_name);
            let pad = room_for_name.saturating_sub(name.chars().count());
            spans.push(s(
                format!("{name}{}", " ".repeat(if focused { pad } else { 0 })),
                name_style,
            ));
            if let Some(badge) = badge {
                let style = if self.blink() {
                    WARN.bold().inverse()
                } else {
                    WARN.bold()
                };
                spans.push(s(badge, style));
            }
            grid.spans(x, y, width, &spans);
            let (glyph, style) = lamp_for(AgentState::Running);
            let detail = if agents.is_empty() {
                vec![s("  no agents", FAINT)]
            } else {
                vec![
                    s(
                        format!("  {running}{glyph}"),
                        if running > 0 { style } else { FAINT },
                    ),
                    s(format!(" of {}", agents.len()), DIM),
                ]
            };
            grid.spans(x, y + 1, width, &detail);
            self.hits.push((
                Rect::new(area.x, y, area.w, 2),
                Action::FocusProject(project.id),
            ));
            y += 3;
        }
        let hint = match (state.projects.is_empty(), width >= 18) {
            (true, _) => ":open <path>",
            (false, true) => "Alt+n · [ ] switch",
            (false, false) => "Alt+n · [ ]",
        };
        grid.text(
            x,
            area.bottom().saturating_sub(1),
            width,
            &fit(hint, width),
            FAINT,
        );
    }

    pub(super) fn draw_sector(
        &mut self,
        grid: &mut Grid,
        area: Rect,
        state: &AppState,
        now: DateTime<Utc>,
    ) {
        draw_starfield(grid, area);
        let groups = groups_of(state);

        // Padding inside the sector, then the hub: groups alternate left and right of
        // the pet, links run block → bus → pet.
        let inner = Rect::new(
            area.x + 3,
            area.y + 1,
            area.w.saturating_sub(6),
            area.h.saturating_sub(2),
        );
        let center_y = inner.y + inner.h / 2;
        let base_top = center_y.saturating_sub(pet::FACE_ROW + 1).max(inner.y);
        let pet_top = base_top + pet::bob(self.tick);
        let face_y = pet_top + pet::FACE_ROW;
        let side_min = BLOCK_WIDTH + 2 * MIN_LINK;
        // Two sides only when their links can stay readable.
        let two_sides = groups.len() > 1 && inner.w >= 2 * (BLOCK_WIDTH + 2 * 6) + pet::WIDTH;
        let one_side = inner.w >= side_min + pet::WIDTH;

        if state.agents.is_empty() || !one_side {
            if state.agents.is_empty() {
                draw_guide(grid, inner, state);
            }
            let pet_x = inner.x + inner.w.saturating_sub(pet::WIDTH) / 2;
            if one_side || state.agents.is_empty() {
                self.draw_pet(grid, pet_x, pet_top, base_top);
            }
            if !state.agents.is_empty() {
                let column: Vec<_> = groups.iter().collect();
                self.draw_blocks(grid, inner, inner.x, &column, inner.y, state, now);
            }
            return;
        }

        let sides = if two_sides { 2 } else { 1 };
        let link = (inner.w.saturating_sub(sides * BLOCK_WIDTH + pet::WIDTH) / (2 * sides))
            .clamp(MIN_LINK, 12);
        let side_w = BLOCK_WIDTH + 2 * link;
        let total = sides * side_w + pet::WIDTH;
        let x0 = inner.x + inner.w.saturating_sub(total) / 2;
        let pet_x = x0 + side_w;

        let (left, right): (Vec<_>, Vec<_>) = if two_sides {
            let (l, r): (Vec<_>, Vec<_>) = groups.iter().enumerate().partition(|(i, _)| i % 2 == 0);
            (
                l.into_iter().map(|(_, g)| g).collect(),
                r.into_iter().map(|(_, g)| g).collect(),
            )
        } else {
            (groups.iter().collect(), Vec::new())
        };

        let mut links = Lines::new(grid.cols, grid.rows);
        let mut packets = Vec::new();
        let mut alarms = Vec::new();
        for (side, column) in [(Side::Left, &left), (Side::Right, &right)] {
            if column.is_empty() {
                continue;
            }
            let block_x = match side {
                Side::Left => x0,
                Side::Right => pet_x + pet::WIDTH + 2 * link,
            };
            let height: usize = column
                .iter()
                .map(|(_, a)| block_height(a.len()) + 1)
                .sum::<usize>()
                - 1;
            let top = center_y.saturating_sub(height / 2).max(inner.y);
            let anchors = self.draw_blocks(grid, inner, block_x, column, top, state, now);
            if anchors.is_empty() {
                continue;
            }
            // `outer` is the end of each agent link, next to its block.
            let (outer, bus, pet_edge) = match side {
                Side::Left => (
                    block_x + BLOCK_WIDTH,
                    block_x + BLOCK_WIDTH + link,
                    pet_x - 1,
                ),
                Side::Right => (block_x - 1, block_x - link, pet_x + pet::WIDTH),
            };
            let first = anchors.first().map_or(face_y, |a| a.y);
            let last = anchors.last().map_or(face_y, |a| a.y);
            let hot = anchors.iter().any(|a| a.alert);
            for anchor in &anchors {
                let weight = if anchor.alert {
                    Weight::Double
                } else {
                    Weight::Single
                };
                links.hline(anchor.y, outer.min(bus), outer.max(bus), weight);
                let (from, to) = (outer.min(bus), outer.max(bus));
                let cells = if side == Side::Left {
                    from..to
                } else {
                    from + 1..to + 1
                };
                if anchor.alert {
                    alarms.push((cells.start, anchor.y, cells.len()));
                } else if anchor.running {
                    // A packet travelling towards the pet while the agent works.
                    let step = self.tick as usize % cells.len();
                    let x = if side == Side::Left {
                        cells.start + step
                    } else {
                        cells.end - 1 - step
                    };
                    packets.push((x, anchor.y));
                }
            }
            links.vline(bus, first.min(face_y), last.max(face_y), Weight::Single);
            let weight = if hot { Weight::Double } else { Weight::Single };
            links.hline(face_y, bus.min(pet_edge), bus.max(pet_edge), weight);
            if hot {
                let (from, to) = (bus.min(pet_edge), bus.max(pet_edge));
                let cells = if side == Side::Left {
                    from + 1..to + 1
                } else {
                    from..to
                };
                alarms.push((cells.start, face_y, cells.len()));
            }
        }
        links.draw(grid, FRAME);
        let alarm_style = if self.blink() { WARN.bold() } else { WARN };
        for (x, y, len) in alarms {
            grid.restyle(x, y, len, alarm_style);
        }
        for (x, y) in packets {
            grid.put(x, y, '•', ACCENT.bold());
        }
        self.draw_pet(grid, pet_x, pet_top, base_top);
    }

    /// Stacks group blocks from `top` at column `x`. Returns where each agent row is.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_blocks(
        &mut self,
        grid: &mut Grid,
        inner: Rect,
        x: usize,
        column: &[&(String, Vec<&Agent>)],
        top: usize,
        state: &AppState,
        now: DateTime<Utc>,
    ) -> Vec<Anchor> {
        let mut anchors = Vec::new();
        let mut boxes = Lines::new(grid.cols, grid.rows);
        let mut labels = Vec::new();
        let mut hidden = 0;
        let mut y = top;
        for (name, agents) in column {
            let height = block_height(agents.len());
            if y + height > inner.bottom() {
                hidden += agents.len();
                continue;
            }
            let block = Rect::new(x, y, BLOCK_WIDTH, height);
            grid.fill(block, ' ', TEXT);
            boxes.rect(block, Weight::Single);
            labels.push((y, name));
            if agents.is_empty() {
                grid.text(x + 2, y + 1, BLOCK_WIDTH - 3, "(no agents)", FAINT);
            }
            for (index, agent) in agents.iter().enumerate() {
                let row = y + 1 + index * 2;
                self.draw_agent_row(grid, x + 1, row, agent, state, now);
                self.hits
                    .push((Rect::new(x, row, BLOCK_WIDTH, 2), Action::Select(agent.id)));
                anchors.push(Anchor {
                    y: row,
                    alert: agent.state == AgentState::AwaitingPermission,
                    running: agent.state == AgentState::Running,
                });
            }
            y += height + 1;
        }
        boxes.draw(grid, FRAME);
        // Group names sit on the top border of their block, like `┌─API────┐`.
        for (y, name) in labels {
            let used = grid.text(
                x + 2,
                y,
                BLOCK_WIDTH - 4,
                &format!(" {} ", name.to_uppercase()),
                ACCENT.bold(),
            );
            // Busy slots when the group has a limit: "API 1/2".
            if let Some(group) = state.groups.iter().find(|g| &g.name == name)
                && let Some(max) = group.max_active
            {
                let busy = state
                    .agents
                    .iter()
                    .filter(|a| {
                        a.group_id == group.id
                            && matches!(
                                a.state,
                                AgentState::Running | AgentState::AwaitingPermission
                            )
                    })
                    .count();
                let style = if busy >= max { WARN } else { DIM };
                grid.text(
                    x + 2 + used,
                    y,
                    BLOCK_WIDTH.saturating_sub(4 + used),
                    &format!("{busy}/{max} "),
                    style,
                );
            }
        }
        if hidden > 0 {
            grid.text(
                x,
                inner.bottom() - 1,
                BLOCK_WIDTH,
                &format!("+{hidden} more · F2"),
                DIM,
            );
        }
        anchors
    }

    /// Draws the pet floating at `top`, with its shadow fixed under the lowest float.
    pub(super) fn draw_pet(&self, grid: &mut Grid, x: usize, top: usize, base_top: usize) {
        let mood = self.mood();
        let style = match mood {
            Mood::Alert => WARN.bold(),
            Mood::Offline => ERROR.bold(),
            Mood::Listen => ACCENT.bold(),
            _ => TEXT.bold(),
        };
        grid.fill(Rect::new(x, top, pet::WIDTH, pet::HEIGHT), ' ', TEXT);
        for (row, line) in self.pet.body(mood, self.tick).iter().enumerate() {
            grid.text(x, top + row, pet::WIDTH, line, style);
        }
        let label = mood.label();
        let label_x = (x + pet::WIDTH / 2).saturating_sub(label.len() / 2);
        let label_style = if mood == Mood::Alert && self.blink() {
            WARN.bold().inverse()
        } else {
            style
        };
        grid.text(label_x, top + 4, label.len(), label, label_style);
        let shadow = if top > base_top {
            " ~~~~~~~~ "
        } else {
            "  ~~~~~~  "
        };
        grid.text(x, base_top + pet::HEIGHT + 1, pet::WIDTH, shadow, FAINT);
    }

    pub(super) fn draw_agent_row(
        &self,
        grid: &mut Grid,
        x: usize,
        y: usize,
        agent: &Agent,
        state: &AppState,
        now: DateTime<Utc>,
    ) {
        let (glyph, lamp) = lamp_for(agent.state);
        let selected = self.selected == Some(agent.id);
        let name_style = if selected {
            ACCENT.bold().inverse()
        } else {
            TEXT.bold()
        };
        let blink_off = agent.state == AgentState::AwaitingPermission && !self.blink();
        grid.put(x, y, if blink_off { ' ' } else { glyph }, lamp);
        grid.text(x + 2, y, 13, &fit(&agent.name, 13), name_style);
        grid.text(x + 16, y, 5, &uptime(agent, now), DIM);

        let running = agent.state == AgentState::Running;
        grid.text(
            x + 2,
            y + 1,
            SPARK_WIDTH,
            &sparkline(state, agent.id, now),
            if running { ACCENT } else { FAINT },
        );
        grid.text(
            x + 3 + SPARK_WIDTH,
            y + 1,
            10,
            &fit(agent.provider.short_name(), 10),
            DIM,
        );
    }
}

pub(super) fn block_height(agents: usize) -> usize {
    2 + agents.max(1) * 2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Left,
    Right,
}

pub(super) struct Anchor {
    pub(super) y: usize,
    pub(super) alert: bool,
    pub(super) running: bool,
}

/// Sparse ASCII starfield behind the map.
fn draw_starfield(grid: &mut Grid, area: Rect) {
    for y in area.y..area.bottom() {
        for x in (area.x..area.right()).filter(|x| x % 3 == 0) {
            match (x / 3 * 7 + y * 13) % 31 {
                0 => grid.put(x, y, '·', FAINT),
                17 => grid.put(x, y, '+', FAINT),
                _ => {}
            }
        }
    }
}

/// Each group's name with its agents; agents whose group is gone come last.
fn groups_of(state: &AppState) -> Vec<(String, Vec<&Agent>)> {
    let mut groups: Vec<(String, Vec<&Agent>)> = state
        .groups
        .iter()
        .map(|g| {
            (
                g.name.clone(),
                state.agents.iter().filter(|a| a.group_id == g.id).collect(),
            )
        })
        .collect();
    let orphans: Vec<&Agent> = state
        .agents
        .iter()
        .filter(|a| state.group(a.group_id).is_none())
        .collect();
    if !orphans.is_empty() {
        groups.push(("UNGROUPED".into(), orphans));
    }
    groups
}
