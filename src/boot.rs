//! Welcome / boot screen.
//!
//! First run: loads the local catalog and downloads official artwork and the latest KytyPS5 build in
//! parallel (one combined progress bar), then preloads the Home screen and the first Library
//! page, so nothing is missing or still loading when the user gets in.
//! Later runs: a short splash while the Home screen decodes from cache. If the emulator has
//! gone missing, it is fetched here too.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::images::prio;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub const OVERLAY_BOOT: i32 = 6;

#[derive(Default)]
pub struct Boot {
    pub active: bool,
    pub first: bool,
    pub ready: bool,
    pub waiting_catalog: bool,
    pub waiting_art: bool,
    pub waiting_covers: bool,
    pub waiting_kyty: bool,
    pub kyty_needed: bool,
    pub collected: bool,
    pub keys: HashSet<String>,
    pub total: usize,
    pub started: Option<Instant>,
    // Progress of each task, 0..1
    pub f_catalog: f32,
    pub f_art: f32,
    pub f_covers: f32,
    pub f_kyty: f32,
    pub f_img: f32,
    pub text_main: String,
    pub cat_text: String,
    pub art_text: String,
    pub covers_text: String,
    pub cat_failed: bool,
    /// Covers requested for the welcome screen's backdrop, and the ones decoded so far.
    pub cover_keys: Vec<String>,
    pub cover_imgs: Vec<slint::Image>,
    pub cover_shown: Vec<String>,
    pub text_kyty: String,
    pub kyty_note: String,
}

fn parse_fraction(s: &str) -> Option<f32> {
    let (a, b) = s.rsplit_once(' ')?.1.split_once('/')?;
    let (a, b): (f32, f32) = (a.parse().ok()?, b.parse().ok()?);
    (b > 0.0).then(|| (a / b).clamp(0.0, 1.0))
}

impl App {
    pub fn boot_start(&mut self, first: bool) {
        let cfg = self.cfg.lock().unwrap().clone();
        let managed = crate::kyty::is_managed(&cfg.emulator_path());
        // First start: always get the latest official KytyPS5 (unless the user turned updates off).
        // Any start: fetch it if the emulator is missing.
        let kyty_needed = cfg.kyty_auto_update && ((first && !(managed && cfg.emulator_ok())) || !cfg.emulator_ok());
        // The start screen waits for the game list and the Home screen's own pictures, nothing
        // more: every game's pictures and details, and the emulators, keep coming in the
        // background (the status line shows them). Waiting for them took minutes on a first
        // start, and on the AzrakOS USB stick every start is a first start.
        self.boot = Boot {
            active: true,
            first,
            waiting_catalog: self.games.is_empty(),
            started: Some(Instant::now()),
            f_catalog: if self.games.is_empty() { 0.0 } else { 1.0 },
            ..Default::default()
        };
        let ui = self.ui();
        ui.set_overlay(OVERLAY_BOOT);
        ui.set_boot_first(first);
        // On the USB stick this comes right after "Welcome to AzrakOS" (Try): not a second welcome.
        ui.set_boot_stick(crate::os::system::on_usb());
        ui.set_boot_ready(false);
        ui.set_boot_progress(0.0);
        self.boot.kyty_note = if cfg.emulator_ok() && !kyty_needed {
            "KytyPS5 emulator found".into()
        } else if kyty_needed {
            "Getting the latest KytyPS5 emulator".into()
        } else {
            "KytyPS5 emulator not found · you can set it in Settings".into()
        };
        self.boot_render();
        if kyty_needed {
            self.kyty.force_install = true;
            self.kyty_check(false);
        }
        self.boot_maybe_ready();
        if !first {
            // Never keep a returning user waiting on a slow network (unless the emulator is missing).
            slint::Timer::single_shot(Duration::from_secs(5), || {
                with_app(|app| {
                    if !app.boot.waiting_kyty {
                        app.boot_ready();
                    }
                })
            });
        }
    }

    fn boot_progress(&self) -> f32 {
        let b = &self.boot;
        let mut parts: Vec<(f32, f32)> = Vec::new();
        if b.first {
            parts.push((0.6, b.f_catalog));
        }
        parts.push((if parts.is_empty() { 1.0 } else { 0.4 }, b.f_img));
        let total: f32 = parts.iter().map(|p| p.0).sum();
        parts.iter().map(|(w, f)| w * f).sum::<f32>() / total.max(0.001)
    }

    pub fn boot_render(&mut self) {
        if !self.boot.active {
            return;
        }
        self.boot_feed_covers();
        let b = &self.boot;
        let status = if b.ready {
            if b.first { "All set".to_string() } else { "Ready".to_string() }
        } else if b.first {
            String::new()
        } else {
            "Loading…".to_string()
        };
        let n = self.locals.len();
        let installed = format!("{n} installed game{} found", if n == 1 { "" } else { "s" });
        let detail = if b.ready && n == 0 {
            "All set · pick a game in the Library to play".to_string()
        } else if b.ready {
            format!("All set · {installed}")
        } else {
            "This only happens once. Skip anytime — downloads continue in the background.".to_string()
        };
        // The step list, in players' words: Settings → Updates has the emulators' names and builds.
        let step = |label: &str, detail: String, state: i32| crate::BootStep { label: label.into(), detail: detail.into(), state };
        let mut steps = Vec::new();
        steps.push(if b.cat_failed {
            step("Game list", "Using the saved list".into(), 3)
        } else if b.waiting_catalog {
            step("Game list", "Loading…".into(), 1)
        } else {
            step("Game list", format!("{} games", self.library_total), 2)
        });
        steps.push(if b.waiting_catalog {
            step("Your Home screen", String::new(), 0)
        } else if !b.ready {
            step("Your Home screen", format!("{}%", (b.f_img * 100.0).round()), 1)
        } else {
            step("Your Home screen", "Ready".into(), 2)
        });
        let progress = self.boot_progress();
        let ui = self.ui();
        ui.set_boot_steps(model(steps));
        ui.set_boot_status(status.into());
        ui.set_boot_detail(detail.into());
        ui.set_boot_progress(progress.max(ui.get_boot_progress()));
    }

    /// Feed the backdrop's cover wall with covers that are already on disk (up to 40).
    fn boot_feed_covers(&mut self) {
        if self.boot.cover_keys.len() >= 40 {
            return;
        }
        let mut order: Vec<usize> = (0..self.games.len()).collect();
        order.sort_by(|a, b| self.games[*b].g.date.cmp(&self.games[*a].g.date));
        for gi in order.into_iter().take(80) {
            if self.boot.cover_keys.len() >= 40 {
                break;
            }
            let Some(r) = self.card_req(gi) else { continue };
            if self.boot.cover_keys.contains(&r.key) || !self.images.on_disk(&r.key, &r.src) {
                continue;
            }
            self.boot.cover_keys.push(r.key.clone());
            if let Some(img) = self.images.want(&r.key, r.src, r.w, r.crop, crate::images::prio::TILE) {
                self.boot.cover_shown.push(r.key.clone());
                self.boot.cover_imgs.push(img);
                self.ui().set_boot_covers(model(self.boot.cover_imgs.clone()));
            }
        }
    }

    pub fn boot_cover_loaded(&mut self, key: &str) {
        if !self.boot.active || !self.boot.cover_keys.iter().any(|k| k == key) || self.boot.cover_shown.iter().any(|k| k == key) {
            return;
        }
        self.boot.cover_shown.push(key.to_string());
        if let Some(img) = self.images.get(key) {
            self.boot.cover_imgs.push(img);
            self.ui().set_boot_covers(model(self.boot.cover_imgs.clone()));
        }
    }

    /// Status text from catalog sync / artwork downloads.
    pub fn boot_status(&mut self, text: &str) {
        if !self.boot.active || self.boot.ready {
            return;
        }
        let f = parse_fraction(text);
        if text.contains("catalog") {
            self.boot.f_catalog = 0.05 + f.unwrap_or(0.0) * 0.95;
            self.boot.cat_text = text.to_string();
        } else if text.contains("artwork") {
            self.boot.f_art = f.unwrap_or(self.boot.f_art);
            self.boot.art_text = text.to_string();
        } else {
            return;
        }
        self.boot.text_main = text.to_string();
        self.boot_render();
    }

    pub fn boot_catalog_done(&mut self) {
        if self.boot.active {
            self.boot.waiting_catalog = false;
            self.boot.f_catalog = 1.0;
            self.boot_maybe_ready();
        }
    }

    pub fn boot_art_done(&mut self) {
        if self.boot.active && self.boot.waiting_art {
            self.boot.waiting_art = false;
            self.boot.f_art = 1.0;
            // Next: every Library cover, so browsing never shows an empty card.
            self.boot.waiting_covers = true;
            self.warm_covers();
            // Artwork changed the image choices: collect again with the final art.
            self.boot.collected = false;
            self.boot_maybe_ready();
        }
    }

    pub fn boot_covers(&mut self, done: usize, total: usize) {
        if self.boot.active && self.boot.waiting_covers {
            self.boot.f_covers = done as f32 / total.max(1) as f32;
            self.boot.text_main = format!("Downloading artwork {done}/{total}");
            self.boot.covers_text = format!("Covers {done}/{total}");
            self.boot_render();
        }
    }

    pub fn boot_covers_done(&mut self) {
        if self.boot.active && self.boot.waiting_covers {
            self.boot.waiting_covers = false;
            self.boot.f_covers = 1.0;
            self.boot.text_main.clear();
            self.boot_maybe_ready();
        }
    }

    pub fn boot_kyty_progress(&mut self, text: &str, frac: f32) {
        if self.boot.active && self.boot.waiting_kyty {
            self.boot.f_kyty = frac;
            self.boot.text_kyty = text.to_string();
            self.boot_render();
        }
    }

    pub fn boot_kyty_done(&mut self, note: &str) {
        if self.boot.active && self.boot.waiting_kyty {
            self.boot.waiting_kyty = false;
            self.boot.f_kyty = 1.0;
            self.boot.kyty_note = note.to_string();
            self.boot_maybe_ready();
        }
    }

    /// Ready when every download is done and the first screen's images are decoded.
    pub fn boot_maybe_ready(&mut self) {
        if !self.boot.active || self.boot.ready {
            return;
        }
        if self.boot.waiting_catalog || self.boot.waiting_art || self.boot.waiting_covers {
            self.boot_render();
            return;
        }
        if !self.boot.collected {
            self.boot_collect();
        }
        if self.boot.keys.is_empty() && !self.boot.waiting_kyty {
            self.boot_ready();
        } else {
            self.boot_render();
        }
    }

    /// Request everything the first screens show: Home tiles, hero art and logo, neighbours,
    /// and the first page of the Library.
    fn boot_collect(&mut self) {
        self.boot.collected = true;
        let mut reqs = Vec::new();
        for (i, it) in self.row.clone().into_iter().enumerate() {
            if i < 14 {
                reqs.extend(self.tile_req(it));
            }
        }
        for i in 0..3.min(self.row.len()) {
            reqs.extend(self.hero_bg_url(i));
            if let Some(it) = self.row.get(i).copied() {
                if it != RowItem::All {
                    let info = self.target_info(self.row_target(i));
                    reqs.extend(info.as_ref().and_then(|x| crate::present::req_url(&x.logo, 960, 0.0)));
                }
            }
        }
        let first_page = (self.cols * 3).min(self.filtered.len());
        for k in 0..first_page {
            reqs.extend(self.card_req(self.filtered[k]));
        }
        let mut keys = HashSet::new();
        for r in reqs {
            if self.images.want(&r.key, r.src.clone(), r.w, r.crop, prio::HERO).is_none() && self.images.is_pending(&r.key) {
                keys.insert(r.key);
            }
        }
        self.boot.total = keys.len();
        self.boot.keys = keys;
        self.boot.f_img = if self.boot.total == 0 { 1.0 } else { 0.0 };
    }

    pub fn boot_image_loaded(&mut self, key: &str) {
        if !self.boot.active || !self.boot.keys.remove(key) {
            return;
        }
        self.boot.f_img = 1.0 - self.boot.keys.len() as f32 / self.boot.total.max(1) as f32;
        self.boot_maybe_ready();
    }

    pub fn boot_ready(&mut self) {
        if !self.boot.active || self.boot.ready {
            return;
        }
        self.boot.ready = true;
        self.boot.f_img = 1.0;
        crate::log!("welcome screen ready");
        self.push_all();
        self.boot_render();
        let ui = self.ui();
        ui.set_boot_progress(1.0);
        if self.boot.first {
            ui.set_boot_ready(true);
            audio::play(Sound::Select);
        } else {
            // Keep the splash up for a short, smooth beat even when everything is cached.
            let min = Duration::from_millis(650);
            let wait = min.saturating_sub(self.boot.started.map(|s| s.elapsed()).unwrap_or(min));
            slint::Timer::single_shot(wait, || with_app(|app| app.boot_finish()));
        }
    }

    /// Confirm on the welcome screen (also allows skipping a slow first download).
    pub fn boot_confirm(&mut self) {
        if !self.boot.active {
            return;
        }
        if !self.boot.ready {
            if !self.boot.first {
                return;
            }
            // Skip waiting: downloads keep going in the background (shown in the status line).
            self.boot.ready = true;
            self.push_all();
        }
        audio::play(Sound::Start);
        self.boot_finish();
    }

    pub fn boot_finish(&mut self) {
        if !self.boot.active {
            return;
        }
        self.boot.active = false;
        self.overlay = Overlay::None;
        crate::log!("home screen shown");
        let ui = self.ui();
        ui.set_overlay(0);
        self.set_focus(self.home_zone(), 0);
        self.show_row_background(self.sel, true);
        self.prefetch_neighbors();
        ui.invoke_focus_root();
        // The Library's covers, in the background (first start too: the start screen no longer
        // waits for them).
        self.warm_covers();
        // Background work that was hidden behind the splash now shows in the status line.
        if self.kyty.busy {
            let p = self.kyty.progress.clone();
            self.set_status(&p, true);
        }
        self.shad_tick();
        // AzrakOS was installed from the USB stick with "Later" before the launcher started.
        if self.osi.tell_installed {
            self.osi_tell_installed();
        }
    }
}
