//! Downloads through a VPN. The user's own VPN (any provider or app: WireGuard, OpenVPN, …) is
//! found by its network interface, and the torrent engine is bound to that interface
//! (SO_BINDTODEVICE), so torrent traffic can't leave by any other route: if the VPN goes away,
//! connections fail instead of falling back to the real address. VPNs can also be added from a
//! provider's file and switched on and off through NetworkManager.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vpn {
    pub name: String,
    pub kind: &'static str,
    /// Up and with an address: traffic can go through it.
    pub connected: bool,
}

/// Interfaces that look like tunnels but don't hide your address from the internet.
const NOT_PRIVACY: [&str; 9] = ["tailscale", "zt", "docker", "virbr", "veth", "br-", "vnet", "lxc", "waydroid"];

/// The VPN interfaces on this PC (connected or not).
pub fn list() -> Vec<Vpn> {
    list_in(Path::new("/sys/class/net"), &addressed())
}

fn list_in(root: &Path, addressed: &HashSet<String>) -> Vec<Vpn> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else { return out };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "lo" || NOT_PRIVACY.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let dir = entry.path();
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        let kind = if read("uevent").lines().any(|l| l.trim() == "DEVTYPE=wireguard") {
            "WireGuard"
        } else if dir.join("tun_flags").exists() {
            "OpenVPN"
        } else if read("type").trim() == "65534" {
            "VPN" // a layer-3 tunnel without hardware addresses
        } else {
            continue;
        };
        let flags = u32::from_str_radix(read("flags").trim().trim_start_matches("0x"), 16).unwrap_or(0);
        let up = flags & 0x1 != 0;
        out.push(Vpn { connected: up && addressed.contains(&name), name, kind });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Interfaces with an IPv4 or global IPv6 address.
fn addressed() -> HashSet<String> {
    let mut names = HashSet::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return names;
    }
    let mut cursor = list;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        if !entry.ifa_addr.is_null() {
            let family = unsafe { (*entry.ifa_addr).sa_family } as i32;
            let usable = match family {
                libc::AF_INET => true,
                libc::AF_INET6 => {
                    let v6 = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
                    let segs = v6.sin6_addr.s6_addr;
                    !(segs[0] == 0xfe && segs[1] & 0xc0 == 0x80) // not link-local
                }
                _ => false,
            };
            if usable {
                let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }.to_string_lossy().into_owned();
                names.insert(name);
            }
        }
        cursor = entry.ifa_next;
    }
    unsafe { libc::freeifaddrs(list) };
    names
}

/// The connected VPN to download through: `preferred` when it's connected, otherwise (with no
/// preference) the first connected one.
pub fn active(preferred: &str) -> Option<String> {
    pick(&list(), preferred)
}

fn pick(vpns: &[Vpn], preferred: &str) -> Option<String> {
    let connected = vpns.iter().filter(|v| v.connected);
    if preferred.is_empty() {
        connected.map(|v| v.name.clone()).next()
    } else {
        connected.map(|v| v.name.clone()).find(|n| n == preferred)
    }
}

// ------------------------------------------------------------------ NetworkManager

/// A VPN connection NetworkManager knows about (WireGuard or VPN plugin type).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub name: String,
    pub active: bool,
}

pub fn nm_available() -> bool {
    Command::new("nmcli").arg("--version").output().is_ok_and(|o| o.status.success())
}

pub fn nm_saved() -> Vec<Saved> {
    let Ok(out) = Command::new("nmcli").args(["-t", "-f", "NAME,TYPE,ACTIVE", "connection", "show"]).output() else { return vec![] };
    parse_saved(&String::from_utf8_lossy(&out.stdout))
}

fn parse_saved(text: &str) -> Vec<Saved> {
    text.lines()
        .filter_map(|line| {
            // NAME may contain escaped colons; TYPE and ACTIVE don't.
            let mut parts = line.rsplitn(3, ':');
            let active = parts.next()? == "yes";
            let kind = parts.next()?;
            let name = parts.next()?.replace("\\:", ":");
            matches!(kind, "wireguard" | "vpn").then_some(Saved { name, active })
        })
        .collect()
}

fn nmcli(args: &[&str]) -> Result<String, String> {
    let out = Command::new("nmcli").args(args).output().map_err(|_| "NetworkManager (nmcli) isn't installed".to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.lines().find(|l| !l.trim().is_empty()).unwrap_or("nmcli failed").trim().trim_start_matches("Error: ").to_string())
    }
}

/// Add a VPN from a provider's file (WireGuard `.conf`, OpenVPN `.ovpn`) and connect it.
/// Returns the connection's name.
pub fn nm_import(file: &Path) -> Result<String, String> {
    let kind = match file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("conf") => "wireguard",
        Some("ovpn") => "openvpn",
        _ => return Err("Choose a WireGuard .conf or OpenVPN .ovpn file".into()),
    };
    let out = nmcli(&["connection", "import", "type", kind, "file", &file.to_string_lossy()])?;
    // "Connection 'wg-se' (uuid) successfully added."
    let name = out.split('\'').nth(1).map(str::to_string).ok_or("NetworkManager didn't name the connection")?;
    nm_up(&name)?;
    Ok(name)
}

pub fn nm_up(name: &str) -> Result<(), String> {
    nmcli(&["connection", "up", "id", name]).map(|_| ())
}

pub fn nm_down(name: &str) -> Result<(), String> {
    nmcli(&["connection", "down", "id", name]).map(|_| ())
}

/// The desktop has a file dialog tool for [`choose_file`].
pub fn has_file_dialog() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| ["kdialog", "zenity"].iter().any(|t| dir.join(t).is_file()))
}

/// A file chooser for a VPN file, from the desktop's own dialog tool. `Err` when there is no
/// dialog tool, `Ok(None)` when the player cancelled.
pub fn choose_file() -> Result<Option<std::path::PathBuf>, ()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let tries: [(&str, Vec<String>); 2] = [
        ("kdialog", vec!["--title".into(), "Add a VPN".into(), "--getopenfilename".into(), home.clone(), "*.conf *.ovpn|VPN files".into()]),
        ("zenity", vec!["--file-selection".into(), "--title=Add a VPN".into(), "--file-filter=VPN files | *.conf *.ovpn".into()]),
    ];
    for (tool, args) in tries {
        if let Ok(out) = Command::new(tool).args(&args).output() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return Ok((out.status.success() && !path.is_empty()).then(|| path.into()));
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(root: &Path, name: &str, files: &[(&str, &str)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (f, v) in files {
            std::fs::write(dir.join(f), v).unwrap();
        }
    }

    #[test]
    fn finds_vpn_tunnels_but_not_networks_that_dont_hide_your_address() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        iface(r, "lo", &[("type", "772"), ("flags", "0x9")]);
        iface(r, "enp3s0", &[("type", "1"), ("flags", "0x1003")]);
        iface(r, "wg-se", &[("type", "65534"), ("flags", "0x91"), ("uevent", "DEVTYPE=wireguard\nINTERFACE=wg-se\n")]);
        iface(r, "tun0", &[("type", "65534"), ("flags", "0x1091"), ("tun_flags", "0x1001")]);
        iface(r, "proton0", &[("type", "65534"), ("flags", "0x90")]); // down
        iface(r, "tailscale0", &[("type", "65534"), ("flags", "0x91"), ("tun_flags", "0x1001")]);
        iface(r, "docker0", &[("type", "1"), ("flags", "0x1003")]);
        let addressed: HashSet<String> = ["wg-se", "tun0", "enp3s0", "tailscale0"].iter().map(|s| s.to_string()).collect();
        let found = list_in(r, &addressed);
        let summary: Vec<_> = found.iter().map(|v| (v.name.as_str(), v.kind, v.connected)).collect();
        assert_eq!(summary, vec![("proton0", "VPN", false), ("tun0", "OpenVPN", true), ("wg-se", "WireGuard", true)]);
        assert_eq!(pick(&found, "").as_deref(), Some("tun0"));
        assert_eq!(pick(&found, "wg-se").as_deref(), Some("wg-se"));
        assert_eq!(pick(&found, "proton0"), None, "a preferred VPN that is down means: wait");
    }

    #[test]
    fn reads_networkmanager_vpn_connections() {
        let text = "Wired connection 1:802-3-ethernet:yes\nwg-se:wireguard:yes\nWork\\: office:vpn:no\nhome wifi:802-11-wireless:no\n";
        assert_eq!(parse_saved(text), vec![
            Saved { name: "wg-se".into(), active: true },
            Saved { name: "Work: office".into(), active: false },
        ]);
    }
}
