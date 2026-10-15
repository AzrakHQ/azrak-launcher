//! Which platform a game is for: PS5 games run on KytyPS5, PS4 games on shadPS4, and PC games
//! (native Linux, or Windows games packed with Wine) run by themselves.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    #[default]
    Ps5,
    Ps4,
    Pc,
}

impl Platform {
    /// Every platform, in the order the Library offers them.
    pub const ALL: [Platform; 3] = [Platform::Ps5, Platform::Ps4, Platform::Pc];

    pub fn label(self) -> &'static str {
        match self {
            Platform::Ps5 => "PS5",
            Platform::Ps4 => "PS4",
            Platform::Pc => "PC",
        }
    }

    /// What runs this platform's games: its emulator, or "Linux" for PC games. When a platform
    /// has several emulators, the one that is used unless the user chose another.
    pub fn emulator(self) -> &'static str {
        crate::emulator::for_platform(self).map_or("Linux", |e| e.name())
    }

    /// From a title ID: PPSA… is a PS5 game, CUSA… a PS4 game.
    pub fn of_title_id(tid: &str) -> Option<Platform> {
        if tid.starts_with("PPSA") {
            Some(Platform::Ps5)
        } else if tid.starts_with("CUSA") {
            Some(Platform::Ps4)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_ids() {
        assert_eq!(Platform::of_title_id("PPSA02929"), Some(Platform::Ps5));
        assert_eq!(Platform::of_title_id("CUSA04118"), Some(Platform::Ps4));
        assert_eq!(Platform::of_title_id("NPUB30000"), None);
        assert_eq!(Platform::Ps4.emulator(), "shadPS4");
    }

    #[test]
    fn all_lists_every_platform() {
        // Adding a variant makes this match fail to compile until ALL is updated too.
        for p in Platform::ALL {
            match p {
                Platform::Ps5 | Platform::Ps4 | Platform::Pc => {}
            }
        }
        let mut labels: Vec<_> = Platform::ALL.iter().map(|p| p.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), Platform::ALL.len(), "labels are folder names: they must differ");
    }
}
