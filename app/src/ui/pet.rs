//! ASCII host pet drawn in the sector map, generated per user.
// TODO(etapa 4): drive the moods from the pet actor (thinking, speaking) instead of the room.

pub const WIDTH: usize = 10;
/// Body rows plus the mood label under them.
pub const HEIGHT: usize = 5;
/// Row of the face, where the links from the agents plug in.
pub const FACE_ROW: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Watch,
    Listen,
    Think,
    Speak,
    Alert,
    Offline,
}

impl Mood {
    pub fn label(self) -> &'static str {
        match self {
            Mood::Idle => "::IDLE::",
            Mood::Watch => "::WATCH::",
            Mood::Listen => "::LISTEN::",
            Mood::Think => "::THINK::",
            Mood::Speak => "::SPEAK::",
            Mood::Alert => "::ALERT::",
            Mood::Offline => "::OFFLINE::",
        }
    }
}

const TOPS: [&str; 6] = [
    " .-====-. ",
    " /\\    /\\ ",
    "   _||_   ",
    " .-^^^^-. ",
    " ,-~~~~-, ",
    "  (\\__/)  ",
];
const FRAMES: [(char, char); 5] = [('(', ')'), ('[', ']'), ('{', '}'), ('|', '|'), ('<', '>')];
const EYES: [char; 7] = ['o', '@', '0', '*', '^', 'e', 'q'];
const MOUTHS: [&str; 5] = ["'-.__.-'", "'-.ww.-'", "'-.vv.-'", "'--==--'", "'-.--.-'"];
const NAMES: [&str; 12] = [
    "NIX", "ORBI", "KAI", "BIT", "ZED", "MOX", "PIX", "RUNE", "VEX", "ECHO", "JUNO", "TAO",
];

/// The look of one user's pet. The same user always gets the same pet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pet {
    top: &'static str,
    frame: (char, char),
    eye: char,
    mouth: &'static str,
    pub name: &'static str,
}

impl Pet {
    pub fn for_user(user: &str) -> Self {
        // FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in user.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        let mut pick = |len: usize| {
            let index = (hash % len as u64) as usize;
            hash /= len as u64;
            index
        };
        Self {
            top: TOPS[pick(TOPS.len())],
            frame: FRAMES[pick(FRAMES.len())],
            eye: EYES[pick(EYES.len())],
            mouth: MOUTHS[pick(MOUTHS.len())],
            name: NAMES[pick(NAMES.len())],
        }
    }

    /// The four body rows for `mood` at animation `tick` (250 ms each).
    pub fn body(&self, mood: Mood, tick: u64) -> [String; 4] {
        let eye = match mood {
            Mood::Offline => 'x',
            Mood::Alert => 'O',
            Mood::Think => ['o', '.', 'o', '*'][(tick % 4) as usize],
            _ if tick.is_multiple_of(16) => '-',
            _ => self.eye,
        };
        // Watching agents: the eyes drift left and right.
        let gaze = if mood == Mood::Watch {
            [1, 0, 1, 2][(tick / 6 % 4) as usize]
        } else {
            1
        };
        let eyes = format!("{}{eye}    {eye}{}", " ".repeat(gaze), " ".repeat(2 - gaze));
        let mouth = match mood {
            Mood::Alert => "'-.!!.-'",
            Mood::Listen | Mood::Speak if tick.is_multiple_of(2) => "'-.oo.-'",
            _ => self.mouth,
        };
        let (left, right) = self.frame;
        [
            self.top.to_owned(),
            format!("{left}{eyes}{right}"),
            format!("[{:^8}]", self.name),
            format!(" {mouth} "),
        ]
    }
}

/// Vertical float offset (0 or 1 row) for animation `tick`, a ~3 s cycle.
pub fn bob(tick: u64) -> usize {
    [0, 0, 1, 1][(tick / 3 % 4) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_has_the_same_width() {
        for user in ["felipe", "ana", "root", "", "a much longer user name"] {
            let pet = Pet::for_user(user);
            for mood in [
                Mood::Idle,
                Mood::Watch,
                Mood::Listen,
                Mood::Think,
                Mood::Speak,
                Mood::Alert,
                Mood::Offline,
            ] {
                for tick in 0..32 {
                    for row in pet.body(mood, tick) {
                        assert_eq!(
                            row.chars().count(),
                            WIDTH,
                            "{user} {mood:?} {tick}: {row:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn same_user_same_pet_and_users_differ() {
        assert_eq!(Pet::for_user("felipe"), Pet::for_user("felipe"));
        let pets: std::collections::HashSet<_> = ["felipe", "ana", "bruno", "carla", "diego"]
            .iter()
            .map(|u| Pet::for_user(u).body(Mood::Idle, 1))
            .collect();
        assert!(pets.len() > 1);
    }
}
