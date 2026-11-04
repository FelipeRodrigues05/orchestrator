//! Character grid the control room is drawn on, plus box-drawing line routing.
//!
//! Plain Rust (no GTK), so the layout can be unit tested.

/// Colour roles; `render` maps them to the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Frame,
    Title,
    Text,
    Dim,
    Faint,
    Accent,
    Ok,
    Warn,
    Error,
    Stopped,
    /// An agent's own colour, an index into `render::AGENT_RGB`.
    Agent(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub tone: Tone,
    pub bold: bool,
    /// Swap foreground and background (selection, active tab, alarms).
    pub inverse: bool,
}

impl Style {
    pub const fn new(tone: Tone) -> Self {
        Self {
            tone,
            bold: false,
            inverse: false,
        }
    }

    pub const fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    pub const fn inverse(self) -> Self {
        Self {
            inverse: true,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

const BLANK: Cell = Cell {
    ch: ' ',
    style: Style::new(Tone::Text),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn right(&self) -> usize {
        self.x + self.w
    }

    pub fn bottom(&self) -> usize {
        self.y + self.h
    }
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![BLANK; cols * rows],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Cell {
        if x < self.cols && y < self.rows {
            self.cells[y * self.cols + x]
        } else {
            BLANK
        }
    }

    pub fn put(&mut self, x: usize, y: usize, ch: char, style: Style) {
        // A newline or tab inside one cell would spill over its neighbours.
        let ch = if ch.is_control() { ' ' } else { ch };
        if x < self.cols && y < self.rows {
            self.cells[y * self.cols + x] = Cell { ch, style };
        }
    }

    /// Writes `text` starting at `x`, cut at `max` cells. Returns the cells used.
    pub fn text(&mut self, x: usize, y: usize, max: usize, text: &str, style: Style) -> usize {
        let mut used = 0;
        for ch in text.chars().take(max) {
            self.put(x + used, y, ch, style);
            used += 1;
        }
        used
    }

    /// Writes styled spans one after the other. Returns the cells used.
    pub fn spans(&mut self, x: usize, y: usize, max: usize, spans: &[(String, Style)]) -> usize {
        let mut used = 0;
        for (text, style) in spans {
            used += self.text(x + used, y, max.saturating_sub(used), text, *style);
        }
        used
    }

    pub fn fill(&mut self, rect: Rect, ch: char, style: Style) {
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                self.put(x, y, ch, style);
            }
        }
    }

    /// Restyles a row segment, keeping the characters (used for highlights).
    pub fn restyle(&mut self, x: usize, y: usize, w: usize, style: Style) {
        for x in x..x + w {
            let ch = self.get(x, y).ch;
            self.put(x, y, ch, style);
        }
    }

    pub fn row(&self, y: usize) -> &[Cell] {
        &self.cells[y * self.cols..(y + 1) * self.cols]
    }
}

pub fn spans_width(spans: &[(String, Style)]) -> usize {
    spans.iter().map(|(text, _)| text.chars().count()).sum()
}

/// Pads or cuts `text` to exactly `width` characters, with `…` when cut.
pub fn fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        format!("{text:<width$}")
    } else if width == 0 {
        String::new()
    } else {
        let mut out: String = text.chars().take(width - 1).collect();
        out.push('…');
        out
    }
}

const NORTH: u8 = 1;
const SOUTH: u8 = 2;
const WEST: u8 = 4;
const EAST: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Single,
    Double,
}

/// Collects horizontal and vertical lines and turns their crossings into the right
/// box-drawing junctions (`╠`, `╦`, `┼`, `╪`…).
pub struct Lines {
    cols: usize,
    rows: usize,
    mask: Vec<u8>,
    /// Weight of the horizontal and vertical strokes through each cell.
    weight: Vec<(Weight, Weight)>,
}

impl Lines {
    pub fn new(cols: usize, rows: usize) -> Self {
        let single = (Weight::Single, Weight::Single);
        Self {
            cols,
            rows,
            mask: vec![0; cols * rows],
            weight: vec![single; cols * rows],
        }
    }

    fn mark(&mut self, x: usize, y: usize, bits: u8, weight: Weight, horizontal: bool) {
        if x >= self.cols || y >= self.rows {
            return;
        }
        let index = y * self.cols + x;
        self.mask[index] |= bits;
        if horizontal {
            self.weight[index].0 = weight;
        } else {
            self.weight[index].1 = weight;
        }
    }

    /// Horizontal line from `x0` to `x1` inclusive.
    pub fn hline(&mut self, y: usize, x0: usize, x1: usize, weight: Weight) {
        for x in x0..=x1 {
            let mut bits = 0;
            if x > x0 {
                bits |= WEST;
            }
            if x < x1 {
                bits |= EAST;
            }
            self.mark(x, y, bits, weight, true);
        }
    }

    /// Vertical line from `y0` to `y1` inclusive.
    pub fn vline(&mut self, x: usize, y0: usize, y1: usize, weight: Weight) {
        for y in y0..=y1 {
            let mut bits = 0;
            if y > y0 {
                bits |= NORTH;
            }
            if y < y1 {
                bits |= SOUTH;
            }
            self.mark(x, y, bits, weight, false);
        }
    }

    pub fn rect(&mut self, rect: Rect, weight: Weight) {
        let (x1, y1) = (rect.right() - 1, rect.bottom() - 1);
        self.hline(rect.y, rect.x, x1, weight);
        self.hline(y1, rect.x, x1, weight);
        self.vline(rect.x, rect.y, y1, weight);
        self.vline(x1, rect.y, y1, weight);
    }

    pub fn draw(&self, grid: &mut Grid, style: Style) {
        for y in 0..self.rows {
            for x in 0..self.cols {
                let index = y * self.cols + x;
                if let Some(ch) = junction(self.mask[index], self.weight[index]) {
                    grid.put(x, y, ch, style);
                }
            }
        }
    }
}

/// Arms of a box-drawing glyph as `[north, south, west, east]`, each `None` when the
/// glyph has no stroke on that side. `None` for any other character.
pub fn arms(ch: char) -> Option<[Option<Weight>; 4]> {
    use Weight::{Double as D, Single as S};
    for (horizontal, vertical) in [(S, S), (D, D), (D, S), (S, D)] {
        let Some(index) = glyphs(horizontal, vertical).iter().position(|&g| g == ch) else {
            continue;
        };
        let mask = MASKS[index];
        let arm = |bit: u8, weight: Weight| (mask & bit != 0).then_some(weight);
        return Some([
            arm(NORTH, vertical),
            arm(SOUTH, vertical),
            arm(WEST, horizontal),
            arm(EAST, horizontal),
        ]);
    }
    None
}

const MASKS: [u8; 11] = [
    WEST | EAST,
    NORTH | SOUTH,
    SOUTH | EAST,
    SOUTH | WEST,
    NORTH | EAST,
    NORTH | WEST,
    NORTH | SOUTH | EAST,
    NORTH | SOUTH | WEST,
    SOUTH | WEST | EAST,
    NORTH | WEST | EAST,
    NORTH | SOUTH | WEST | EAST,
];

fn glyphs(horizontal: Weight, vertical: Weight) -> [char; 11] {
    use Weight::{Double as D, Single as S};
    match (horizontal, vertical) {
        (S, S) => ['─', '│', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼'],
        (D, D) => ['═', '║', '╔', '╗', '╚', '╝', '╠', '╣', '╦', '╩', '╬'],
        (D, S) => ['═', '│', '╒', '╕', '╘', '╛', '╞', '╡', '╤', '╧', '╪'],
        (S, D) => ['─', '║', '╓', '╖', '╙', '╜', '╟', '╢', '╥', '╨', '╫'],
    }
}

fn junction(mask: u8, (horizontal, vertical): (Weight, Weight)) -> Option<char> {
    let has_h = mask & (WEST | EAST) != 0;
    let has_v = mask & (NORTH | SOUTH) != 0;
    // A lone end of a line keeps the line's own glyph.
    let mask = match (has_h, has_v) {
        (true, false) => WEST | EAST,
        (false, true) => NORTH | SOUTH,
        (false, false) => return None,
        _ => mask,
    };
    let index = MASKS
        .iter()
        .position(|&m| m == mask)
        .unwrap_or(MASKS.len() - 1);
    Some(glyphs(horizontal, vertical)[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(grid: &Grid, y: usize) -> String {
        grid.row(y).iter().map(|cell| cell.ch).collect()
    }

    #[test]
    fn frames_share_junctions() {
        let mut lines = Lines::new(7, 4);
        lines.rect(Rect::new(0, 0, 7, 4), Weight::Double);
        lines.hline(2, 0, 6, Weight::Double);
        lines.vline(3, 0, 2, Weight::Double);
        let mut grid = Grid::new(7, 4);
        lines.draw(&mut grid, Style::new(Tone::Frame));
        assert_eq!(line(&grid, 0), "╔══╦══╗");
        assert_eq!(line(&grid, 1), "║  ║  ║");
        assert_eq!(line(&grid, 2), "╠══╩══╣");
        assert_eq!(line(&grid, 3), "╚═════╝");
    }

    #[test]
    fn double_link_crosses_single_bus() {
        let mut lines = Lines::new(5, 3);
        lines.vline(2, 0, 2, Weight::Single);
        lines.hline(1, 0, 4, Weight::Double);
        let mut grid = Grid::new(5, 3);
        lines.draw(&mut grid, Style::new(Tone::Frame));
        assert_eq!(line(&grid, 1), "══╪══");
        assert_eq!(line(&grid, 0), "  │  ");
    }

    #[test]
    fn arms_read_back_the_junctions() {
        use Weight::{Double as D, Single as S};
        assert_eq!(arms('╔'), Some([None, Some(D), None, Some(D)]));
        assert_eq!(arms('╪'), Some([Some(S), Some(S), Some(D), Some(D)]));
        assert_eq!(arms('─'), Some([None, None, Some(S), Some(S)]));
        assert_eq!(arms('A'), None);
    }

    #[test]
    fn control_characters_never_reach_a_cell() {
        let mut grid = Grid::new(6, 1);
        grid.text(0, 0, 6, "a\nb\tc", Style::new(Tone::Text));
        assert_eq!(line(&grid, 0), "a b c ");
    }

    #[test]
    fn fit_pads_and_cuts() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 0), "");
    }
}
