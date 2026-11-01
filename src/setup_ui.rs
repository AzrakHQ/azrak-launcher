//! Download & install: the panel that slides in when Download is pressed. It looks the torrent up
//! straight away and shows its files with a checkbox each, where it downloads and installs, and
//! the space that takes on each drive. Download & install starts the download, and the game
//! installs by itself when it's done. Closing the panel forgets a download that never started.
use crate::app::*;
use crate::audio::{self, Sound};
use crate::downloads::{self, State};
use crate::util;
use slint::Model;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus { Row(usize), ChangeDownload, ChangeInstall, Go, Cancel }

pub struct Setup {
    /// The transfer being looked up (none if it couldn't even start).
    pub key: Option<String>,
    pub platform: crate::platform::Platform,
    /// Whether this panel made the transfer (closing it then forgets the transfer).
    pub created: bool,
    pub entries: Vec<downloads::Entry>,
    /// Ticked files, one per entry.
    pub on: Vec<bool>,
    /// Why it couldn't be looked up.
    pub error: String,
    /// Why the download couldn't start.
    pub begin_error: String,
    pub focus: Focus,
    /// Focus was moved by the player (so it isn't moved for them when the files arrive).
    pub moved: bool,
    /// Which folder is being typed: 1 download, 2 install.
    pub edit: i32,
    pub begun: bool,
}

/// "21.6 GB", "84 MB", "2 KB".
pub fn size_label(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes >= 1 << 30 { format!("{:.1} GB", b / (1u64 << 30) as f64) }
    else if bytes >= 1 << 20 { format!("{:.0} MB", b / (1u64 << 20) as f64) }
    else { format!("{} KB", (bytes / 1024).max(1)) }
}

/// What the panel shows, from the transfer and the player's choices.
struct View {
    state: i32,
    message_title: String,
    message: String,
    total: u64,
    picked: usize,
    choosable: usize,
    notice: String,
    notice_kind: i32,
    go_label: String,
    go_enabled: bool,
    dl: (String, f32, bool),
    inst: (String, f32, bool),
}

impl App {
    /// Download pressed: open the panel and look the torrent up.
    pub fn open_setup(&mut self, target: Target) {
        let Some(index) = target.game else { return };
        let game = &self.games[index];
        let (topic, name, magnet, platform) = (game.g.id, game.name.clone(), game.g.magnet.clone(), game.g.platform);
        let existing = self.download_for_target(target);
        let (download_dir, install_dir) = { let cfg = self.cfg.lock().unwrap(); (cfg.download_dir.clone(), cfg.install_dir.clone()) };
        let (key, created, error) = match existing {
            Some(job) => (Some(job.key), false, String::new()),
            None => match self.downloads.prepare(topic, &name, &magnet, util::expand_home(&download_dir), platform) {
                Ok(key) => (Some(key), true, String::new()),
                Err(error) => (None, false, format!("{error:#}")),
            },
        };
        self.setup = Some(Setup { key, platform, created, entries: vec![], on: vec![], error, begin_error: String::new(),
            focus: Focus::Cancel, moved: false, edit: 0, begun: false });
        self.setup_model.set_vec(vec![]);
        let ui = self.ui();
        ui.set_setup_title(name.into());
        ui.set_setup_dl_path(download_dir.into());
        ui.set_setup_inst_path(install_dir.into());
        ui.set_setup_edit(0);
        ui.set_setup_editing(false);
        self.push_overlay(Overlay::Setup, Z_SETUP, 0);
        audio::play(Sound::Select);
        self.push_setup();
    }

    /// Closing the panel (Back, Cancel, or Download & install): forget a lookup that never started.
    pub fn setup_closed(&mut self) {
        if let Some(setup) = self.setup.take() {
            if !setup.begun && setup.created {
                if let Some(key) = setup.key { let _ = self.downloads.discard(&key); }
            }
        }
        let ui = self.ui();
        ui.set_setup_edit(0);
        ui.set_setup_editing(false);
        ui.invoke_focus_root();
    }

    fn setup_view(&self) -> Option<View> {
        let setup = self.setup.as_ref()?;
        let job = setup.key.as_ref().and_then(|key| self.downloads.snapshot().into_iter().find(|job| &job.key == key));
        let ui = self.ui();
        let total: u64 = setup.entries.iter().zip(&setup.on).filter(|(_, on)| **on).map(|(e, _)| e.size).sum();
        let picked = setup.on.iter().filter(|on| **on).count();
        let choosable = setup.entries.iter().filter(|e| !e.blocked).count();
        let mut view = View { state: 0, message_title: String::new(), message: String::new(), total, picked, choosable,
            notice: String::new(), notice_kind: 0, go_label: "Getting the file list…".into(), go_enabled: false,
            dl: (String::new(), 0.0, false), inst: (String::new(), 0.0, false) };
        let failed = |title: &str, message: &str| (title.to_string(), message.to_string());
        let error = if !setup.error.is_empty() { Some(setup.error.clone()) }
            else { job.as_ref().filter(|job| job.state == State::Failed && setup.entries.is_empty()).map(|job| job.error.clone()) };
        if let Some(error) = error {
            // The release itself is the problem (it can't be installed, or its torrent is unsafe),
            // or else the file list just didn't arrive (nobody answered, or this PC is offline).
            const RELEASE: [&str; 9] = ["Azrak can't install", "Nothing in this release", "Torrent", "Unsafe", "Invalid", "Symlink",
                "Duplicate", "Conflicting", "Metadata info hash"];
            let nobody = !RELEASE.iter().any(|start| error.starts_with(start));
            let (title, message) = if nobody {
                view.go_label = "Keep trying".into();
                view.go_enabled = true;
                failed("The file list didn't arrive", "No other player answered, or this PC is offline. Keep trying, or close and pick a release with more players in the Game Hub.")
            } else if error.starts_with("Azrak can't install") {
                view.go_label = "Can't install this release".into();
                failed("This release can't be installed", &error)
            } else {
                view.go_label = "Can't download this release".into();
                failed("This release can't be downloaded", &error)
            };
            view.state = if nobody { 2 } else { 3 };
            view.message_title = title;
            view.message = message;
        } else if !setup.entries.is_empty() {
            view.state = 1;
        }
        // Where it goes, and the space on each drive.
        let (dl_path, inst_path) = (util::expand_home(ui.get_setup_dl_path().trim()), util::expand_home(ui.get_setup_inst_path().trim()));
        let known = view.state == 1;
        let need = if known { total } else { 0 };
        let mut short = None;
        let dl_drive = dl_path.is_absolute().then(|| downloads::drive(&dl_path)).flatten();
        let inst_drive = inst_path.is_absolute().then(|| downloads::drive(&inst_path)).flatten();
        view.dl = match &dl_drive {
            None => ("Use a full folder path, like ~/Downloads".into(), 0.0, true),
            Some(d) if !known => (format!("{} free on {}", size_label(d.free), d.mount.display()), 0.0, false),
            Some(d) => {
                if need > d.free { short = Some((d.mount.clone(), need - d.free, "the download", "choose another folder")); }
                (format!("Needs {} · {} free on {}", size_label(need), size_label(d.free), d.mount.display()), need as f32 / d.free.max(1) as f32, need > d.free)
            }
        };
        view.inst = match &inst_drive {
            None => ("Use a full folder path, like ~/Games".into(), 0.0, true),
            Some(d) if !known => (format!("{} free on {}", size_label(d.free), d.mount.display()), 0.0, false),
            Some(d) => {
                // On the same drive as the download, the download takes its space first.
                let same = dl_drive.as_ref().is_some_and(|dl| dl.dev == d.dev);
                let free = if same { d.free.saturating_sub(need) } else { d.free };
                if need > free && short.is_none() { short = Some((d.mount.clone(), need - free, "the installed game", "install to another drive")); }
                let after = if same { " after the download" } else { "" };
                (format!("Needs about {} · {} free on {}{after}", size_label(need), size_label(free), d.mount.display()), need as f32 / free.max(1) as f32, need > free)
            }
        };
        if known {
            let blocked: Vec<&downloads::Entry> = setup.entries.iter().filter(|e| e.blocked).collect();
            let biggest = setup.entries.iter().enumerate().max_by_key(|(_, e)| e.size).map(|(i, e)| (i, e.path.rsplit('/').next().unwrap_or(&e.path).to_string()));
            let console = if setup.platform == crate::platform::Platform::Ps5 { "PS5" } else { "PS4" };
            if let Some((mount, missing, what, fix)) = &short {
                view.notice = format!("Not enough space on {} for {what}. Free up {}, or {fix}.", mount.display(), size_label(*missing));
                view.notice_kind = 2;
            } else if blocked.len() == 1 {
                let name = blocked[0].path.rsplit('/').next().unwrap_or(&blocked[0].path);
                view.notice = format!("{name} was left out. It's a Windows program, and {console} games never need one.");
                view.notice_kind = 1;
            } else if blocked.len() > 1 {
                view.notice = format!("{} Windows programs were left out. {console} games never need them.", blocked.len());
                view.notice_kind = 1;
            } else if let Some((i, name)) = biggest.filter(|(i, _)| picked > 0 && !setup.on[*i]) {
                let _ = i;
                view.notice = format!("Without {name} the game probably won't install.");
                view.notice_kind = 1;
            } else if let Some(warning) = Some(self.pc_download_warning(setup.platform)).filter(|w| !w.is_empty()) {
                view.notice = warning;
                view.notice_kind = 1;
            }
            let paths_bad = view.dl.2 && dl_drive.is_none() || view.inst.2 && inst_drive.is_none();
            (view.go_label, view.go_enabled) = if picked == 0 { ("Choose at least one file".into(), false) }
                else if short.is_some() { ("Not enough space".into(), false) }
                else if paths_bad { ("Choose the folders".into(), false) }
                else { (format!("Download & install · {}", size_label(total)), true) };
        }
        if !setup.begin_error.is_empty() && view.state == 1 {
            view.notice = setup.begin_error.clone();
            view.notice_kind = 2;
        }
        Some(view)
    }

    /// Draw the panel from the transfer (called every second, and after every change).
    pub fn push_setup(&mut self) {
        if self.overlay != Overlay::Setup { return; }
        // The file list arrived: every file the player can choose starts ticked.
        if let Some(setup) = self.setup.as_mut() {
            if setup.entries.is_empty() {
                let job = setup.key.as_ref().and_then(|key| self.downloads.snapshot().into_iter().find(|job| &job.key == key));
                if let Some(job) = job.filter(|job| job.state == State::Ready && !job.entries.is_empty()) {
                    setup.on = job.entries.iter().enumerate()
                        .map(|(i, e)| !e.blocked && job.selected.as_ref().is_none_or(|s| s.contains(&i))).collect();
                    setup.entries = job.entries;
                }
            }
        }
        let Some(view) = self.setup_view() else { return };
        let setup = self.setup.as_mut().unwrap();
        if !setup.moved {
            setup.focus = if view.go_enabled { Focus::Go } else { Focus::Cancel };
        }
        if let Focus::Row(i) = setup.focus {
            if view.state != 1 || setup.entries.is_empty() { setup.focus = Focus::Cancel; }
            else if i >= setup.entries.len() { setup.focus = Focus::Row(setup.entries.len() - 1); }
        }
        if setup.focus == Focus::Go && !view.go_enabled { setup.focus = Focus::Cancel; }
        let rows: Vec<crate::SetupFile> = if view.state == 1 {
            setup.entries.iter().zip(&setup.on).map(|(e, on)| {
                let (dir, name) = match e.path.rsplit_once('/') { Some((dir, name)) => (format!("{dir}/"), name.to_string()), None => (String::new(), e.path.clone()) };
                crate::SetupFile { dir: dir.into(), name: name.into(), size: if e.blocked { "Left out".into() } else { size_label(e.size).into() }, on: *on, blocked: e.blocked }
            }).collect()
        } else { vec![] };
        if self.setup_model.row_count() != rows.len() {
            self.setup_model.set_vec(rows);
        } else {
            for (i, row) in rows.into_iter().enumerate() {
                if self.setup_model.row_data(i).as_ref() != Some(&row) { self.setup_model.set_row_data(i, row); }
            }
        }
        let (focus, edit, platform) = (setup.focus, setup.edit, setup.platform);
        let vpn = {
            let cfg = self.cfg.lock().unwrap();
            match cfg.download_vpn() {
                None => ("No VPN: other players can see your IP address".to_string(), false),
                Some(preferred) => match crate::vpn::active(&preferred) {
                    Some(iface) => (format!("Through your VPN ({iface})"), true),
                    None => ("Waiting for your VPN to connect".to_string(), false),
                },
            }
        };
        let _ = platform;
        // Over the Game Hub, the panel is glass over its art.
        let over_hub = self.stack.iter().any(|(overlay, _, _)| *overlay == Overlay::Hub);
        let glass = if over_hub { let key = self.bg_key.clone(); self.frost_of(&key) } else { None };
        let ui = self.ui();
        ui.set_setup_has_glass(glass.is_some());
        ui.set_setup_glass(glass.unwrap_or_default());
        ui.set_setup_state(view.state);
        ui.set_setup_message_title(view.message_title.into());
        ui.set_setup_message(view.message.into());
        ui.set_setup_count(if view.state == 1 { format!("{} of {} files", view.picked, view.choosable) } else { String::new() }.into());
        ui.set_setup_total(if view.state == 1 { size_label(view.total) } else { String::new() }.into());
        ui.set_setup_all_state(if view.picked == 0 { 0 } else if view.picked == view.choosable { 2 } else { 1 });
        ui.set_setup_notice(view.notice.into());
        ui.set_setup_notice_kind(view.notice_kind);
        ui.set_setup_vpn(vpn.0.into());
        ui.set_setup_vpn_ok(vpn.1);
        ui.set_setup_dl_line(view.dl.0.into());
        ui.set_setup_dl_frac(view.dl.1);
        ui.set_setup_dl_bad(view.dl.2);
        ui.set_setup_inst_line(view.inst.0.into());
        ui.set_setup_inst_frac(view.inst.1);
        ui.set_setup_inst_bad(view.inst.2);
        ui.set_setup_go_label(view.go_label.into());
        ui.set_setup_go_enabled(view.go_enabled);
        ui.set_setup_focus_row(if let Focus::Row(i) = focus { i as i32 } else { -1 });
        ui.set_setup_focus_item(match focus { Focus::Row(_) => 0, Focus::ChangeDownload => 1, Focus::ChangeInstall => 2, Focus::Go => 3, Focus::Cancel => 4 });
        ui.set_setup_edit(edit);
    }

    fn setup_focus(&mut self, focus: Focus) {
        if let Some(setup) = self.setup.as_mut() {
            if setup.focus != focus { audio::play(Sound::Move); }
            setup.focus = focus;
            setup.moved = true;
        }
        self.push_setup();
    }

    pub fn act_setup(&mut self, a: Act) {
        let Some(setup) = self.setup.as_ref() else { return self.back() };
        let rows = if self.ui().get_setup_state() == 1 { setup.entries.len() } else { 0 };
        let go = self.ui().get_setup_go_enabled();
        let bottom = if go { Focus::Go } else { Focus::Cancel };
        let next = match (a, setup.focus) {
            (Act::Back, _) => return self.back(),
            (Act::Search, _) => return self.setup_toggle(-1),
            (Act::Up, Focus::Row(i)) => Focus::Row(i.saturating_sub(1)),
            (Act::Up, Focus::ChangeDownload) if rows > 0 => Focus::Row(rows - 1),
            (Act::Up, Focus::ChangeInstall) => Focus::ChangeDownload,
            (Act::Up, Focus::Go | Focus::Cancel) => Focus::ChangeInstall,
            (Act::Down, Focus::Row(i)) if i + 1 < rows => Focus::Row(i + 1),
            (Act::Down, Focus::Row(_)) => Focus::ChangeDownload,
            (Act::Down, Focus::ChangeDownload) => Focus::ChangeInstall,
            (Act::Down, Focus::ChangeInstall) => bottom,
            (Act::Left, Focus::Cancel) if go => Focus::Go,
            (Act::Right, Focus::Go) => Focus::Cancel,
            (Act::PageUp, Focus::Row(i)) => Focus::Row(i.saturating_sub(8)),
            (Act::PageDown, Focus::Row(i)) => Focus::Row((i + 8).min(rows.saturating_sub(1))),
            (Act::First, _) if rows > 0 => Focus::Row(0),
            (Act::Last, _) => bottom,
            (Act::Confirm, Focus::Row(i)) => return self.setup_toggle(i as i32),
            (Act::Confirm, Focus::ChangeDownload) => return self.setup_change(1),
            (Act::Confirm, Focus::ChangeInstall) => return self.setup_change(2),
            (Act::Confirm, Focus::Go) => return self.setup_go(),
            (Act::Confirm, Focus::Cancel) => return self.back(),
            (_, focus) => focus,
        };
        self.setup_focus(next);
    }

    /// Tick or untick a file (-1: all of them).
    pub fn setup_toggle(&mut self, index: i32) {
        let Some(setup) = self.setup.as_mut() else { return };
        if setup.entries.is_empty() { return; }
        if index < 0 {
            let all = setup.entries.iter().zip(&setup.on).all(|(e, on)| e.blocked || *on);
            for (on, e) in setup.on.iter_mut().zip(&setup.entries) { *on = !all && !e.blocked; }
        } else if let Some(i) = Some(index as usize).filter(|&i| i < setup.entries.len() && !setup.entries[i].blocked) {
            setup.on[i] = !setup.on[i];
            setup.focus = Focus::Row(i);
            setup.moved = true;
        }
        setup.begin_error.clear();
        audio::play(Sound::Move);
        self.push_setup();
    }

    /// Change: type the download (1) or install (2) folder.
    pub fn setup_change(&mut self, which: i32) {
        if let Some(setup) = self.setup.as_mut() {
            setup.edit = which;
            setup.focus = if which == 1 { Focus::ChangeDownload } else { Focus::ChangeInstall };
            setup.moved = true;
        }
        audio::play(Sound::Select);
        self.push_setup();
    }

    pub fn setup_edit_done(&mut self) {
        if let Some(setup) = self.setup.as_mut() { setup.edit = 0; }
        let ui = self.ui();
        ui.set_setup_editing(false);
        ui.invoke_focus_root();
        self.push_setup();
    }

    /// Download & install (or Keep trying, when nobody answered).
    pub fn setup_go(&mut self) {
        let Some(view) = self.setup_view() else { return };
        if !view.go_enabled { return; }
        let Some(key) = self.setup.as_ref().and_then(|setup| setup.key.clone()) else { return };
        if view.state == 2 {
            if let Some(setup) = self.setup.as_mut() { setup.moved = false; }
            let _ = self.downloads.action(&key, "retry");
            audio::play(Sound::Select);
            self.push_setup();
            return;
        }
        let selected: Vec<usize> = self.setup.as_ref().unwrap().on.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect();
        let ui = self.ui();
        let (download_dir, install_dir) = (ui.get_setup_dl_path().trim().to_string(), ui.get_setup_inst_path().trim().to_string());
        match self.downloads.begin(&key, util::expand_home(&download_dir), selected, Some(util::expand_home(&install_dir))) {
            Ok(()) => {
                { let mut cfg = self.cfg.lock().unwrap(); cfg.download_dir = download_dir; cfg.install_dir = install_dir; cfg.save(); }
                if let Some(setup) = self.setup.as_mut() { setup.begun = true; }
                audio::play(Sound::Select);
                self.back();
                self.dl_filter = 0;
                self.open_downloads(Some(&key));
            }
            Err(error) => {
                if let Some(setup) = self.setup.as_mut() { setup.begin_error = format!("{error:#}"); }
                self.push_setup();
            }
        }
    }
}
