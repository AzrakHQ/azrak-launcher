//! Settings → SYSTEM, on AzrakOS only: internet, controllers and headsets, sound output,
//! system updates, restart and shut down. Slow tools run off the UI thread; Settings shows what
//! they last reported and refreshes every few seconds while it's open.

use crate::app::*;
use crate::settings::SId;
use crate::os::power::Power;
use crate::os::system::{self, Device, Network, Output, Update, Wifi};
use crate::SettingData;

#[derive(Default)]
pub struct SysUi {
    pub network: Network,
    pub paired: Vec<(Device, bool)>,
    pub outputs: Vec<Output>,
    pub output: Option<usize>,
    pub update: Option<Update>,
    /// The Wi-Fi list is open, and what it found.
    pub wifi_open: bool,
    pub wifi: Vec<Wifi>,
    /// "More networks…" was pressed: the list shows them all, not only the strongest.
    pub wifi_all: bool,
    /// Devices in pairing mode found by the last search.
    pub found: Vec<Device>,
    /// Something in progress, shown on its row: (row, text).
    pub busy: Option<(SId, String)>,
    /// A paired device pressed once: pressing it again within a few seconds forgets it.
    pub forget: Option<(usize, f64)>,
    polling: bool,
    pub polled: f64,
    /// Bumped when the launcher itself changes the system (joins a network, pairs): a reading
    /// started before that is out of date and is read again.
    epoch: u32,
}

fn row(kind: i32, label: &str) -> SettingData {
    SettingData { kind, label: label.into(), ..Default::default() }
}

impl App {
    /// Read the system's state in the background (at most every 4 s), then refresh Settings.
    pub fn sys_poll(&mut self) {
        let now = crate::util::now_secs();
        if !system::on_os() || self.sys.polling || now - self.sys.polled < 4.0 {
            return;
        }
        self.sys.polling = true;
        let epoch = self.sys.epoch;
        std::thread::spawn(move || {
            let (network, paired, (outputs, output), update) = (system::network(), system::bt_paired(), system::outputs(), system::update_status());
            let _ = slint::invoke_from_event_loop(move || with_app(move |app| {
                if app.sys.epoch != epoch {
                    (app.sys.polling, app.sys.polled) = (false, 0.0);
                    return app.sys_poll();
                }
                let changed = app.sys.network != network || app.sys.paired != paired || app.sys.outputs != outputs
                    || app.sys.output != output || app.sys.update != update;
                (app.sys.network, app.sys.paired, app.sys.outputs, app.sys.output, app.sys.update) = (network, paired, outputs, output, update);
                app.sys.polling = false;
                app.sys.polled = crate::util::now_secs();
                if changed && app.overlay == Overlay::Settings && app.edit_index < 0 {
                    app.refresh_settings();
                }
            }));
        });
    }

    /// Rows for the SYSTEM section (empty off the OS).
    pub fn system_rows(&self) -> Vec<(SId, SettingData)> {
        if !system::on_os() {
            return vec![];
        }
        let s = &self.sys;
        let busy = |id: SId| s.busy.as_ref().filter(|(b, _)| *b == id).map(|(_, t)| t.clone());
        let mut rows = vec![(SId::Header, row(0, "SYSTEM"))];
        if system::on_usb() {
            let mut r = row(4, "Install AzrakOS on this PC");
            (r.value, r.value_kind) = ("Start".into(), 3);
            r.hint = "You're running AzrakOS from the USB stick · installing erases the disk you choose".into();
            rows.push((SId::InstallOs, r));
        }

        let mut r = row(4, "Internet");
        (r.value, r.value_kind) = match (busy(SId::Internet), s.network.kind) {
            (Some(text), _) => (text.into(), 0),
            (None, "") => ("Not connected · choose Wi-Fi".into(), 3),
            (None, "Cable") => ("Cable · Connected".into(), 1),
            (None, kind) => (format!("{kind} · {}", s.network.name).into(), 1),
        };
        let cable = s.network.kind == "Cable";
        r.hint = match (s.wifi_open, cable) {
            (true, true) => "Press a network to join · the cable is used while it's plugged in",
            (true, false) => "Press a network to join",
            (false, _) => "Press to see Wi-Fi networks",
        }.into();
        rows.push((SId::Internet, r));
        if s.wifi_open {
            // The strongest few, so the rest of SYSTEM (pairing a controller) stays in sight.
            let shown = if s.wifi_all { s.wifi.len() } else { s.wifi.len().min(WIFI_SHOWN) };
            for (i, w) in s.wifi.iter().enumerate().take(shown) {
                let mut r = row(4, &w.ssid);
                (r.signal, r.lock) = (bars(w.signal), w.secured);
                if let Some(text) = busy(SId::WifiNet(i as u16)) {
                    r.value = text.into();
                } else if w.in_use {
                    // With a cable in, Wi-Fi stays connected but isn't the one used.
                    let text = if cable { "Connected · backup" } else { "Connected" };
                    (r.value, r.value_kind, r.icon) = (text.into(), 1, "check".into());
                } else if !w.secured {
                    r.value = "Open · no password".into();
                }
                rows.push((SId::WifiNet(i as u16), r));
            }
            if shown < s.wifi.len() {
                let mut r = row(4, "More networks…");
                (r.value, r.value_kind) = (format!("{} more", s.wifi.len() - shown).into(), 3);
                rows.push((SId::WifiMore, r));
            }
        }

        if !system::bt_available() {
            let mut r = row(4, "Bluetooth");
            r.value = "Not on this PC".into();
            r.hint = "Wired controllers work as they are · a USB Bluetooth adapter adds wireless ones".into();
            rows.push((SId::BtPair, r));
            return self.system_rows_rest(rows);
        }
        let mut r = row(4, "Pair a controller or headset");
        (r.value, r.value_kind) = match busy(SId::BtPair) {
            Some(text) => (text.into(), 0),
            None => ("Search".into(), 3),
        };
        r.hint = "DualSense: hold PS and Create until the light flashes · Xbox: hold the pair button".into();
        rows.push((SId::BtPair, r));
        for (i, d) in s.found.iter().enumerate() {
            let mut r = row(4, &d.name);
            r.value = busy(SId::BtFound(i as u16)).unwrap_or_else(|| "Pair".into()).into();
            r.value_kind = 3;
            rows.push((SId::BtFound(i as u16), r));
        }
        for (i, (d, on)) in s.paired.iter().enumerate() {
            let mut r = row(4, &d.name);
            let armed = s.forget.is_some_and(|(j, at)| j == i && crate::util::now_secs() - at < 5.0);
            (r.value, r.value_kind) = match busy(SId::BtDevice(i as u16)) {
                Some(text) => (text.into(), 0),
                None if armed => ("Press again to forget it".into(), 2),
                None if *on => ("Connected".into(), 1),
                None => ("Not connected".into(), 0),
            };
            rows.push((SId::BtDevice(i as u16), r));
        }

        self.system_rows_rest(rows)
    }

    /// Sound output and system updates.
    fn system_rows_rest(&self, mut rows: Vec<(SId, SettingData)>) -> Vec<(SId, SettingData)> {
        let s = &self.sys;
        if !s.outputs.is_empty() {
            let mut r = row(3, "Sound output");
            r.value = s.output.and_then(|i| s.outputs.get(i)).map(|o| o.label.clone()).unwrap_or_else(|| "Choose".into()).into();
            rows.push((SId::SoundOutput, r));
        }

        if let Some(u) = &s.update {
            let mut r = row(4, "System");
            (r.value, r.value_kind, r.hint) = match &u.staged {
                Some(next) => (format!("Restart to update to {next}").into(), 3, format!("Press to restart and update · now running {}", u.version).into()),
                None => (format!("{} · Up to date", u.version).into(), 1, "Updates download by themselves and wait for a restart".into()),
            };
            rows.push((SId::SystemUpdate, r));
        }
        rows
    }

    /// Activate a SYSTEM row. Returns false for rows that aren't SYSTEM rows.
    pub fn system_activate(&mut self, id: SId, _index: usize) -> bool {
        match id {
            SId::Internet => {
                self.sys.wifi_open = !self.sys.wifi_open;
                self.sys.wifi_all = false;
                if self.sys.wifi_open {
                    self.sys_task(SId::Internet, "Looking for Wi-Fi…", |app, list: Result<Vec<Wifi>, String>| match list {
                        Ok(list) => app.sys.wifi = list,
                        Err(e) => app.toast("Wi-Fi isn't available", &e, 2),
                    }, system::wifi_list);
                }
            }
            SId::WifiMore => {
                self.sys.wifi_all = true;
                crate::audio::play(crate::audio::Sound::Select);
                self.refresh_settings();
                // The first of the networks just shown, where "More networks…" was.
                self.select_setting(SId::WifiNet(WIFI_SHOWN as u16));
                return true;
            }
            SId::WifiNet(i) => {
                let Some(w) = self.sys.wifi.get(i as usize).cloned() else { return true };
                if w.in_use || self.sys.busy.is_some() {
                    return true;
                }
                // Saved networks and open ones connect straight away; others ask for the password.
                if w.secured && !saved_wifi(&w.ssid) {
                    self.wifi_join_open(&w.ssid, strength(w.signal), w.wep);
                    return true;
                }
                self.wifi_connect(i);
            }
            SId::BtPair => {
                if self.sys.busy.is_some() || !system::bt_available() {
                    return true;
                }
                self.sys_task(SId::BtPair, "Searching for 15 seconds…", |app, found: Vec<Device>| {
                    if found.is_empty() {
                        app.toast("Nothing found", "Put the controller in pairing mode, then search again.", 0);
                    }
                    app.sys.found = found;
                }, || system::bt_scan(15));
            }
            SId::BtFound(i) => {
                let Some(d) = self.sys.found.get(i as usize).cloned() else { return true };
                if self.sys.busy.is_some() {
                    return true;
                }
                let address = d.address.clone();
                self.sys_task(SId::BtFound(i), "Pairing…", move |app, r: Result<(), String>| {
                    match r {
                        Ok(()) => {
                            app.toast("Paired", &d.name, 1);
                            app.sys.found.retain(|f| f != &d);
                            // Listed (and selected) at once: its row in "Found" is gone.
                            if !app.sys.paired.iter().any(|(p, _)| p.address == d.address) {
                                app.sys.paired.push((d.clone(), true));
                            }
                            if app.overlay == Overlay::Settings {
                                app.refresh_settings();
                                if let Some(i) = app.sys.paired.iter().position(|(p, _)| p.address == d.address) {
                                    app.select_setting(SId::BtDevice(i as u16));
                                }
                            }
                        }
                        Err(e) => app.toast("Couldn't pair", &e, 2),
                    }
                    app.sys_changed();
                }, move || system::bt_pair(&address));
            }
            SId::BtDevice(i) => {
                let i = i as usize;
                let Some((d, _)) = self.sys.paired.get(i).cloned() else { return true };
                let now = crate::util::now_secs();
                if self.sys.forget.is_some_and(|(j, at)| j == i && now - at < 5.0) {
                    self.sys.forget = None;
                    let address = d.address.clone();
                    self.sys_task(SId::BtDevice(i as u16), "Forgetting…", move |app, r: Result<(), String>| {
                        if let Err(e) = r { app.toast("Couldn't forget it", &e, 2) } else { app.toast("Forgotten", &d.name, 1) }
                        app.sys_changed();
                    }, move || system::bt_remove(&address));
                } else {
                    self.sys.forget = Some((i, now));
                    slint::Timer::single_shot(std::time::Duration::from_secs(5), || with_app(|app| {
                        if app.overlay == Overlay::Settings { app.refresh_settings(); }
                    }));
                }
            }
            SId::SoundOutput => {
                self.system_change(id, 1);
                return true;
            }
            SId::SystemUpdate => {
                if self.sys.update.as_ref().is_some_and(|u| u.staged.is_some()) {
                    self.power_ask(Power::Update);
                }
                return true;
            }
            SId::InstallOs => self.os_install_open(),
            SId::Restart => {
                self.power_ask(Power::Restart);
                return true;
            }
            SId::ShutDown => {
                self.power_ask(Power::ShutDown);
                return true;
            }
            _ => return false,
        }
        crate::audio::play(crate::audio::Sound::Select);
        self.refresh_settings();
        true
    }

    /// Left/right on a SYSTEM row. Returns false for rows that aren't SYSTEM rows.
    pub fn system_change(&mut self, id: SId, dir: i32) -> bool {
        if id != SId::SoundOutput || self.sys.outputs.is_empty() {
            return false;
        }
        let n = self.sys.outputs.len() as i32;
        let next = (self.sys.output.map_or(0, |i| i as i32 + dir)).rem_euclid(n) as usize;
        if let Err(e) = system::set_output(&self.sys.outputs[next].name) {
            self.toast("Sound output didn't change", &e, 2);
            return true;
        }
        self.sys.output = Some(next);
        crate::audio::play(crate::audio::Sound::Move);
        self.refresh_settings();
        true
    }

    /// A saved or open network: it connects without a password.
    fn wifi_connect(&mut self, i: u16) {
        let Some(w) = self.sys.wifi.get(i as usize).cloned() else { return };
        let ssid = w.ssid.clone();
        self.sys_task(SId::WifiNet(i), "Connecting…", move |app, r: Result<(), String>| {
            match r {
                Ok(()) => app.wifi_joined(&w.ssid),
                Err(e) => app.toast("Couldn't connect", &if e.contains("Secrets were required") || e.contains("password") {
                    "Check the password and try again.".to_string() } else { e }, 2),
            }
            app.sys_changed();
        }, move || system::wifi_connect(&ssid, None));
    }

    /// A network was joined (from the list or the Join card): show it at once, without waiting
    /// for the next reading, and select the Internet row, which now names it. The list closes,
    /// so the selection must not stay where the network was.
    pub fn wifi_joined(&mut self, ssid: &str) {
        self.toast("Connected", ssid, 1);
        for w in &mut self.sys.wifi {
            w.in_use = w.ssid == ssid;
        }
        self.sys.network.wifi = ssid.into();
        if self.sys.network.kind != "Cable" {
            self.sys.network = Network { kind: "Wi-Fi", name: ssid.into(), wifi: ssid.into() };
        }
        self.sys.wifi_open = false;
        self.sys_changed();
        if self.overlay == Overlay::Settings {
            self.refresh_settings();
            self.select_setting(SId::Internet);
        }
    }

    /// The launcher changed the system: read it again now.
    pub fn sys_changed(&mut self) {
        self.sys.epoch += 1;
        self.sys.polled = 0.0;
        self.sys_poll();
    }

    /// Run `job` off the UI thread with `text` on row `id`, then `done` with its result.
    fn sys_task<T: Send + 'static>(&mut self, id: SId, text: &str, done: impl FnOnce(&mut App, T) + Send + 'static,
        job: impl FnOnce() -> T + Send + 'static) {
        self.sys.busy = Some((id, text.into()));
        std::thread::spawn(move || {
            let value = job();
            let _ = slint::invoke_from_event_loop(move || with_app(move |app| {
                app.sys.busy = None;
                done(app, value);
                if app.overlay == Overlay::Settings {
                    app.refresh_settings();
                }
            }));
        });
    }
}

/// A saved Wi-Fi connection exists for this network (it connects without asking).
fn saved_wifi(ssid: &str) -> bool {
    std::process::Command::new("nmcli").args(["-t", "-f", "NAME,TYPE", "connection", "show"]).output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.replace("\\:", ":").strip_suffix(":802-11-wireless") == Some(ssid)))
}

/// Networks listed before "More networks…".
const WIFI_SHOWN: usize = 6;

/// Signal bars (1–4) for a signal strength in percent.
fn bars(signal: u8) -> i32 {
    match signal {
        75.. => 4,
        50..=74 => 3,
        30..=49 => 2,
        _ => 1,
    }
}

fn strength(signal: u8) -> &'static str {
    match signal {
        70.. => "Strong",
        40..=69 => "Good",
        _ => "Weak",
    }
}
