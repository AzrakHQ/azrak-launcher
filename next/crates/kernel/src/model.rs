use crate::ids::{GameKey, GameRef, ProviderId, SystemId};
use crate::path::RelPath;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role { Game, Update, Dlc }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Acquire {
    Magnet { uri: String },
    Http { url: String },
}

/// One listing from a catalog source. `extra` is opaque: only the owning plugin reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub id: String,
    pub source: ProviderId,
    pub system: SystemId,
    pub title: String,
    pub size: Option<u64>,
    pub published: Option<String>,
    pub acquire: Acquire,
    #[serde(default)]
    pub games: Vec<GameKey>,
    #[serde(default)]
    pub extra: serde_json::Value,
}

/// A game found inside some files, and where it should live.
#[derive(Clone, Debug, PartialEq)]
pub struct Recognised {
    pub key: GameKey,
    pub name: String,
    pub role: Role,
    /// Subtree of the source that belongs to this game.
    pub root: RelPath,
    /// Opaque data the system wants kept with the installed game (the runner of this system reads it).
    pub extra: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstalledGame {
    pub game: GameRef,
    pub name: String,
    /// Absolute path of the game folder.
    pub root: PathBuf,
    #[serde(default)]
    pub extra: serde_json::Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameInfo {
    pub name: Option<String>,
    pub publisher: Option<String>,
    pub genres: Vec<String>,
    pub released: Option<String>,
    pub cover: Option<String>,
    pub hero: Option<String>,
}

impl GameInfo {
    /// Merge ordered providers: the first provider with a value for a field wins.
    pub fn merge(infos: impl IntoIterator<Item = GameInfo>) -> GameInfo {
        let mut out = GameInfo::default();
        for i in infos {
            out.name = out.name.or(i.name);
            out.publisher = out.publisher.or(i.publisher);
            if out.genres.is_empty() { out.genres = i.genres; }
            out.released = out.released.or(i.released);
            out.cover = out.cover.or(i.cover);
            out.hero = out.hero.or(i.hero);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_prefers_earlier_providers_per_field() {
        let a = GameInfo { name: Some("A".into()), ..Default::default() };
        let b = GameInfo { name: Some("B".into()), publisher: Some("P".into()), ..Default::default() };
        let m = GameInfo::merge([a, b]);
        assert_eq!(m.name.as_deref(), Some("A"));
        assert_eq!(m.publisher.as_deref(), Some("P"));
    }
}
