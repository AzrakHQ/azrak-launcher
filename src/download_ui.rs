//! The Downloads page: every download from looking it up to Play, newest first, over the selected
//! one's key art. Downloads start from the Download & install panel (setup_ui.rs) and install by
//! themselves when they finish; installing by hand stays for downloads from before that.
use crate::app::*;
use crate::audio::{self, Sound};
use crate::downloads::{Job, State};
use crate::installer::{Record, State as Install};
use crate::present::req_url;
use crate::setup_ui::size_label;
use crate::{ActionData, DlStat, DlTab};
use slint::{Model, ModelRc, VecModel};

/// Tile width and gap on the shelf, and the extra gap before each group (design units).
const TILE: f32 = 288.0;
const GAP: f32 = 20.0;
const GROUP_GAP: f32 = 40.0;
const GROUPS: [&str; 3] = ["In progress", "Needs you", "Ready to play"];

/// How one download looks on the page.
struct Desc {
    group: i32,
    kind: i32,
    status: String,
    big: String,
    unit: String,
    stats: Vec<(String, String)>,
    spark: bool,
    note: String,
    dl: f32,
    inst: f32,
    ready: bool,
    dl_label: String,
    inst_label: String,
    bar: i32,
    /// (id, label, icon, primary, danger)
    actions: Vec<(&'static str, &'static str, &'static str)>,
    line: String,
    tile: f32,
}

fn mbps(v: f64) -> String { format!("{v:.1} MB/s") }

fn pct(f: f32) -> String { format!("{:.0}", (f * 100.0).clamp(0.0, 100.0).floor()) }

/// "Added 4 minutes ago", "Added yesterday", "Added Oct 30".
fn added_label(added: u64) -> String {
    if added == 0 { return String::new(); }
    let ago = (crate::util::now_secs() as i64 - added as i64).max(0);
    match ago {
        0..60 => "Added just now".into(),
        60..3600 => { let m = ago / 60; format!("Added {m} minute{} ago", if m == 1 { "" } else { "s" }) }
        3600..86400 => { let h = ago / 3600; format!("Added {h} hour{} ago", if h == 1 { "" } else { "s" }) }
        _ => format!("Added {}", crate::util::fmt_last_played(added as f64).to_lowercase().replace("today", "today")),
    }
}

/// The speed line: the last minutes of download speed, scaled to its own peak, on a 400×160 box.
fn spark_path(speeds: &std::collections::VecDeque<f32>) -> String {
    if speeds.len() < 2 { return String::new(); }
    let peak = speeds.iter().copied().fold(0.1f32, f32::max);
    let step = 400.0 / (speeds.len() - 1) as f32;
    speeds.iter().enumerate().map(|(i, v)| {
        format!("{}{:.1} {:.1}", if i == 0 { "M " } else { " L " }, i as f32 * step, 150.0 - v / peak * 140.0)
    }).collect()
}

fn describe(job: &Job, install: Option<&Record>, failure: Option<&String>, queued_behind: Option<&str>, sharing_on: bool) -> Desc {
    let mut d = Desc { group: 0, kind: 0, status: String::new(), big: String::new(), unit: String::new(), stats: vec![], spark: false,
        note: String::new(), dl: job.progress(), inst: 0.0, ready: false, dl_label: "Downloading".into(),
        inst_label: if job.install_to.is_some() { "Install".into() } else { "Install by hand".into() }, bar: 0, actions: vec![],
        line: String::new(), tile: job.progress() };
    let size = size_label(job.total);
    let progress = |d: &mut Desc, f: f32| { d.big = pct(f); d.unit = "%".into(); };
    match job.state {
        State::Resolving => {
            d.status = "Getting the file list".into();
            d.line = "Getting the file list".into();
            d.actions = vec![("cancel", "Cancel", "close")];
        }
        State::Ready => {
            (d.group, d.kind) = (1, 1);
            d.stats = vec![(size.clone(), format!("{} files", job.file_count))];
            // One looked up before files could be chosen downloads them all.
            if job.entries.is_empty() {
                d.status = "Ready to download".into();
                d.line = "Ready to download".into();
                d.actions = vec![("start", "Start download", "disk"), ("cancel", "Remove", "trash")];
            } else {
                d.status = "Waiting for you to choose its files".into();
                d.line = "Choose its files".into();
                d.actions = vec![("choose", "Choose files", "check"), ("cancel", "Remove", "trash")];
            }
        }
        State::Checking | State::Downloading => {
            d.status = if job.state == State::Checking { "Checking what's already downloaded".into() } else { "Downloading".into() };
            progress(&mut d, job.progress());
            if job.state == State::Downloading {
                d.stats.push((mbps(job.speed), "Download speed".into()));
                d.spark = true;
                if !job.eta.is_empty() { d.stats.push((job.eta.clone(), "Time left".into())); }
            }
            d.stats.push((format!("{} of {}", size_label(job.done), size), "Downloaded".into()));
            d.line = if job.eta.is_empty() { format!("{}%", pct(job.progress())) } else { format!("{}% · {} left", pct(job.progress()), job.eta) };
            d.actions = vec![("pause", "Pause", "pause"), ("cancel", "Cancel", "close")];
        }
        State::Paused => {
            (d.group, d.kind, d.bar) = (1, 1, 1);
            d.dl_label = "Paused".into();
            progress(&mut d, job.progress());
            d.stats = vec![(format!("{} of {}", size_label(job.done), size), "Downloaded".into()),
                (size_label(job.total.saturating_sub(job.done)), "Still to download".into())];
            if job.waiting_vpn {
                d.status = "Paused until your VPN connects".into();
                d.stats.push(("By itself".into(), "Resumes".into()));
                d.line = "Waiting for your VPN".into();
                d.actions = vec![("vpn", "VPN settings", "gear"), ("cancel", "Cancel", "close")];
            } else {
                d.status = "Paused".into();
                d.line = format!("Paused · {}%", pct(job.progress()));
                d.actions = vec![("resume", "Resume", "play"), ("cancel", "Cancel", "close")];
            }
        }
        State::Failed => {
            (d.group, d.kind, d.bar) = (1, 2, 2);
            progress(&mut d, job.progress());
            d.status = "Stopped".into();
            d.dl_label = "Stopped".into();
            d.note = job.error.clone();
            d.stats = vec![(format!("{} of {}", size_label(job.done), size), "Downloaded, and kept".into())];
            d.line = "Stopped".into();
            d.actions = vec![("retry", "Retry", "retry"), ("remove", "Remove", "trash")];
        }
        State::Cancelled => {
            (d.group, d.kind, d.bar) = (1, 1, 1);
            progress(&mut d, job.progress());
            d.status = "Cancelled · what's downloaded is kept".into();
            d.dl_label = "Cancelled".into();
            d.line = "Cancelled".into();
            d.actions = vec![("retry", "Download again", "retry"), ("remove", "Remove", "trash")];
        }
        State::Complete => {
            d.dl = 1.0;
            d.dl_label = "Downloaded".into();
            let sharing = job.seeding || job.verifying;
            let share = if sharing { ("stop_seed", "Stop sharing", "unshare") } else { ("remove", "Remove", "trash") };
            match install.map(|r| r.state) {
                Some(state) if state.active() => {
                    let record = install.unwrap();
                    progress(&mut d, record.progress());
                    d.status = "Installing".into();
                    d.inst = record.progress();
                    d.inst_label = "Installing".into();
                    d.stats = vec![(record.state.label().into(), "Now".into()),
                        (format!("{} of {}", size_label(record.done), size_label(record.total.max(record.done))), "Unpacked".into())];
                    d.line = format!("Installing · {}%", pct(record.progress()));
                    d.tile = record.progress();
                    d.actions = vec![("cancel_install", "Cancel install", "close")];
                }
                Some(Install::Installed) => {
                    (d.group, d.kind, d.bar) = (2, 3, 3);
                    d.status = "Ready to play".into();
                    (d.inst, d.ready) = (1.0, true);
                    d.inst_label = "Installed".into();
                    d.stats = if job.seeding {
                        vec![(format!("{} player{}", job.peers, if job.peers == 1 { "" } else { "s" }), "Connected to you".into()),
                            (mbps(job.upload_speed), "Sharing speed".into()), (size, "Size".into())]
                    } else {
                        vec![(size, "Size".into()), ("Not sharing".into(), if sharing_on { "Stopped until the launcher restarts" } else { "Sharing is off in Settings" }.into())]
                    };
                    d.line = if job.seeding { "Ready · sharing".into() } else { "Ready to play".into() };
                    d.tile = 1.0;
                    d.actions = vec![("play_install", "Play", "play"), share];
                }
                Some(Install::Failed) => {
                    (d.group, d.kind, d.bar) = (1, 2, 2);
                    d.status = "Couldn't install".into();
                    d.note = install.unwrap().error.clone();
                    d.inst_label = "Install failed".into();
                    d.line = "Couldn't install".into();
                    d.actions = vec![("install", "Retry install", "retry"), share];
                }
                Some(_) => {
                    (d.group, d.kind) = (1, 1);
                    d.status = "Install cancelled".into();
                    d.line = "Install cancelled".into();
                    d.actions = vec![("install", "Install", "disk"), share];
                }
                None if failure.is_some() => {
                    (d.group, d.kind, d.bar) = (1, 2, 2);
                    d.status = "Couldn't install".into();
                    d.note = failure.unwrap().clone();
                    d.inst_label = "Install failed".into();
                    d.line = "Couldn't install".into();
                    d.actions = vec![("install", "Retry install", "retry"), share];
                }
                None if job.install_to.is_some() => {
                    d.status = "Waiting to install".into();
                    d.inst_label = "Install · next in line".into();
                    d.stats = vec![(queued_behind.map_or("Next".into(), |name| format!("After {name}")), "Installs by itself".into()), (size, "Downloaded".into())];
                    d.line = "Installs next".into();
                    d.tile = 1.0;
                    d.actions = vec![share];
                }
                None => {
                    (d.group, d.kind) = (1, 1);
                    d.status = "Downloaded · not installed yet".into();
                    d.stats = vec![(size, "Downloaded".into())];
                    d.line = "Not installed".into();
                    d.tile = 1.0;
                    d.actions = vec![("install", "Install", "disk"), share];
                }
            }
        }
    }
    d
}

impl App {
    /// Download pressed: the Download & install panel, or the download if it's already under way.
    pub fn prepare_download(&mut self, target: Target) {
        if let Some(job) = self.download_for_target(target).filter(|job| job.state != State::Ready || job.entries.is_empty()) {
            self.open_downloads(Some(&job.key));
            return;
        }
        self.open_setup(target);
    }

    pub fn open_downloads(&mut self, key: Option<&str>) {
        if self.overlay != Overlay::Downloads {
            self.push_overlay(Overlay::Downloads, Z_DOWNLOADS, 0);
            self.dl_shelf = 0.0;
        }
        let ui = self.ui();
        ui.set_download_consent(false);
        ui.set_download_is_install(false);
        ui.set_download_focus_path(false);
        ui.set_download_editing(false);
        if let Some(key) = key {
            self.dl_sel = Some(key.into());
            self.dl_filter = 0;
        }
        self.dl_btn = -1;
        self.push_downloads();
        ui.invoke_focus_root();
    }

    /// Only installing by hand asks for confirmation now (downloads start from the panel).
    pub fn confirm_download(&mut self) {
        if self.overlay != Overlay::Downloads || !self.ui().get_download_consent() { return; }
        if self.ui().get_download_is_install() { self.confirm_install(); }
    }

    pub fn download_action(&mut self, key: &str, action: &str) {
        if self.overlay != Overlay::Downloads || self.ui().get_download_consent() { return; }
        let jobs = self.downloads.snapshot();
        let Some(job) = jobs.iter().find(|job| job.key == key) else { return };
        let install = self.installer.snapshot().into_iter().find(|record| record.key == key);
        match action {
            "folder" => {
                let path = install.as_ref().filter(|record| record.state == Install::Installed).and_then(|record| record.path.as_ref()).unwrap_or(&job.folder);
                if path.is_dir() { open_url(&path.to_string_lossy()); }
            }
            "install" => {
                // Retrying where it was set to install skips the question; otherwise ask where.
                let to = job.install_to.as_ref().map(|path| path.to_string_lossy().into_owned());
                match to {
                    Some(to) => match self.start_install(key, &to) {
                        Ok(()) => { audio::play(Sound::Select); self.push_installs(); }
                        Err(error) => { self.install_failures.insert(key.into(), format!("{error:#}")); }
                    },
                    None => self.prepare_install(key),
                }
            }
            "cancel_install" => {
                if let Err(error) = self.installer.cancel(key) { self.toast("Installation unavailable", &error.to_string(), 2); }
            }
            "play_install" => {
                self.push_installs();
                if let Some(path) = install.as_ref().filter(|record| record.state == Install::Installed).and_then(|record| record.path.as_ref()) {
                    if let Some(local) = self.locals.iter().position(|game| &game.l.path == path) {
                        self.run_action("play", Target { game: self.locals[local].cat, local: Some(local) });
                    } else { self.toast("Game folder unavailable", "The installed folder is missing; check Game folders in Settings.", 2); }
                }
                return;
            }
            "vpn" => { self.open_settings(); return; }
            "choose" => {
                if let Some(game) = self.games.iter().position(|game| game.g.id == job.topic) {
                    let local = self.games[game].local;
                    self.open_setup(Target { game: Some(game), local });
                }
                return;
            }
            _ if action != "stop_seed" && install.as_ref().is_some_and(|record| record.state.active()) => {
                self.toast("Installation in progress", "Cancel the installation before removing its download.", 0);
            }
            _ => {
                if let Err(error) = self.downloads.action(key, action) { self.toast("Transfer unavailable", &error.to_string(), 2); }
                else { audio::play(Sound::Select); }
            }
        }
        self.push_downloads();
    }

    pub fn push_downloads(&mut self) {
        self.auto_install();
        let jobs = self.downloads.snapshot();
        let installs = self.installer.snapshot();
        let mut changed = false;
        for job in &jobs {
            let old = self.download_states.insert(job.key.clone(), job.state);
            changed |= old != Some(job.state);
            if old.is_some_and(|old| old != job.state) {
                if job.state == State::Complete {
                    if job.install_to.is_some() { self.toast_action("Download complete", &format!("{} · it installs by itself", job.name), 1, "downloads", "Show"); }
                    else { self.toast_action("Download complete", &job.name, 1, "downloads", "Install from Downloads"); }
                } else if job.state == State::Failed && job.done > 0 {
                    self.toast_action("Download stopped", &job.name, 2, "downloads", "See why");
                }
            }
            let speeds = self.dl_speeds.entry(job.key.clone()).or_default();
            if job.state == State::Downloading {
                speeds.push_back(job.speed as f32);
                if speeds.len() > 90 { speeds.pop_front(); }
            } else if job.state != State::Checking { speeds.clear(); }
        }
        // A download that went away (a lookup the panel forgot) changes the Game Hub's button too.
        let before = self.download_states.len();
        self.download_states.retain(|key, _| jobs.iter().any(|job| &job.key == key));
        changed |= self.download_states.len() != before;
        self.dl_speeds.retain(|key, _| jobs.iter().any(|job| &job.key == key));
        if changed && self.overlay == Overlay::Hub { self.push_hub(); }
        if self.overlay == Overlay::Setup { self.push_setup(); }
        let active: Vec<_> = jobs.iter().filter(|job| job.state.active()).collect();
        let seeding = jobs.iter().filter(|job| job.seeding).count();
        let done = active.iter().fold(0u64, |sum, job| sum.saturating_add(job.done));
        let total = active.iter().fold(0u64, |sum, job| sum.saturating_add(job.total));
        let progress = if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 };
        let ui = self.ui();
        ui.set_download_active(!active.is_empty() || seeding > 0);
        ui.set_download_progress(progress);
        ui.set_download_badge(if active.is_empty() { format!("{seeding} sharing") }
            else if total == 0 { "Getting file list".to_string() } else { format!("{} active · {:.0}%", active.len(), progress * 100.0) }.into());
        if self.overlay != Overlay::Downloads { return; }

        // Newest first within each group: in progress, needs you, ready to play.
        let sharing_on = self.cfg.lock().unwrap().seed_after_download;
        let mut rows: Vec<(usize, Desc)> = jobs.iter().enumerate().map(|(i, job)| {
            let install = installs.iter().find(|record| record.key == job.key);
            let behind = installs.iter().find(|record| record.state.active() && record.key != job.key).map(|record| record.name.as_str());
            (i, describe(job, install, self.install_failures.get(&job.key), behind, sharing_on))
        }).collect();
        let counts = [rows.len(), rows.iter().filter(|r| r.1.group == 0).count(), rows.iter().filter(|r| r.1.group == 1).count(), rows.iter().filter(|r| r.1.group == 2).count()];
        rows.retain(|(_, d)| self.dl_filter == 0 || d.group == self.dl_filter as i32 - 1);
        rows.sort_by_key(|(i, d)| (d.group, std::cmp::Reverse((jobs[*i].added, *i))));
        let sel = self.dl_sel.as_ref().and_then(|key| rows.iter().position(|(i, _)| &jobs[*i].key == key)).unwrap_or(0);
        self.dl_sel = rows.get(sel).map(|(i, _)| jobs[*i].key.clone());
        let actions = rows.get(sel).map_or(0, |(_, d)| d.actions.len() + 1) as i32;
        if self.dl_btn >= actions { self.dl_btn = actions - 1; }

        // Keep the selected tile in view.
        let (w, _) = self.logical_size();
        let x_of = |n: usize| n as f32 * (TILE + GAP) + (1..=n).filter(|&k| rows[k].1.group != rows[k - 1].1.group).count() as f32 * GROUP_GAP;
        if !rows.is_empty() {
            let x = x_of(sel);
            let view = (w - 240.0).max(TILE);
            if x < self.dl_shelf { self.dl_shelf = x; }
            if x + TILE > self.dl_shelf + view { self.dl_shelf = x + TILE - view; }
        } else { self.dl_shelf = 0.0; }

        self.dl_keys.clear();
        let scale = self.scale;
        let mut data = Vec::with_capacity(rows.len());
        for (n, (i, d)) in rows.iter().enumerate() {
            let job = &jobs[*i];
            let game = self.games.iter().position(|game| game.g.id == job.topic);
            let target = Target { game, local: game.and_then(|g| self.games[g].local) };
            let info = game.and_then(|_| self.target_info(target));
            let want = |app: &mut App, r: Option<crate::present::ImgReq>, prio: u8| -> Option<slint::Image> {
                let r = r?;
                app.dl_keys.insert(r.key.clone());
                app.images.want(&r.key, r.src.clone(), r.w, r.crop, prio)
            };
            // The same picture as the big art (usually on disk already), decoded small.
            let tile_req = info.as_ref().filter(|i| !i.hub.is_empty()).map(|i| crate::present::ImgReq {
                key: format!("{}#tile", i.hub), src: crate::images::Src::Url(crate::psn::sized(&i.hub, 1920)), w: 640, crop: 0.0,
            });
            let tile = want(self, tile_req.or_else(|| game.and_then(|_| self.cover_req(target))), crate::images::prio::TILE);
            let (art, logo) = if n == sel {
                (want(self, game.and_then(|_| self.target_bg_url(target)), crate::images::prio::HERO),
                 want(self, info.as_ref().and_then(|i| req_url(&i.logo, 960, 0.0)), crate::images::prio::HERO))
            } else { (None, None) };
            let (logo_w, logo_h) = logo.as_ref().map(|img| {
                let s = img.size();
                let k = (560.0 / s.width.max(1) as f32).min(140.0 / s.height.max(1) as f32);
                (s.width as f32 * k * scale, s.height as f32 * k * scale)
            }).unwrap_or((0.0, 0.0));
            let first = n == 0 || rows[n - 1].1.group != d.group;
            let mut actions: Vec<ActionData> = d.actions.iter().enumerate().map(|(k, (id, label, icon))| ActionData {
                id: (*id).into(), label: (*label).into(), icon: (*icon).into(), primary: k == 0, danger: false, round: false,
            }).collect();
            actions.push(ActionData { id: "folder".into(), label: "Open folder".into(), icon: "folder".into(), primary: false, danger: false, round: true });
            data.push(crate::DownloadData {
                key: job.key.clone().into(), title: job.name.clone().into(),
                has_art: art.is_some(), art: art.unwrap_or_default(),
                has_tile_art: tile.is_some(), tile_art: tile.unwrap_or_default(),
                has_logo: logo.is_some(), logo: logo.unwrap_or_default(), logo_w, logo_h,
                status: d.status.clone().into(), kind: d.kind, when: added_label(job.added).into(),
                big: d.big.clone().into(), big_unit: d.unit.clone().into(),
                stats: ModelRc::new(VecModel::from(d.stats.iter().map(|(v, l)| DlStat { value: v.clone().into(), label: l.clone().into() }).collect::<Vec<_>>())),
                spark: if d.spark { self.dl_speeds.get(&job.key).map(spark_path).unwrap_or_default() } else { String::new() }.into(),
                note: d.note.clone().into(),
                dl_frac: d.dl, inst_frac: d.inst, ready: d.ready, dl_label: d.dl_label.clone().into(), inst_label: d.inst_label.clone().into(), bar_kind: d.bar,
                actions: ModelRc::new(VecModel::from(actions)),
                line: d.line.clone().into(), tile_frac: d.tile,
                group: d.group, group_first: first, group_label: GROUPS[d.group as usize].into(),
            });
        }
        let tabs: Vec<DlTab> = ["All", "In progress", "Needs you", "Ready to play"].iter().enumerate().map(|(k, label)| DlTab {
            label: (*label).into(), count: counts[k].to_string().into(), warn: k == 2 && counts[2] > 0,
        }).collect();
        let (down, up) = jobs.iter().fold((0.0, 0.0), |(d, u), job| (d + job.speed, u + job.upload_speed));
        let vpn = { let cfg = self.cfg.lock().unwrap(); cfg.download_vpn().map(|preferred| crate::vpn::active(&preferred)) };
        let ui = self.ui();
        // Rebuilding the model every second would restart its images; only changed rows are set.
        if self.download_model.row_count() != data.len() {
            self.download_model.set_vec(data);
        } else {
            for (n, row) in data.into_iter().enumerate() {
                let old = self.download_model.row_data(n);
                let same = old.as_ref().is_some_and(|old| old.key == row.key && old.status == row.status && old.big == row.big
                    && old.line == row.line && old.spark == row.spark && old.has_art == row.has_art && old.has_logo == row.has_logo
                    && old.has_tile_art == row.has_tile_art && old.note == row.note && old.when == row.when && old.dl_frac == row.dl_frac
                    && old.inst_frac == row.inst_frac && old.tile_frac == row.tile_frac && old.group_first == row.group_first
                    && old.stats.iter().eq(row.stats.iter()) && old.actions.iter().eq(row.actions.iter()));
                if !same { self.download_model.set_row_data(n, row); }
            }
        }
        ui.set_download_tabs(ModelRc::new(VecModel::from(tabs)));
        ui.set_download_filter(self.dl_filter as i32);
        ui.set_download_sel(sel as i32);
        ui.set_download_focus_btn(self.dl_btn);
        ui.set_download_shelf_x(self.dl_shelf * scale);
        ui.set_download_down(mbps(down).into());
        ui.set_download_up(mbps(up).into());
        let (vpn_text, vpn_kind) = match vpn { None => ("", 0), Some(Some(_)) => ("VPN on", 1), Some(None) => ("Waiting for your VPN", 2) };
        ui.set_download_has_any(!jobs.is_empty());
        ui.set_download_empty_title(["Nothing downloading", "Nothing in progress", "Nothing needs you", "Nothing ready to play yet"][self.dl_filter].into());
        ui.set_download_vpn(vpn_text.into());
        ui.set_download_vpn_kind(vpn_kind);
        ui.set_zone(Z_DOWNLOADS);
    }

    /// The downloads the shelf shows, in its order (after the filter).
    fn shelf_keys(&self) -> Vec<String> {
        self.download_model.iter().map(|row| row.key.to_string()).collect()
    }

    pub fn select_download(&mut self, index: usize, mouse: bool) {
        let keys = self.shelf_keys();
        let Some(key) = keys.get(index) else { return };
        if self.dl_sel.as_deref() != Some(key.as_str()) { audio::play(Sound::Move); }
        self.dl_sel = Some(key.clone());
        if mouse || self.dl_btn >= 0 { self.dl_btn = -1; }
        self.push_downloads();
    }

    pub fn filter_downloads(&mut self, filter: usize) {
        if filter > 3 { return; }
        if filter != self.dl_filter { audio::play(Sound::Move); }
        self.dl_filter = filter;
        self.dl_btn = -1;
        self.push_downloads();
    }

    pub fn act_downloads(&mut self, action: Act) {
        if self.ui().get_download_consent() {
            if action == Act::Back {
                self.install_pending = None;
                self.ui().set_download_consent(false);
                self.push_downloads();
                return;
            }
            // Enter while typing only finishes editing; another explicit confirmation is required.
            if action == Act::Confirm { self.confirm_download(); }
            return;
        }
        let keys = self.shelf_keys();
        let sel = self.dl_sel.as_ref().and_then(|key| keys.iter().position(|k| k == key)).unwrap_or(0);
        let buttons = self.download_model.row_data(sel).map_or(0, |row| row.actions.row_count()) as i32;
        match action {
            Act::Back if self.dl_btn >= 0 => { self.dl_btn = -1; audio::play(Sound::Back); }
            Act::Back => return self.back(),
            Act::TabPrev => return self.filter_downloads((self.dl_filter + 3) % 4),
            Act::TabNext => return self.filter_downloads((self.dl_filter + 1) % 4),
            _ if keys.is_empty() => return,
            Act::Left if self.dl_btn > 0 => { self.dl_btn -= 1; audio::play(Sound::Move); }
            Act::Right if self.dl_btn >= 0 && self.dl_btn + 1 < buttons => { self.dl_btn += 1; audio::play(Sound::Move); }
            Act::Left | Act::Right if self.dl_btn >= 0 => {}
            Act::Left => return self.select_download(sel.saturating_sub(1), false),
            Act::Right => return self.select_download((sel + 1).min(keys.len() - 1), false),
            Act::First => return self.select_download(0, false),
            Act::Last => return self.select_download(keys.len() - 1, false),
            Act::Up | Act::Confirm if self.dl_btn < 0 && buttons > 0 => { self.dl_btn = 0; audio::play(Sound::Move); }
            Act::Down if self.dl_btn >= 0 => { self.dl_btn = -1; audio::play(Sound::Move); }
            Act::Confirm if self.dl_btn >= 0 => {
                if let Some(id) = self.download_model.row_data(sel).and_then(|row| row.actions.row_data(self.dl_btn as usize)).map(|a| a.id.to_string()) {
                    return self.download_action(&keys[sel], &id);
                }
            }
            _ => return,
        }
        self.push_downloads();
    }
}
