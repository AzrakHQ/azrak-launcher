//! shadPS4 (PS4 games) inside the app: installed in the background like KytyPS5, so the first
//! PS4 game starts at once, then kept up to date, with rollback.

use crate::app::*;
use crate::shad::{self, Release};
use std::time::Duration;

#[derive(Default)]
pub struct ShadUi {
    pub busy: bool,
    pub progress: String,
    pub error: String,
    /// A PS4 game (by local game id) to start once shadPS4 is installed.
    pub launch_after: Option<String>,
    /// An update found while a game was running, and whether you asked for it. Installed when
    /// the game closes, if still wanted.
    pub pending: Option<(Release, bool)>,
    /// A download failed for lack of network: check again when it's back, not hours later.
    pub retry: bool,
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

impl App {
    pub fn shad_start(&mut self) {
        self.shad_tick();
        let t = slint::Timer::default();
        t.start(slint::TimerMode::Repeated, Duration::from_secs(30 * 60), || with_app(|app| app.shad_tick()));
        std::mem::forget(t); // lives for the whole session
    }

    /// With automatic updates on, install shadPS4 when it's missing and update it every few
    /// hours. Waits for the start-up screen and for KytyPS5's own download to finish.
    pub fn shad_tick(&mut self) {
        let auto = self.cfg.lock().unwrap().shad_auto_update;
        if !auto || self.boot.active || self.kyty.busy || self.shad.busy {
            return;
        }
        if self.shad.retry || !shad::installed() || crate::util::now_secs() - shad::load_state().last_check > shad::CHECK_INTERVAL {
            self.shad_check(false);
        }
    }

    /// Look for a newer build; install it when there is one (unless rolled back from it).
    pub fn shad_check(&mut self, manual: bool) {
        if self.shad.busy {
            return;
        }
        std::thread::spawn(move || {
            let res = shad::latest_release();
            post(move |app| match res {
                Ok(rel) => {
                    app.shad.retry = false;
                    shad::mark_checked(&rel.tag);
                    let st = shad::load_state();
                    // Automatic updates may have been switched off while this check ran.
                    let wanted = manual || app.cfg.lock().unwrap().shad_auto_update;
                    if wanted && rel.tag != st.installed && (manual || rel.tag != st.skip) {
                        // A background first install that fails (offline) retries quietly later.
                        let quiet = !manual && !shad::installed();
                        app.shad_install(rel, None, quiet, manual);
                    } else if manual {
                        app.toast("shadPS4 is up to date", &format!("You have the latest release, {}.", shad::pretty(&rel.tag)), 1);
                    }
                    app.refresh_settings();
                }
                Err(e) => {
                    crate::log!("shadPS4 update check failed: {e}");
                    if crate::util::is_network_error(&e) {
                        app.net_down = true;
                    }
                    if manual {
                        app.toast("Couldn't check for shadPS4 updates", &e, 2);
                    }
                }
            });
        });
    }

    /// PS4 games need shadPS4: download it now, then start the game.
    pub fn shad_install_then_launch(&mut self, game_id: String) {
        self.toast("Getting PS4 support", "It downloads once (about 35 MB), then your game starts.", 0);
        self.shad.launch_after = Some(game_id);
        if self.shad.busy {
            return;
        }
        self.shad.busy = true;
        std::thread::spawn(|| {
            let res = shad::latest_release();
            post(move |app| {
                app.shad.busy = false;
                match res {
                    Ok(rel) => {
                        let after = app.shad.launch_after.clone();
                        app.shad_install(rel, after, false, true);
                    }
                    Err(e) => {
                        app.shad.launch_after = None;
                        if crate::util::is_network_error(&e) {
                            app.net_down = true;
                        }
                        app.toast("Couldn't get shadPS4", &format!("{e}. Check your connection and try again."), 2);
                    }
                }
            });
        });
    }

    /// `manual`: you asked for it (Settings, or playing a PS4 game), so it doesn't depend on
    /// automatic updates.
    pub fn shad_install(&mut self, rel: Release, launch_after: Option<String>, quiet: bool, manual: bool) {
        if self.shad.busy {
            return;
        }
        if !self.live.is_empty() && launch_after.is_none() {
            let asked = manual || self.shad.pending.as_ref().is_some_and(|(_, m)| *m);
            self.shad.pending = Some((rel, asked));
            return;
        }
        self.shad.busy = true;
        self.shad.error.clear();
        self.shad.progress = "Starting download…".into();
        self.refresh_settings();
        let updating = shad::installed();
        std::thread::spawn(move || {
            let last = std::sync::Mutex::new(String::new());
            let res = shad::install(&rel, &|p, _| {
                let mut l = last.lock().unwrap();
                if *l != p {
                    *l = p.clone();
                    post(move |app| {
                        app.shad.progress = p.clone();
                        app.set_status(&p, true);
                    });
                }
            });
            post(move |app| {
                app.shad.busy = false;
                app.shad.progress.clear();
                app.set_status("", false);
                match res {
                    Ok(()) => {
                        // What it means for the player; the version is in Settings → Updates.
                        if updating {
                            app.toast("PS4 support updated", "Version details are in Settings → Updates.", 1);
                        } else {
                            app.toast("PS4 games are ready to play", "", 1);
                        }
                        if let Some(id) = launch_after.or(app.shad.launch_after.take()) {
                            app.shad.launch_after = None;
                            if let Some(l) = app.locals.iter().position(|g| g.l.id == id) {
                                app.launch(l);
                            }
                        }
                    }
                    Err(e) => {
                        crate::log!("shadPS4 install failed: {e}");
                        if crate::util::is_network_error(&e) {
                            app.net_down = true;
                            app.shad.retry = true;
                        }
                        app.shad.error = e.clone();
                        app.shad.launch_after = None;
                        if !quiet {
                            app.toast("Couldn't install shadPS4", &e, 2);
                        }
                    }
                }
                app.refresh_settings();
            });
        });
    }

    /// Called when all games have closed: install a queued update you asked for, or an automatic
    /// one if automatic updates are still on.
    pub fn shad_games_closed(&mut self) {
        let Some((rel, manual)) = self.shad.pending.take() else { return };
        if !manual && !self.cfg.lock().unwrap().shad_auto_update {
            crate::log!("queued shadPS4 update dropped: automatic updates are off");
            return;
        }
        self.shad_install(rel, None, false, manual);
    }

    pub fn shad_rollback(&mut self) {
        match shad::rollback() {
            Ok(tag) => self.toast("shadPS4 rolled back", &format!("PS4 games now run on {}.", shad::pretty(&tag)), 1),
            Err(e) => self.toast("Can't roll back shadPS4", &e, 2),
        }
        self.refresh_settings();
    }
}
