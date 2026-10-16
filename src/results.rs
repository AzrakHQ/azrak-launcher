//! Your own KytyPS5 results, saved on this PC in ~/.config/azrak/results.json.
//! A result becomes the game's tag here right away, and is shared with the community list
//! (KytyPS5's game status reports) in batches, from Settings.

use crate::compat::Status;
use crate::util::{atomic_write, config_dir};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MyResult {
    pub name: String,
    /// Same spelling as the community list: "InGame", "MainMenu", "Logo", "DoesntBoot".
    pub status: String,
    /// The KytyPS5 build it was played on.
    pub kyty: String,
    pub rated: f64,
    /// When its report was opened for sharing; 0 = not yet.
    pub shared: f64,
    /// The emulator log saved with this result (the crash log, if the game crashed).
    pub log: String,
}

impl MyResult {
    pub fn status(&self) -> Option<Status> {
        Status::parse(&self.status)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Results {
    /// By title ID.
    pub games: BTreeMap<String, MyResult>,
    /// Games whose rating prompt was skipped, with the KytyPS5 build: not asked again on it.
    pub skipped: BTreeMap<String, String>,
    /// Last "you have results to share" reminder.
    pub reminded: f64,
}

/// Remind about unshared results at most this often.
pub const REMIND_EVERY: f64 = 7.0 * 24.0 * 3600.0;
/// Reports opened in one go; the rest wait for the next batch.
pub const BATCH: usize = 10;
/// Saved game logs kept on disk (oldest removed first).
const KEEP_LOGS: usize = 40;

/// Keep a copy of a game's emulator log, which the next launch of that game would overwrite.
/// Returns the saved copy's path.
pub fn save_log(log: &std::path::Path, title_id: &str, crashed: bool) -> Option<std::path::PathBuf> {
    let dir = crate::util::data_dir().join("game-logs");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{title_id}-{}{}.log", crate::util::now_secs() as u64, if crashed { "-crash" } else { "" });
    let saved = dir.join(name);
    std::fs::copy(log, &saved).ok()?;
    // Oldest first: file names sort by title ID, so sort by modification time.
    let mut logs: Vec<_> = std::fs::read_dir(&dir).ok()?.flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    logs.sort();
    for (_, old) in logs.iter().take(logs.len().saturating_sub(KEEP_LOGS)) {
        let _ = std::fs::remove_file(old);
    }
    Some(saved)
}

fn path() -> std::path::PathBuf {
    config_dir().join("results.json")
}

pub fn load() -> Results {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

impl Results {
    pub fn save(&self) {
        if let Ok(j) = serde_json::to_vec_pretty(self) {
            let _ = atomic_write(&path(), &j);
        }
    }

    pub fn rate(&mut self, title_id: &str, name: &str, status: Status, kyty: &str, now: f64, log: &str) {
        self.skipped.remove(title_id);
        self.games.insert(title_id.to_string(), MyResult { name: name.into(), status: status.key().into(), kyty: kyty.into(), rated: now, shared: 0.0, log: log.into() });
    }

    pub fn skip(&mut self, title_id: &str, kyty: &str) {
        self.skipped.insert(title_id.to_string(), kyty.to_string());
    }

    /// Ask for a rating after playing, unless this game was rated or skipped on this build.
    pub fn should_ask(&self, title_id: &str, kyty: &str) -> bool {
        !title_id.is_empty()
            && self.games.get(title_id).is_none_or(|r| !kyty.is_empty() && r.kyty != kyty)
            && self.skipped.get(title_id).is_none_or(|k| k != kyty)
    }

    pub fn unshared(&self) -> Vec<(&String, &MyResult)> {
        self.games.iter().filter(|(_, r)| r.shared == 0.0 && r.status().is_some()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_skipping_and_sharing() {
        let mut r = Results::default();
        assert!(r.should_ask("PPSA1", "build-1"));
        r.skip("PPSA1", "build-1");
        assert!(!r.should_ask("PPSA1", "build-1"));
        assert!(r.should_ask("PPSA1", "build-2"), "a new KytyPS5 build asks again");
        r.rate("PPSA1", "Game", Status::InGame, "build-2", 1.0, "");
        assert!(!r.should_ask("PPSA1", "build-2"));
        assert_eq!(r.unshared().len(), 1);
        r.games.get_mut("PPSA1").unwrap().shared = 2.0;
        assert!(r.unshared().is_empty());
        r.rate("PPSA1", "Game", Status::Logo, "build-3", 3.0, "");
        assert_eq!(r.unshared().len(), 1, "a new rating is shared again");
        assert!(!r.should_ask("", "build-3"));
    }
}
