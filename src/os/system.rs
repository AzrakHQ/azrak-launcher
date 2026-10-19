//! The console side of AzrakOS: there is no desktop to fall back to, so the launcher
//! itself connects to Wi-Fi, pairs controllers, picks the sound output, restarts, shuts down and
//! reports system updates. Everything goes through the standard tools (NetworkManager, BlueZ,
//! PipeWire, systemd, rpm-ostree); on an ordinary desktop none of it is shown.

use std::path::Path;
use std::process::Command;

/// Written into the OS image (built in github.com/AzrakHQ/azrak-os).
pub const MARKER: &str = "/usr/lib/azrak-os/release";

/// Running on AzrakOS (or pretending to, for screenshots: `AZRAK_OS=1`). Also on a PS5 Launcher
/// OS image that hasn't updated to AzrakOS yet.
pub fn on_os() -> bool {
    std::env::var_os("AZRAK_OS").is_some_and(|v| v == "1") || Path::new(MARKER).exists() || Path::new(crate::rename::OLD_OS_MARKER).exists()
}

/// Run a system tool and wait at most `seconds` (some wait forever for missing hardware, e.g.
/// bluetoothctl without a Bluetooth adapter).
fn run_for(seconds: u64, program: &str, args: &[&str]) -> Result<String, String> {
    use std::process::Stdio;
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|_| format!("{program} isn't installed"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} didn't answer"));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().chain(text.lines()).map(str::trim).find(|l| !l.is_empty()).unwrap_or("failed")
            .trim_start_matches("Error: ").to_string())
    }
}

/// Split one line of `nmcli -t` output: fields are ':'-separated, with '\:' inside a field.
fn nm_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => fields.last_mut().unwrap().extend(chars.next()),
            ':' => fields.push(String::new()),
            c => fields.last_mut().unwrap().push(c),
        }
    }
    fields
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    run_for(10, program, args)
}

// ------------------------------------------------------------------ network

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Network {
    /// "Wi-Fi", "Cable" or empty for none.
    pub kind: &'static str,
    /// The Wi-Fi network's name (or the cable's connection).
    pub name: String,
    /// The Wi-Fi network connected too, even when a cable is the one in use.
    pub wifi: String,
}

pub fn network() -> Network {
    run("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]).map(|t| parse_network(&t)).unwrap_or_default()
}

fn parse_network(text: &str) -> Network {
    let rows: Vec<Vec<String>> = text.lines().map(nm_fields).collect();
    let connected = |kind: &str| rows.iter().find(|f| f.len() >= 3 && f[0] == kind && f[1] == "connected");
    let wifi = connected("wifi").map(|f| f[2].clone()).unwrap_or_default();
    if let Some(f) = connected("ethernet") {
        return Network { kind: "Cable", name: f[2].clone(), wifi };
    }
    if wifi.is_empty() { Network::default() } else { Network { kind: "Wi-Fi", name: wifi.clone(), wifi } }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wifi {
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
    /// The old WEP security, whose passwords can be as short as 5 characters.
    pub wep: bool,
    pub in_use: bool,
}

/// Nearby Wi-Fi networks, strongest first, one entry per name.
pub fn wifi_list() -> Result<Vec<Wifi>, String> {
    run_for(30, "nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", "yes"]).map(|t| parse_wifi(&t))
}

fn parse_wifi(text: &str) -> Vec<Wifi> {
    let mut list: Vec<Wifi> = text.lines().map(nm_fields).filter(|f| f.len() >= 4 && !f[1].is_empty()).map(|f| Wifi {
        in_use: f[0] == "*",
        ssid: f[1].clone(),
        signal: f[2].parse().unwrap_or(0),
        secured: !f[3].is_empty() && f[3] != "--",
        wep: f[3].contains("WEP"),
    }).collect();
    list.sort_by(|a, b| b.in_use.cmp(&a.in_use).then(b.signal.cmp(&a.signal)));
    let mut seen = std::collections::HashSet::new();
    list.retain(|w| seen.insert(w.ssid.clone()));
    list
}

/// Join a Wi-Fi network (a saved one needs no password).
pub fn wifi_connect(ssid: &str, password: Option<&str>) -> Result<(), String> {
    let mut args = vec!["device", "wifi", "connect", ssid];
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        args.extend(["password", p]);
    }
    run_for(60, "nmcli", &args).map(|_| ())
}

// ------------------------------------------------------------------ Bluetooth

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub address: String,
    pub name: String,
}

fn parse_devices(text: &str) -> Vec<Device> {
    text.lines().filter_map(|l| {
        let rest = l.trim().strip_prefix("Device ")?;
        let (address, name) = rest.split_once(' ').unwrap_or((rest, ""));
        // Unnamed devices show their address again (with dashes): not worth listing.
        (address.len() == 17 && !name.is_empty() && name.replace('-', ":") != address)
            .then(|| Device { address: address.into(), name: name.trim().into() })
    }).collect()
}

/// This PC has a Bluetooth adapter.
pub fn bt_available() -> bool {
    std::fs::read_dir("/sys/class/bluetooth").is_ok_and(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("hci")))
}

/// Paired controllers and headsets, and which are connected.
pub fn bt_paired() -> Vec<(Device, bool)> {
    if !bt_available() {
        return vec![];
    }
    let connected: Vec<Device> = run("bluetoothctl", &["devices", "Connected"]).map(|t| parse_devices(&t)).unwrap_or_default();
    run("bluetoothctl", &["devices", "Paired"]).map(|t| parse_devices(&t)).unwrap_or_default().into_iter()
        .map(|d| { let on = connected.contains(&d); (d, on) }).collect()
}

/// Look for devices in pairing mode for `seconds`, then list the new ones.
pub fn bt_scan(seconds: u32) -> Vec<Device> {
    let _ = run("bluetoothctl", &["power", "on"]);
    let _ = run_for(seconds as u64 + 5, "bluetoothctl", &["--timeout", &seconds.to_string(), "scan", "on"]);
    let paired: Vec<Device> = bt_paired().into_iter().map(|(d, _)| d).collect();
    run("bluetoothctl", &["devices"]).map(|t| parse_devices(&t)).unwrap_or_default()
        .into_iter().filter(|d| !paired.contains(d)).collect()
}

/// Pair, trust (so it reconnects by itself) and connect.
pub fn bt_pair(address: &str) -> Result<(), String> {
    run_for(30, "bluetoothctl", &["--timeout", "25", "pair", address])?;
    run("bluetoothctl", &["trust", address])?;
    run_for(20, "bluetoothctl", &["--timeout", "15", "connect", address]).map(|_| ())
}

/// Connect a paired device that's on (a controller turned on reconnects by itself; this is for
/// one that didn't, e.g. a headset).
pub fn bt_connect(address: &str) -> Result<(), String> {
    run_for(20, "bluetoothctl", &["--timeout", "15", "connect", address]).map(|_| ())
}

pub fn bt_remove(address: &str) -> Result<(), String> {
    run("bluetoothctl", &["remove", address]).map(|_| ())
}

// ------------------------------------------------------------------ sound

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub name: String,
    pub label: String,
}

/// Sound outputs and the current one's index.
pub fn outputs() -> (Vec<Output>, Option<usize>) {
    let list = run("pactl", &["-f", "json", "list", "sinks"]).map(|t| parse_outputs(&t)).unwrap_or_default();
    let current = run("pactl", &["get-default-sink"]).ok().map(|s| s.trim().to_string());
    let index = current.and_then(|c| list.iter().position(|o| o.name == c));
    (list, index)
}

fn parse_outputs(json: &str) -> Vec<Output> {
    let Ok(serde_json::Value::Array(sinks)) = serde_json::from_str(json) else { return vec![] };
    sinks.iter().filter_map(|s| {
        let name = s["name"].as_str()?.to_string();
        let description = s["description"].as_str().unwrap_or(&name);
        let port = s["active_port"].as_str().unwrap_or("");
        // "GA102 High Definition Audio Controller Digital Stereo (HDMI)" says less than "TV (HDMI)".
        let label = if port.starts_with("hdmi") || description.contains("HDMI") { "TV or monitor (HDMI)".to_string() }
            else if port.contains("headphones") { "Headphones".to_string() }
            else { description.replace(" Analog Stereo", "").replace(" Digital Stereo", "") };
        Some(Output { name, label })
    }).collect()
}

pub fn set_output(name: &str) -> Result<(), String> {
    run("pactl", &["set-default-sink", name]).map(|_| ())
}

// ------------------------------------------------------------------ installing from the USB stick

/// The AzrakOS USB stick's installer (github.com/AzrakHQ/azrak-os, live/), or a stand-in for
/// screenshots and tests (`AZRAK_INSTALLER`, run as the user instead of through pkexec).
fn installer() -> (String, bool) {
    match std::env::var("AZRAK_INSTALLER") {
        Ok(p) if !p.is_empty() => (p, true),
        _ => ("/usr/libexec/azrak-os/install".into(), false),
    }
}

/// Running from the AzrakOS USB stick, so "Install AzrakOS on this PC" is offered (or pretending
/// to, for screenshots: `AZRAK_USB=1`).
pub fn on_usb() -> bool {
    if std::env::var_os("AZRAK_USB").is_some_and(|v| v == "1") {
        return true;
    }
    on_os() && Path::new(&installer().0).exists()
        && std::fs::read_to_string("/proc/cmdline").is_ok_and(|c| c.split_whitespace().any(|a| a == "rd.live.image"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub path: String,
    pub bytes: u64,
    pub model: String,
    /// Windows (an NTFS or BitLocker partition) is on it.
    pub windows: bool,
    pub usb: bool,
}

impl Disk {
    /// "512 GB": disks are sold in decimal gigabytes, so that's the number on the box.
    pub fn size(&self) -> String {
        let gb = self.bytes as f64 / 1e9;
        if gb >= 1000.0 { format!("{:.1} TB", gb / 1000.0).replace(".0 TB", " TB") } else { format!("{gb:.0} GB") }
    }
}

/// The disks AzrakOS can be installed on (not the USB stick it runs from, nothing under 40 GB).
pub fn install_disks() -> Result<Vec<Disk>, String> {
    run(&installer().0, &["--disks"]).map(|t| parse_disks(&t))
}

fn parse_disks(text: &str) -> Vec<Disk> {
    text.lines().filter_map(|l| {
        let mut f = l.splitn(6, ' ');
        if f.next()? != "disk" {
            return None;
        }
        let path = f.next()?.to_string();
        let bytes = f.next()?.parse().ok()?;
        let windows = f.next()? == "1";
        let usb = f.next()? == "1";
        let model = f.next().unwrap_or("Disk").replace('_', " ").trim().to_string();
        Some(Disk { path, bytes, model, windows, usb })
    }).collect()
}

/// One line the installer printed.
#[derive(Clone, Debug, PartialEq)]
pub enum InstallEvent {
    Progress(f32, String),
    /// Installed; the edition (`minimal` or `minimal-nvidia`), whether Secure Boot's key screen
    /// comes at the restart, and whether NVIDIA's driver downloads on the installed system.
    Done { tag: String, mok: bool, nvidia_later: bool },
    Failed(String),
}

pub fn parse_install_line(line: &str) -> Option<InstallEvent> {
    let (word, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
    match word {
        "progress" => {
            let (pct, text) = rest.split_once(' ').unwrap_or((rest, ""));
            Some(InstallEvent::Progress(pct.parse::<f32>().ok()? / 100.0, text.to_string()))
        }
        "done" => {
            let mut f = rest.split_whitespace();
            let tag = f.next().unwrap_or("minimal").to_string();
            let flags: Vec<&str> = f.collect();
            Some(InstallEvent::Done { tag, mok: flags.contains(&"mok"), nvidia_later: flags.contains(&"nvidia-later") })
        }
        "error" => Some(InstallEvent::Failed(rest.to_string())),
        _ => None,
    }
}

/// Erase `disk` and install AzrakOS on it, as root through pkexec (the stick allows it for the
/// console's user without a password). `each` gets every line the installer prints; the result
/// is how it ended.
pub fn install(disk: &str, mut each: impl FnMut(InstallEvent)) -> InstallEvent {
    use std::io::BufRead;
    use std::process::Stdio;
    let (program, stand_in) = installer();
    let mut command = if stand_in { Command::new(&program) } else { Command::new("pkexec") };
    if !stand_in {
        command.arg(&program);
    }
    let child = command.arg(disk).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return InstallEvent::Failed(format!("The installer didn't start: {e}")),
    };
    let mut end = None;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            match parse_install_line(&line) {
                Some(e @ InstallEvent::Progress(..)) => each(e),
                Some(e) => end = Some(e),
                None => {}
            }
        }
    }
    let ok = child.wait().is_ok_and(|s| s.success());
    end.unwrap_or_else(|| InstallEvent::Failed(if ok { "The installer stopped without finishing.".into() } else { "The installer stopped.".into() }))
}

// ------------------------------------------------------------------ power and updates

pub fn restart() -> Result<(), String> {
    run("systemctl", &["reboot"]).map(|_| ())
}

pub fn shut_down() -> Result<(), String> {
    run("systemctl", &["poweroff"]).map(|_| ())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Update {
    /// The running system's version, e.g. "44.20261007".
    pub version: String,
    /// A newer system is downloaded and starts at the next restart.
    pub staged: Option<String>,
}

pub fn update_status() -> Option<Update> {
    run("rpm-ostree", &["status", "--json"]).ok().and_then(|t| parse_update(&t))
}

fn parse_update(json: &str) -> Option<Update> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let deployments = v["deployments"].as_array()?;
    let version = |d: &serde_json::Value| d["version"].as_str().map(str::to_string)
        .or_else(|| d["container-image-reference-digest"].as_str().map(|s| s.chars().rev().take(7).collect::<String>().chars().rev().collect()))
        .unwrap_or_default();
    let booted = deployments.iter().find(|d| d["booted"].as_bool() == Some(true))?;
    let staged = deployments.iter().find(|d| d["staged"].as_bool() == Some(true)).map(version);
    Some(Update { version: version(booted), staged })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_networkmanager() {
        assert_eq!(parse_network("ethernet:unavailable:\nwifi:connected:Home\\: 5G\nloopback:connected (externally):lo\n"),
            Network { kind: "Wi-Fi", name: "Home: 5G".into(), wifi: "Home: 5G".into() });
        // A cable is used before Wi-Fi; the Wi-Fi network stays connected as a backup.
        assert_eq!(parse_network("ethernet:connected:Wired connection 1\nwifi:connected:Home\n"),
            Network { kind: "Cable", name: "Wired connection 1".into(), wifi: "Home".into() });
        assert_eq!(parse_network("wifi:disconnected:\n"), Network::default());
        let list = parse_wifi(":Cafe:40:\n*:Home\\: 5G:78:WPA2\n:Neighbour:90:WPA1 WPA2\n:Neighbour:30:WPA2\n::70:WPA2\n:Old:20:WEP\n");
        let names: Vec<_> = list.iter().map(|w| (w.ssid.as_str(), w.signal, w.secured, w.in_use)).collect();
        assert_eq!(names, vec![("Home: 5G", 78, true, true), ("Neighbour", 90, true, false), ("Cafe", 40, false, false), ("Old", 20, true, false)]);
        assert_eq!(list.iter().map(|w| w.wep).collect::<Vec<_>>(), vec![false, false, false, true]);
    }

    #[test]
    fn reads_bluetooth_devices() {
        let text = "Device 70:5A:6F:6D:EF:70 EarFun Wave Pro\nDevice A0:5A:5C:11:22:33 DualSense Wireless Controller\nDevice 4C:11:22:33:44:55 4C-11-22-33-44-55\n";
        assert_eq!(parse_devices(text).iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["EarFun Wave Pro", "DualSense Wireless Controller"]);
    }

    #[test]
    fn reads_sound_outputs() {
        let json = r#"[{"name":"alsa_output.usb-Shure_Inc_Shure_MV7-00.analog-stereo","description":"Shure MV7 Analog Stereo","active_port":"analog-output"},
            {"name":"alsa_output.pci-0000_01_00.1.hdmi-stereo","description":"GA102 High Definition Audio Controller Digital Stereo (HDMI)","active_port":"hdmi-output-0"},
            {"name":"bluez_output.70_5A_6F_6D_EF_70.1","description":"EarFun Wave Pro","active_port":"headset-output"}]"#;
        let labels: Vec<_> = parse_outputs(json).into_iter().map(|o| o.label).collect();
        assert_eq!(labels, vec!["Shure MV7", "TV or monitor (HDMI)", "EarFun Wave Pro"]);
    }

    #[test]
    fn reads_the_installer() {
        let disks = parse_disks("disk /dev/nvme0n1 512110190592 1 0 Samsung_SSD 970 EVO\ndisk /dev/sdb 2000398934016 0 1 CT2000BX500SSD1\nnoise\n");
        assert_eq!(disks.len(), 2);
        assert_eq!((disks[0].model.as_str(), disks[0].size(), disks[0].windows, disks[0].usb), ("Samsung SSD 970 EVO", "512 GB".to_string(), true, false));
        assert_eq!((disks[1].size(), disks[1].usb), ("2 TB".to_string(), true));
        assert_eq!(parse_install_line("progress 35 Copying AzrakOS"), Some(InstallEvent::Progress(0.35, "Copying AzrakOS".into())));
        assert_eq!(parse_install_line("done minimal-nvidia mok"), Some(InstallEvent::Done { tag: "minimal-nvidia".into(), mok: true, nvidia_later: false }));
        assert_eq!(parse_install_line("done minimal nvidia-later"), Some(InstallEvent::Done { tag: "minimal".into(), mok: false, nvidia_later: true }));
        assert_eq!(parse_install_line("done minimal"), Some(InstallEvent::Done { tag: "minimal".into(), mok: false, nvidia_later: false }));
        assert_eq!(parse_install_line("error Installing on /dev/sda failed."), Some(InstallEvent::Failed("Installing on /dev/sda failed.".into())));
        assert_eq!(parse_install_line("Deploying container image"), None);
    }

    #[test]
    fn reads_system_updates() {
        let json = r#"{"deployments":[{"booted":false,"staged":true,"version":"44.20261008"},{"booted":true,"version":"44.20261007"},{"booted":false,"version":"44.20261001"}]}"#;
        assert_eq!(parse_update(json), Some(Update { version: "44.20261007".into(), staged: Some("44.20261008".into()) }));
        assert_eq!(parse_update(r#"{"deployments":[{"booted":true,"version":"44.1"}]}"#).unwrap().staged, None);
    }
}
