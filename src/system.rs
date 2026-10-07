//! The console side of PS5 Launcher OS: there is no desktop to fall back to, so the launcher
//! itself connects to Wi-Fi, pairs controllers, picks the sound output, restarts, shuts down and
//! reports system updates. Everything goes through the standard tools (NetworkManager, BlueZ,
//! PipeWire, systemd, rpm-ostree); on an ordinary desktop none of it is shown.

use std::path::Path;
use std::process::Command;

/// Written into the OS image (os/Containerfile).
const MARKER: &str = "/usr/lib/ps5-launcher-os/release";

/// Running on PS5 Launcher OS (or pretending to, for screenshots: `PS5_LAUNCHER_OS=1`).
pub fn on_os() -> bool {
    std::env::var_os("PS5_LAUNCHER_OS").is_some_and(|v| v == "1") || Path::new(MARKER).exists()
}

/// Started from the USB stick (a live session), so "Install to this PC" makes sense.
pub fn live() -> bool {
    std::env::var_os("PS5_LAUNCHER_OS_LIVE").is_some_and(|v| v == "1")
        || std::fs::read_to_string("/proc/cmdline").is_ok_and(|c| c.split_whitespace().any(|a| a == "rd.live.image"))
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|_| format!("{program} isn't installed"))?;
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

// ------------------------------------------------------------------ network

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Network {
    /// "Wi-Fi", "Cable" or empty for none.
    pub kind: &'static str,
    /// The Wi-Fi network's name.
    pub name: String,
}

pub fn network() -> Network {
    run("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]).map(|t| parse_network(&t)).unwrap_or_default()
}

fn parse_network(text: &str) -> Network {
    let rows: Vec<Vec<String>> = text.lines().map(nm_fields).collect();
    let connected = |kind: &str| rows.iter().find(|f| f.len() >= 3 && f[0] == kind && f[1] == "connected");
    if let Some(f) = connected("ethernet") {
        return Network { kind: "Cable", name: f[2].clone() };
    }
    connected("wifi").map(|f| Network { kind: "Wi-Fi", name: f[2].clone() }).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wifi {
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
    pub in_use: bool,
}

/// Nearby Wi-Fi networks, strongest first, one entry per name.
pub fn wifi_list() -> Result<Vec<Wifi>, String> {
    run("nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", "yes"]).map(|t| parse_wifi(&t))
}

fn parse_wifi(text: &str) -> Vec<Wifi> {
    let mut list: Vec<Wifi> = text.lines().map(nm_fields).filter(|f| f.len() >= 4 && !f[1].is_empty()).map(|f| Wifi {
        in_use: f[0] == "*",
        ssid: f[1].clone(),
        signal: f[2].parse().unwrap_or(0),
        secured: !f[3].is_empty() && f[3] != "--",
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
    run("nmcli", &args).map(|_| ())
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

/// Paired controllers and headsets, and which are connected.
pub fn bt_paired() -> Vec<(Device, bool)> {
    let connected: Vec<Device> = run("bluetoothctl", &["devices", "Connected"]).map(|t| parse_devices(&t)).unwrap_or_default();
    run("bluetoothctl", &["devices", "Paired"]).map(|t| parse_devices(&t)).unwrap_or_default().into_iter()
        .map(|d| { let on = connected.contains(&d); (d, on) }).collect()
}

/// Look for devices in pairing mode for `seconds`, then list the new ones.
pub fn bt_scan(seconds: u32) -> Vec<Device> {
    let _ = run("bluetoothctl", &["power", "on"]);
    let _ = run("bluetoothctl", &["--timeout", &seconds.to_string(), "scan", "on"]);
    let paired: Vec<Device> = bt_paired().into_iter().map(|(d, _)| d).collect();
    run("bluetoothctl", &["devices"]).map(|t| parse_devices(&t)).unwrap_or_default()
        .into_iter().filter(|d| !paired.contains(d)).collect()
}

/// Pair, trust (so it reconnects by itself) and connect.
pub fn bt_pair(address: &str) -> Result<(), String> {
    run("bluetoothctl", &["--timeout", "25", "pair", address])?;
    run("bluetoothctl", &["trust", address])?;
    run("bluetoothctl", &["--timeout", "15", "connect", address]).map(|_| ())
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
            Network { kind: "Wi-Fi", name: "Home: 5G".into() });
        assert_eq!(parse_network("ethernet:connected:Wired connection 1\nwifi:connected:Home\n").kind, "Cable");
        assert_eq!(parse_network("wifi:disconnected:\n"), Network::default());
        let list = parse_wifi(":Cafe:40:\n*:Home\\: 5G:78:WPA2\n:Neighbour:90:WPA1 WPA2\n:Neighbour:30:WPA2\n::70:WPA2\n");
        let names: Vec<_> = list.iter().map(|w| (w.ssid.as_str(), w.signal, w.secured, w.in_use)).collect();
        assert_eq!(names, vec![("Home: 5G", 78, true, true), ("Neighbour", 90, true, false), ("Cafe", 40, false, false)]);
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
    fn reads_system_updates() {
        let json = r#"{"deployments":[{"booted":false,"staged":true,"version":"44.20261008"},{"booted":true,"version":"44.20261007"},{"booted":false,"version":"44.20261001"}]}"#;
        assert_eq!(parse_update(json), Some(Update { version: "44.20261007".into(), staged: Some("44.20261008".into()) }));
        assert_eq!(parse_update(r#"{"deployments":[{"booted":true,"version":"44.1"}]}"#).unwrap().staged, None);
    }
}
