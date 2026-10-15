//! The launcher-managed copy of an emulator on disk, shared by every emulator:
//!
//!   <root>/versions/<tag>/   one unpacked release per folder
//!   <root>/current -> versions/<tag>
//!   <root>/state.json        installed / previous tag, last update check
//!
//! Only the installed and the previous version are kept, so a bad release can be rolled back.
//! An emulator's own installer unpacks into `staging_dir`, checks that the result runs, then calls
//! `activate`.

use crate::util::{atomic_write, now_secs};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A downloadable emulator build.
#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct State {
    pub installed: String,
    pub previous: String,
    pub last_check: f64,
    pub latest: String,
    /// A build the user rolled back from: not re-installed automatically.
    pub skip: String,
}

#[derive(Clone, Debug)]
pub struct ManagedStore {
    root: PathBuf,
}

impl ManagedStore {
    pub fn new(root: PathBuf) -> Self {
        ManagedStore { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// Where a release is unpacked before it is known to work.
    pub fn staging_dir(&self, tag: &str) -> PathBuf {
        self.versions_dir().join(format!("{tag}.tmp"))
    }

    /// Where a download is saved while it arrives.
    pub fn download_path(&self, tag: &str, ext: &str) -> PathBuf {
        self.root.join(format!("{tag}.{ext}.part"))
    }

    pub fn load_state(&self) -> State {
        std::fs::read(self.root.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save_state(&self, s: &State) {
        if let Ok(j) = serde_json::to_vec_pretty(s) {
            let _ = atomic_write(&self.root.join("state.json"), &j);
        }
    }

    pub fn mark_checked(&self, latest: &str) {
        let mut s = self.load_state();
        s.last_check = now_secs();
        s.latest = latest.to_string();
        self.save_state(&s);
    }

    /// Whether a previous version is still on disk to roll back to.
    pub fn has_previous(&self) -> bool {
        let st = self.load_state();
        !st.previous.is_empty() && self.versions_dir().join(&st.previous).is_dir()
    }

    fn switch_current(&self, tag: &str) -> Result<(), String> {
        let tmp = self.root.join("current.new");
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(Path::new("versions").join(tag), &tmp).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.root.join("current")).map_err(|e| e.to_string())
    }

    /// Make an unpacked release the current one: move `unpacked` to `versions/<tag>`, point
    /// `current` at it, remember the old version for rollback and delete anything older.
    pub fn activate(&self, tag: &str, unpacked: &Path) -> Result<(), String> {
        let final_dir = self.versions_dir().join(tag);
        let _ = std::fs::remove_dir_all(&final_dir);
        std::fs::rename(unpacked, &final_dir).map_err(|e| e.to_string())?;
        let mut st = self.load_state();
        if st.installed != tag {
            st.previous = std::mem::take(&mut st.installed);
        }
        st.installed = tag.to_string();
        st.latest = tag.to_string();
        if st.skip == tag {
            st.skip.clear();
        }
        st.last_check = now_secs();
        self.switch_current(tag)?;
        self.save_state(&st);
        self.cleanup(&st);
        Ok(())
    }

    /// Switch back to the previously installed version and return its tag.
    pub fn rollback(&self) -> Result<String, String> {
        let mut st = self.load_state();
        if !self.has_previous() {
            return Err("no previous version to roll back to".into());
        }
        self.switch_current(&st.previous)?;
        st.skip = st.installed.clone();
        std::mem::swap(&mut st.installed, &mut st.previous);
        self.save_state(&st);
        Ok(st.installed)
    }

    /// Keep only the installed and previous versions.
    fn cleanup(&self, st: &State) {
        let Ok(rd) = std::fs::read_dir(self.versions_dir()) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name != st.installed && name != st.previous {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_dir(store: &ManagedStore, tag: &str) -> PathBuf {
        let dir = store.staging_dir(tag);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("emu"), tag).unwrap();
        dir
    }

    #[test]
    fn installs_roll_back_and_keep_two_versions() {
        let t = tempfile::tempdir().unwrap();
        let store = ManagedStore::new(t.path().to_path_buf());
        assert!(!store.has_previous());
        assert!(store.rollback().is_err(), "nothing to go back to yet");

        for tag in ["v1", "v2", "v3"] {
            let dir = release_dir(&store, tag);
            store.activate(tag, &dir).unwrap();
        }
        let st = store.load_state();
        assert_eq!((st.installed.as_str(), st.previous.as_str()), ("v3", "v2"));
        assert_eq!(std::fs::read_to_string(t.path().join("current/emu")).unwrap(), "v3");
        assert!(!store.versions_dir().join("v1").exists(), "older than the previous version: deleted");

        assert_eq!(store.rollback().unwrap(), "v2");
        assert_eq!(std::fs::read_to_string(t.path().join("current/emu")).unwrap(), "v2");
        let st = store.load_state();
        assert_eq!((st.installed.as_str(), st.previous.as_str(), st.skip.as_str()), ("v2", "v3", "v3"), "the build rolled back from is skipped");

        // Installing the skipped build on purpose clears the skip.
        let dir = release_dir(&store, "v3");
        store.activate("v3", &dir).unwrap();
        assert!(store.load_state().skip.is_empty());
    }

    #[test]
    fn a_check_is_remembered() {
        let t = tempfile::tempdir().unwrap();
        let store = ManagedStore::new(t.path().to_path_buf());
        store.mark_checked("v9");
        let st = store.load_state();
        assert_eq!(st.latest, "v9");
        assert!(st.last_check > 0.0);
    }
}
