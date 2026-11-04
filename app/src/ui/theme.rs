//! Colour themes (ADR 0021). A theme maps each `Tone` of the grid to a colour; the
//! layout never changes. In every theme red is kept for what needs the operator.

use super::grid::Tone;

pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    /// Black, greys and white; the original room.
    #[default]
    Mono,
    /// Amber phosphor.
    Amber,
    /// Green phosphor.
    Green,
    /// Cold blues (Nord).
    Nord,
    /// Dark text on a light background, for daylight.
    Light,
}

pub struct Palette {
    pub background: Rgb,
    frame: Rgb,
    title: Rgb,
    text: Rgb,
    dim: Rgb,
    faint: Rgb,
    accent: Rgb,
    ok: Rgb,
    urgent: Rgb,
    stopped: Rgb,
    /// Agents' colours, in the order of `room::format::AGENT_COLORS`.
    agents: [Rgb; 10],
    /// CRT scanlines over the room (dark themes only).
    pub scanlines: bool,
}

/// Soft tones that read on a dark background.
const AGENTS_ON_DARK: [Rgb; 10] = [
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

/// The same hues, darker, for a light background.
const AGENTS_ON_LIGHT: [Rgb; 10] = [
    (0x00, 0x7a, 0x99),
    (0x1d, 0x7a, 0x3a),
    (0x8a, 0x6d, 0x00),
    (0x9c, 0x2f, 0x9c),
    (0x2a, 0x4f, 0xc2),
    (0xb3, 0x5c, 0x00),
    (0x00, 0x7d, 0x72),
    (0xc2, 0x3a, 0x7a),
    (0x6a, 0x45, 0xc2),
    (0x4d, 0x7a, 0x00),
];

impl Theme {
    pub const ALL: [Theme; 5] = [
        Theme::Mono,
        Theme::Amber,
        Theme::Green,
        Theme::Nord,
        Theme::Light,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Mono => "mono",
            Theme::Amber => "amber",
            Theme::Green => "green",
            Theme::Nord => "nord",
            Theme::Light => "light",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Theme::Mono => "black and greys, red only for what is urgent",
            Theme::Amber => "amber phosphor terminal",
            Theme::Green => "green phosphor terminal",
            Theme::Nord => "cold blues",
            Theme::Light => "dark text on light, for daylight",
        }
    }

    /// `amber`, `âmbar`, `claro`…; `None` for an unknown name.
    pub fn parse(name: &str) -> Option<Theme> {
        match name.trim().to_lowercase().as_str() {
            "mono" | "default" | "padrão" | "padrao" => Some(Theme::Mono),
            "amber" | "âmbar" | "ambar" => Some(Theme::Amber),
            "green" | "verde" | "phosphor" => Some(Theme::Green),
            "nord" | "blue" | "azul" => Some(Theme::Nord),
            "light" | "claro" | "day" => Some(Theme::Light),
            _ => None,
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Theme::Mono => Palette {
                background: (0x00, 0x00, 0x00),
                frame: (0x44, 0x44, 0x44),
                title: (0xe6, 0xe6, 0xe6),
                text: (0xc8, 0xc8, 0xc8),
                dim: (0x80, 0x80, 0x80),
                faint: (0x2e, 0x2e, 0x2e),
                accent: (0xff, 0xff, 0xff),
                ok: (0xd8, 0xd8, 0xd8),
                urgent: (0xff, 0x3b, 0x30),
                stopped: (0x5a, 0x5a, 0x5a),
                agents: AGENTS_ON_DARK,
                scanlines: true,
            },
            Theme::Amber => Palette {
                background: (0x0c, 0x08, 0x00),
                frame: (0x5c, 0x3d, 0x00),
                title: (0xff, 0xcc, 0x66),
                text: (0xff, 0xb0, 0x00),
                dim: (0xa0, 0x6e, 0x00),
                faint: (0x3a, 0x27, 0x00),
                accent: (0xff, 0xe4, 0xa3),
                ok: (0xff, 0xc0, 0x40),
                urgent: (0xff, 0x3b, 0x30),
                stopped: (0x6b, 0x4a, 0x00),
                agents: AGENTS_ON_DARK,
                scanlines: true,
            },
            Theme::Green => Palette {
                background: (0x00, 0x0a, 0x02),
                frame: (0x0f, 0x4d, 0x1f),
                title: (0x7d, 0xff, 0x9b),
                text: (0x33, 0xff, 0x66),
                dim: (0x1f, 0x9c, 0x3e),
                faint: (0x0b, 0x2e, 0x14),
                accent: (0xc8, 0xff, 0xd4),
                ok: (0x5c, 0xff, 0x85),
                urgent: (0xff, 0x3b, 0x30),
                stopped: (0x1a, 0x5c, 0x2b),
                agents: AGENTS_ON_DARK,
                scanlines: true,
            },
            Theme::Nord => Palette {
                background: (0x2e, 0x34, 0x40),
                frame: (0x4c, 0x56, 0x6a),
                title: (0xec, 0xef, 0xf4),
                text: (0xd8, 0xde, 0xe9),
                dim: (0x8f, 0x9b, 0xb3),
                faint: (0x3b, 0x42, 0x52),
                accent: (0x88, 0xc0, 0xd0),
                ok: (0xa3, 0xbe, 0x8c),
                urgent: (0xbf, 0x61, 0x6a),
                stopped: (0x61, 0x6e, 0x88),
                agents: AGENTS_ON_DARK,
                scanlines: false,
            },
            Theme::Light => Palette {
                background: (0xf5, 0xf5, 0xf2),
                frame: (0xb8, 0xb8, 0xb8),
                title: (0x11, 0x11, 0x11),
                text: (0x2b, 0x2b, 0x2b),
                dim: (0x6b, 0x6b, 0x6b),
                faint: (0xd8, 0xd8, 0xd4),
                accent: (0x00, 0x00, 0x00),
                ok: (0x3a, 0x3a, 0x3a),
                urgent: (0xd0, 0x02, 0x1b),
                stopped: (0x9a, 0x9a, 0x9a),
                agents: AGENTS_ON_LIGHT,
                scanlines: false,
            },
        }
    }
}

impl Palette {
    pub fn tone(&self, tone: Tone) -> Rgb {
        match tone {
            Tone::Frame => self.frame,
            Tone::Title => self.title,
            Tone::Text => self.text,
            Tone::Dim => self.dim,
            Tone::Faint => self.faint,
            Tone::Accent => self.accent,
            Tone::Ok => self.ok,
            Tone::Warn | Tone::Error => self.urgent,
            Tone::Stopped => self.stopped,
            Tone::Agent(index) => self.agents[usize::from(index) % self.agents.len()],
        }
    }

    /// The window around the grid (header bar included), as CSS.
    pub fn css(&self) -> String {
        let hex = |(r, g, b): Rgb| format!("#{r:02x}{g:02x}{b:02x}");
        format!(
            "window, .control-room {{ background: {bg}; }}
             window.pet-float {{ background: transparent; }}
             headerbar {{ background: {bg}; color: {title}; font-family: monospace;
                         font-weight: 700; box-shadow: none; border-bottom: 1px solid {frame}; }}",
            bg = hex(self.background),
            title = hex(self.title),
            frame = hex(self.frame),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_keeps_red_for_urgency_and_parses_its_names() {
        for theme in Theme::ALL {
            let palette = theme.palette();
            let (r, g, b) = palette.tone(Tone::Error);
            assert!(r > g && r > b, "{theme:?} urgency must stay red");
            assert_eq!(Theme::parse(theme.name()), Some(theme));
        }
        assert_eq!(Theme::parse("Âmbar"), Some(Theme::Amber));
        assert_eq!(Theme::parse("claro"), Some(Theme::Light));
        assert_eq!(Theme::parse("pink"), None);
    }
}
