//! Emulators as plugins. Everything the launcher needs to know about an emulator lives behind the
//! [`Emulator`] trait; the rest of the app asks the registry ([`all`], [`selected`], [`by_id`])
//! instead of naming KytyPS5 or shadPS4.
//!
//! Adding an emulator is one new file in this folder that implements [`Emulator`], plus one line
//! in [`REGISTRY`]. See docs/ADDING_AN_EMULATOR.md for the steps and what is still wired by hand.
//!
//! The plugins are compiled in (no dynamic loading): Rust has no stable ABI, and every emulator
//! here is first-party. What varies between emulators is data and a little logic (where releases
//! come from, how a build is unpacked, which command line starts a game), which is exactly what
//! the trait captures.

pub mod download;
pub mod kytyps5;
pub mod shadps4;
pub mod store;

use crate::config::Config;
use crate::platform::Platform;
use crate::util::expand_home;
use std::path::{Path, PathBuf};

pub use store::{ManagedStore, Release};

/// How long after a check the launcher looks for a new release again.
pub const CHECK_INTERVAL: f64 = 6.0 * 3600.0;

/// How Settings edits an option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    Toggle,
    /// Typed text, e.g. extra command-line arguments.
    Text,
    /// One of these (value, name) pairs, cycled with left/right.
    Choice(&'static [(&'static str, &'static str)]),
}

/// One option of an emulator. Settings builds its row from this; nothing else is needed.
#[derive(Clone, Copy, Debug)]
pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: SettingKind,
    /// The value when nothing was chosen ("true"/"false" for a toggle).
    pub default: &'static str,
    /// Shown only under Settings → Advanced.
    pub advanced: bool,
}

/// The keyboard layout an emulator uses when no controller is connected. Shown while a game
/// starts and on the Controls sheet.
#[derive(Clone, Copy, Debug)]
pub struct Controls {
    /// While a game starts: (what, keys), a short strip.
    pub splash: &'static [(&'static str, &'static str)],
    /// The Controls sheet: columns of (what, keys).
    pub columns: &'static [&'static [(&'static str, &'static str)]],
    /// A line under the sheet.
    pub note: &'static str,
}

pub trait Emulator: Sync + Send {
    // ------------------------------------------------------------------ identity

    /// Stable key used in settings files and as the folder name's source of truth. Never change it.
    fn id(&self) -> &'static str;

    /// What the user sees: "KytyPS5".
    fn name(&self) -> &'static str;

    /// The platforms whose games this emulator can run.
    fn platforms(&self) -> &'static [Platform];

    // ------------------------------------------------------------------ running games

    /// The emulator's process names, so a game started outside the launcher is noticed too.
    fn process_names(&self) -> &'static [&'static str];

    /// The command-line options that carry the game's folder ("--game", "-g").
    fn game_flags(&self) -> &'static [&'static str];

    /// Arguments that start `game` (a game folder), from the user's settings.
    fn launch_args(&self, cfg: &Config, game: &Path) -> Vec<String>;

    // ------------------------------------------------------------------ the managed install

    /// The launcher's copy of the emulator on disk.
    fn store(&self) -> ManagedStore;

    /// Stable path of the managed emulator's executable (the same across updates).
    fn binary(&self) -> PathBuf;

    /// The newest official build for this OS.
    fn latest_release(&self) -> Result<Release, String>;

    /// Download, verify, unpack and activate `rel` in [`Emulator::store`]. Report
    /// (message, fraction 0..1) through `progress`.
    fn install(&self, rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String>;

    /// A tag as shown to people ("v.0.18.0" -> "0.18.0").
    fn pretty(&self, tag: &str) -> String {
        tag.to_string()
    }

    // ------------------------------------------------------------------ optional behaviour

    /// What a game was played on, for its rating: the installed release, or a custom build's
    /// version. May run the emulator, so call it off the UI thread.
    fn version_for(&self, _cfg: &Config) -> String {
        self.pretty(&self.store().load_state().installed)
    }

    /// Find an emulator the user already has (PATH, a source checkout, ...).
    fn detect(&self) -> Option<PathBuf> {
        None
    }

    /// Whether a path in settings that no longer exists is replaced by the managed or a detected
    /// emulator. Default: a path the user set is kept, so they see it is wrong.
    fn keeps_missing_custom_path(&self) -> bool {
        true
    }

    /// Whether a running process named like the emulator the user configured counts as this
    /// emulator, whatever it is called (a self-built or renamed binary).
    fn watches_custom_binary_name(&self) -> bool {
        false
    }

    /// Whether the welcome screen waits for this emulator on the first start (and whenever it
    /// is missing), so games can be played the moment the launcher opens.
    fn fetch_on_first_run(&self) -> bool {
        false
    }

    /// Whether games use the user's own build and the launcher leaves it alone. A path that no
    /// longer exists is not a build (when the emulator replaces such paths).
    fn uses_own_build(&self, cfg: &Config) -> bool {
        self.is_own(cfg) && (self.keeps_missing_custom_path() || self.runnable(cfg))
    }

    /// Version text for the user's own build ("6799ecb (2026-09-29)"). May run the emulator, so
    /// it is called off the UI thread.
    fn describe_own_build(&self, _path: &Path) -> Option<String> {
        None
    }

    /// Added to the Settings hint of an own build ("... · switching keeps your saves").
    fn switch_hint(&self) -> &'static str {
        ""
    }

    /// Runs after a managed build was installed, off the UI thread. `previous` is the emulator
    /// the games used before, `was_managed` whether it was the launcher's own copy. Return a
    /// sentence to show with the "installed" notice (e.g. that saves were copied over).
    fn after_install(&self, _previous: &Path, _was_managed: bool) -> Option<String> {
        None
    }

    /// Why a game can't start: the emulator isn't installed, or the configured path is wrong.
    fn unavailable_message(&self, cfg: &Config) -> String {
        if self.is_own(cfg) {
            format!("{} not found or not executable:\n{}", self.name(), self.path(cfg).display())
        } else {
            format!("{} isn't installed yet", self.name())
        }
    }

    // ------------------------------------------------------------------ settings storage
    //
    // By default an emulator keeps its settings in `Config::emulator_paths` and
    // `Config::emulator_auto_update`, under its id. KytyPS5 and shadPS4 predate those maps and
    // keep reading their original fields, so existing settings files keep working.

    /// The path as typed in settings ("" = use the managed copy).
    fn path_setting<'a>(&self, cfg: &'a Config) -> &'a str {
        cfg.emulator_paths.get(self.id()).map_or("", String::as_str)
    }

    fn set_path_setting(&self, cfg: &mut Config, path: String) {
        cfg.emulator_paths.insert(self.id().to_string(), path);
    }

    /// The community compatibility list's page, opened from a game's menu.
    fn compat_page(&self) -> Option<&'static str> {
        None
    }

    /// A reminder shown when the player's ratings for this emulator are opened for sharing.
    fn report_note(&self) -> &'static str {
        ""
    }

    /// Its keyboard layout, if it has one the launcher should show.
    fn controls(&self) -> Option<Controls> {
        None
    }

    /// The options Settings shows for this emulator (see `Setting`). Rows appear by themselves.
    fn settings(&self) -> &'static [Setting] {
        &[]
    }

    /// An option's current value (a toggle is "true" or "false").
    fn setting(&self, cfg: &Config, key: &str) -> String {
        let default = self.settings().iter().find(|s| s.key == key).map_or("", |s| s.default);
        cfg.emulator_settings.get(&format!("{}.{key}", self.id())).cloned().unwrap_or_else(|| default.to_string())
    }

    fn set_setting(&self, cfg: &mut Config, key: &str, value: String) {
        cfg.emulator_settings.insert(format!("{}.{key}", self.id()), value);
    }

    /// Whether the launcher keeps its managed copy on the latest official build.
    fn auto_update(&self, cfg: &Config) -> bool {
        cfg.emulator_auto_update.get(self.id()).copied().unwrap_or(true)
    }

    fn set_auto_update(&self, cfg: &mut Config, on: bool) {
        cfg.emulator_auto_update.insert(self.id().to_string(), on);
    }

    // ------------------------------------------------------------------ derived (rarely overridden)

    /// The emulator to start, with `~` expanded.
    fn path(&self, cfg: &Config) -> PathBuf {
        expand_home(self.path_setting(cfg).trim())
    }

    /// Whether the configured emulator exists and can be run.
    fn runnable(&self, cfg: &Config) -> bool {
        crate::config::runnable(&self.path(cfg))
    }

    /// Whether `path` is inside the launcher's folder for this emulator.
    fn is_managed(&self, path: &Path) -> bool {
        path.starts_with(self.store().root())
    }

    /// Whether games use the user's own build: a path set outside the launcher's folder. The
    /// launcher then neither installs nor updates this emulator.
    fn is_own(&self, cfg: &Config) -> bool {
        !self.path_setting(cfg).trim().is_empty() && !self.is_managed(&self.path(cfg))
    }

    /// Whether the launcher's copy is installed.
    fn installed(&self) -> bool {
        self.binary().is_file() && !self.store().load_state().installed.is_empty()
    }

    fn rollback(&self) -> Result<String, String> {
        self.store().rollback()
    }
}

/// "PS4", or "PS5, PS4" for an emulator that runs several platforms' games.
pub fn platform_labels(emu: &dyn Emulator) -> String {
    emu.platforms().iter().map(|p| p.label()).collect::<Vec<_>>().join(", ")
}

/// The emulators with a keyboard layout to show, as Controls sheet tabs.
pub fn with_controls() -> Vec<&'static dyn Emulator> {
    all().iter().copied().filter(|e| e.controls().is_some()).collect()
}

/// "PS5 games on KytyPS5 · PS4 games on shadPS4", for the welcome screen.
pub fn summary() -> String {
    all().iter().map(|e| format!("{} games on {}", platform_labels(*e), e.name())).collect::<Vec<_>>().join(" · ")
}

/// Every emulator, in the order they are offered. Register new ones here.
static REGISTRY: &[&dyn Emulator] = &[&kytyps5::KytyPs5, &shadps4::ShadPs4];

pub fn all() -> &'static [&'static dyn Emulator] {
    REGISTRY
}

pub fn by_id(id: &str) -> Option<&'static dyn Emulator> {
    all().iter().copied().find(|e| e.id() == id)
}

/// The first registered emulator that runs `platform`'s games.
pub fn for_platform(platform: Platform) -> Option<&'static dyn Emulator> {
    all().iter().copied().find(|e| e.platforms().contains(&platform))
}

/// The emulator that runs `platform`'s games: the user's choice (`Config::platform_emulators`)
/// when it still names an emulator that supports the platform, otherwise the first registered.
pub fn selected(cfg: &Config, platform: Platform) -> Option<&'static dyn Emulator> {
    cfg.platform_emulators
        .get(platform.label())
        .and_then(|id| by_id(id))
        .filter(|e| e.platforms().contains(&platform))
        .or_else(|| for_platform(platform))
}

/// Process names of all emulators.
pub fn process_names() -> Vec<&'static str> {
    all().iter().flat_map(|e| e.process_names().iter().copied()).collect()
}

/// The options that carry a game folder, for all emulators.
pub fn game_flags() -> Vec<&'static str> {
    let mut flags: Vec<&'static str> = all().iter().flat_map(|e| e.game_flags().iter().copied()).collect();
    flags.dedup();
    flags
}

/// Settings a first run (or an upgrade) fills in for `emu`: the managed copy once it exists, or
/// one found on this computer.
pub fn adopt_default_path(cfg: &mut Config, emu: &dyn Emulator) {
    let empty = emu.path_setting(cfg).trim().is_empty();
    let usable = if emu.keeps_missing_custom_path() { !empty } else { emu.path(cfg).is_file() };
    if usable {
        return;
    }
    let managed = emu.binary();
    if managed.is_file() {
        emu.set_path_setting(cfg, managed.to_string_lossy().into_owned());
    } else if let Some(found) = emu.detect() {
        emu.set_path_setting(cfg, found.to_string_lossy().into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Checks that hold for every registered emulator, so a new one is covered by adding it to
    /// the registry: a mistake here is a mistake the launcher would only show at run time.
    #[test]
    fn every_emulator_setting_is_usable_by_settings() {
        for emu in all() {
            let mut keys = std::collections::HashSet::new();
            let mut cfg = Config::default();
            for s in emu.settings() {
                assert!(keys.insert(s.key), "{}: duplicate setting {}", emu.id(), s.key);
                let value = emu.setting(&cfg, s.key);
                match s.kind {
                    SettingKind::Toggle => assert!(matches!(value.as_str(), "true" | "false"), "{}.{}: {value}", emu.id(), s.key),
                    SettingKind::Choice(options) => {
                        assert!(options.iter().any(|(v, _)| *v == value), "{}.{}: default {value} is not a choice", emu.id(), s.key);
                        for (v, _) in options {
                            emu.set_setting(&mut cfg, s.key, v.to_string());
                            assert_eq!(emu.setting(&cfg, s.key), *v, "{}.{} must keep what it was given", emu.id(), s.key);
                        }
                    }
                    SettingKind::Text => {
                        emu.set_setting(&mut cfg, s.key, "--x".into());
                        assert_eq!(emu.setting(&cfg, s.key), "--x");
                    }
                }
            }
            if let Some(c) = emu.controls() {
                assert!(!c.splash.is_empty() && !c.columns.is_empty());
            }
        }
    }

    #[test]
    fn every_registered_emulator_honours_the_contract() {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for e in all() {
            assert!(ids.insert(e.id()), "emulator id {:?} is used twice", e.id());
            assert!(names.insert(e.name()), "emulator name {:?} is used twice", e.name());
            assert!(!e.id().is_empty() && e.id().chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()), "{}: the id is lowercase letters and digits (it is a settings key)", e.id());
            assert!(!e.platforms().is_empty(), "{}: runs no platform", e.name());
            assert!(!e.process_names().is_empty(), "{}: without a process name its games can't be detected", e.name());
            assert!(!e.game_flags().is_empty(), "{}: without a game flag its games can't be detected", e.name());
            assert!(e.game_flags().iter().all(|f| f.starts_with('-')), "{}: game flags are options", e.name());
            let root = e.store().root().to_path_buf();
            assert!(root.starts_with(crate::util::data_dir()), "{}: managed files belong in the launcher's data folder", e.name());
            assert!(e.binary().starts_with(&root), "{}: the managed binary is inside its store", e.name());
            assert_eq!(e.binary().strip_prefix(&root).unwrap().components().next().unwrap().as_os_str(), "current", "{}: the binary is reached through the `current` link, so updates keep its path", e.name());
            assert!(e.is_managed(&e.binary()));
            assert!(!e.is_managed(Path::new("/usr/bin/anything")));
        }
        let roots: HashSet<PathBuf> = all().iter().map(|e| e.store().root().to_path_buf()).collect();
        assert_eq!(roots.len(), all().len(), "two emulators share a folder");
    }

    #[test]
    fn every_platform_but_pc_has_an_emulator() {
        for p in Platform::ALL {
            assert_eq!(for_platform(p).is_some(), p != Platform::Pc, "{}: games that no emulator runs can't be played", p.label());
        }
    }

    #[test]
    fn every_console_platform_has_an_emulator() {
        assert_eq!(for_platform(Platform::Ps5).unwrap().id(), "kytyps5");
        assert_eq!(for_platform(Platform::Ps4).unwrap().id(), "shadps4");
        assert!(for_platform(Platform::Pc).is_none(), "PC games run by themselves");
        assert_eq!(Platform::Ps5.emulator(), "KytyPS5");
        assert_eq!(Platform::Ps4.emulator(), "shadPS4");
        assert_eq!(Platform::Pc.emulator(), "Linux");
    }

    #[test]
    fn the_choice_for_a_platform_is_honoured_only_when_it_is_valid() {
        let mut cfg = Config::default();
        assert_eq!(selected(&cfg, Platform::Ps4).unwrap().id(), "shadps4");
        cfg.platform_emulators.insert("PS4".into(), "kytyps5".into());
        assert_eq!(selected(&cfg, Platform::Ps4).unwrap().id(), "shadps4", "KytyPS5 doesn't run PS4 games");
        cfg.platform_emulators.insert("PS4".into(), "gone".into());
        assert_eq!(selected(&cfg, Platform::Ps4).unwrap().id(), "shadps4", "an emulator that no longer exists");
        cfg.platform_emulators.insert("PS5".into(), "kytyps5".into());
        assert_eq!(selected(&cfg, Platform::Ps5).unwrap().id(), "kytyps5");
    }

    #[test]
    fn detection_knows_every_emulators_names_and_flags() {
        let names = process_names();
        assert!(names.contains(&"kyty_emulator") && names.contains(&"shadps4"));
        let flags = game_flags();
        assert!(flags.contains(&"--game") && flags.contains(&"-g"));
    }

    #[test]
    fn existing_settings_keep_working() {
        let mut cfg: Config = serde_json::from_str(r#"{"emulator": "/k/kyty_emulator", "shad_emulator": "/s/shadps4", "kyty_auto_update": false}"#).unwrap();
        let (kyty, shad) = (by_id("kytyps5").unwrap(), by_id("shadps4").unwrap());
        assert_eq!(kyty.path_setting(&cfg), "/k/kyty_emulator");
        assert_eq!(shad.path_setting(&cfg), "/s/shadps4");
        assert!(!kyty.auto_update(&cfg));
        assert!(shad.auto_update(&cfg));
        shad.set_path_setting(&mut cfg, "/other/shadps4".into());
        assert_eq!(cfg.shad_emulator, "/other/shadps4", "written to the original field, which older launcher versions read");
        assert!(cfg.emulator_paths.is_empty());
    }

    /// An emulator written only with the required methods works: this is the smallest plugin.
    struct Minimal;
    impl Emulator for Minimal {
        fn id(&self) -> &'static str { "minimal" }
        fn name(&self) -> &'static str { "Minimal" }
        fn platforms(&self) -> &'static [Platform] { &[Platform::Ps4] }
        fn process_names(&self) -> &'static [&'static str] { &["minimal"] }
        fn game_flags(&self) -> &'static [&'static str] { &["--rom"] }
        fn launch_args(&self, _cfg: &Config, game: &Path) -> Vec<String> { vec!["--rom".into(), game.to_string_lossy().into_owned()] }
        fn store(&self) -> ManagedStore { ManagedStore::new(crate::util::data_dir().join("minimal")) }
        fn binary(&self) -> PathBuf { self.store().root().join("current").join("minimal") }
        fn latest_release(&self) -> Result<Release, String> { Err("offline".into()) }
        fn install(&self, _rel: &Release, _progress: &dyn Fn(String, f32)) -> Result<(), String> { Err("offline".into()) }
    }

    #[test]
    fn a_minimal_emulator_stores_its_settings_by_id_with_working_defaults() {
        let (e, mut cfg) = (Minimal, Config::default());
        assert!(e.auto_update(&cfg), "automatic updates default to on");
        assert!(!e.is_own(&cfg) && !e.runnable(&cfg));
        assert_eq!(e.unavailable_message(&cfg), "Minimal isn't installed yet");
        e.set_path_setting(&mut cfg, "/opt/minimal/minimal".into());
        assert!(e.is_own(&cfg));
        assert_eq!(e.unavailable_message(&cfg), "Minimal not found or not executable:\n/opt/minimal/minimal");
        assert_eq!(cfg.emulator_paths.get("minimal").map(String::as_str), Some("/opt/minimal/minimal"));
        // The settings round-trip through the file.
        let again: Config = serde_json::from_slice(&serde_json::to_vec(&cfg).unwrap()).unwrap();
        assert_eq!(e.path_setting(&again), "/opt/minimal/minimal");
        assert_eq!(e.pretty("v1"), "v1");
        assert!(!e.installed());
    }

    #[test]
    fn a_missing_path_is_kept_unless_the_emulator_replaces_it() {
        let t = tempfile::tempdir().unwrap();
        let managed = t.path().join("current").join("minimal");
        struct InTemp(PathBuf, bool);
        impl Emulator for InTemp {
            fn id(&self) -> &'static str { "intemp" }
            fn name(&self) -> &'static str { "InTemp" }
            fn platforms(&self) -> &'static [Platform] { &[Platform::Ps4] }
            fn process_names(&self) -> &'static [&'static str] { &["intemp"] }
            fn game_flags(&self) -> &'static [&'static str] { &["-g"] }
            fn launch_args(&self, _: &Config, _: &Path) -> Vec<String> { vec![] }
            fn store(&self) -> ManagedStore { ManagedStore::new(self.0.parent().unwrap().parent().unwrap().to_path_buf()) }
            fn binary(&self) -> PathBuf { self.0.clone() }
            fn latest_release(&self) -> Result<Release, String> { Err(String::new()) }
            fn install(&self, _: &Release, _: &dyn Fn(String, f32)) -> Result<(), String> { Err(String::new()) }
            fn keeps_missing_custom_path(&self) -> bool { self.1 }
        }
        std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
        std::fs::write(&managed, "").unwrap();

        let mut cfg = Config::default();
        let keeps = InTemp(managed.clone(), true);
        adopt_default_path(&mut cfg, &keeps);
        assert_eq!(keeps.path(&cfg), managed, "nothing set: the managed copy");
        keeps.set_path_setting(&mut cfg, "/gone/minimal".into());
        adopt_default_path(&mut cfg, &keeps);
        assert_eq!(keeps.path_setting(&cfg), "/gone/minimal", "a path you set is kept");

        let replaces = InTemp(managed.clone(), false);
        adopt_default_path(&mut cfg, &replaces);
        assert_eq!(replaces.path(&cfg), managed, "a path that no longer exists is replaced");
    }
}
