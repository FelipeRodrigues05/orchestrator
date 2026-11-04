//! Paints a `Grid` with Cairo + Pango, one monospace cell per character.

use gtk::{cairo, pango, prelude::*};

use super::grid::{self, Grid, Style, Tone, Weight};

const FONT: &str = "Monospace 10";
const BACKGROUND: (f64, f64, f64) = rgb(0x00, 0x00, 0x00);

const fn rgb(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

fn color(tone: Tone) -> (f64, f64, f64) {
    match tone {
        // Monochrome board; red is reserved for what needs the operator now.
        Tone::Frame => rgb(0x44, 0x44, 0x44),
        Tone::Title => rgb(0xe6, 0xe6, 0xe6),
        Tone::Text => rgb(0xc8, 0xc8, 0xc8),
        Tone::Dim => rgb(0x80, 0x80, 0x80),
        Tone::Faint => rgb(0x2e, 0x2e, 0x2e),
        Tone::Accent => rgb(0xff, 0xff, 0xff),
        Tone::Ok => rgb(0xd8, 0xd8, 0xd8),
        Tone::Warn | Tone::Error => rgb(0xff, 0x3b, 0x30),
        Tone::Stopped => rgb(0x5a, 0x5a, 0x5a),
        Tone::Agent(index) => {
            let (r, g, b) = AGENT_RGB[usize::from(index) % AGENT_RGB.len()];
            rgb(r, g, b)
        }
    }
}

/// Agents' colours, in the order of `room::format::AGENT_COLORS`: soft tones that
/// read on black, and no red (red stays for what is urgent).
const AGENT_RGB: [(u8, u8, u8); 10] = [
    (0x5f, 0xd7, 0xff),
    (0x7c, 0xd9, 0x92),
    (0xf0, 0xd0, 0x6e),
    (0xe0, 0x7b, 0xe0),
    (0x7a, 0xa2, 0xf7),
    (0xf5, 0xa9, 0x5b),
    (0x4f, 0xd1, 0xc5),
    (0xff, 0x9e, 0xc8),
    (0xb3, 0x94, 0xf4),
    (0xc3, 0xe8, 0x8d),
];

/// Where the grid sits inside the widget, to map clicks back to cells.
#[derive(Debug, Clone, Copy, Default)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub cell_w: f64,
    pub cell_h: f64,
}

impl Viewport {
    pub fn cell_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        if self.cell_w <= 0.0 || x < self.x || y < self.y {
            return None;
        }
        Some((
            ((x - self.x) / self.cell_w) as usize,
            ((y - self.y) / self.cell_h) as usize,
        ))
    }
}

pub struct Painter {
    regular: pango::FontDescription,
    bold: pango::FontDescription,
    cell_w: f64,
    cell_h: f64,
    /// Black background and scanlines; off for the floating pet.
    opaque: bool,
}

impl Painter {
    /// Draws only the glyphs, on a transparent surface.
    pub fn transparent(mut self) -> Self {
        self.opaque = false;
        self
    }

    /// Pixel size of `cols` × `rows` cells.
    pub fn size_of(&self, cols: usize, rows: usize) -> (i32, i32) {
        (
            (cols as f64 * self.cell_w).ceil() as i32,
            (rows as f64 * self.cell_h).ceil() as i32,
        )
    }

    pub fn new(widget: &impl IsA<gtk::Widget>) -> Self {
        let regular = pango::FontDescription::from_string(FONT);
        let mut bold = regular.clone();
        bold.set_weight(pango::Weight::Bold);
        // Average over many cells so fractional advances do not drift.
        let probe = widget.create_pango_layout(Some(&"M".repeat(100)));
        probe.set_font_description(Some(&regular));
        let (_, logical) = probe.extents();
        let scale = f64::from(pango::SCALE);
        Self {
            regular,
            bold,
            cell_w: f64::from(logical.width()) / scale / 100.0,
            cell_h: f64::from(logical.height()) / scale,
            opaque: true,
        }
    }

    /// Grid size that fits `width` × `height` pixels, and where it goes.
    pub fn fit(&self, width: i32, height: i32) -> (usize, usize, Viewport) {
        let cols = (f64::from(width) / self.cell_w) as usize;
        let rows = (f64::from(height) / self.cell_h) as usize;
        let viewport = Viewport {
            x: ((f64::from(width) - cols as f64 * self.cell_w) / 2.0).floor(),
            y: ((f64::from(height) - rows as f64 * self.cell_h) / 2.0).floor(),
            cell_w: self.cell_w,
            cell_h: self.cell_h,
        };
        (cols, rows, viewport)
    }

    pub fn paint(
        &self,
        widget: &impl IsA<gtk::Widget>,
        cr: &cairo::Context,
        grid: &Grid,
        view: Viewport,
        size: (i32, i32),
    ) {
        if self.opaque {
            let (r, g, b) = BACKGROUND;
            cr.set_source_rgb(r, g, b);
            let _ = cr.paint();
        } else {
            cr.set_operator(cairo::Operator::Clear);
            let _ = cr.paint();
            cr.set_operator(cairo::Operator::Over);
        }

        let layout = widget.create_pango_layout(None);
        for y in 0..grid.rows {
            let row = grid.row(y);
            let top = view.y + y as f64 * view.cell_h;

            // Over the desktop, a dark backing keeps the glyphs readable on any wallpaper.
            if !self.opaque {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.72);
                for (x, cell) in row.iter().enumerate() {
                    if cell.ch != ' ' {
                        cr.rectangle(
                            view.x + x as f64 * view.cell_w - 1.0,
                            top,
                            view.cell_w + 2.0,
                            view.cell_h,
                        );
                    }
                }
                let _ = cr.fill();
            }

            for (x, cell) in row.iter().enumerate() {
                if cell.style.inverse {
                    let (r, g, b) = color(cell.style.tone);
                    cr.set_source_rgb(r, g, b);
                    cr.rectangle(
                        view.x + x as f64 * view.cell_w,
                        top,
                        view.cell_w + 0.5,
                        view.cell_h,
                    );
                    let _ = cr.fill();
                }
            }

            // ASCII runs share one layout; every other glyph gets its own cell so a
            // fallback font with a different advance cannot shift the columns.
            let mut x = 0;
            while x < row.len() {
                let cell = row[x];
                if !cell.ch.is_ascii() {
                    let left = view.x + x as f64 * view.cell_w;
                    if let Some(arms) = grid::arms(cell.ch) {
                        self.box_glyph(cr, arms, cell.style, left, top);
                    } else {
                        self.glyph(cr, &layout, &cell.ch.to_string(), cell.style, left, top);
                    }
                    x += 1;
                    continue;
                }
                let start = x;
                let mut text = String::new();
                while x < row.len() && row[x].ch.is_ascii() && row[x].style == cell.style {
                    text.push(row[x].ch);
                    x += 1;
                }
                if !text.trim().is_empty() {
                    self.glyph(
                        cr,
                        &layout,
                        &text,
                        cell.style,
                        view.x + start as f64 * view.cell_w,
                        top,
                    );
                }
            }
        }

        if !self.opaque {
            return;
        }
        // Faint CRT scanlines.
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.22);
        let mut line = 0;
        while line < size.1 {
            cr.rectangle(0.0, f64::from(line), f64::from(size.0), 1.0);
            line += 3;
        }
        let _ = cr.fill();
    }

    fn glyph(
        &self,
        cr: &cairo::Context,
        layout: &pango::Layout,
        text: &str,
        style: Style,
        x: f64,
        y: f64,
    ) {
        layout.set_font_description(Some(if style.bold {
            &self.bold
        } else {
            &self.regular
        }));
        layout.set_text(text);
        let (r, g, b) = if style.inverse {
            BACKGROUND
        } else {
            color(style.tone)
        };
        cr.set_source_rgb(r, g, b);
        // A fallback glyph wider than its cells is squeezed instead of overlapping.
        let wanted = text.chars().count() as f64 * self.cell_w;
        let width = f64::from(layout.pixel_size().0);
        if width > wanted * 1.05 {
            let scale = wanted / width;
            let _ = cr.save();
            cr.translate(x, y + self.cell_h * (1.0 - scale) / 2.0);
            cr.scale(scale, scale);
            cr.move_to(0.0, 0.0);
            pangocairo::functions::show_layout(cr, layout);
            let _ = cr.restore();
        } else {
            cr.move_to(x, y);
            pangocairo::functions::show_layout(cr, layout);
        }
    }

    /// Box-drawing characters are stroked rather than typeset, so lines meet across
    /// cells whatever the font's line height is.
    fn box_glyph(
        &self,
        cr: &cairo::Context,
        arms: [Option<Weight>; 4],
        style: Style,
        x: f64,
        y: f64,
    ) {
        let [north, south, west, east] = arms;
        let (r, g, b) = if style.inverse {
            BACKGROUND
        } else {
            color(style.tone)
        };
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(1.0);
        let (right, bottom) = (x + self.cell_w, y + self.cell_h);
        // Centre snapped to the pixel grid so 1 px lines stay crisp.
        let cx = (x + self.cell_w / 2.0).floor() + 0.5;
        let cy = (y + self.cell_h / 2.0).floor() + 0.5;
        let d = (self.cell_w * 0.2).round().max(1.0);
        let double = |arm: Option<Weight>| arm == Some(Weight::Double);
        let h_double = double(west) || double(east);
        let v_double = double(north) || double(south);
        let line = |x0: f64, y0: f64, x1: f64, y1: f64| {
            cr.move_to(x0, y0);
            cr.line_to(x1, y1);
        };

        for (arm, outward, toward) in [(west, x, 1.0), (east, right, -1.0)] {
            match arm {
                // A single stroke crossing double rails reaches the far rail.
                Some(Weight::Single) => {
                    line(outward, cy, if v_double { cx + toward * d } else { cx }, cy);
                }
                Some(Weight::Double) if v_double => {
                    // Each rail stops where the perpendicular rail on its side starts.
                    let end = |side: Option<Weight>| {
                        if side.is_some() {
                            cx - toward * d
                        } else {
                            cx + toward * d
                        }
                    };
                    line(outward, cy - d, end(north), cy - d);
                    line(outward, cy + d, end(south), cy + d);
                }
                Some(Weight::Double) => {
                    line(outward, cy - d, cx, cy - d);
                    line(outward, cy + d, cx, cy + d);
                }
                None => {}
            }
        }
        for (arm, outward, toward) in [(north, y, 1.0), (south, bottom, -1.0)] {
            match arm {
                Some(Weight::Single) => {
                    line(cx, outward, cx, if h_double { cy + toward * d } else { cy });
                }
                Some(Weight::Double) if h_double => {
                    let end = |side: Option<Weight>| {
                        if side.is_some() {
                            cy - toward * d
                        } else {
                            cy + toward * d
                        }
                    };
                    line(cx - d, outward, cx - d, end(west));
                    line(cx + d, outward, cx + d, end(east));
                }
                Some(Weight::Double) => {
                    line(cx - d, outward, cx - d, cy);
                    line(cx + d, outward, cx + d, cy);
                }
                None => {}
            }
        }
        let _ = cr.stroke();
    }
}
