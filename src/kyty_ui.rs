//! KytyPS5 updates inside the app: background checks, installs, status and settings actions.

use crate::app::*;
use crate::kyty::{self, Release};
use std::time::Duration;

#[derive(Default)]
pub struct KytyUi {
    pub latest: Option<Release>,
    pub checking: bool,
    pub busy: bool,
    pub progress: String,
    pub error: String,
    /// Installed build as shown to the user.
    pub version: String,
    /// Commit of the current emulator binary (to compare with the latest release).
    pub commit: String,
    /// An update found while a game was running, and whether you asked for it (`true`) or it
    /// came from the automatic checks. Installed when the game closes, if still wanted.
    pub pending: Option<bool>,
    /// Install the latest build even over a self-built one (first start / missing emulator).
    pub force_install: bool,
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

impl App {
    /// Background start-up work: read the installed version, then check if due.
    pub fn kyty_start(&mut self) {
        self.kyty_refresh_version();
        let due = crate::util::now_secs() - kyty::load_state().last_check > kyty::CHECK_INTERVAL;
        let missing = !self.cfg.lock().unwrap().emulator_ok();
        if due || missing {
            self.kyty_check(false);
        }
        // Keep checking while the launcher stays open.
        let t = slint::Timer::default();
        t.start(slint::TimerMode::Repeated, Duration::from_secs(30 * 60), || {
            with_app(|app| {
                if crate::util::now_secs() - kyty::load_state().last_check > kyty::CHECK_INTERVAL {
                    app.kyty_check(false);
                }
            })
        });
        std::mem::forget(t); // lives for the whole session
    }

    pub fn kyty_managed(&self) -> bool {
        kyty::is_managed(&self.cfg.lock().unwrap().emulator_path())
    }

    pub fn kyty_refresh_version(&mut self) {
        let emu = self.cfg.lock().unwrap().emulator_path();
        let managed = kyty::is_managed(&emu);
        std::thread::spawn(move || {
            let (text, commit) = if managed {
                let st = kyty::load_state();
                (format!("Official build {}", kyty::pretty(&st.installed)), kyty::tag_commit(&st.installed).to_string())
            } else {
                match kyty::binary_version(&emu) {
                    Some((git, date)) => (format!("Self-built {git} ({date})"), git),
                    None => ("Not installed".to_string(), String::new()),
                }
            };
            post(move |app| {
                app.kyty.version = text;
                app.kyty.commit = commit;
                app.kyty_refresh_settings();
            });
        });
    }

    pub fn kyty_update_available(&self) -> bool {
        match &self.kyty.latest {
            Some(r) => !self.kyty.commit.is_empty() && kyty::tag_commit(&r.tag) != self.kyty.commit,
            None => false,
        }
    }

    /// Ask GitHub for the latest build. `manual` = the user pressed the button.
    pub fn kyty_check(&mut self, manual: bool) {
        if self.kyty.checking || self.kyty.busy {
            return;
        }
        self.kyty.checking = true;
        self.kyty.error.clear();
        self.kyty_refresh_settings();
        std::thread::spawn(move || {
            let res = kyty::latest_release();
            post(move |app| {
                app.kyty.checking = false;
                match res {
                    Ok(rel) => {
                        kyty::mark_checked(&rel.tag);
                        app.kyty.latest = Some(rel.clone());
                        app.kyty_after_check(rel, manual);
                    }
                    Err(e) => {
                        crate::log!("KytyPS5 update check failed: {e}");
                        if crate::util::is_network_error(&e) {
                            app.net_down = true;
                        }
                        // Like the launcher's: only a check you asked for shows an error.
                        if manual {
                            app.kyty.error = format!("Update check failed: {e}");
                            app.toast("Couldn't check for KytyPS5 updates", &e, 2);
                        }
                        if app.kyty.force_install {
                            app.kyty.force_install = false;
                            let note = if app.cfg.lock().unwrap().emulator_ok() { "KytyPS5 emulator found" } else { "Could not download KytyPS5 · retry from Settings" };
                            app.boot_kyty_done(note);
                        }
                    }
                }
                app.kyty_refresh_settings();
            });
        });
    }

    fn kyty_after_check(&mut self, rel: Release, manual: bool) {
        let (auto, emulator_ok) = {
            let c = self.cfg.lock().unwrap();
            (c.kyty_auto_update, c.emulator_ok())
        };
        let managed = self.kyty_managed();
        if self.kyty.force_install {
            self.kyty.force_install = false;
            if managed && emulator_ok && !self.kyty_update_available() && !self.kyty.commit.is_empty() {
                self.boot_kyty_done(&format!("KytyPS5 {} · up to date", kyty::pretty(&rel.tag)));
            } else {
                self.kyty_install(rel, true); // first run: games can't start without it
            }
            return;
        }
        if managed {
            let skipped = kyty::load_state().skip == rel.tag;
            if self.kyty_update_available() {
                if (auto && !skipped) || manual {
                    self.kyty_install(rel, manual);
                }
            } else if manual {
                self.toast("KytyPS5 is up to date", &format!("You have the latest build, {}.", kyty::pretty(&rel.tag)), 1);
            }
        } else if !emulator_ok {
            // Fresh machine: fetch the emulator so games can be played right away.
            if auto || manual {
                self.kyty_install(rel, manual);
            }
        } else if manual {
            if self.kyty_update_available() {
                self.toast("Newer KytyPS5 build available", "To get it, and future updates automatically, choose Settings → Updates → KytyPS5.", 0);
            } else {
                self.toast("Your KytyPS5 is up to date", &format!("The latest official build is {}.", kyty::pretty(&rel.tag)), 1);
            }
        }
    }

    /// Download and activate `rel` (switching to the managed install if needed). `manual`: you
    /// asked for it (or the emulator is missing), so it doesn't depend on automatic updates.
    pub fn kyty_install(&mut self, rel: Release, manual: bool) {
        if self.kyty.busy {
            return;
        }
        if !self.live.is_empty() {
            self.kyty.pending = Some(manual || self.kyty.pending == Some(true));
            self.toast("KytyPS5 update waiting", "It installs when you finish playing.", 0);
            self.boot_kyty_done("KytyPS5 update will install after your game");
            return;
        }
        self.kyty.busy = true;
        self.kyty.pending = None;
        self.kyty.error.clear();
        self.kyty.progress = "Starting download…".into();
        self.kyty_refresh_settings();
        let old_emu = self.cfg.lock().unwrap().emulator_path();
        let was_managed = kyty::is_managed(&old_emu);
        std::thread::spawn(move || {
            let last = std::sync::Mutex::new(String::new());
            let res = kyty::install(&rel, &|p, frac| {
                let mut l = last.lock().unwrap();
                if *l != p {
                    *l = p.clone();
                    post(move |app| {
                        app.kyty.progress = p.clone();
                        if app.boot.active {
                            app.boot_kyty_progress(&p, frac);
                        } else {
                            app.set_status(&p, true);
                        }
                        app.kyty_refresh_settings();
                    });
                }
            });
            // Bring saves and shader caches over from a self-built install (copied, not moved).
            let imported = if res.is_ok() && !was_managed { old_emu.parent().map(kyty::import_data_from).unwrap_or(0) } else { 0 };
            post(move |app| {
                app.kyty.busy = false;
                app.kyty.progress.clear();
                app.set_status("", false);
                match res {
                    Ok(()) => {
                        {
                            let mut c = app.cfg.lock().unwrap();
                            c.emulator = kyty::managed_emulator().to_string_lossy().into_owned();
                            c.save();
                        }
                        let sub = if imported > 0 { "Your saves and shader caches were copied over.".to_string() } else { String::new() };
                        let title = if was_managed { "KytyPS5 updated" } else { "KytyPS5 installed" };
                        if app.boot.active {
                            app.boot_kyty_done(&format!("KytyPS5 {} ready", kyty::pretty(&rel.tag)));
                        } else {
                            app.toast(title, &format!("Now on build {}.{}", kyty::pretty(&rel.tag), if sub.is_empty() { String::new() } else { format!(" {sub}") }), 1);
                        }
                    }
                    Err(e) => {
                        crate::log!("KytyPS5 install failed: {e}");
                        app.kyty.error = format!("Update failed: {e}");
                        app.toast("KytyPS5 update failed", &e, 2);
                        let note = if app.cfg.lock().unwrap().emulator_ok() { "KytyPS5 emulator found" } else { "Could not download KytyPS5 · retry from Settings" };
                        app.boot_kyty_done(note);
                    }
                }
                app.kyty_refresh_version();
                app.shad_tick(); // shadPS4 waits for KytyPS5's download
            });
        });
    }

    /// Called when all games have closed: install a queued update you asked for, or an automatic
    /// one if automatic updates are still on.
    pub fn kyty_games_closed(&mut self) {
        let Some(manual) = self.kyty.pending.take() else { return };
        if !manual && !self.cfg.lock().unwrap().kyty_auto_update {
            crate::log!("queued KytyPS5 update dropped: automatic updates are off");
            return;
        }
        if let Some(rel) = self.kyty.latest.clone() {
            self.kyty_install(rel, manual);
        }
    }

    pub fn kyty_rollback(&mut self) {
        match kyty::rollback() {
            Ok(tag) => {
                self.toast("KytyPS5 rolled back", &format!("Now on build {}.", kyty::pretty(&tag)), 1);
                self.kyty_refresh_version();
            }
            Err(e) => self.toast("Couldn't roll back", &e, 2),
        }
    }

    /// Switch from a self-built/custom emulator to auto-updated official builds.
    pub fn kyty_switch_to_managed(&mut self) {
        match self.kyty.latest.clone() {
            Some(rel) => self.kyty_install(rel, true),
            None => {
                // Check first; install right after.
                self.kyty.checking = true;
                std::thread::spawn(|| {
                    let res = kyty::latest_release();
                    post(move |app| {
                        app.kyty.checking = false;
                        match res {
                            Ok(rel) => {
                                app.kyty.latest = Some(rel.clone());
                                app.kyty_install(rel, true);
                            }
                            Err(e) => app.toast("Couldn't reach GitHub", &e, 2),
                        }
                    });
                });
            }
        }
    }

    pub fn kyty_refresh_settings(&mut self) {
        if self.overlay == Overlay::Settings {
            self.build_settings();
            self.push_settings();
        }
    }
}
