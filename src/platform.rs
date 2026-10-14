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
    pub fn label(self) -> &'static str {
        match self {
            Platform::Ps5 => "PS5",
            Platform::Ps4 => "PS4",
            Platform::Pc => "PC",
        }
    }

    /// What runs this platform's games.
    pub fn emulator(self) -> &'static str {
        match self {
            Platform::Ps5 => "KytyPS5",
            Platform::Ps4 => "shadPS4",
            Platform::Pc => "Linux",
        }
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
}
