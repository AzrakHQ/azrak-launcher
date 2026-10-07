//! Explicit consent, metadata review and controller/mouse-friendly transfer snapshots.
use crate::app::*;
use crate::downloads::{bytes_label, State};
use crate::util;
use slint::Model;

impl App {
    pub fn prepare_download(&mut self, target: Target) {
        let Some(index) = target.game else { return };
        let game = &self.games[index];
        if let Some(job) = self.download_for_target(target) {
            let key = job.key.clone();
            self.open_downloads(Some(&key));
            return;
        }
        self.download_pending = Some((game.g.id, game.name.clone(), game.g.magnet.clone()));
        let ui = self.ui();
        ui.set_download_title(game.name.clone().into());
        ui.set_download_path(self.cfg.lock().unwrap().download_dir.clone().into());
        ui.set_download_error("".into());
        let privacy = match self.cfg.lock().unwrap().download_vpn() {
            None => String::new(),
            Some(preferred) => match crate::vpn::active(&preferred) {
                Some(iface) => format!("It goes only through your VPN ({iface}): trackers, DHT and peers see the VPN's address, not yours."),
                None => "Your VPN isn't connected: the download waits and starts through it once it is. Trackers, DHT and peers will see the VPN's address, not yours.".into(),
            },
        };
        ui.set_download_privacy(privacy.into());
        ui.set_download_warning(self.pc_download_warning(self.games[index].g.platform).into());
        self.open_downloads(None);
        ui.set_download_consent(true);
    }

    pub fn open_downloads(&mut self, key: Option<&str>) {
        if self.overlay != Overlay::Downloads {
            self.push_overlay(Overlay::Downloads, Z_DOWNLOADS, 0);
        }
        let ui = self.ui();
        ui.set_download_consent(false);
        ui.set_download_is_install(false);
        ui.set_download_focus_path(false);
        ui.set_download_editing(false);
        self.push_downloads();
        let index = key.and_then(|key| self.downloads.snapshot().iter().position(|job| job.key == key)).unwrap_or(0);
        self.set_focus(Z_DOWNLOADS, index as i32 * 3);
        self.scroll_downloads();
        ui.invoke_focus_root();
    }

    pub fn confirm_download(&mut self) {
        if self.overlay != Overlay::Downloads || !self.ui().get_download_consent() { return; }
        if self.ui().get_download_is_install() { self.confirm_install(); return; }
        let Some((topic, name, magnet)) = self.download_pending.clone() else { return };
        let path = self.ui().get_download_path().trim().to_owned();
        match self.downloads.prepare(topic, &name, &magnet, util::expand_home(&path)) {
            Ok(key) => {
                self.download_pending = None;
                { let mut cfg = self.cfg.lock().unwrap(); cfg.download_dir = path; cfg.save(); }
                self.open_downloads(Some(&key));
            }
            Err(error) => self.ui().set_download_error(error.to_string().into()),
        }
    }

    pub fn download_action(&mut self, key: &str, action: &str) {
        if self.overlay != Overlay::Downloads || self.ui().get_download_consent() { return; }
        let jobs = self.downloads.snapshot();
        let Some((index, job)) = jobs.iter().enumerate().find(|(_, job)| job.key == key) else { return };
        let slot = if action == "folder" { 2 } else if matches!(action, "cancel" | "remove" | "cancel_install" | "stop_seed") { 1 } else { 0 };
        self.set_focus(Z_DOWNLOADS, index as i32 * 3 + slot);
        let install = self.installer.snapshot().into_iter().find(|record| record.key == key);
        if action == "install" { self.prepare_install(key); return; }
        if action == "cancel_install" {
            if let Err(error) = self.installer.cancel(key) { self.toast("Installation unavailable", &error.to_string(), 2); }
            return;
        }
        if action == "play_install" {
            self.push_installs();
            if let Some(path) = install.as_ref().filter(|record| record.state == crate::installer::State::Installed).and_then(|record| record.path.as_ref()) {
                if let Some(local) = self.locals.iter().position(|game| &game.l.path == path) {
                    self.run_action("play", Target { game: self.locals[local].cat, local: Some(local) });
                } else { self.toast("Game folder unavailable", "The installed folder is missing; check Game folders in Settings.", 2); }
            }
            return;
        }
        if action == "folder" {
            let path = install.as_ref().filter(|record| record.state == crate::installer::State::Installed).and_then(|record| record.path.as_ref()).unwrap_or(&job.folder);
            if path.is_dir() { open_url(&path.to_string_lossy()); }
        } else if action != "stop_seed" && install.as_ref().is_some_and(|record| record.state.active()) {
            self.toast("Installation in progress", "Cancel installation before removing its transfer history.", 0);
        } else if let Err(error) = self.downloads.action(key, action) {
            self.toast("Transfer unavailable", &error.to_string(), 2);
        }
        self.scroll_downloads();
    }

    pub fn push_downloads(&mut self) {
        let jobs = self.downloads.snapshot();
        let installs = self.installer.snapshot();
        let mut changed = false;
        for job in &jobs {
            if self.download_states.insert(job.key.clone(), job.state).is_some_and(|old| old != job.state) {
                changed = true;
                if job.state == State::Complete {
                    self.toast_action("Download complete", &format!("{} · click to install it from Downloads", job.name), 1, "downloads");
                } else if job.state == State::Failed {
                    self.toast_action("Download stopped", &format!("{} · click to see why in Downloads", job.name), 2, "downloads");
                }
            }
        }
        self.download_states.retain(|key, _| jobs.iter().any(|job| &job.key == key));
        if changed && self.overlay == Overlay::Hub { self.push_hub(); }
        let active: Vec<_> = jobs.iter().filter(|job| job.state.active()).collect();
        let seeding = jobs.iter().filter(|job| job.seeding).count();
        let done = active.iter().fold(0u64, |sum, job| sum.saturating_add(job.done));
        let total = active.iter().fold(0u64, |sum, job| sum.saturating_add(job.total));
        let progress = if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 };
        let ui = self.ui();
        ui.set_download_active(!active.is_empty() || seeding > 0);
        ui.set_download_progress(progress);
        ui.set_download_badge(if active.is_empty() { format!("{seeding} seeding") }
            else if total == 0 { "Resolving metadata".to_string() } else { format!("{} active · {:.0}%", active.len(), progress * 100.0) }.into());
        let vpn = {
            let cfg = self.cfg.lock().unwrap();
            match cfg.download_vpn().map(|preferred| crate::vpn::active(&preferred)) {
                Some(Some(iface)) => format!("Protected by your VPN · {iface} · "),
                Some(None) => "Waiting for your VPN · ".to_string(),
                None => String::new(),
            }
        };
        ui.set_download_summary(format!("{vpn}{} transfers · {} downloading · {seeding} seeding · original files are kept", jobs.len(), active.len()).into());
        if self.overlay != Overlay::Downloads { return; }
        let rows: Vec<_> = jobs.iter().map(|job| {
            let install = installs.iter().find(|record| record.key == job.key);
            let installing = install.is_some_and(|record| record.state.active());
            let installed = install.is_some_and(|record| record.state == crate::installer::State::Installed);
            let detail = if let Some(record) = install.filter(|record| record.state.active()) {
                format!("{} / {} · {:.1}% · extraction runs in the background", bytes_label(record.done), bytes_label(record.total), record.progress() * 100.0)
            } else if job.seeding {
                format!("{} complete · ↑ {:.2} MiB/s upload · {} connected peers · 128 KiB/s limit",
                    bytes_label(job.total), job.upload_speed, job.peers)
            } else if job.total == 0 { "Waiting for torrent metadata; no game files downloaded".to_string() }
                else { format!("{} / {} · {:.1}% · {:.2} MiB/s · {} connected peers{}",
                    bytes_label(job.done), bytes_label(job.total), job.progress() * 100.0, job.speed, job.peers,
                    if job.eta.is_empty() { String::new() } else { format!(" · ETA {}", job.eta) }) };
            let review = job.state == State::Ready;
            crate::DownloadData {
                key: job.key.clone().into(), title: job.name.clone().into(),
                state: install.map(|record| format!("{} · {}", job.label(), record.state.label())).unwrap_or_else(|| job.label().into()).into(),
                progress: if installing { install.unwrap().progress() } else { job.progress() }, detail: detail.into(),
                path: install.and_then(|record| record.path.as_ref()).unwrap_or(&job.folder).to_string_lossy().into_owned().into(),
                error: install.map(|record| record.error.clone()).filter(|error| !error.is_empty()).unwrap_or_else(|| job.error.clone()).into(),
                failed: job.state == State::Failed || install.is_some_and(|record| record.state == crate::installer::State::Failed),
                complete: job.state == State::Complete && !installing,
                primary: if installing { "Installing…" } else if installed { "Play" } else if job.state == State::Complete { if install.is_some() { "Retry install" } else { "Install" } } else { job.primary() }.into(),
                primary_action: if installing { "" } else if installed { "play_install" } else if job.state == State::Complete { "install" } else { job.action_id() }.into(),
                secondary: if installing { "Cancel install" } else if job.seeding || job.verifying { "Stop seeding" } else if job.state.active() || review { "Cancel" } else { "Remove" }.into(),
                secondary_action: if installing { "cancel_install" } else if job.seeding || job.verifying { "stop_seed" } else if job.state.active() || review { "cancel" } else { "remove" }.into(),
                review, files: job.files.join("\n").into(),
                file_summary: format!("{} files · download all {}{}", job.file_count, bytes_label(job.total),
                    if job.file_count > job.files.len() { " · first 100 shown" } else { "" }).into(),
            }
        }).collect();
        // Preserve row instances, animations, mouse presses and file-list scroll positions.
        if self.download_model.row_count() != rows.len() {
            self.download_model.set_vec(rows);
        } else {
            for (index, row) in rows.into_iter().enumerate() {
                if self.download_model.row_data(index).as_ref() != Some(&row) {
                    self.download_model.set_row_data(index, row);
                }
            }
        }
        let max = (jobs.len() as i32 * 3 - 1).max(0);
        if self.idx > max { self.set_focus(Z_DOWNLOADS, max); }
    }

    pub fn scroll_downloads(&mut self) {
        let jobs = self.downloads.snapshot();
        let row = (self.idx.max(0) / 3) as usize;
        let start: f32 = jobs.iter().take(row).map(|job| if job.state == State::Ready { 428.0 } else { 272.0 }).sum();
        let height = jobs.get(row).map(|job| if job.state == State::Ready { 428.0 } else { 272.0 }).unwrap_or(272.0);
        let available = (self.logical_size().1 - 310.0).max(100.0);
        let current = -self.ui().get_downloads_y() / self.scale;
        if start < current { self.ui().set_downloads_y(-start * self.scale); }
        else if start + height > current + available {
            self.ui().set_downloads_y(-(start + height - available).max(0.0) * self.scale);
        }
    }

    pub fn act_downloads(&mut self, action: Act) {
        if action == Act::Back {
            self.download_pending = None;
            self.install_pending = None;
            self.ui().set_download_consent(false);
            self.back();
            return;
        }
        if self.ui().get_download_consent() {
            // Enter while typing only finishes editing; another explicit confirmation is required.
            if action == Act::Confirm { self.confirm_download(); }
            return;
        }
        let jobs = self.downloads.snapshot();
        if jobs.is_empty() { return; }
        let last = jobs.len() as i32 * 3 - 1;
        let next = match action {
            Act::Left => (self.idx - 1).max(0), Act::Right => (self.idx + 1).min(last),
            Act::Up => (self.idx - 3).max(0), Act::Down => (self.idx + 3).min(last),
            Act::First => 0, Act::Last => last,
            Act::PageUp => (self.idx - 9).max(0), Act::PageDown => (self.idx + 9).min(last),
            Act::Confirm => {
                if let Some(job) = jobs.get((self.idx.max(0) / 3) as usize) {
                    let action = match self.idx % 3 {
                        0 => job.action_id(),
                        1 => if job.seeding || job.verifying { "stop_seed" } else if job.state.active() || job.state == State::Ready { "cancel" } else { "remove" },
                        _ => "folder",
                    };
                    let install = self.installer.snapshot().into_iter().find(|record| record.key == job.key);
                    let action = if self.idx % 3 == 0 && job.state == State::Complete && !install.as_ref().is_some_and(|record| record.state.active() || record.state == crate::installer::State::Installed) { "install" }
                        else if self.idx % 3 == 0 && install.as_ref().is_some_and(|record| record.state == crate::installer::State::Installed) { "play_install" }
                        else if self.idx % 3 == 1 && install.as_ref().is_some_and(|record| record.state.active()) { "cancel_install" }
                        else { action };
                    self.download_action(&job.key, action);
                }
                return;
            }
            _ => return,
        };
        self.set_focus(Z_DOWNLOADS, next);
        self.scroll_downloads();
    }
}