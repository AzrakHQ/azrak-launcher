//! KytyPS5, the PS5 emulator. The install, update and version logic is in `crate::kyty`.

use super::{Emulator, ManagedStore, Release};
use crate::config::{Config, VIDEO_OUT_MODES};
use crate::platform::Platform;
use crate::sessions::shell_split;
use std::path::{Path, PathBuf};

pub struct KytyPs5;

impl Emulator for KytyPs5 {
    fn id(&self) -> &'static str {
        "kytyps5"
    }

    fn name(&self) -> &'static str {
        "KytyPS5"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::Ps5]
    }

    fn process_names(&self) -> &'static [&'static str] {
        &["kyty_emulator", "kyty_emulator.exe"]
    }

    fn game_flags(&self) -> &'static [&'static str] {
        &["--game"]
    }

    /// The window size (--screen-width and --screen-height) only sizes the window;
    /// --video-out-resolution is what the game is told its screen is. Extra arguments come last,
    /// so they can override the rest.
    fn launch_args(&self, cfg: &Config, game: &Path) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--game".into(),
            game.to_string_lossy().into_owned(),
            "--screen-width".into(),
            cfg.width.to_string(),
            "--screen-height".into(),
            cfg.height.to_string(),
            "--present-mode".into(),
            cfg.present_mode.clone(),
            "--video-out-resolution".into(),
            // A value the launcher does not know (a hand-edited settings file) falls back to the default.
            VIDEO_OUT_MODES.iter().find(|(k, _)| *k == cfg.video_out).map_or(VIDEO_OUT_MODES[0].0, |(k, _)| *k).to_string(),
        ];
        if cfg.fullscreen {
            args.push("--fullscreen".into());
        }
        if cfg.amd_cpu {
            args.push("--amd-cpu".into());
        }
        args.extend(shell_split(&cfg.extra_args));
        args
    }

    fn store(&self) -> ManagedStore {
        crate::kyty::store()
    }

    fn binary(&self) -> PathBuf {
        crate::kyty::managed_emulator()
    }

    fn latest_release(&self) -> Result<Release, String> {
        crate::kyty::latest_release()
    }

    fn install(&self, rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String> {
        crate::kyty::install(rel, progress)
    }

    fn pretty(&self, tag: &str) -> String {
        crate::kyty::pretty(tag)
    }

    /// The managed build's tag, or a custom build's "2026-09-30 (b7a1fac)".
    fn version_for(&self, cfg: &Config) -> String {
        crate::kyty::version_for(&self.path(cfg))
    }

    fn detect(&self) -> Option<PathBuf> {
        crate::config::detect_emulator()
    }

    fn keeps_missing_custom_path(&self) -> bool {
        false
    }

    fn watches_custom_binary_name(&self) -> bool {
        true
    }

    fn unavailable_message(&self, cfg: &Config) -> String {
        format!("Emulator not found or not executable:\n{}", self.path(cfg).display())
    }

    fn path_setting<'a>(&self, cfg: &'a Config) -> &'a str {
        &cfg.emulator
    }

    fn set_path_setting(&self, cfg: &mut Config, path: String) {
        cfg.emulator = path;
    }

    fn auto_update(&self, cfg: &Config) -> bool {
        cfg.kyty_auto_update
    }

    fn set_auto_update(&self, cfg: &mut Config, on: bool) {
        cfg.kyty_auto_update = on;
    }

    fn fetch_on_first_run(&self) -> bool {
        true
    }

    /// "6799ecb (2026-09-29)", from the build's `--help` banner.
    fn describe_own_build(&self, path: &Path) -> Option<String> {
        crate::kyty::binary_version(path).map(|(git, date)| format!("{git} ({date})"))
    }

    fn switch_hint(&self) -> &'static str {
        " · switching keeps your saves"
    }

    /// Saves and shader caches are copied (never moved) from a self-built install.
    fn after_install(&self, previous: &Path, was_managed: bool) -> Option<String> {
        if was_managed {
            return None;
        }
        let copied = previous.parent().map(crate::kyty::import_data_from).unwrap_or(0);
        (copied > 0).then(|| "Your saves and shader caches were copied over.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kyty_gets_the_video_out_resolution_and_extra_arguments_last() {
        let game = Path::new("/g/PPSA1");
        let mut cfg = Config::default();
        let args = KytyPs5.launch_args(&cfg, game);
        assert_eq!(&args[..2], ["--game", "/g/PPSA1"]);
        let at = args.iter().position(|a| a == "--video-out-resolution").expect("always passed");
        assert_eq!(args[at + 1], "Title", "the default is what a PS5 reports for the game");
        // The window size is separate from what the game is told.
        assert!(args.windows(2).any(|w| w == ["--screen-width", "1920"]));
        for (mode, value) in [("FullHd", "FullHd"), ("Uhd", "Uhd")] {
            cfg.video_out = mode.into();
            let args = KytyPs5.launch_args(&cfg, game);
            let at = args.iter().position(|a| a == "--video-out-resolution").unwrap();
            assert_eq!(args[at + 1], value);
        }
        cfg.video_out = "Bogus".into();
        let args = KytyPs5.launch_args(&cfg, game);
        let at = args.iter().position(|a| a == "--video-out-resolution").unwrap();
        assert_eq!(args[at + 1], "Title", "an unknown value falls back to the default");
        cfg.extra_args = "--tessellation".into();
        assert_eq!(KytyPs5.launch_args(&cfg, game).last().map(String::as_str), Some("--tessellation"));
    }

    #[test]
    fn fullscreen_and_amd_cpu_are_options() {
        let mut cfg = Config { fullscreen: false, amd_cpu: false, ..Config::default() };
        let args = KytyPs5.launch_args(&cfg, Path::new("/g/x"));
        assert!(!args.contains(&"--fullscreen".to_string()) && !args.contains(&"--amd-cpu".to_string()));
        (cfg.fullscreen, cfg.amd_cpu) = (true, true);
        let args = KytyPs5.launch_args(&cfg, Path::new("/g/x"));
        assert!(args.contains(&"--fullscreen".to_string()) && args.contains(&"--amd-cpu".to_string()));
    }

    #[test]
    fn settings_live_in_the_original_fields() {
        let mut cfg = Config::default();
        KytyPs5.set_path_setting(&mut cfg, "/k/kyty_emulator".into());
        assert_eq!(cfg.emulator, "/k/kyty_emulator");
        assert_eq!(KytyPs5.path(&cfg), PathBuf::from("/k/kyty_emulator"));
        assert_eq!(KytyPs5.unavailable_message(&cfg), "Emulator not found or not executable:\n/k/kyty_emulator");
        cfg.kyty_auto_update = false;
        assert!(!KytyPs5.auto_update(&cfg));
    }
}
