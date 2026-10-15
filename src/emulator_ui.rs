//! Installs and updates for every emulator in the registry (`crate::emulator`), the same for all:
//!
//! - With automatic updates on, a missing emulator is installed in the background and kept on the
//!   latest official release, one download at a time.
//! - Playing a game whose emulator isn't installed installs it first, then starts the game.
//! - An emulator that asks for it (`fetch_on_first_run`) is fetched on the welcome screen.
//! - Settings gets an update row, a rollback row and (under Advanced) a location row for each.
//! - An update found while a game runs waits until the game closes.
//!
//! A user's own build (a path outside the launcher's folder) is never installed over or updated,
//! except by choosing "Switch to official builds".

use crate::app::*;
use crate::config::Config;
use crate::emulator::{self, Emulator, Release};
use crate::settings::{row, SId};
use crate::util;
use crate::SettingData;
use std::collections::HashMap;
use std::time::Duration;

type Emu = &'static dyn Emulator;

#[derive(Default)]
pub struct EmulatorState {
    pub busy: bool,
    pub checking: bool,
    /// Version text of the user's own build, read off the UI thread.
    pub own_label: String,
    /// Install the latest build even over your own (the welcome screen's first start).
    pub force_install: bool,
    pub progress: String,
    pub error: String,
    /// A game (by local game id) to start once the emulator is installed.
    pub launch_after: Option<String>,
    /// An update found while a game was running, and whether you asked for it. Installed when
    /// the game closes, if still wanted.
    pub pending: Option<(Release, bool)>,
    /// A download failed for lack of network: check again when it's back, not hours later.
    pub retry: bool,
}

#[derive(Default)]
pub struct EmulatorUis {
    states: HashMap<&'static str, EmulatorState>,
}

impl EmulatorUis {
    pub fn get_mut(&mut self, emu: Emu) -> &mut EmulatorState {
        self.states.entry(emu.id()).or_default()
    }

    pub fn get(&self, emu: Emu) -> Option<&EmulatorState> {
        self.states.get(emu.id())
    }

    pub fn any_busy(&self) -> bool {
        self.states.values().any(|s| s.busy)
    }

    /// What the status line shows: the first running download.
    pub fn progress(&self) -> Option<&str> {
        self.states.values().find(|s| s.busy).map(|s| s.progress.as_str())
    }

    fn others_busy(&self, emu: Emu) -> bool {
        self.states.iter().any(|(id, s)| *id != emu.id() && s.busy)
    }
}

/// Settings rows refer to an emulator by its place in the registry.
fn index_of(emu: Emu) -> u8 {
    emulator::all().iter().position(|e| e.id() == emu.id()).unwrap_or(0) as u8
}

fn nth(n: u8) -> Option<Emu> {
    emulator::all().get(n as usize).copied()
}

#[derive(Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    Install,
    UpToDate,
    Nothing,
}

/// What a finished update check means. `wanted`: the launcher manages this emulator and either you
/// asked or automatic updates are on. A build you rolled back from (`skip`) is only installed on
/// request.
pub fn after_check(wanted: bool, installed: &str, skip: &str, latest: &str, manual: bool) -> CheckOutcome {
    if wanted && latest != installed && (manual || latest != skip) {
        CheckOutcome::Install
    } else if manual {
        CheckOutcome::UpToDate
    } else {
        CheckOutcome::Nothing
    }
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

fn each() -> impl Iterator<Item = Emu> {
    emulator::all().iter().copied()
}

impl App {
    pub fn emu_start(&mut self) {
        for emu in each() {
            self.emu_refresh_own_label(emu);
        }
        self.emu_tick();
        // Keep checking while the launcher stays open.
        let t = slint::Timer::default();
        t.start(slint::TimerMode::Repeated, Duration::from_secs(30 * 60), || with_app(|app| app.emu_tick()));
        std::mem::forget(t); // lives for the whole session
    }

    /// With automatic updates on, install each emulator that's missing and update it every few
    /// hours. Waits for the start-up screen (which fetches what it needs itself).
    pub fn emu_tick(&mut self) {
        if self.boot.active {
            return;
        }
        for emu in each() {
            let (auto, own) = {
                let c = self.cfg.lock().unwrap();
                (emu.auto_update(&c), emu.uses_own_build(&c))
            };
            if !auto || own {
                continue;
            }
            let due = util::now_secs() - emu.store().load_state().last_check > emulator::CHECK_INTERVAL;
            if self.emus.get_mut(emu).retry || !emu.installed() || due {
                self.emu_check(emu, false);
            }
        }
    }

    /// The network is back: fetch again what the welcome screen was waiting for, and check the rest.
    pub fn emu_network_back(&mut self) {
        for emu in each().filter(|e| e.fetch_on_first_run()) {
            let (auto, missing) = {
                let c = self.cfg.lock().unwrap();
                (emu.auto_update(&c), !emu.runnable(&c))
            };
            if auto && missing {
                // Back on the welcome screen's list, if it's still up.
                self.boot_emu_wait_again(emu.id());
                self.emus.get_mut(emu).force_install = true;
            }
            self.emu_check(emu, false);
        }
        self.emu_tick();
    }

    /// Ask the emulator's release feed for the latest build. `manual` = you pressed the button.
    pub fn emu_check(&mut self, emu: Emu, manual: bool) {
        let state = self.emus.get_mut(emu);
        if state.checking || state.busy {
            return;
        }
        state.checking = true;
        state.error.clear();
        self.refresh_settings_if_open();
        std::thread::spawn(move || {
            let res = emu.latest_release();
            post(move |app| {
                app.emus.get_mut(emu).checking = false;
                match res {
                    Ok(rel) => app.emu_checked(emu, rel, manual),
                    Err(e) => {
                        crate::log!("{} update check failed: {e}", emu.name());
                        if util::is_network_error(&e) {
                            app.net_down = true;
                        }
                        // Like the launcher's: only a check you asked for shows an error.
                        if manual {
                            app.emus.get_mut(emu).error = format!("Update check failed: {e}");
                            app.toast(&format!("Couldn't check for {} updates", emu.name()), &e, 2);
                        }
                        if std::mem::take(&mut app.emus.get_mut(emu).force_install) {
                            let note = app.boot_failure_note(emu);
                            app.boot_emu_done(emu.id(), &note);
                        }
                    }
                }
                app.refresh_settings_if_open();
            });
        });
    }

    fn emu_checked(&mut self, emu: Emu, rel: Release, manual: bool) {
        self.emus.get_mut(emu).retry = false;
        emu.store().mark_checked(&rel.tag);
        let st = emu.store().load_state();
        let (own, runnable, auto) = {
            let c = self.cfg.lock().unwrap();
            (emu.uses_own_build(&c), emu.runnable(&c), emu.auto_update(&c))
        };
        if std::mem::take(&mut self.emus.get_mut(emu).force_install) {
            if !own && runnable && st.installed == rel.tag {
                self.boot_emu_done(emu.id(), &format!("{} {} · up to date", emu.name(), emu.pretty(&rel.tag)));
            } else {
                self.emu_install(emu, rel, None, false, true); // games can't start without it
            }
            return;
        }
        // Automatic updates may have been switched off, or your own build set, while this check ran.
        let wanted = !own && (manual || auto);
        match after_check(wanted, &st.installed, &st.skip, &rel.tag, manual) {
            CheckOutcome::Install => {
                // A background first install that fails (offline) retries quietly later.
                let quiet = !manual && !emu.installed();
                self.emu_install(emu, rel, None, quiet, manual);
            }
            CheckOutcome::UpToDate => {
                self.toast(&format!("{} is up to date", emu.name()), &format!("You have the latest release, {}.", emu.pretty(&rel.tag)), 1);
            }
            CheckOutcome::Nothing => {}
        }
    }

    /// Switch from your own build to the launcher's copy. Installs it first when it isn't there.
    pub fn emu_switch_to_managed(&mut self, emu: Emu) {
        if self.emus.get_mut(emu).busy {
            return;
        }
        if emu.installed() {
            let (previous, was_managed) = {
                let c = self.cfg.lock().unwrap();
                let p = emu.path(&c);
                let managed = emu.is_managed(&p);
                (p, managed)
            };
            std::thread::spawn(move || {
                let note = emu.after_install(&previous, was_managed);
                post(move |app| {
                    {
                        let mut c = app.cfg.lock().unwrap();
                        emu.set_path_setting(&mut c, emu.binary().to_string_lossy().into_owned());
                        c.save();
                    }
                    let tag = emu.store().load_state().installed;
                    app.toast("Switched to official builds", &format!("{} games run on {} {}.{}", emulator::platform_labels(emu), emu.name(), emu.pretty(&tag), note.map(|n| format!(" {n}")).unwrap_or_default()), 1);
                    app.emus.get_mut(emu).own_label.clear();
                    app.refresh_settings_if_open();
                });
            });
            return;
        }
        self.emus.get_mut(emu).busy = true;
        std::thread::spawn(move || {
            let res = emu.latest_release();
            post(move |app| {
                app.emus.get_mut(emu).busy = false;
                match res {
                    Ok(rel) => app.emu_install(emu, rel, None, false, true),
                    Err(e) => app.toast(&format!("Couldn't get {}", emu.name()), &format!("{e}. Check your connection and try again."), 2),
                }
            });
        });
    }

    /// A game needs an emulator that isn't installed: download it now, then start the game.
    pub fn emu_install_then_launch(&mut self, emu: Emu, game_id: String) {
        self.toast(&format!("Getting {} for {} games", emu.name(), emulator::platform_labels(emu)), "It downloads once, then your game starts.", 0);
        self.emus.get_mut(emu).launch_after = Some(game_id);
        if self.emus.get_mut(emu).busy {
            return;
        }
        self.emus.get_mut(emu).busy = true;
        std::thread::spawn(move || {
            let res = emu.latest_release();
            post(move |app| {
                app.emus.get_mut(emu).busy = false;
                match res {
                    Ok(rel) => {
                        let after = app.emus.get_mut(emu).launch_after.clone();
                        app.emu_install(emu, rel, after, false, true);
                    }
                    Err(e) => {
                        app.emus.get_mut(emu).launch_after = None;
                        if util::is_network_error(&e) {
                            app.net_down = true;
                        }
                        app.toast(&format!("Couldn't get {}", emu.name()), &format!("{e}. Check your connection and try again."), 2);
                    }
                }
            });
        });
    }

    /// Download and activate `rel`. `manual`: you asked for it (Settings, or playing a game), so
    /// it doesn't depend on automatic updates.
    pub fn emu_install(&mut self, emu: Emu, rel: Release, launch_after: Option<String>, quiet: bool, manual: bool) {
        let game_running = !self.live.is_empty();
        let others_busy = self.emus.others_busy(emu);
        let state = self.emus.get_mut(emu);
        if state.busy {
            return;
        }
        if game_running && launch_after.is_none() {
            let asked = manual || state.pending.as_ref().is_some_and(|(_, m)| *m);
            state.pending = Some((rel, asked));
            self.toast(&format!("{} update waiting", emu.name()), "It installs when you finish playing.", 0);
            self.boot_emu_done(emu.id(), &format!("{} update will install after your game", emu.name()));
            return;
        }
        if others_busy && !manual && launch_after.is_none() {
            // One download at a time: try again when the other one is done.
            state.retry = true;
            return;
        }
        state.busy = true;
        state.error.clear();
        state.progress = "Starting download…".into();
        self.refresh_settings_if_open();
        let updating = emu.installed();
        let (previous, was_managed) = {
            let c = self.cfg.lock().unwrap();
            let p = emu.path(&c);
            let managed = emu.is_managed(&p);
            (p, managed)
        };
        std::thread::spawn(move || {
            let last = std::sync::Mutex::new(String::new());
            let res = emu.install(&rel, &|p, frac| {
                let mut l = last.lock().unwrap();
                if *l != p {
                    *l = p.clone();
                    post(move |app| {
                        app.emus.get_mut(emu).progress = p.clone();
                        if app.boot.active {
                            app.boot_emu_progress(emu.id(), &p, frac);
                        } else {
                            app.set_status(&p, true);
                        }
                        app.refresh_settings_if_open();
                    });
                }
            });
            let note = if res.is_ok() { emu.after_install(&previous, was_managed) } else { None };
            post(move |app| {
                {
                    let s = app.emus.get_mut(emu);
                    s.busy = false;
                    s.progress.clear();
                }
                app.set_status("", false);
                match res {
                    Ok(()) => {
                        {
                            let mut c = app.cfg.lock().unwrap();
                            emu.set_path_setting(&mut c, emu.binary().to_string_lossy().into_owned());
                            c.save();
                        }
                        app.emus.get_mut(emu).own_label.clear();
                        if app.boot.active {
                            app.boot_emu_done(emu.id(), &format!("{} {} ready", emu.name(), emu.pretty(&rel.tag)));
                        } else {
                            let what = if updating { format!("{} updated", emu.name()) } else { format!("{} installed", emu.name()) };
                            app.toast(&what, &format!("{} games run on {} {}.{}", emulator::platform_labels(emu), emu.name(), emu.pretty(&rel.tag), note.map(|n| format!(" {n}")).unwrap_or_default()), 1);
                        }
                        if let Some(id) = launch_after.or(app.emus.get_mut(emu).launch_after.take()) {
                            app.emus.get_mut(emu).launch_after = None;
                            if let Some(l) = app.locals.iter().position(|g| g.l.id == id) {
                                app.launch(l);
                            }
                        }
                    }
                    Err(e) => {
                        crate::log!("{} install failed: {e}", emu.name());
                        let s = app.emus.get_mut(emu);
                        s.error = e.clone();
                        s.launch_after = None;
                        if util::is_network_error(&e) {
                            s.retry = true;
                            app.net_down = true;
                        }
                        if !quiet {
                            app.toast(&format!("Couldn't install {}", emu.name()), &e, 2);
                        }
                        let note = app.boot_failure_note(emu);
                        app.boot_emu_done(emu.id(), &note);
                    }
                }
                app.refresh_settings_if_open();
                app.emu_tick(); // the next emulator waiting for its turn
            });
        });
    }

    /// What the welcome screen says when `emu` could not be fetched.
    fn boot_failure_note(&self, emu: Emu) -> String {
        if emu.runnable(&self.cfg.lock().unwrap()) {
            format!("{} emulator found", emu.name())
        } else {
            format!("Could not download {} · retry from Settings", emu.name())
        }
    }

    /// Called when all games have closed: install queued updates you asked for, or automatic
    /// ones if automatic updates are still on.
    pub fn emu_games_closed(&mut self) {
        for emu in each() {
            let Some((rel, manual)) = self.emus.get_mut(emu).pending.take() else { continue };
            let (own, auto) = {
                let c = self.cfg.lock().unwrap();
                (emu.uses_own_build(&c), emu.auto_update(&c))
            };
            if own {
                crate::log!("queued {} update dropped: you use your own build", emu.name());
            } else if !manual && !auto {
                crate::log!("queued {} update dropped: automatic updates are off", emu.name());
            } else {
                self.emu_install(emu, rel, None, false, manual);
            }
        }
    }

    pub fn emu_rollback(&mut self, emu: Emu) {
        match emu.rollback() {
            Ok(tag) => self.toast(&format!("{} rolled back", emu.name()), &format!("{} games now run on {}.", emulator::platform_labels(emu), emu.pretty(&tag)), 1),
            Err(e) => self.toast(&format!("Can't roll back {}", emu.name()), &e, 2),
        }
        self.refresh_settings();
    }

    /// Read the version of the user's own build (it may have to run, so not on the UI thread).
    pub fn emu_refresh_own_label(&mut self, emu: Emu) {
        let path = {
            let c = self.cfg.lock().unwrap();
            emu.uses_own_build(&c).then(|| emu.path(&c))
        };
        let Some(path) = path else {
            self.emus.get_mut(emu).own_label.clear();
            return;
        };
        std::thread::spawn(move || {
            let label = emu.describe_own_build(&path).unwrap_or_default();
            post(move |app| {
                app.emus.get_mut(emu).own_label = label;
                app.refresh_settings_if_open();
            });
        });
    }

    // ------------------------------------------------------------------ Settings

    /// One row per emulator under UPDATES: install, switch to official builds, or the version.
    pub fn emu_update_rows(&self, cfg: &Config) -> Vec<(SId, SettingData)> {
        each().map(|emu| {
            let state = emu.store().load_state();
            let installed = emu.installed();
            let own = emu.uses_own_build(cfg);
            let ui = self.emus.get(emu);
            let games = emulator::platform_labels(emu);
            let mut r = row(4, emu.name());
            r.hint = if own {
                let label = ui.map(|u| u.own_label.as_str()).filter(|l| !l.is_empty()).map(|l| format!(" ({l})")).unwrap_or_default();
                format!("Runs {games} games · you use your own build{label}{}", emu.switch_hint())
            } else if installed {
                format!("Runs {games} games")
            } else if emu.auto_update(cfg) {
                format!("Runs {games} games · installs by itself in the background")
            } else {
                format!("Runs {games} games · installs when you first play one")
            }
            .into();
            let (value, kind) = if ui.is_some_and(|u| u.busy) {
                (ui.map(|u| u.progress.replace(&format!("Downloading {} ", emu.name()), "")).unwrap_or_default(), 0)
            } else if ui.is_some_and(|u| u.checking) {
                ("Checking…".to_string(), 0)
            } else if own {
                ("Switch to official builds".to_string(), 3)
            } else if !installed {
                ("Install".to_string(), 3)
            } else if !state.latest.is_empty() && state.latest != state.installed {
                (format!("Update to {}", emu.pretty(&state.latest)), 3)
            } else if state.latest == state.installed {
                (format!("{} · Up to date", emu.pretty(&state.installed)), 1)
            } else {
                (emu.pretty(&state.installed), 0)
            };
            (r.value, r.value_kind) = (value.into(), kind);
            if let Some(u) = ui.filter(|u| !u.error.is_empty()) {
                (r.hint, r.hint_kind) = (u.error.clone().into(), 2);
            }
            (SId::EmuUpdate(index_of(emu)), r)
        }).collect()
    }

    /// Rollback rows, only for emulators that have a previous version to go back to. `first`:
    /// no rollback row came before, so this one explains what it is for.
    pub fn emu_rollback_rows(&self, cfg: &Config, mut first: bool) -> Vec<(SId, SettingData)> {
        each().filter(|emu| !emu.uses_own_build(cfg) && emu.store().has_previous()).map(|emu| {
            let mut r = row(4, &format!("Go back to the previous {}", emu.name()));
            r.value = emu.pretty(&emu.store().load_state().previous).into();
            if std::mem::take(&mut first) {
                r.hint = "For when an update breaks a game".into();
            }
            (SId::EmuRollback(index_of(emu)), r)
        }).collect()
    }

    /// Location rows (Advanced): where each emulator is, editable.
    pub fn emu_path_rows(&self, cfg: &Config) -> Vec<(SId, SettingData)> {
        each().map(|emu| {
            let mut r = row(1, &format!("{} location", emu.name()));
            r.value = util::display_path(emu.path_setting(cfg)).into();
            (r.hint, r.hint_kind) = if emu.runnable(cfg) { ("Found".into(), 1) } else { ("Nothing runnable at this path".into(), 2) };
            (SId::EmuPath(index_of(emu)), r)
        }).collect()
    }

    /// The text to edit for a location row.
    pub fn emu_path_text(&self, n: u8) -> String {
        nth(n).map(|emu| emu.path_setting(&self.cfg.lock().unwrap()).to_string()).unwrap_or_default()
    }

    /// The update row was chosen: switch to official builds, or install / update / check.
    pub fn emu_update_pressed(&mut self, n: u8) {
        let Some(emu) = nth(n) else { return };
        let state = self.emus.get_mut(emu);
        if state.busy || state.checking {
            return;
        }
        if emu.uses_own_build(&self.cfg.lock().unwrap()) {
            self.emu_switch_to_managed(emu);
            return;
        }
        // Installs when the emulator isn't there yet, otherwise updates it if there's a newer release.
        self.emu_check(emu, true);
    }

    pub fn emu_rollback_pressed(&mut self, n: u8) {
        if let Some(emu) = nth(n) {
            self.emu_rollback(emu);
        }
    }

    /// A new location was typed. An empty one goes back to the launcher's copy.
    pub fn emu_path_committed(&mut self, n: u8, text: String) {
        let Some(emu) = nth(n) else { return };
        let ok = {
            let mut c = self.cfg.lock().unwrap();
            let empty = text.trim().is_empty();
            emu.set_path_setting(&mut c, text);
            if empty {
                emulator::adopt_default_path(&mut c, emu);
            }
            c.save();
            emu.runnable(&c)
        };
        self.emu_refresh_own_label(emu);
        if ok {
            self.toast(&format!("{} path saved", emu.name()), &format!("{} games will start with this {}.", emulator::platform_labels(emu), emu.name()), 1);
        } else {
            self.toast(&format!("{} not found", emu.name()), &format!("There's no runnable {} at that path.", emu.name()), 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_installs_a_new_release_unless_you_rolled_back_from_it() {
        use CheckOutcome::*;
        assert_eq!(after_check(true, "v1", "", "v2", false), Install);
        assert_eq!(after_check(true, "v1", "v2", "v2", false), Nothing, "rolled back from v2: not automatically");
        assert_eq!(after_check(true, "v1", "v2", "v2", true), Install, "but you can ask for it");
        assert_eq!(after_check(true, "v2", "", "v2", false), Nothing);
        assert_eq!(after_check(true, "v2", "", "v2", true), UpToDate, "only a check you asked for says so");
        assert_eq!(after_check(false, "v1", "", "v2", false), Nothing, "automatic updates off, or your own build");
    }

    #[test]
    fn rows_refer_to_emulators_by_registry_position() {
        for emu in each() {
            assert_eq!(nth(index_of(emu)).map(|e| e.id()), Some(emu.id()));
        }
        assert!(nth(200).is_none());
    }
}
