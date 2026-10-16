//! User settings, stored in ~/.config/ps5-launcher/config.json (same format as v1).

use crate::platform::Platform;
use crate::util::{atomic_write, config_dir, expand_home};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub emulator: String,
    /// The shadPS4 that PS4 games start with: the launcher's copy, or your own build.
    pub shad_emulator: String,
    /// The path of every other emulator (see `crate::emulator`), by emulator id. KytyPS5 and
    /// shadPS4 keep theirs in the two fields above.
    pub emulator_paths: BTreeMap<String, String>,
    /// Automatic updates of every other emulator, by emulator id (missing = on).
    pub emulator_auto_update: BTreeMap<String, bool>,
    /// The options an emulator declares in `Emulator::settings`, as "<emulator id>.<key>" and
    /// text. KytyPS5 and shadPS4 keep theirs in the older fields above.
    pub emulator_settings: BTreeMap<String, String>,
    /// Which emulator runs a platform's games, by platform label ("PS4") and emulator id. A
    /// platform that isn't listed uses the first emulator registered for it.
    pub platform_emulators: BTreeMap<String, String>,
    /// Where games live: PS5 games install into its PS5 folder, PS4 games into its PS4 folder.
    pub games_dir: String,
    /// More folders to look for games in, besides the games folder.
    pub game_dirs: Vec<String>,
    /// Where downloads go: PS5 releases into its PS5 folder, PS4 releases into its PS4 folder.
    pub downloads_dir: String,
    /// Where older versions save downloads; kept as the downloads folder's PS5 folder for them.
    pub download_dir: String,
    /// Where older versions install games; kept as the games folder's PS5 folder for them.
    pub install_dir: String,
    /// Continue sharing completed downloads until stopped or the launcher exits.
    pub seed_after_download: bool,
    pub library_compact: bool,
    pub fullscreen: bool,
    pub width: u32,
    pub height: u32,
    pub present_mode: String,
    /// What the emulator tells a PS5 game its screen is: "Title" (what a PS5 reports for that
    /// game), "FullHd" or "Uhd". Passed as --video-out-resolution.
    pub video_out: String,
    pub amd_cpu: bool,
    pub extra_args: String,
    pub sounds: bool,
    pub return_on_exit: bool,
    /// Output name (e.g. "DP-2"); empty = primary display.
    pub monitor: String,
    /// Keep the launcher-managed KytyPS5 on the latest official build.
    pub kyty_auto_update: bool,
    /// Install shadPS4 in the background and keep it on the latest official release.
    pub shad_auto_update: bool,
    /// Install new PS5 Launcher releases automatically (applied on the next start).
    pub app_auto_update: bool,
    /// Download (and seed) only through a VPN; stop when it isn't connected.
    pub vpn_only: bool,
    /// The VPN's network interface to use ("" = any connected VPN).
    pub vpn_interface: String,
    /// The PC (its specs summary) last warned about games that can't run on it.
    pub pc_noticed: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            emulator: String::new(),
            shad_emulator: String::new(),
            emulator_paths: BTreeMap::new(),
            emulator_auto_update: BTreeMap::new(),
            emulator_settings: BTreeMap::new(),
            platform_emulators: BTreeMap::new(),
            games_dir: String::new(),
            game_dirs: vec!["~/Games/PS5".into()],
            downloads_dir: String::new(),
            download_dir: "~/Downloads/PS5".into(),
            install_dir: "~/Games/PS5".into(),
            seed_after_download: true,
            library_compact: false,
            fullscreen: true,
            width: 1920,
            height: 1080,
            present_mode: "Mailbox".into(),
            video_out: "Title".into(),
            amd_cpu: false,
            extra_args: String::new(),
            sounds: true,
            return_on_exit: true,
            monitor: String::new(),
            kyty_auto_update: true,
            shad_auto_update: true,
            app_auto_update: true,
            vpn_only: false,
            vpn_interface: String::new(),
            pc_noticed: String::new(),
        }
    }
}

/// The values of --video-out-resolution and the names Settings shows for them.
pub const VIDEO_OUT_MODES: [(&str, &str); 3] =
    [("Title", "Game default"), ("FullHd", "1080p (Full HD)"), ("Uhd", "4K (Ultra HD)")];

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.json")
    }

    pub fn load() -> Config {
        let mut cfg: Config = std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        cfg.one_update_switch();
        cfg.one_games_folder();
        cfg.one_downloads_folder();
        for emu in crate::emulator::all() {
            crate::emulator::adopt_default_path(&mut cfg, *emu);
        }
        cfg
    }

    /// Settings has one "Update automatically" switch for the launcher and every emulator.
    /// Older settings files set them separately (and may have no entry for an emulator, which
    /// defaults to on): if any was turned off, all are off, so the switch shows what happens.
    fn one_update_switch(&mut self) {
        let all = self.app_auto_update && crate::emulator::all().iter().all(|e| e.auto_update(self));
        self.app_auto_update = all;
        for emu in crate::emulator::all() {
            emu.set_auto_update(self, all);
        }
    }

    /// Older settings files have no games folder, only the folder new games install to. Its parent
    /// becomes the games folder when it is a console's folder (~/Games/PS5 → ~/Games), but never
    /// the home or root folder. More game folders that are now scanned anyway are then dropped.
    fn one_games_folder(&mut self) {
        if self.games_dir.trim().is_empty() {
            self.games_dir = root_or(&root_from(&self.install_dir), GAMES_DIR);
            self.install_dir = self.install_path(Platform::Ps5);
        }
        let covered = self.covered_dirs();
        self.game_dirs.retain(|d| !covered.contains(&expand_home(d.trim())));
    }

    /// Older settings files have no downloads folder, only the folder downloads were saved to. The
    /// old default (~/Downloads/PS5) becomes the new default; another folder follows the games
    /// folder's rule. Transfers already saved keep their own folders.
    fn one_downloads_folder(&mut self) {
        if self.downloads_dir.trim().is_empty() {
            let old = self.download_dir.trim().trim_end_matches('/');
            let default = expand_home(old) == expand_home(OLD_DOWNLOAD_DIR);
            self.downloads_dir = if default { DOWNLOADS_DIR.into() } else { root_or(&root_from(old), DOWNLOADS_DIR) };
            self.download_dir = self.download_path(Platform::Ps5);
        }
    }

    /// The games folder as typed, or ~/Games.
    fn games_root(&self) -> String {
        root_or(&self.games_dir, GAMES_DIR)
    }

    /// The games folder and its platform folders. Each is scanned, so a game is found up to two
    /// folders deep in any of them, as it was when the platform folder was a game folder itself
    /// (and a PC game, which sits in `<name>/<name>-<id>`).
    fn covered_dirs(&self) -> Vec<PathBuf> {
        let root = expand_home(&self.games_root());
        let mut dirs = vec![root.clone()];
        dirs.extend(Platform::ALL.map(|p| root.join(p.label())));
        dirs
    }

    /// Where a console's games install: its folder in the games folder, as typed (e.g. "~/Games/PS4").
    pub fn install_path(&self, platform: Platform) -> String {
        format!("{}/{}", self.games_root(), platform.label())
    }

    /// Where a console's releases download: its folder in the downloads folder, as typed.
    pub fn download_path(&self, platform: Platform) -> String {
        format!("{}/{}", root_or(&self.downloads_dir, DOWNLOADS_DIR), platform.label())
    }

    /// Whether the library already scans this folder. Symlinks are resolved on both sides.
    pub fn scans(&self, dir: &Path) -> bool {
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let dir = real(dir);
        self.game_dir_paths().iter().any(|d| real(d) == dir)
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            if let Err(e) = atomic_write(&Self::path(), &json) {
                crate::log!("could not save config: {e}");
            }
        }
    }

    /// "Download only through a VPN": `Some(preferred interface)` ("" = any connected VPN).
    pub fn download_vpn(&self) -> Option<String> {
        self.vpn_only.then(|| self.vpn_interface.clone())
    }

    /// The games folder and its console folders first, then the more game folders, each once.
    pub fn game_dir_paths(&self) -> Vec<PathBuf> {
        let mut dirs = self.covered_dirs();
        for d in self.game_dirs.iter().filter(|d| !d.trim().is_empty()).map(|d| expand_home(d.trim())) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        dirs
    }
}

const GAMES_DIR: &str = "~/Games";
const DOWNLOADS_DIR: &str = "~/Downloads/Games";
/// Where versions before the downloads folder saved downloads.
const OLD_DOWNLOAD_DIR: &str = "~/Downloads/PS5";

/// The folder that holds console folders, from an older single folder: its parent when it is a
/// console's folder (~/Games/PS5 → ~/Games), but never the home or root folder.
fn root_from(old: &str) -> String {
    let old = old.trim().trim_end_matches('/');
    let path = Path::new(old);
    let console = path.file_name().is_some_and(|n| Platform::ALL.iter().any(|p| n == p.label()));
    let parent = path.parent().filter(|p| {
        let full = expand_home(&p.to_string_lossy());
        console && !p.as_os_str().is_empty() && full != Path::new("/") && full != expand_home("~")
    });
    parent.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| old.to_string())
}

/// A folder as typed, without a trailing "/". Empty or relative gives `fallback`.
fn root_or(root: &str, fallback: &str) -> String {
    let root = root.trim().trim_end_matches('/');
    if root.is_empty() || !expand_home(root).is_absolute() { fallback.into() } else { root.into() }
}

pub(crate) fn runnable(path: &std::path::Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && is_executable(&m)).unwrap_or(false)
}

pub(crate) fn is_executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_update_opt_out_turns_every_automatic_update_off() {
        let mut old: Config = serde_json::from_str(r#"{"kyty_auto_update": false, "app_auto_update": false}"#).unwrap();
        assert!(old.shad_auto_update, "a missing shadPS4 entry defaults to on");
        old.one_update_switch();
        assert!(!old.app_auto_update && !old.kyty_auto_update && !old.shad_auto_update);
        let mut partial: Config = serde_json::from_str(r#"{"app_auto_update": false}"#).unwrap();
        partial.one_update_switch();
        assert!(!partial.kyty_auto_update && !partial.shad_auto_update);
        let mut fresh = Config::default();
        fresh.one_update_switch();
        assert!(fresh.app_auto_update && fresh.kyty_auto_update && fresh.shad_auto_update);
    }

    #[test]
    fn seeding_defaults_on_for_old_configs_and_preserves_explicit_opt_out() {
        let old: Config = serde_json::from_str(r#"{"sounds":false}"#).unwrap();
        assert!(old.seed_after_download);
        let opted_out: Config = serde_json::from_str(r#"{"seed_after_download":false}"#).unwrap();
        assert!(!opted_out.seed_after_download);
        let restored: Config = serde_json::from_slice(&serde_json::to_vec(&opted_out).unwrap()).unwrap();
        assert!(!restored.seed_after_download);
    }

    fn migrated(install: &str, dirs: &[String]) -> Config {
        let mut c: Config = serde_json::from_value(serde_json::json!({ "install_dir": install, "game_dirs": dirs })).unwrap();
        c.one_games_folder();
        c
    }

    #[test]
    fn older_settings_get_one_games_folder() {
        let full = expand_home("~/Games/PS5").to_string_lossy().into_owned();
        let c = migrated("~/Games/PS5", &["~/Games/PS5".into(), full, "~/Games".into(), "/mnt/other".into(), "~/Games/Other".into()]);
        assert_eq!(c.games_dir, "~/Games");
        assert_eq!(c.game_dirs, vec!["/mnt/other", "~/Games/Other"], "only folders the games folder scan covers are dropped");
        assert_eq!(migrated("/mnt/x/PS4/", &[]).games_dir, "/mnt/x");
        assert_eq!(migrated("/mnt/games", &[]).games_dir, "/mnt/games");
        assert_eq!(migrated("/mnt/games", &[]).install_dir, "/mnt/games/PS5", "older versions get the PS5 folder");
        assert_eq!(migrated("~/PS5", &[]).games_dir, "~/PS5", "never the whole home folder");
        assert_eq!(migrated("/PS5", &[]).games_dir, "/PS5", "never the root folder");
        let mut set: Config = serde_json::from_str(r#"{"games_dir": "/data", "install_dir": "~/Games/PS5"}"#).unwrap();
        set.one_games_folder();
        assert_eq!(set.games_dir, "/data", "a games folder you set is kept");
        let mut fresh = Config::default();
        fresh.one_games_folder();
        assert_eq!(fresh.games_dir, "~/Games");
        assert!(fresh.game_dirs.is_empty());
    }

    #[test]
    fn each_console_installs_into_its_own_subfolder() {
        let mut c = Config { games_dir: "~/Games/".into(), ..Config::default() };
        assert_eq!(c.install_path(Platform::Ps5), "~/Games/PS5");
        assert_eq!(c.install_path(Platform::Ps4), "~/Games/PS4");
        c.game_dirs = vec!["~/Games".into(), "/mnt/other".into()];
        let root = expand_home("~/Games");
        assert_eq!(c.game_dir_paths(), vec![root.clone(), root.join("PS5"), root.join("PS4"), root.join("PC"), PathBuf::from("/mnt/other")], "the games folder first, each once");
        assert_eq!(c.install_path(Platform::Pc), "~/Games/PC");
        for bad in ["", "  ", "Games"] {
            c.games_dir = bad.into();
            assert_eq!(c.install_path(Platform::Ps4), "~/Games/PS4", "{bad:?} is not a games folder");
        }
    }

    fn downloads_migrated(download: &str) -> Config {
        let mut c: Config = serde_json::from_value(serde_json::json!({ "download_dir": download })).unwrap();
        c.one_downloads_folder();
        c
    }

    #[test]
    fn older_settings_get_one_downloads_folder() {
        let old = downloads_migrated("~/Downloads/PS5");
        assert_eq!(old.downloads_dir, "~/Downloads/Games", "the old default becomes the new default");
        assert_eq!(old.download_dir, "~/Downloads/Games/PS5", "older versions get the PS5 folder");
        let full = expand_home("~/Downloads/PS5/").to_string_lossy().into_owned();
        assert_eq!(downloads_migrated(&full).downloads_dir, "~/Downloads/Games", "the old default, typed in full");
        assert_eq!(downloads_migrated("/mnt/t/PS4/").downloads_dir, "/mnt/t");
        assert_eq!(downloads_migrated("/mnt/torrents").downloads_dir, "/mnt/torrents");
        assert_eq!(downloads_migrated("~/PS5").downloads_dir, "~/PS5", "never the whole home folder");
        let mut fresh = Config::default();
        fresh.one_downloads_folder();
        assert_eq!(fresh.downloads_dir, "~/Downloads/Games");
        let mut set: Config = serde_json::from_str(r#"{"downloads_dir": "/data/dl", "download_dir": "~/Downloads/PS5"}"#).unwrap();
        set.one_downloads_folder();
        assert_eq!(set.downloads_dir, "/data/dl", "a downloads folder you set is kept");
    }

    #[test]
    fn each_console_downloads_into_its_own_subfolder() {
        let mut c = Config { downloads_dir: "~/Downloads/Games/".into(), ..Config::default() };
        assert_eq!(c.download_path(Platform::Ps5), "~/Downloads/Games/PS5");
        assert_eq!(c.download_path(Platform::Ps4), "~/Downloads/Games/PS4");
        for bad in ["", "  ", "Downloads"] {
            c.downloads_dir = bad.into();
            assert_eq!(c.download_path(Platform::Ps5), "~/Downloads/Games/PS5", "{bad:?} is not a downloads folder");
        }
    }

    #[test]
    fn games_stay_found_one_folder_deeper_after_migration() {
        // Before the games folder, ~/Games/PS5 was itself scanned, so ~/Games/PS5/<title>/<game> was found.
        let t = tempfile::tempdir().unwrap();
        let game = t.path().join("PS5").join("Some Title").join("PPSA12345-app0");
        std::fs::create_dir_all(game.join("sce_sys")).unwrap();
        std::fs::write(game.join("sce_sys/param.json"), r#"{"titleId":"PPSA12345"}"#).unwrap();
        let mut c = migrated(&t.path().join("PS5").to_string_lossy(), &[]);
        assert_eq!(PathBuf::from(&c.games_dir), t.path());
        assert_eq!(crate::library::scan(&c.game_dir_paths()).len(), 1);
        c.games_dir = t.path().to_string_lossy().into_owned();
        assert!(c.scans(&t.path().join("PS4")), "an install into a console folder needs no extra game folder");
        assert!(!c.scans(Path::new("/mnt/elsewhere")));
    }

    #[test]
    fn shad_is_your_own_only_outside_the_launchers_folder() {
        use crate::emulator::{shadps4::ShadPs4, Emulator};
        let mut c = Config::default();
        assert!(!ShadPs4.is_own(&c), "nothing set yet: the launcher installs its own");
        c.shad_emulator = "  ".into();
        assert!(!ShadPs4.is_own(&c), "blank text is not a path");
        c.shad_emulator = ShadPs4.binary().to_string_lossy().into_owned();
        assert!(!ShadPs4.is_own(&c), "the launcher's copy");
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("shadps4");
        c.shad_emulator = exe.to_string_lossy().into_owned();
        assert!(ShadPs4.is_own(&c) && !ShadPs4.runnable(&c), "a missing file is not runnable");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        assert!(!ShadPs4.runnable(&c), "not executable yet");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ShadPs4.runnable(&c));
        assert_eq!(ShadPs4.path(&c), exe);
    }

    #[test]
    fn new_settings_fields_are_optional_and_round_trip() {
        let old: Config = serde_json::from_str(r#"{"emulator": "/k/kyty_emulator"}"#).unwrap();
        assert!(old.emulator_paths.is_empty() && old.emulator_auto_update.is_empty() && old.platform_emulators.is_empty());
        let mut c = Config::default();
        c.emulator_paths.insert("other".into(), "/o/other".into());
        c.platform_emulators.insert("PS4".into(), "other".into());
        let again: Config = serde_json::from_slice(&serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(again.emulator_paths["other"], "/o/other");
        assert_eq!(again.platform_emulators["PS4"], "other");
    }
}
