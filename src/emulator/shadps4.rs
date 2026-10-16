//! shadPS4, the PS4 emulator. The install, update and version logic is in `crate::shad`.

use super::{Controls, Emulator, ManagedStore, Release};
use crate::config::Config;
use crate::platform::Platform;
use std::path::{Path, PathBuf};

pub struct ShadPs4;

impl Emulator for ShadPs4 {
    fn id(&self) -> &'static str {
        "shadps4"
    }

    fn name(&self) -> &'static str {
        "shadPS4"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::Ps4]
    }

    fn process_names(&self) -> &'static [&'static str] {
        &["shadps4"]
    }

    fn game_flags(&self) -> &'static [&'static str] {
        &["-g"]
    }

    /// The game folder and fullscreen; shadPS4's other settings live in its own config.
    fn launch_args(&self, cfg: &Config, game: &Path) -> Vec<String> {
        vec!["-g".into(), game.to_string_lossy().into_owned(), "-f".into(), cfg.fullscreen.to_string()]
    }

    fn store(&self) -> ManagedStore {
        crate::shad::store()
    }

    fn binary(&self) -> PathBuf {
        crate::shad::emulator()
    }

    fn latest_release(&self) -> Result<Release, String> {
        crate::shad::latest_release()
    }

    fn install(&self, rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String> {
        crate::shad::install(rel, progress)
    }

    fn pretty(&self, tag: &str) -> String {
        crate::shad::pretty(tag)
    }

    fn version_for(&self, cfg: &Config) -> String {
        if self.is_own(cfg) { "own build".into() } else { self.pretty(&self.store().load_state().installed) }
    }

    fn compat_page(&self) -> Option<&'static str> {
        Some(crate::compat::SHAD_LIST_PAGE)
    }

    fn report_note(&self) -> &'static str {
        "shadPS4 only accepts reports for games dumped from a copy you own, unmodified."
    }

    fn controls(&self) -> Option<Controls> {
        Some(Controls {
            splash: &[("✕", "N"), ("△", "C"), ("□", "V"), ("○", "B"), ("Move", "W A S D"), ("D-pad", "Arrows"), ("Options", "Enter")],
            columns: &[
                &[("✕  Cross", "N  (or keypad 2)"), ("○  Circle", "B  (or keypad 6)"), ("□  Square", "V  (or keypad 4)"),
                  ("△  Triangle", "C  (or keypad 8)"), ("D-pad", "Arrow keys"), ("Options", "Enter")],
                &[("Left stick", "W  A  S  D"), ("Right stick", "I  J  K  L"), ("L1  /  R1", "Q  /  U"), ("L2  /  R2", "E  /  O"),
                  ("L3  /  R3", "X  /  M"), ("Touchpad", "Space")],
            ],
            note: "shadPS4 keeps its key layout in its own settings file; these are its defaults.",
        })
    }

    fn path_setting<'a>(&self, cfg: &'a Config) -> &'a str {
        &cfg.shad_emulator
    }

    fn set_path_setting(&self, cfg: &mut Config, path: String) {
        cfg.shad_emulator = path;
    }

    fn auto_update(&self, cfg: &Config) -> bool {
        cfg.shad_auto_update
    }

    fn set_auto_update(&self, cfg: &mut Config, on: bool) {
        cfg.shad_auto_update = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shad_gets_the_game_folder_and_fullscreen() {
        let mut cfg = Config::default();
        assert_eq!(ShadPs4.launch_args(&cfg, Path::new("/g/CUSA1")), ["-g", "/g/CUSA1", "-f", "true"]);
        cfg.fullscreen = false;
        assert_eq!(ShadPs4.launch_args(&cfg, Path::new("/g/CUSA1")), ["-g", "/g/CUSA1", "-f", "false"]);
    }

    #[test]
    fn a_path_you_set_is_your_own_build() {
        let mut cfg = Config::default();
        assert!(!ShadPs4.is_own(&cfg));
        assert_eq!(ShadPs4.unavailable_message(&cfg), "shadPS4 isn't installed yet");
        ShadPs4.set_path_setting(&mut cfg, "/home/me/shadps4".into());
        assert_eq!(cfg.shad_emulator, "/home/me/shadps4");
        assert!(ShadPs4.is_own(&cfg));
        assert_eq!(ShadPs4.unavailable_message(&cfg), "shadPS4 not found or not executable:\n/home/me/shadps4");
        assert_eq!(ShadPs4.version_for(&cfg), "own build");
    }
}
