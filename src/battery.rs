//! The PC's battery, for laptops: the top bar shows how much is left (next to the clock), and a
//! notice comes up when it runs low, so a game doesn't just stop when it runs out. On AzrakOS the
//! launcher is the whole screen, so this is the only place it shows; elsewhere too, it's handy
//! in full screen. PCs without a battery show nothing.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    /// 0–100.
    pub percent: u8,
    /// Plugged in (charging, or full on the charger).
    pub plugged: bool,
}

/// Low-battery notices: at 20% and again at 10%, once each while it's discharging.
pub const LOW: u8 = 20;
pub const CRITICAL: u8 = 10;

/// The PC's own batteries (not a controller's or a mouse's, which report `scope=Device`), as one:
/// laptops with two add up.
pub fn read() -> Option<Battery> {
    read_from(Path::new("/sys/class/power_supply"))
}

fn read_from(root: &Path) -> Option<Battery> {
    let read = |dir: &Path, file: &str| std::fs::read_to_string(dir.join(file)).ok().map(|s| s.trim().to_string());
    let (mut now, mut full, mut percents) = (0u64, 0u64, Vec::new());
    let mut plugged = false;
    let mut found = false;
    let mut entries: Vec<_> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for dir in entries {
        match read(&dir, "type").as_deref() {
            Some("Mains") | Some("USB") if read(&dir, "online").as_deref() == Some("1") => plugged = true,
            Some("Battery") if read(&dir, "scope").as_deref() != Some("Device") => {
                // A battery that's absent (a removable one, taken out) reports present=0.
                if read(&dir, "present").as_deref() == Some("0") {
                    continue;
                }
                found = true;
                match read(&dir, "status").as_deref() {
                    Some("Charging") | Some("Full") | Some("Not charging") => plugged = true,
                    _ => {}
                }
                let num = |f: &str| read(&dir, f).and_then(|s| s.parse::<u64>().ok());
                match (num("energy_now").or(num("charge_now")), num("energy_full").or(num("charge_full"))) {
                    (Some(n), Some(f)) if f > 0 => {
                        now += n;
                        full += f;
                    }
                    _ => percents.extend(num("capacity")),
                }
            }
            _ => {}
        }
    }
    if !found {
        return None;
    }
    let percent = if full > 0 {
        (now * 100 / full).min(100)
    } else if !percents.is_empty() {
        (percents.iter().sum::<u64>() / percents.len() as u64).min(100)
    } else {
        return None;
    };
    Some(Battery { percent: percent as u8, plugged })
}

/// The notice to show now, if any: `warned` is the lowest level already warned about while
/// discharging (100 when none), and is reset once the PC is plugged in.
pub fn notice(b: Battery, warned: &mut u8) -> Option<(&'static str, String)> {
    if b.plugged {
        *warned = 100;
        return None;
    }
    let level = if b.percent <= CRITICAL { CRITICAL } else if b.percent <= LOW { LOW } else { return None };
    if level >= *warned {
        return None;
    }
    *warned = level;
    Some(if level == CRITICAL {
        ("Battery almost empty", format!("{}% left. Plug in the charger now, or save your game.", b.percent))
    } else {
        ("Battery low", format!("{}% left. Plug in the charger soon.", b.percent))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (f, v) in files {
            std::fs::write(dir.join(f), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn reads_laptop_batteries() {
        let root = std::env::temp_dir().join(format!("azrak-battery-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // No battery (a desktop): nothing to show.
        supply(&root, "AC", &[("type", "Mains"), ("online", "0")]);
        assert_eq!(read_from(&root), None);
        // The test laptop: capacity only, charging.
        supply(&root, "BAT0", &[("type", "Battery"), ("status", "Charging"), ("capacity", "8")]);
        assert_eq!(read_from(&root), Some(Battery { percent: 8, plugged: true }));
        // A controller's battery doesn't count; two laptop batteries add up by energy.
        supply(&root, "ps-controller-battery", &[("type", "Battery"), ("scope", "Device"), ("capacity", "100")]);
        supply(&root, "BAT0", &[("type", "Battery"), ("status", "Discharging"), ("energy_now", "10000000"), ("energy_full", "40000000")]);
        supply(&root, "BAT1", &[("type", "Battery"), ("status", "Discharging"), ("energy_now", "30000000"), ("energy_full", "40000000")]);
        assert_eq!(read_from(&root), Some(Battery { percent: 50, plugged: false }));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn warns_once_per_level() {
        let mut warned = 100;
        let at = |p: u8, plugged: bool, w: &mut u8| notice(Battery { percent: p, plugged }, w).map(|(t, _)| t);
        assert_eq!(at(50, false, &mut warned), None);
        assert_eq!(at(20, false, &mut warned), Some("Battery low"));
        assert_eq!(at(18, false, &mut warned), None);
        assert_eq!(at(9, false, &mut warned), Some("Battery almost empty"));
        assert_eq!(at(5, false, &mut warned), None);
        // Plugged in, then unplugged low again: warned again.
        assert_eq!(at(7, true, &mut warned), None);
        assert_eq!(at(7, false, &mut warned), Some("Battery almost empty"));
    }
}
