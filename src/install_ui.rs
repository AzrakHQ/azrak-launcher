//! Installation is explicit and separate from downloading. The worker never runs payload code.
use crate::{app::*, downloads, installer, util};

/// What a download can be installed from: archives on both consoles; PKG packages only for PS4,
/// whose PKG extractor unpacks the game, its update and its DLC for shadPS4.
fn install_note(platform: Option<crate::platform::Platform>) -> &'static str {
    use crate::platform::Platform;
    match platform {
        Some(Platform::Ps4) => "Folders, exFAT images and RAR, ZIP, 7z or TAR archives (these depend on the system's libarchive; password-protected ones aren't supported). PS4 PKG packages are unpacked for shadPS4, with their update and DLC. Installing checks the game's structure, not that its content is authentic, and never runs the downloaded files.",
        Some(Platform::Ps5) => "Folders, exFAT images and RAR, ZIP, 7z or TAR archives (these depend on the system's libarchive; password-protected ones aren't supported). PS5 PKG packages can't be installed: choose a release of this game that isn't a PKG. Installing checks the game's structure, not that its content is authentic, and never runs the downloaded files.",
        Some(Platform::Pc) => "Folders, RAR, ZIP, 7z or TAR archives (these depend on the system's libarchive), GOG Linux installers (.sh) and AppImages. Installing unpacks the game and finds how to start it; nothing in it runs until you press Play. Unlike console games, which run inside an emulator, PC games run with your user's access to your files: only install releases you trust.",
        None => "Folders, exFAT images, RAR, ZIP, 7z or TAR archives, and PS4 PKG packages (not PS5 PKG). Installing checks the game's structure, not that its content is authentic, and never runs the downloaded files.",
    }
}

impl App {
    pub fn download_for_target(&self, target: Target) -> Option<downloads::Job> {
        let game = &self.games.get(target.game?)?.g;
        let hash = downloads::normalize_magnet(&game.magnet).ok().map(|(key, _)| key);
        self.downloads.snapshot().into_iter().find(|job| job.topic == game.id || hash.as_ref() == Some(&job.key))
    }

    pub fn prepare_install(&mut self, key: &str) {
        let Some(job) = self.downloads.snapshot().into_iter().find(|job| job.key == key && job.state == downloads::State::Complete) else { return; };
        if self.installer.snapshot().iter().any(|record| record.key == key && record.state.active()) {
            self.open_downloads(Some(key)); return;
        }
        self.open_downloads(Some(key));
        self.install_pending = Some(key.into());
        let ui = self.ui();
        ui.set_download_is_install(true);
        ui.set_download_title(job.name.into());
        let platform = self.games.iter().find(|game| game.g.id == job.topic).map(|game| game.g.platform);
        // A game not in the catalog goes to the PS5 folder, like any game of unknown platform.
        ui.set_download_path(self.cfg.lock().unwrap().install_path(platform.unwrap_or_default()).into());
        ui.set_download_error("".into());
        ui.set_download_install_note(install_note(platform).into());
        ui.set_download_consent(true);
    }

    pub fn confirm_install(&mut self) {
        let Some(key) = self.install_pending.clone() else { return; };
        let Some(job) = self.downloads.snapshot().into_iter().find(|job| job.key == key && job.state == downloads::State::Complete) else { return; };
        let destination = self.ui().get_download_path().trim().to_owned();
        let ids = self.games.iter().find(|game| game.g.id == job.topic).map(|game| {
            std::iter::once(game.g.title_id.clone()).chain(game.g.title_ids.clone())
                .filter(|id| crate::psn::valid_title_id(id)).collect()
        }).unwrap_or_default();
        let game = self.games.iter().find(|game| game.g.id == job.topic);
        let platform = game.map(|game| game.g.platform).unwrap_or_default();
        // PC games are named after the game (their folder and Library name), not the release.
        let name = match game { Some(game) if platform == crate::platform::Platform::Pc => game.name.clone(), _ => job.name };
        let request = installer::Request { key: key.clone(), topic: job.topic, name,
            source: crate::hostos::resolve_existing(&job.folder),
            destination: crate::hostos::resolve_existing(&util::expand_home(&destination)), expected_ids: ids, platform };
        match self.installer.start(request) {
            Ok(()) => {
                // A path changed here is for this game only: the games folder stays.
                self.install_states.insert(key.clone(), installer::State::Inspecting);
                self.install_pending = None;
                self.push_installs();
                self.open_downloads(Some(&key));
            }
            Err(error) => self.ui().set_download_error(format!("{error:#}").into()),
        }
    }

    pub fn push_installs(&mut self) {
        let records = self.installer.snapshot();
        let mut changed = false;
        let mut register = false;
        for record in &records {
            let previous = self.install_states.insert(record.key.clone(), record.state);
            if previous == Some(record.state) { continue; }
            changed = true;
            if record.state == installer::State::Installed {
                if let Some(path) = record.path.as_ref().filter(|path| crate::library::read_param(path).is_some()) {
                    if let Some(parent) = path.parent() {
                        let mut cfg = self.cfg.lock().unwrap();
                        let folder = parent.to_string_lossy().into_owned();
                        if !cfg.scans(parent) {
                            cfg.game_dirs.push(folder); cfg.save();
                        }
                        register = true;
                    }
                }
                if previous.is_some() { self.toast("Installation complete", &format!("{} · ready in your game library", record.name), 1); }
            } else if previous.is_some() && record.state == installer::State::Failed {
                self.toast("Installation stopped", &format!("{} · open Downloads for details", record.name), 2);
            }
        }
        self.install_states.retain(|key, _| records.iter().any(|record| &record.key == key));
        if register { self.rescan_library(); }
        if changed && self.overlay == Overlay::Hub { self.push_hub(); }
        if let Some(record) = records.iter().find(|record| record.state.active()) {
            self.ui().set_install_badge(format!("{} · {:.0}%", record.state.label(), record.progress() * 100.0).into());
            self.ui().set_install_progress(record.progress());
            self.ui().set_install_active(true);
        } else { self.ui().set_install_active(false); }
    }
}