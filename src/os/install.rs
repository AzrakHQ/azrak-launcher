//! "Install AzrakOS on this PC", on the AzrakOS USB stick only: a welcome that offers to try or
//! install, the disk to install on, a last check before it's erased, the progress, and the end.
//! The work is the stick's installer (system::install); this is its screen, in the launcher's
//! own design and driven by the controller like everything else.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::os::system::{self, Disk, InstallEvent};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Phase {
    /// "Welcome to AzrakOS": try it, or install it.
    #[default]
    Intro = 0,
    Pick = 1,
    Confirm = 2,
    Installing = 3,
    Done = 4,
    Failed = 5,
}

#[derive(Default)]
pub struct OsInstall {
    pub phase: Phase,
    pub disks: Vec<Disk>,
    pub loading: bool,
    /// The focused disk (Pick) or button (the other phases).
    pub sel: usize,
    pub disk: Option<Disk>,
    pub progress: f32,
    pub step: String,
    pub end: Option<InstallEvent>,
    /// "Later" after installing while the launcher sets itself up: its toast waits for Home.
    pub tell_installed: bool,
}

impl App {
    /// Starting from the USB stick: the welcome comes first, before the launcher sets itself up.
    pub fn os_install_welcome(&mut self) {
        self.osi.phase = Phase::Intro;
        self.osi.sel = 0;
        self.push_overlay(Overlay::OsInstall, Z_OS_INSTALL, 0);
        self.osi_push();
    }

    /// Settings → Install AzrakOS on this PC: straight to choosing the disk.
    pub fn os_install_open(&mut self) {
        audio::play(Sound::Select);
        self.push_overlay(Overlay::OsInstall, Z_OS_INSTALL, 0);
        self.osi_pick();
    }

    fn osi_pick(&mut self) {
        self.osi.phase = Phase::Pick;
        self.osi.sel = 0;
        self.osi.loading = true;
        self.osi_push();
        std::thread::spawn(|| {
            let disks = system::install_disks();
            let _ = slint::invoke_from_event_loop(move || with_app(move |app| {
                app.osi.loading = false;
                match disks {
                    Ok(d) => app.osi.disks = d,
                    Err(e) => {
                        app.osi.disks.clear();
                        app.toast("Couldn't list the disks", &e, 2);
                    }
                }
                app.osi.sel = first_choice(&app.osi.disks);
                app.osi_push();
            }));
        });
    }

    /// The buttons under each phase (Pick: after the disks).
    fn osi_buttons(&self) -> &'static [&'static str] {
        match self.osi.phase {
            Phase::Intro => &["Install on this PC", "Try AzrakOS"],
            Phase::Pick if self.osi_no_disk() => &["Try again", "Back"],
            Phase::Pick => &["Back"],
            Phase::Confirm => &["Erase and install", "Back"],
            Phase::Installing => &[],
            Phase::Done => &["Restart", "Later"],
            Phase::Failed => &["Try again", "Back"],
        }
    }

    /// Looked, and there's no disk to install on (the stick itself is never offered).
    fn osi_no_disk(&self) -> bool {
        self.osi.phase == Phase::Pick && !self.osi.loading && self.osi.disks.is_empty()
    }

    /// What can be focused: the disks (Pick), then the buttons.
    fn osi_count(&self) -> usize {
        let disks = if self.osi.phase == Phase::Pick { self.osi.disks.len() } else { 0 };
        disks + self.osi_buttons().len()
    }

    pub fn act_os_install(&mut self, a: Act) {
        let n = self.osi_count();
        match a {
            Act::Up | Act::Left if self.osi.sel > 0 => {
                self.osi.sel -= 1;
                audio::play(Sound::Move);
                self.osi_push();
            }
            Act::Down | Act::Right if self.osi.sel + 1 < n => {
                self.osi.sel += 1;
                audio::play(Sound::Move);
                self.osi_push();
            }
            Act::Confirm => self.osi_choose(self.osi.sel),
            Act::Back => self.osi_back(),
            _ => {}
        }
    }

    pub fn click_os_install(&mut self, idx: i32) {
        if idx >= 0 && (idx as usize) < self.osi_count() {
            self.osi.sel = idx as usize;
            self.osi_choose(idx as usize);
        }
    }

    /// Leave for the launcher: on the stick, the launcher starts now that AzrakOS is tried.
    fn osi_leave(&mut self) {
        self.back();
        if let Some((first_run, stale)) = self.held_start.take() {
            self.start_launcher(first_run, stale);
        }
    }

    /// Back to the welcome while the launcher hasn't started (there's nothing behind it yet).
    fn osi_welcome_again(&mut self) {
        audio::play(Sound::Back);
        self.osi.phase = Phase::Intro;
        self.osi.sel = 0;
        self.osi_push();
    }

    /// Installed, restarting later: say once how to start it, where it's seen (on Home, once the
    /// launcher's setup is done when it's only starting now).
    fn osi_later(&mut self) {
        self.osi_leave();
        if self.boot.active {
            self.osi.tell_installed = true;
        } else {
            self.osi_tell_installed();
        }
    }

    pub fn osi_tell_installed(&mut self) {
        self.osi.tell_installed = false;
        self.toast("AzrakOS is installed", "Restart without the USB stick to start it.", 1);
    }

    fn osi_back(&mut self) {
        match self.osi.phase {
            Phase::Installing => {}
            Phase::Intro => self.osi_leave(),
            Phase::Pick | Phase::Failed if self.held_start.is_some() => self.osi_welcome_again(),
            Phase::Confirm => {
                audio::play(Sound::Back);
                let i = self.osi.disk.as_ref().and_then(|d| self.osi.disks.iter().position(|x| x == d)).unwrap_or(0);
                self.osi.phase = Phase::Pick;
                self.osi.sel = i;
                self.osi_push();
            }
            Phase::Done => self.osi_later(),
            _ => self.osi_leave(),
        }
    }

    fn osi_choose(&mut self, i: usize) {
        match self.osi.phase {
            Phase::Intro => {
                if i == 0 {
                    audio::play(Sound::Select);
                    self.osi_pick();
                } else {
                    audio::play(Sound::Start);
                    self.osi_leave();
                }
            }
            Phase::Pick => match self.osi.disks.get(i).cloned() {
                Some(d) => {
                    audio::play(Sound::Select);
                    self.osi.disk = Some(d);
                    self.osi.phase = Phase::Confirm;
                    // Back is the safe default: erasing takes a deliberate move.
                    self.osi.sel = 1;
                    self.osi_push();
                }
                // No disk: look again (after connecting one).
                None if self.osi_no_disk() && i == 0 => {
                    audio::play(Sound::Select);
                    self.osi_pick();
                }
                None => self.osi_back(),
            },
            Phase::Confirm => {
                if i == 0 {
                    self.osi_start();
                } else {
                    self.osi_back();
                }
            }
            Phase::Installing => {}
            Phase::Done => {
                if i == 0 {
                    if let Err(e) = system::restart() {
                        self.toast("Couldn't restart", &e, 2);
                    }
                } else {
                    self.osi_later();
                }
            }
            Phase::Failed => {
                if i == 0 {
                    self.osi_pick();
                } else {
                    self.osi_back();
                }
            }
        }
    }

    fn osi_start(&mut self) {
        let Some(disk) = self.osi.disk.clone() else { return };
        if !self.live.is_empty() {
            self.toast("A game is running", "Close it first, then install.", 2);
            return;
        }
        audio::play(Sound::Start);
        crate::log!("installing AzrakOS on {}", disk.path);
        self.osi.phase = Phase::Installing;
        self.osi.progress = 0.0;
        self.osi.step = "Getting ready".into();
        self.osi.end = None;
        self.osi_push();
        std::thread::spawn(move || {
            let end = system::install(&disk.path, |e| {
                let _ = slint::invoke_from_event_loop(move || with_app(move |app| {
                    if let InstallEvent::Progress(p, text) = e {
                        app.osi.progress = app.osi.progress.max(p);
                        app.osi.step = text;
                        app.osi_push();
                    }
                }));
            });
            let _ = slint::invoke_from_event_loop(move || with_app(move |app| {
                crate::log!("installing AzrakOS: {end:?}");
                app.osi.phase = if matches!(end, InstallEvent::Done { .. }) { Phase::Done } else { Phase::Failed };
                app.osi.progress = 1.0;
                app.osi.sel = 0;
                app.osi.end = Some(end);
                audio::play(if app.osi.phase == Phase::Done { Sound::Start } else { Sound::Error });
                app.osi_push();
            }));
        });
    }

    /// Show the current phase.
    pub fn osi_push(&self) {
        let ui = self.ui();
        let s = &self.osi;
        let disk = s.disk.as_ref();
        // Under the title: the disk's name on the last check (a long model name made the title
        // fill the screen). The text turns bold and amber when Windows is about to go.
        let mut sub = String::new();
        let mut text_warn = false;
        // Secure Boot's key screen comes at the restart: its steps get their own panel.
        let mut mok = false;
        let (title, text, note): (String, String, String) = match s.phase {
            Phase::Intro => ("Welcome to AzrakOS".into(),
                "You're running it from the USB stick, and nothing on this PC has changed. Try it as long as you like, or install it so the PC starts straight into it.".into(),
                String::new()),
            Phase::Pick if self.osi_no_disk() => ("No disk to install on".into(),
                "AzrakOS needs a disk of 40 GB or more (not the USB stick). Connect one, then try again.".into(),
                String::new()),
            Phase::Pick => ("Where should AzrakOS go?".into(),
                if s.loading { "Looking at this PC's disks…".into() } else { "Everything on the disk you choose is erased.".into() },
                String::new()),
            Phase::Confirm => {
                let d = disk.cloned().unwrap_or_else(|| Disk { path: String::new(), bytes: 0, model: "Disk".into(), windows: false, usb: false });
                sub = format!("{} · {}", d.model, d.size());
                text_warn = d.windows;
                ("Erase this disk?".into(),
                 if d.windows { "Windows and everything on this disk will be deleted.".into() }
                 else { "AzrakOS replaces everything on this disk.".into() },
                 "This can't be undone.".into())
            }
            Phase::Installing => ("Installing AzrakOS".into(), s.step.clone(), "This takes a few minutes. Keep the PC on.".into()),
            Phase::Done => {
                let nvidia_later = match &s.end {
                    Some(InstallEvent::Done { mok: m, nvidia_later, .. }) => { mok = *m; *nvidia_later }
                    _ => false,
                };
                ("AzrakOS is installed".into(),
                 "Remove the USB stick, then restart: the PC starts straight into AzrakOS.".into(),
                 // With the key screen, its panel says what the driver needs instead.
                 if nvidia_later && !mok { "This PC has an NVIDIA graphics card: its driver downloads by itself the first time the PC is online, then asks for a restart.".into() }
                 else { String::new() })
            }
            Phase::Failed => ("Installation didn't finish".into(),
                match &s.end { Some(InstallEvent::Failed(e)) => e.clone(), _ => "Something went wrong.".into() },
                "Try again, or keep using AzrakOS from the USB stick.".into()),
        };
        ui.set_osi_phase(s.phase as i32);
        ui.set_osi_title(title.into());
        ui.set_osi_sub(sub.into());
        ui.set_osi_text(text.into());
        ui.set_osi_text_warn(text_warn);
        ui.set_osi_mok(mok);
        ui.set_osi_note(note.into());
        ui.set_osi_sel(s.sel as i32);
        ui.set_osi_progress(s.progress);
        ui.set_osi_busy(s.loading);
        let rows: Vec<crate::OsiDisk> = s.disks.iter().map(|d| crate::OsiDisk {
            label: d.model.clone().into(),
            // A USB disk here is never the stick itself (the installer leaves it out).
            detail: [Some(d.size()), d.usb.then(|| "External drive · USB".to_string()), d.windows.then(|| "Windows is on it".to_string())]
                .into_iter().flatten().collect::<Vec<_>>().join(" · ").into(),
            warn: d.windows,
        }).collect();
        ui.set_osi_disks(std::rc::Rc::new(slint::VecModel::from(rows)).into());
        let buttons: Vec<slint::SharedString> = self.osi_buttons().iter().map(|b| (*b).into()).collect();
        ui.set_osi_buttons(std::rc::Rc::new(slint::VecModel::from(buttons)).into());
        ui.set_osi_button_base(if s.phase == Phase::Pick { s.disks.len() as i32 } else { 0 });
    }
}

/// The disk focused first. Never one with Windows on it, so a quick ✕ doesn't head for it; an
/// internal disk before a USB one (more likely someone's backup drive). With Windows on every
/// disk, Back.
fn first_choice(disks: &[Disk]) -> usize {
    disks.iter().position(|d| !d.windows && !d.usb)
        .or_else(|| disks.iter().position(|d| !d.windows))
        .unwrap_or(disks.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(windows: bool, usb: bool) -> Disk {
        Disk { path: "/dev/sda".into(), bytes: 500_000_000_000, model: "Disk".into(), windows, usb }
    }

    #[test]
    fn never_focuses_windows_first() {
        assert_eq!(first_choice(&[disk(true, false), disk(false, false), disk(false, true)]), 1);
        assert_eq!(first_choice(&[disk(false, true), disk(true, false), disk(false, false)]), 2);
        assert_eq!(first_choice(&[disk(true, false), disk(false, true)]), 1);
        // Only Windows disks: Back (the button after them).
        assert_eq!(first_choice(&[disk(true, false), disk(true, true)]), 2);
        // None at all: the first button (Try again).
        assert_eq!(first_choice(&[]), 0);
    }
}
