//! KytyPS5, the PS5 emulator. The install, update and version logic is in `crate::emulator::kytyps5::install`.

pub mod install;
pub mod pkg;

use super::{Controls, Emulator, ManagedStore, Release, Setting, SettingKind};
use crate::config::{Config, VIDEO_OUT_MODES};

const RESOLUTION_CHOICES: &[(&str, &str)] =
    &[("1280x720", "1280 × 720"), ("1600x900", "1600 × 900"), ("1920x1080", "1920 × 1080"), ("2560x1440", "2560 × 1440"), ("3840x2160", "3840 × 2160")];
const VIDEO_OUT_CHOICES: &[(&str, &str)] = &VIDEO_OUT_MODES;
const PRESENT_CHOICES: &[(&str, &str)] = &[("Mailbox", "Mailbox (low latency)"), ("Fifo", "Fifo (V-Sync)"), ("Immediate", "Immediate (uncapped)")];

const SETTINGS: &[Setting] = &[
    Setting { key: "resolution", label: "Resolution (PS5 games)", hint: "The size of the window. What the game is told is Game output resolution", kind: SettingKind::Choice(RESOLUTION_CHOICES), default: "1920x1080", advanced: true },
    Setting { key: "video_out", label: "Game output resolution (PS5 games)", hint: "The screen resolution the game is told it runs on. The game renders at this size.", kind: SettingKind::Choice(VIDEO_OUT_CHOICES), default: "Title", advanced: true },
    Setting { key: "present_mode", label: "Present mode (PS5 games)", hint: "Try V-Sync if the picture tears", kind: SettingKind::Choice(PRESENT_CHOICES), default: "Mailbox", advanced: true },
    Setting { key: "amd_cpu", label: "AMD CPU patches (PS5 games)", hint: "Only for AMD processors", kind: SettingKind::Toggle, default: "false", advanced: true },
    Setting { key: "extra_args", label: "Extra KytyPS5 arguments", hint: "For example --vblank-frequency 60 --tessellation", kind: SettingKind::Text, default: "", advanced: true },
];
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
        install::store()
    }

    fn binary(&self) -> PathBuf {
        install::managed_emulator()
    }

    fn latest_release(&self) -> Result<Release, String> {
        install::latest_release()
    }

    fn install(&self, rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String> {
        install::install(rel, progress)
    }

    fn pretty(&self, tag: &str) -> String {
        install::pretty(tag)
    }

    /// The managed build's tag, or a custom build's "2026-09-30 (b7a1fac)".
    fn version_for(&self, cfg: &Config) -> String {
        install::version_for(&self.path(cfg))
    }

    fn detect(&self) -> Option<PathBuf> {
        detect_emulator()
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

    fn compat_page(&self) -> Option<&'static str> {
        Some(crate::compat::LIST_PAGE)
    }

    fn controls(&self) -> Option<Controls> {
        Some(Controls {
            splash: &[("✕", "J"), ("△", "I"), ("□", "K"), ("○", "L"), ("Move", "W A S D"), ("D-pad", "Arrows"), ("Options", "Enter")],
            columns: &[
                &[("✕  Cross", "J"), ("○  Circle", "L"), ("□  Square", "K"), ("△  Triangle", "I"), ("D-pad", "Arrow keys"), ("Options", "Enter")],
                &[("Left stick", "W  A  S  D"), ("Right stick", "T  F  G  H"), ("L1  /  R1", "Q  /  E"), ("L2  /  R2", "Z  /  C"),
                  ("L3  /  R3", "Left Shift  /  Left Ctrl"), ("Touchpad", "Backspace  /  Tab")],
            ],
            note: "F7 turns the mouse into the right stick (press it again to release the mouse).",
        })
    }

    fn settings(&self) -> &'static [Setting] {
        SETTINGS
    }

    // These options predate the settings map, so they keep their original fields.
    fn setting(&self, cfg: &Config, key: &str) -> String {
        match key {
            "resolution" => format!("{}x{}", cfg.width, cfg.height),
            "video_out" => cfg.video_out.clone(),
            "present_mode" => cfg.present_mode.clone(),
            "amd_cpu" => cfg.amd_cpu.to_string(),
            "extra_args" => cfg.extra_args.clone(),
            _ => String::new(),
        }
    }

    fn set_setting(&self, cfg: &mut Config, key: &str, value: String) {
        match key {
            "resolution" => {
                if let Some((w, h)) = value.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))) {
                    (cfg.width, cfg.height) = (w, h);
                }
            }
            "video_out" => cfg.video_out = value,
            "present_mode" => cfg.present_mode = value,
            "amd_cpu" => cfg.amd_cpu = value == "true",
            "extra_args" => cfg.extra_args = value,
            _ => {}
        }
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
        install::binary_version(path).map(|(git, date)| format!("{git} ({date})"))
    }

    fn switch_hint(&self) -> &'static str {
        " · switching keeps your saves"
    }

    /// Saves and shader caches are copied (never moved) from a self-built install.
    fn after_install(&self, previous: &Path, was_managed: bool) -> Option<String> {
        if was_managed {
            return None;
        }
        let copied = previous.parent().map(install::import_data_from).unwrap_or(0);
        (copied > 0).then(|| "Your saves and shader caches were copied over.".to_string())
    }
}

/// Look for kyty_emulator in PATH and a few common build locations.
pub(super) fn detect_emulator() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("kyty_emulator")));
    }
    let home = crate::util::expand_home("~");
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let mut roots = vec![home.clone(), home.join("projects"), home.join("Games"), home.join("Applications"), home.join("src")];
    if let Some(d) = &exe_dir {
        roots.extend(d.ancestors().take(4).map(|p| p.to_path_buf()));
    }
    for root in roots {
        for sub in ["KytyPS5", "kyty", "Kyty"] {
            for build in ["_Build/release-linux", "_Build/linux/install", "_Build/linux", "build", ""] {
                candidates.push(root.join(sub).join(build).join("kyty_emulator"));
            }
        }
        // One level deeper, e.g. ~/projects/PS5/KytyPS5
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten().take(200) {
                let p = e.path();
                if p.is_dir() {
                    for build in ["KytyPS5/_Build/release-linux", "KytyPS5/_Build/linux/install"] {
                        candidates.push(p.join(build).join("kyty_emulator"));
                    }
                }
            }
        }
    }
    candidates.into_iter().find(|p| std::fs::metadata(p).map(|m| m.is_file() && crate::config::is_executable(&m)).unwrap_or(false))
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
