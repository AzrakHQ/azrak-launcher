//! Settings sheet: sections on the left, the chosen one's rows on the right; controller-friendly
//! editing, instant save.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::config::{PRESENT_MODES, RESOLUTIONS};
use crate::util;
use crate::{SettingData, SettingSection};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SId {
    Header,
    // System (AzrakOS only; see os/system_ui.rs)
    Internet,
    WifiNet(u16),
    WifiMore,
    BtPair,
    BtFound(u16),
    BtDevice(u16),
    SoundOutput,
    SystemUpdate,
    InstallOs,
    Restart,
    ShutDown,
    // Games
    Dirs,
    InstallDir,
    DownloadDir,
    Rescan,
    // Playing
    Fullscreen,
    ReturnOnExit,
    Controls,
    ThisPc,
    // Downloads
    VpnOnly,
    Vpn,
    VpnAdd,
    SeedCompleted,
    Downloads,
    // Updates
    AppUpdate,
    KytyUpdate,
    ShadUpdate,
    AutoUpdate,
    KytyRollback,
    ShadRollback,
    // Launcher
    Display,
    Sounds,
    Share,
    // Advanced
    Emulator,
    Resolution,
    Present,
    Amd,
    Extra,
    Refresh,
    Quit,
}

fn row(kind: i32, label: &str) -> SettingData {
    SettingData { kind, label: label.into(), ..Default::default() }
}

/// What Settings shows about VPNs, to notice when one connects or drops.
pub fn vpn_signature() -> String {
    crate::vpn::list().iter().map(|v| format!("{}:{} ", v.name, v.connected)).collect()
}

/// Where a row went after Settings was rebuilt: the same row and name; else the same kind of row
/// with that name (a device that moved in the list); else the same row.
fn same_row(row: (SId, &str), rows: &[(SId, &str)]) -> Option<usize> {
    let kind = std::mem::discriminant(&row.0);
    rows.iter().position(|r| *r == row)
        .or_else(|| rows.iter().position(|r| std::mem::discriminant(&r.0) == kind && r.1 == row.1))
        .or_else(|| rows.iter().position(|r| r.0 == row.0))
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

impl App {
    /// Sections in the order players need them; expert options folded under Advanced.
    pub fn build_settings(&mut self) {
        let cfg = self.cfg.lock().unwrap().clone();
        let mut rows: Vec<(SId, SettingData)> = Vec::new();
        let header = |rows: &mut Vec<(SId, SettingData)>, label: &str| rows.push((SId::Header, row(0, label)));

        rows.extend(self.system_rows());
        header(&mut rows, "GAMES");
        let mut r = row(1, "Game folders");
        r.value = cfg.game_dirs.iter().map(|d| util::display_path(d)).collect::<Vec<_>>().join("; ").into();
        r.hint = format!("{} found · more than one folder? Put a ; between them", plural(self.locals.len(), "installed game")).into();
        rows.push((SId::Dirs, r));
        let mut r = row(1, "Install new games to");
        r.value = util::display_path(&cfg.install_dir).into();
        r.hint = "Downloaded games are set up here when you press Install".into();
        rows.push((SId::InstallDir, r));
        let mut r = row(1, "Save downloads to");
        r.value = util::display_path(&cfg.download_dir).into();
        r.hint = "Each download gets its own folder".into();
        rows.push((SId::DownloadDir, r));
        rows.push((SId::Rescan, row(4, "Rescan game folders")));

        header(&mut rows, "PLAYING");
        let mut r = row(2, "Play games in fullscreen");
        r.on = cfg.fullscreen;
        rows.push((SId::Fullscreen, r));
        let mut r = row(2, "Return to the launcher when a game closes");
        r.on = cfg.return_on_exit;
        rows.push((SId::ReturnOnExit, r));
        let connected = !crate::gamepad::connected().is_empty();
        let mut r = row(4, "Controller & keyboard");
        r.value = crate::gamepad::status().into();
        r.value_kind = if connected { 1 } else { 0 };
        r.hint = if connected { "Your controller works in games with no setup" } else { "Games play best with a controller · open to see the keyboard keys" }.into();
        rows.push((SId::Controls, r));
        rows.push((SId::ThisPc, self.this_pc_row()));

        header(&mut rows, "DOWNLOADS");
        // VPN: the toggle keeps downloads off every other connection; the row below manages the
        // VPN itself through NetworkManager, or just reports one that another app connected.
        self.vpn_seen = vpn_signature();
        let connected = crate::vpn::active(&cfg.vpn_interface);
        let nm = crate::vpn::nm_available();
        let saved = if nm { crate::vpn::nm_saved() } else { Vec::new() };
        let mut r = row(2, "Download only through a VPN");
        r.on = cfg.vpn_only;
        (r.hint, r.hint_kind) = match (&connected, cfg.vpn_only) {
            (_, false) => ("Hides your IP address from other players · uses your own VPN".into(), 0),
            (Some(iface), true) => (format!("Protected · downloads only use {iface}").into(), 1),
            (None, true) => ("Downloads wait until your VPN is connected".into(), 2),
        };
        rows.push((SId::VpnOnly, r));
        let mut r = row(4, "Your VPN");
        let active_saved = saved.iter().find(|v| v.active);
        (r.value, r.value_kind, r.hint) = if !self.vpn_busy.is_empty() {
            (self.vpn_busy.clone().into(), 0, "".into())
        } else if let Some(v) = active_saved {
            (format!("{} · Connected", v.name).into(), 1, "Press to disconnect".into())
        } else if let Some(iface) = &connected {
            (format!("{iface} · Connected").into(), 1, "Connected by your VPN app".into())
        } else if let Some(v) = saved.first() {
            (format!("{} · Connect", v.name).into(), 3, "Not connected".into())
        } else if nm {
            ("Add one".into(), 3, "From your VPN provider: a WireGuard .conf or OpenVPN .ovpn file".into())
        } else {
            ("None".into(), 0, "Connect your VPN app, and downloads use it".into())
        };
        rows.push((SId::Vpn, r));
        if nm && !saved.is_empty() {
            let dialog = crate::vpn::has_file_dialog();
            let mut r = row(if dialog { 4 } else { 1 }, "Add a VPN from a file");
            r.value = if dialog { "WireGuard .conf · OpenVPN .ovpn".into() } else { "".into() };
            r.hint = if dialog { "Download the file from your VPN provider's website" } else { "Type the path of a WireGuard .conf or OpenVPN .ovpn file" }.into();
            rows.push((SId::VpnAdd, r));
        }
        let mut r = row(2, "Keep sharing finished downloads");
        r.on = cfg.seed_after_download;
        r.hint = "Helps other players download faster · uploads at most 128 KiB/s".into();
        rows.push((SId::SeedCompleted, r));
        // No controller button opens Downloads (the top bar's icon does), so a controller player
        // just presses this row; Ctrl+D is only worth showing to keyboard players.
        let mut r = row(4, "Open downloads");
        r.value = if self.pad_hints { "Open".into() } else { "Open · Ctrl+D".into() };
        rows.push((SId::Downloads, r));

        header(&mut rows, "UPDATES");
        // Azrak Launcher
        let cur = crate::update::current_version();
        let latest = self.upd.latest.as_ref().map(|r| r.version.clone());
        let mut r = row(4, "Azrak Launcher");
        r.hint = "This app".into();
        let (value, kind) = if let Some(v) = &self.upd.installed {
            (format!("Restart to finish · {v}"), 3)
        } else if self.upd.busy {
            (self.upd.progress.replace("Downloading Azrak Launcher ", ""), 0)
        } else if self.upd.checking {
            ("Checking…".to_string(), 0)
        } else if self.app_update_available() {
            (latest.map(|l| format!("Update to {l}")).unwrap_or_default(), 3)
        } else if latest.is_some() {
            (format!("{cur} · Up to date"), 1)
        } else {
            (cur.to_string(), 0)
        };
        (r.value, r.value_kind) = (value.into(), kind);
        if !self.upd.error.is_empty() {
            (r.hint, r.hint_kind) = (self.upd.error.clone().into(), 2);
        }
        rows.push((SId::AppUpdate, r));

        // KytyPS5
        let ok = cfg.emulator_ok();
        let managed = self.kyty_managed();
        let kyty_state = crate::kyty::load_state();
        let mut r = row(4, "KytyPS5");
        r.hint = "Runs PS5 games".into();
        let (value, kind) = if self.kyty.busy {
            (self.kyty.progress.replace("Downloading KytyPS5 ", ""), 0)
        } else if self.kyty.checking {
            ("Checking…".to_string(), 0)
        } else if !managed && !ok {
            ("Install".to_string(), 3)
        } else if !managed {
            r.hint = format!("Runs PS5 games · you use your own build{} · switching keeps your saves",
                self.kyty.version.strip_prefix("Self-built").map(|v| format!(" ({})", v.trim())).unwrap_or_default()).into();
            ("Switch to official builds".to_string(), 3)
        } else if self.kyty_update_available() {
            (self.kyty.latest.as_ref().map(|l| format!("Update to {}", crate::kyty::pretty(&l.tag))).unwrap_or_default(), 3)
        } else if self.kyty.latest.is_some() {
            (format!("{} · Up to date", crate::kyty::pretty(&kyty_state.installed)), 1)
        } else {
            (crate::kyty::pretty(&kyty_state.installed), 0)
        };
        (r.value, r.value_kind) = (value.into(), kind);
        if !self.kyty.error.is_empty() {
            (r.hint, r.hint_kind) = (self.kyty.error.clone().into(), 2);
        }
        rows.push((SId::KytyUpdate, r));

        // shadPS4
        let shad_state = crate::shad::load_state();
        let shad_installed = crate::shad::installed();
        let mut r = row(4, "shadPS4");
        r.hint = if shad_installed {
            "Runs PS4 games"
        } else if cfg.shad_auto_update {
            "Runs PS4 games · installs by itself in the background"
        } else {
            "Runs PS4 games · installs when you first play one"
        }
        .into();
        let (value, kind) = if self.shad.busy {
            (self.shad.progress.clone(), 0)
        } else if !shad_installed {
            ("Install".to_string(), 3)
        } else if shad_state.latest == shad_state.installed {
            (format!("{} · Up to date", crate::shad::pretty(&shad_state.installed)), 1)
        } else {
            (crate::shad::pretty(&shad_state.installed), 0)
        };
        (r.value, r.value_kind) = (value.into(), kind);
        if !self.shad.error.is_empty() {
            (r.hint, r.hint_kind) = (self.shad.error.clone().into(), 2);
        }
        rows.push((SId::ShadUpdate, r));

        let mut r = row(2, "Update automatically");
        r.on = cfg.app_auto_update && cfg.kyty_auto_update && cfg.shad_auto_update;
        r.hint = "Checks every few hours · updates wait until you close your game".into();
        rows.push((SId::AutoUpdate, r));
        // Rollbacks only when there is something to go back to.
        let mut rollback_hint = "For when an update breaks a game";
        if managed && !kyty_state.previous.is_empty() && crate::kyty::root().join("versions").join(&kyty_state.previous).is_dir() {
            let mut r = row(4, "Go back to the previous KytyPS5");
            r.value = crate::kyty::pretty(&kyty_state.previous).into();
            r.hint = std::mem::take(&mut rollback_hint).into();
            rows.push((SId::KytyRollback, r));
        }
        if !shad_state.previous.is_empty() && crate::shad::root().join("versions").join(&shad_state.previous).is_dir() {
            let mut r = row(4, "Go back to the previous shadPS4");
            r.value = crate::shad::pretty(&shad_state.previous).into();
            r.hint = rollback_hint.into();
            rows.push((SId::ShadRollback, r));
        }

        header(&mut rows, "LAUNCHER");
        let mut r = row(3, "Display");
        r.value = match self.monitors.iter().find(|m| m.name == cfg.monitor) {
            Some(m) => format!("{} · {}×{}", m.name, m.w, m.h),
            None => "Primary display".into(),
        }
        .into();
        rows.push((SId::Display, r));
        let mut r = row(2, "Interface sounds");
        r.on = cfg.sounds;
        rows.push((SId::Sounds, r));
        let unshared = self.my_results.unshared().len();
        let mut r = row(4, "Share your game ratings");
        (r.value, r.value_kind) = if unshared > 0 { (format!("{unshared} to share").into(), 3) } else { ("Nothing new".into(), 0) };
        r.hint = if self.my_results.games.is_empty() {
            "Rate a game from its Options menu after you play it, then share it here".into()
        } else {
            "Tells the emulator teams which games work on Linux · needs a free GitHub account".into()
        };
        rows.push((SId::Share, r));

        // Its own section: out of the way without a switch to show it.
        header(&mut rows, "ADVANCED");
        {
            let mut r = row(1, "KytyPS5 location");
            r.value = util::display_path(&cfg.emulator).into();
            (r.hint, r.hint_kind) = if ok { ("Found".into(), 1) } else { ("Nothing runnable at this path".into(), 2) };
            rows.push((SId::Emulator, r));
            let mut r = row(3, "Resolution (PS5 games)");
            r.value = format!("{} × {}", cfg.width, cfg.height).into();
            rows.push((SId::Resolution, r));
            let mut r = row(3, "Present mode (PS5 games)");
            r.value = PRESENT_MODES.iter().find(|(k, _)| *k == cfg.present_mode).map(|(_, l)| *l).unwrap_or(cfg.present_mode.as_str()).into();
            r.hint = "Try V-Sync if the picture tears".into();
            rows.push((SId::Present, r));
            let mut r = row(2, "AMD CPU patches (PS5 games)");
            r.on = cfg.amd_cpu;
            r.hint = "Only for AMD processors".into();
            rows.push((SId::Amd, r));
            let mut r = row(1, "Extra KytyPS5 arguments");
            r.value = cfg.extra_args.clone().into();
            r.hint = "For example --vblank-frequency 60 --tessellation".into();
            rows.push((SId::Extra, r));

            let mut r = row(4, "Check for new releases");
            r.value = if self.syncing {
                "Reloading…".into()
            } else {
                let snapshot = self.games.first().map(|game| util::fmt_date(util::parse_iso_date(&game.g.peers_observed))).unwrap_or_default();
                format!("{} · {}", plural(self.games.len(), "release"), if snapshot.is_empty() { "date unknown" } else { &snapshot }).into()
            };
            r.hint = "Checks by itself every 6 hours · the list is updated daily, with seeder counts from that day".into();
            rows.push((SId::Refresh, r));
        }

        header(&mut rows, "");
        if crate::os::system::on_os() {
            // The launcher is the whole screen here: quitting it would only start it again.
            rows.push((SId::Restart, row(4, "Restart")));
            let mut r = row(4, "Shut down");
            r.danger = true;
            rows.push((SId::ShutDown, r));
        } else {
            let mut r = row(4, "Quit Azrak Launcher");
            r.value = "Ctrl+Q".into();
            r.danger = true;
            rows.push((SId::Quit, r));
        }

        // Each header starts a section (the left column); the last, unnamed one holds Quit (or
        // Restart and Shut down), under the sections.
        let mut titles: Vec<String> = Vec::new();
        let mut cur = -1;
        for (_, r) in rows.iter_mut() {
            if r.kind == 0 {
                cur = if r.label.is_empty() { -1 } else { titles.push(r.label.to_string()); titles.len() as i32 - 1 };
            }
            r.section = cur;
        }
        titles.push("ABOUT".into());
        self.settings_sections = titles.iter().map(|t| self.section_info(t)).collect();
        self.settings_headers = titles;
        self.settings_section = self.settings_section.min(self.settings_sections.len() - 1);
        self.settings_ids = rows.iter().map(|(id, _)| *id).collect();
        self.settings_rows = rows.into_iter().map(|(_, r)| r).collect();
    }

    /// A section's name in the left column, what it's for, and a note beside it (what needs you).
    fn section_info(&self, header: &str) -> SettingSection {
        let mut chars = header.chars();
        let label: String = chars.next().into_iter().chain(chars.flat_map(|c| c.to_lowercase())).collect();
        let (sub, (note, note_kind)) = match header {
            "SYSTEM" => ("Internet, sound and power", (String::new(), 0)),
            "GAMES" => ("Where your games live", (String::new(), 0)),
            "PLAYING" => ("What happens when you start a game", match crate::gamepad::connected().first() {
                Some(name) => (name.clone(), 0),
                None => (String::new(), 0),
            }),
            "DOWNLOADS" => ("How games are downloaded and shared", {
                let active = self.downloads.snapshot().iter().filter(|j| j.state.active()).count();
                if active > 0 { (format!("{active} active"), 1) } else { (String::new(), 0) }
            }),
            "UPDATES" => ("The launcher and its emulators", {
                let ready = [self.app_update_available(), self.kyty_update_available()].iter().filter(|r| **r).count();
                if ready > 0 { (format!("{ready} ready"), 2) } else { (String::new(), 0) }
            }),
            "LAUNCHER" => ("Display, sounds and your game ratings", (String::new(), 0)),
            "ADVANCED" => ("KytyPS5 location and video options, the game catalog", (String::new(), 0)),
            _ => ("", (String::new(), 0)),
        };
        let sub = if header == "ABOUT" { format!("Azrak Launcher {}", crate::update::current_version()) } else { sub.to_string() };
        SettingSection { label: label.into(), sub: sub.into(), note: note.into(), note_kind }
    }

    /// The launcher's version and whether it's up to date, under the sections.
    fn version_line(&self) -> String {
        let cur = crate::update::current_version();
        if self.upd.installed.is_some() { format!("Azrak Launcher {cur} · restart to update") }
        else if self.app_update_available() { format!("Azrak Launcher {cur} · update ready") }
        else if self.upd.latest.is_some() { format!("Azrak Launcher {cur} · up to date") }
        else { format!("Azrak Launcher {cur}") }
    }

    /// The rows under the sections (Quit, or Restart and Shut down), by row number.
    fn settings_footer(&self) -> Vec<usize> {
        self.settings_rows.iter().enumerate().filter(|(_, r)| r.kind != 0 && r.section < 0).map(|(i, _)| i).collect()
    }

    /// The rows of the chosen section that can have focus, by row number.
    fn section_rows(&self) -> Vec<usize> {
        let s = self.settings_section as i32;
        self.settings_rows.iter().enumerate().filter(|(_, r)| r.kind != 0 && r.section == s).map(|(i, _)| i).collect()
    }

    /// Focus a row, showing its section.
    pub fn focus_setting_row(&mut self, i: usize) {
        if let Some(s) = self.settings_rows.get(i).map(|r| r.section).filter(|s| *s >= 0) {
            self.settings_section = s as usize;
            self.set_focus(Z_SETTINGS, i as i32);
            self.push_settings_nav();
            self.scroll_settings();
        }
    }

    /// The left column: section `i`, or a row under them (Quit) past the last section. Silent:
    /// callers play the sound that fits (Move, or Back from the rows).
    pub fn focus_settings_nav(&mut self, i: usize) {
        let n = self.settings_sections.len();
        if i < n && i != self.settings_section {
            self.settings_section = i;
            self.ui().set_settings_y(0.0);
        }
        self.set_focus(Z_SETTINGS_NAV, i as i32);
        self.push_settings_nav();
    }

    /// The notes beside the sections ("1 active", "1 ready") as they are now.
    pub fn refresh_settings_notes(&mut self) {
        let sections: Vec<SettingSection> = self.settings_headers.iter().map(|h| self.section_info(h)).collect();
        if sections != self.settings_sections {
            self.settings_sections = sections;
            self.ui().set_settings_sections(model(self.settings_sections.clone()));
        }
    }

    /// The glass behind the sheet: the art behind it, blurred (none over the dimmed Library).
    pub fn push_settings_glass(&mut self) {
        let glass = self.bg_glass();
        let ui = self.ui();
        ui.set_settings_has_glass(glass.is_some());
        ui.set_settings_glass(glass.unwrap_or_default());
    }

    pub fn act_settings_nav(&mut self, a: Act) {
        let n = self.settings_sections.len();
        let footer = self.settings_footer();
        let i = self.idx.max(0) as usize;
        match a {
            Act::Back => self.back(),
            Act::Up if i > 0 => { audio::play(Sound::Move); self.focus_settings_nav(i - 1); }
            Act::Down if i + 1 < n + footer.len() => { audio::play(Sound::Move); self.focus_settings_nav(i + 1); }
            Act::Right | Act::Confirm if i < n => {
                if let Some(&j) = self.section_rows().first() {
                    audio::play(Sound::Move);
                    self.focus_setting_row(j);
                }
            }
            Act::Confirm => {
                if let Some(&j) = footer.get(i - n) { self.settings_activate(j); }
            }
            _ => {}
        }
    }

    pub fn act_settings_rows(&mut self, a: Act) {
        let rows = self.section_rows();
        let pos = rows.iter().position(|&j| j as i32 == self.idx);
        let kind = self.settings_rows.get(self.idx.max(0) as usize).map_or(0, |r| r.kind);
        let to_nav = |app: &mut App| { audio::play(Sound::Back); let s = app.settings_section; app.focus_settings_nav(s); };
        match a {
            // Back leaves the rows for their section; Back there closes Settings.
            Act::Back => to_nav(self),
            Act::Up | Act::Down => {
                let next = match (pos, a) {
                    (Some(p), Act::Up) => p.checked_sub(1),
                    (Some(p), _) => Some(p + 1),
                    (None, _) => Some(0),
                };
                if let Some(&j) = next.and_then(|p| rows.get(p)) {
                    self.move_focus(Z_SETTINGS, j as i32);
                    self.scroll_settings();
                }
            }
            // Left changes a switch or a choice; anywhere else it goes back to the sections.
            Act::Left if !matches!(kind, 2 | 3) => to_nav(self),
            Act::Left | Act::Right => self.settings_change(self.idx as usize, if a == Act::Left { -1 } else { 1 }),
            Act::Confirm => self.settings_activate(self.idx as usize),
            _ => {}
        }
    }

    /// The left column's state (which section, its title, the rows under the sections).
    pub fn push_settings_nav(&mut self) {
        let ui = self.ui();
        let s = self.settings_sections.get(self.settings_section).cloned().unwrap_or_default();
        ui.set_settings_section(self.settings_section as i32);
        ui.set_settings_title(s.label);
        ui.set_settings_sub(s.sub);
    }

    /// Open Settings with "Share your results with KytyPS5" selected.
    pub fn open_share_setting(&mut self) {
        self.open_setting(SId::Share);
    }

    /// Open Settings with this row selected.
    pub fn open_setting(&mut self, id: SId) {
        self.open_settings();
        if let Some(i) = self.settings_ids.iter().position(|s| *s == id) {
            self.focus_setting_row(i);
        }
    }

    pub fn push_settings(&mut self) {
        let footer: Vec<SettingData> = self.settings_footer().into_iter().map(|i| self.settings_rows[i].clone()).collect();
        self.push_settings_glass();
        let ui = self.ui();
        ui.set_settings(model(self.settings_rows.clone()));
        ui.set_settings_sections(model(self.settings_sections.clone()));
        ui.set_settings_footer(model(footer));
        ui.set_settings_version(self.version_line().into());
        ui.set_edit_index(self.edit_index);
        self.push_settings_nav();
    }

    pub fn scroll_settings(&mut self) {
        // Estimated row heights matching app.slint (layout units). Text follows the text scale,
        // which on small windows is larger than the layout scale.
        let r = (self.scale.max(crate::present::TEXT_SCALE_MIN) / self.scale).max(1.0);
        let (mut y, mut target) = (0.0, 0.0);
        for i in self.section_rows() {
            if i as i32 == self.idx {
                target = y;
            }
            let row = &self.settings_rows[i];
            // Hints wrap at about 95 characters (the column's width at 17 px), or about 55 beside
            // a long value (a folder takes up to half the row).
            // Text grows faster than the column on small windows (r), so fewer fit on a line.
            let per_line = (if row.kind == 1 || row.value.chars().count() > 24 { 55.0 } else { 95.0 } / r).max(10.0) as usize;
            let hint_lines = row.hint.chars().count().div_ceil(per_line) as f32;
            let h = 32.0 + 26.0 * r + if hint_lines > 0.0 { 5.0 + hint_lines * 22.0 * r } else { 0.0 } + if self.edit_index == i as i32 { 60.0 * r } else { 0.0 };
            y += f32::max(h, 84.0) + 10.0;
        }
        // The rows' window on the sheet (as tall as the window allows): under the title, above
        // the button hints. A little room to spare at the end, as the heights are estimates.
        let (_, h) = self.logical_size();
        let view = f32::min(960.0, h - 40.0) - 48.0 - (42.0 + 24.0) * r - 8.0 - 26.0 - 26.0 * r - 48.0;
        let max = (y + 24.0 - view).max(0.0);
        self.ui().set_settings_y(-(target - view * 0.35).clamp(0.0, max) * self.scale);
    }

    fn save_cfg(&mut self, f: impl FnOnce(&mut crate::config::Config)) {
        let mut c = self.cfg.lock().unwrap();
        f(&mut c);
        c.save();
    }

    pub fn refresh_settings(&mut self) {
        // Rows come and go (the Wi-Fi list, devices found or paired): the selection stays on the
        // same row, not at the same place, or the next press would land on a stranger.
        let before: Vec<(SId, String)> = self.settings_ids.iter().zip(&self.settings_rows).map(|(id, r)| (*id, r.label.to_string())).collect();
        self.build_settings();
        self.push_settings();
        if self.overlay != Overlay::Settings || self.zone != Z_SETTINGS || self.edit_index >= 0 {
            return;
        }
        let after: Vec<(SId, &str)> = self.settings_ids.iter().zip(&self.settings_rows).map(|(id, r)| (*id, r.label.as_str())).collect();
        // A row that's gone (a network once the list closes) leaves the selection on the nearest
        // row above it that's still there (Internet).
        let Some(i) = before.iter().take(self.idx as usize + 1).rev()
            .filter(|(id, _)| *id != SId::Header)
            .find_map(|(id, label)| same_row((*id, label), &after)) else { return };
        if i as i32 != self.idx {
            self.focus_setting_row(i);
        }
    }

    /// Select a row of Settings while it's open (e.g. Internet once a network is joined).
    pub fn select_setting(&mut self, id: SId) {
        if self.overlay != Overlay::Settings {
            return;
        }
        if let Some(i) = self.settings_ids.iter().position(|s| *s == id) {
            self.focus_setting_row(i);
        }
    }

    pub fn settings_change(&mut self, i: usize, dir: i32) {
        let Some(id) = self.settings_ids.get(i).copied() else { return };
        if self.system_change(id, dir) {
            return;
        }
        match id {
            SId::Resolution => {
                let cur = { let c = self.cfg.lock().unwrap(); (c.width, c.height) };
                let pos = RESOLUTIONS.iter().position(|r| *r == cur).unwrap_or(2) as i32;
                let (w, h) = RESOLUTIONS[(pos + dir).rem_euclid(RESOLUTIONS.len() as i32) as usize];
                self.save_cfg(|c| {
                    c.width = w;
                    c.height = h;
                });
            }
            SId::Present => {
                let cur = self.cfg.lock().unwrap().present_mode.clone();
                let pos = PRESENT_MODES.iter().position(|(k, _)| *k == cur).unwrap_or(0) as i32;
                let next = PRESENT_MODES[(pos + dir).rem_euclid(PRESENT_MODES.len() as i32) as usize].0.to_string();
                self.save_cfg(|c| c.present_mode = next);
            }
            SId::Display => {
                let names: Vec<String> = std::iter::once(String::new()).chain(self.monitors.iter().map(|m| m.name.clone())).collect();
                let cur = self.cfg.lock().unwrap().monitor.clone();
                let pos = names.iter().position(|n| *n == cur).unwrap_or(0) as i32;
                let next = names[(pos + dir).rem_euclid(names.len() as i32) as usize].clone();
                self.save_cfg(|c| c.monitor = next.clone());
                self.move_to_monitor(&next);
            }
            SId::VpnOnly => {
                let on = dir > 0;
                let vpn = { let c = self.cfg.lock().unwrap(); on.then(|| c.vpn_interface.clone()) };
                if let Err(error) = self.downloads.set_vpn(vpn) {
                    self.toast("VPN setting unavailable", &error.to_string(), 2);
                    return;
                }
                self.save_cfg(|c| c.vpn_only = on);
                if on && crate::vpn::active(&self.cfg.lock().unwrap().vpn_interface).is_none() {
                    self.toast("Downloads wait for your VPN", "They start as soon as it's connected.", 0);
                }
                self.push_downloads();
            }
            SId::SeedCompleted => {
                let on = dir > 0;
                if let Err(error) = self.downloads.set_seed_after_download(on) {
                    self.toast("Seeding setting unavailable", &error.to_string(), 2);
                    return;
                }
                self.save_cfg(|c| c.seed_after_download = on);
                self.push_downloads();
            }
            SId::Fullscreen | SId::Amd | SId::ReturnOnExit | SId::Sounds | SId::AutoUpdate => {
                let on = dir > 0;
                self.save_cfg(|c| match id {
                    SId::Fullscreen => c.fullscreen = on,
                    SId::Amd => c.amd_cpu = on,
                    SId::ReturnOnExit => c.return_on_exit = on,
                    SId::AutoUpdate => (c.app_auto_update, c.kyty_auto_update, c.shad_auto_update) = (on, on, on),
                    _ => c.sounds = on,
                });
                if id == SId::AutoUpdate && on {
                    self.kyty_check(false);
                    self.shad_tick();
                }
                if id == SId::Sounds {
                    audio::set_enabled(on);
                }
            }
            _ => return,
        }
        audio::play(Sound::Move);
        self.refresh_settings();
    }

    pub fn settings_activate(&mut self, i: usize) {
        let Some(id) = self.settings_ids.get(i).copied() else { return };
        if self.system_activate(id, i) {
            return;
        }
        match id {
            SId::Emulator | SId::Dirs | SId::Extra | SId::DownloadDir | SId::InstallDir => {
                let cfg = self.cfg.lock().unwrap().clone();
                let text = match id {
                    SId::Emulator => cfg.emulator,
                    SId::Dirs => cfg.game_dirs.join("; "),
                    SId::Extra => cfg.extra_args,
                    SId::DownloadDir => cfg.download_dir,
                    SId::InstallDir => cfg.install_dir,
                    _ => String::new(),
                };
                audio::play(Sound::Select);
                self.edit_index = i as i32;
                let ui = self.ui();
                ui.set_edit_text(text.into());
                ui.set_edit_index(i as i32);
            }
            SId::Fullscreen | SId::Amd | SId::ReturnOnExit | SId::Sounds | SId::AutoUpdate | SId::SeedCompleted | SId::VpnOnly => {
                let on = self.settings_rows[i].on;
                self.settings_change(i, if on { -1 } else { 1 });
            }
            SId::Resolution | SId::Present | SId::Display => self.settings_change(i, 1),
            SId::KytyUpdate => {
                audio::play(Sound::Select);
                if self.kyty.busy || self.kyty.checking {
                    return;
                }
                let managed = self.kyty_managed();
                if !managed {
                    self.kyty_switch_to_managed();
                } else if self.kyty_update_available() {
                    if let Some(rel) = self.kyty.latest.clone() {
                        self.kyty_install(rel, true);
                    }
                } else {
                    self.kyty_check(true);
                }
                self.refresh_settings();
            }
            SId::AppUpdate => {
                audio::play(Sound::Select);
                if self.upd.installed.is_some() {
                    self.app_restart();
                } else if self.upd.busy || self.upd.checking {
                    return;
                } else if self.app_update_available() {
                    if let Some(rel) = self.upd.latest.clone() {
                        self.app_update_install(rel);
                    }
                } else {
                    self.app_update_check(true);
                }
                self.refresh_settings();
            }
            SId::KytyRollback => {
                audio::play(Sound::Select);
                self.kyty_rollback();
                self.refresh_settings();
            }
            SId::ShadUpdate => {
                audio::play(Sound::Select);
                // Installs when shadPS4 isn't there yet, otherwise updates it if there's a newer release.
                self.shad_check(true);
            }
            SId::ShadRollback => {
                audio::play(Sound::Select);
                self.shad_rollback();
            }
            SId::Refresh => {
                audio::play(Sound::Select);
                self.start_sync(true);
                self.refresh_settings();
            }
            SId::Rescan => {
                audio::play(Sound::Select);
                self.rescan_library();
                let n = self.locals.len();
                self.toast("Installed games rescanned", &format!("{n} game{} found.", if n == 1 { "" } else { "s" }), 1);
                self.refresh_settings();
            }
            SId::Quit => {
                crate::app::quit("Settings → Quit Azrak Launcher");
            }
            SId::Downloads => self.open_downloads(None),
            SId::Vpn => {
                audio::play(Sound::Select);
                if !self.vpn_busy.is_empty() || !crate::vpn::nm_available() {
                    return;
                }
                let saved = crate::vpn::nm_saved();
                if let Some(v) = saved.iter().find(|v| v.active) {
                    let name = v.name.clone();
                    self.vpn_task("Disconnecting…", move || crate::vpn::nm_down(&name).map(|_| ("VPN disconnected".to_string(), name)));
                } else if crate::vpn::active("").is_some() {
                    self.toast("Connected by your VPN app", "Switch it off there.", 0);
                } else if let Some(v) = saved.first() {
                    let name = v.name.clone();
                    self.vpn_task("Connecting…", move || crate::vpn::nm_up(&name).map(|_| ("VPN connected".to_string(), name)));
                } else {
                    self.vpn_add(i);
                }
            }
            SId::VpnAdd => {
                audio::play(Sound::Select);
                self.vpn_add(i);
            }
            SId::Share => self.share_results(),
            SId::Controls => self.open_controls(),
            SId::ThisPc => {
                audio::play(Sound::Select);
                self.pc_check();
                self.refresh_settings();
            }
            _ => {}
        }
    }

    pub fn settings_commit_text(&mut self, i: usize, text: String) {
        let Some(id) = self.settings_ids.get(i).copied() else { return };
        let t = text.trim().to_string();
        match id {
            SId::Emulator => {
                self.save_cfg(|c| c.emulator = t);
                self.kyty_refresh_version();
                let ok = self.cfg.lock().unwrap().emulator_ok();
                if ok { self.toast("Emulator path saved", "Games will start with this KytyPS5.", 1) } else { self.toast("Emulator not found", "There's no runnable kyty_emulator at that path.", 2) }
            }
            SId::Dirs => {
                let dirs: Vec<String> = t.split([';', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                self.save_cfg(|c| c.game_dirs = dirs);
                self.rescan_library();
                let n = self.locals.len();
                self.toast("Installed games rescanned", &format!("{n} game{} found.", if n == 1 { "" } else { "s" }), 1);
            }
            SId::Extra => self.save_cfg(|c| c.extra_args = t),
            SId::VpnAdd if !t.is_empty() => {
                let file = util::expand_home(&t);
                self.vpn_task("Adding…", move || crate::vpn::nm_import(&file).map(|name| ("VPN added and connected".to_string(), name)));
            }
            SId::DownloadDir => {
                if t.is_empty() || !util::expand_home(&t).is_absolute() {
                    self.toast("Invalid download folder", "Use an absolute path or ~/Downloads/PS5.", 2);
                } else {
                    self.save_cfg(|c| c.download_dir = t);
                }
            }
            SId::InstallDir => {
                if t.is_empty() || !util::expand_home(&t).is_absolute() {
                    self.toast("Invalid installation folder", "Use an absolute path or ~/Games/PS5.", 2);
                } else { self.save_cfg(|c| c.install_dir = t); }
            }
            _ => {}
        }
    }

    /// "This PC": whether PS5 and PS4 games run here, the specs, and what holds it back.
    fn this_pc_row(&self) -> SettingData {
        use crate::pc::Verdict;
        use crate::platform::Platform;
        let mut r = row(4, "This PC");
        let Some(specs) = self.pc.as_ref().map(|pc| pc.specs()).filter(|_| !self.pc_checking) else {
            (r.value, r.hint) = ("Checking…".into(), "Reads the processor, memory and graphics card".into());
            return r;
        };
        r.hint = specs.summary().into();
        let (ps5, ps4) = (self.pc_verdict(Platform::Ps5).cloned(), self.pc_verdict(Platform::Ps4).cloned());
        let (Some(ps5), Some(ps4)) = (ps5, ps4) else {
            (r.value, r.hint_kind) = ("Check again".into(), 2);
            r.hint = format!("{} · the graphics check didn't finish", r.hint).into();
            return r;
        };
        let lower = |v: &Verdict| v.headline().to_lowercase();
        r.value = if ps5 == ps4 {
            format!("PS5 and PS4 games {}", lower(&ps5)).replace("games won't start", "games won't start here")
        } else {
            format!("PS5: {} · PS4: {}", lower(&ps5), lower(&ps4))
        }
        .into();
        r.value_kind = if ps5 == Verdict::Good && ps4 == Verdict::Good { 1 } else if matches!((&ps5, &ps4), (Verdict::No(_), _) | (_, Verdict::No(_))) { 2 } else { 0 };
        // The first thing holding it back, in words.
        if let Some((v, p)) = [(&ps5, Platform::Ps5), (&ps4, Platform::Ps4)].into_iter().find(|(v, _)| matches!(v, Verdict::No(_)))
            .or_else(|| [(&ps5, Platform::Ps5), (&ps4, Platform::Ps4)].into_iter().find(|(v, _)| matches!(v, Verdict::Slow(_))))
        {
            r.hint = format!("{} · {}", crate::pc_ui::explain(v, p), r.hint).into();
        }
        r
    }

    /// Add a VPN from a provider's file: the desktop's file dialog, or typing the path.
    fn vpn_add(&mut self, i: usize) {
        if !self.vpn_busy.is_empty() {
            return;
        }
        if !crate::vpn::has_file_dialog() {
            self.edit_index = i as i32;
            let ui = self.ui();
            ui.set_edit_text("~/Downloads/".into());
            ui.set_edit_index(i as i32);
            return;
        }
        self.vpn_task("Choose the file…", || match crate::vpn::choose_file() {
            Ok(Some(file)) => crate::vpn::nm_import(&file).map(|name| ("VPN added and connected".to_string(), name)),
            Ok(None) => Ok((String::new(), String::new())),
            Err(()) => Err("There's no file dialog on this PC".into()),
        });
    }

    /// Run a NetworkManager step off the UI thread. `job` returns (toast title, VPN name), or an
    /// empty title for nothing to report (a cancelled dialog).
    fn vpn_task(&mut self, busy: &str, job: impl FnOnce() -> Result<(String, String), String> + Send + 'static) {
        self.vpn_busy = busy.into();
        self.refresh_settings();
        std::thread::spawn(move || {
            let result = job();
            let _ = slint::invoke_from_event_loop(move || {
                with_app(move |app| {
                    app.vpn_busy.clear();
                    match result {
                        Ok((title, _)) if title.is_empty() => {}
                        Ok((title, name)) => app.toast(&title, &name, 1),
                        Err(error) => {
                            app.toast("VPN didn't change", &error, 2);
                            audio::play(Sound::Error);
                        }
                    }
                    if app.overlay == Overlay::Settings {
                        app.refresh_settings();
                    }
                })
            });
        });
    }

    /// Switch to another display. The UI scale is fixed per window at startup, so the
    /// launcher restarts itself on the new display (about a second).
    pub fn move_to_monitor(&mut self, name: &str) {
        let label = if name.is_empty() { "the primary display".to_string() } else { name.to_string() };
        if !self.live.is_empty() {
            self.toast(&format!("Moves to {label} next time"), "The launcher can't restart while a game is running.", 0);
            return;
        }
        self.toast(&format!("Moving to {label}…"), "The launcher restarts on that display.", 0);
        slint::Timer::single_shot(std::time::Duration::from_millis(600), || with_app(|app| app.app_restart()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_follows_its_row() {
        let rows = [(SId::Header, "SYSTEM"), (SId::Internet, "Internet"), (SId::BtPair, "Pair"),
            (SId::BtDevice(0), "DualSense"), (SId::BtDevice(1), "Xbox")];
        // The Wi-Fi list closed above it: still Pair, not whatever took its place.
        assert_eq!(same_row((SId::BtPair, "Pair"), &rows), Some(2));
        // A device that moved in the list is followed by its name.
        assert_eq!(same_row((SId::BtDevice(0), "Xbox"), &rows), Some(4));
        // A network row that's gone has no place.
        assert_eq!(same_row((SId::WifiNet(3), "Office"), &rows), None);
    }
}
