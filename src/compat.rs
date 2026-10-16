//! Community compatibility lists, by title ID. Each emulator brings its own list and report form
//! (`Emulator::compat`); this file holds what they share: the statuses, the cache and refresh, and
//! your own results on top.

use crate::util::{atomic_write, cache_dir, http_get};
use std::collections::HashMap;

pub const REFRESH: f64 = 6.0 * 3600.0;
/// The system the report comes from, as the report forms' "Operating System" lists spell it.
pub(crate) const THIS_OS: &str = if cfg!(target_os = "macos") { "macOS" } else { "Linux" };

/// What a report form needs to know about a rating.
pub struct Report<'a> {
    pub name: &'a str,
    pub title_id: &'a str,
    pub emulator_version: &'a str,
    pub game_version: &'a str,
    pub status: Status,
    pub log: &'a str,
}

/// An emulator's community compatibility list (`Emulator::compat`): where to get it, how to read
/// it, and how to file a report. Everything else (the cache, refresh, merging with your own
/// results, the chips and tags) is shared.
#[derive(Clone, Copy)]
pub struct Source {
    pub url: &'static str,
    /// The cached copy's file name in the launcher's cache folder.
    pub cache: &'static str,
    /// The human-readable list, opened from a game's menu.
    pub page: &'static str,
    pub parse: fn(&[u8]) -> Option<Db>,
    /// The report form, pre-filled. Nothing is sent until the player reviews and submits it.
    pub report: fn(&Report) -> String,
}

fn sources() -> Vec<Source> {
    crate::emulator::all().iter().filter_map(|e| e.compat()).collect()
}

/// The community list page for a console's emulator.
pub fn list_page(platform: crate::platform::Platform) -> &'static str {
    match platform {
        // How Windows games run through Proton/Wine, reported by players.
        crate::platform::Platform::Pc => "https://www.protondb.com/",
        _ => crate::emulator::for_platform(platform).and_then(|e| e.compat()).map_or("", |c| c.page),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status {
    // Ordered best first (used for sorting).
    InGame,
    MainMenu,
    Logo,
    DoesntBoot,
}

impl Status {
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "InGame" => Status::InGame,
            "MainMenu" => Status::MainMenu,
            "Logo" => Status::Logo,
            "DoesntBoot" => Status::DoesntBoot,
            _ => return None,
        })
    }

    /// The community list's spelling, also used in results.json.
    pub fn key(self) -> &'static str {
        match self {
            Status::InGame => "InGame",
            Status::MainMenu => "MainMenu",
            Status::Logo => "Logo",
            Status::DoesntBoot => "DoesntBoot",
        }
    }

    /// The option text in KytyPS5's report form.
    pub(crate) fn form_label(self) -> &'static str {
        match self {
            Status::InGame => "In game",
            Status::MainMenu => "Main menu",
            Status::Logo => "Logo",
            Status::DoesntBoot => "Doesn't boot",
        }
    }

    /// Short label for badges and chips.
    pub fn label(self) -> &'static str {
        match self {
            Status::InGame => "In-game",
            Status::MainMenu => "Menus",
            Status::Logo => "Boots",
            Status::DoesntBoot => "Doesn't boot",
        }
    }

    /// One-line explanation for the Game Hub.
    pub fn meaning(self) -> &'static str {
        match self {
            Status::InGame => "Reaches gameplay",
            Status::MainMenu => "Main menu, not gameplay",
            Status::Logo => "Intro logos, then stops",
            Status::DoesntBoot => "Doesn't start",
        }
    }

    /// Index into the colour palette in ui/theme.slint (Theme.compat-color).
    pub fn level(self) -> i32 {
        match self {
            Status::InGame => 1,
            Status::MainMenu => 2,
            Status::Logo => 3,
            Status::DoesntBoot => 4,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// The Linux result when someone tested on Linux; otherwise the best result elsewhere
    /// (`on_linux` says which). The list's own top-level status is the best of any OS, which
    /// would let a Windows "In-game" hide a Linux "Doesn't boot".
    pub status: Status,
    pub on_linux: bool,
    /// `status` is your own result from this PC (results.json), not the community's.
    pub mine: bool,
    /// The community's Linux result, when someone tested there.
    pub linux: Option<Status>,
    /// Status on Windows, when someone tested there.
    pub windows: Option<Status>,
    pub reports: u64,
    /// Kyty build it was last tested on, e.g. "KytyPS5-2026-08-16-bc2f077".
    pub version: String,
    /// Platforms with reports, e.g. ["Linux", "Windows"].
    pub platforms: Vec<String>,
}

impl Entry {
    /// Where `status` comes from: "Linux", or the OS it was tested on instead.
    pub fn source(&self) -> &'static str {
        if self.on_linux {
            "Linux"
        } else if self.windows.is_some() {
            "Windows"
        } else if self.platforms.iter().any(|p| p == "macOS") {
            "macOS"
        } else {
            "another OS"
        }
    }

    /// Reaches gameplay in any report: Linux, Windows, another OS or your own.
    pub fn in_game_anywhere(&self) -> bool {
        [Some(self.status), self.linux, self.windows].contains(&Some(Status::InGame))
    }

    /// Short tag for Library covers: "In-game", or "Win · In-game" for a result from elsewhere.
    pub fn tag(&self) -> String {
        match self.source() {
            "Linux" => self.status.label().to_string(),
            "Windows" => format!("Win · {}", self.status.label()),
            "macOS" => format!("Mac · {}", self.status.label()),
            _ => format!("Other OS · {}", self.status.label()),
        }
    }

    /// Chip text: "In-game on Linux", or "In-game on Windows · untested on Linux".
    pub fn chip_text(&self) -> String {
        if self.mine {
            format!("{} on Linux · your result", self.status.label())
        } else if self.on_linux {
            format!("{} on Linux", self.status.label())
        } else {
            format!("{} on {} · untested on Linux", self.status.label(), self.source())
        }
    }

    /// "2026-08-16" out of whatever version text the report has.
    pub fn tested_date(&self) -> String {
        let v = &self.version;
        for (i, _) in v.char_indices() {
            let s = &v[i..];
            if s.len() >= 10 && crate::util::parse_iso_date(s).is_some() && s.as_bytes()[4] == b'-' {
                return s[..10].to_string();
            }
        }
        v.trim_start_matches('v').to_string()
    }
}

pub type Db = HashMap<String, Entry>;

/// The community list with your own results on top: on this PC, what you saw wins.
pub fn with_mine(mut db: Db, mine: &crate::results::Results) -> Db {
    for (tid, r) in &mine.games {
        let Some(status) = r.status() else { continue };
        let e = db.entry(tid.clone()).or_insert_with(|| Entry {
            status,
            on_linux: true,
            mine: true,
            linux: None,
            windows: None,
            reports: 0,
            version: String::new(),
            platforms: Vec::new(),
        });
        e.status = status;
        e.on_linux = true;
        e.mine = true;
    }
    db
}

/// Every emulator's list from the cache (instant), and the age in seconds of the oldest one.
pub fn load() -> (Db, f64) {
    let age_of = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok()
        .and_then(|t| t.elapsed().ok()).map(|d| d.as_secs_f64()).unwrap_or(f64::MAX);
    let (mut db, mut age) = (Db::new(), 0.0f64);
    for source in sources() {
        let file = cache_dir().join(source.cache);
        age = age.max(age_of(&file));
        db.extend(std::fs::read(&file).ok().and_then(|b| (source.parse)(&b)).unwrap_or_default());
    }
    (db, age)
}

/// Download every list; a list that can't be fetched keeps its cached copy.
pub fn fetch() -> Result<Db, String> {
    let sources = sources();
    let mut errors = Vec::new();
    for source in &sources {
        match http_get(source.url).and_then(|b| (source.parse)(&b).map(|_| b).ok_or_else(|| "unexpected data".to_string())) {
            Ok(bytes) => { let _ = atomic_write(&cache_dir().join(source.cache), &bytes); }
            Err(e) => {
                crate::log!("compatibility list {} unavailable: {e}", source.url);
                errors.push(e);
            }
        }
    }
    if !sources.is_empty() && errors.len() == sources.len() {
        return Err(errors.join("; "));
    }
    Ok(load().0)
}

/// The report form for the game's console, from its emulator. Empty for a console without one.
pub fn report_url_for(name: &str, title_id: &str, emulator_version: &str, game_version: &str, status: Status, log: &str) -> String {
    let report = Report { name, title_id, emulator_version, game_version, status, log };
    crate::platform::Platform::of_title_id(title_id)
        .and_then(crate::emulator::for_platform)
        .and_then(|e| e.compat())
        .map_or_else(String::new, |c| (c.report)(&report))
}

/// The last lines of a log, sized to keep the link well under GitHub's URL limit.
pub(crate) fn log_excerpt(log: &str) -> String {
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    // KytyPS5 prints what went wrong after "--- Error ---": lead with that block when there is one.
    let error = lines.iter().rposition(|l| l.trim() == "--- Error ---");
    fn fit<'a>(lines: &[&'a str], budget: usize) -> Vec<&'a str> {
        let mut out = Vec::new();
        let mut len = 0;
        for line in lines.iter().take(40) {
            len += line.len() + 1;
            if len > budget {
                break;
            }
            out.push(*line);
        }
        out
    }
    let excerpt = match error {
        Some(i) => fit(&lines[i..], 2500),
        None => {
            let mut tail = fit(&lines.iter().rev().copied().collect::<Vec<_>>(), 2500);
            tail.reverse();
            tail
        }
    };
    if excerpt.is_empty() {
        return String::new();
    }
    let what = if error.is_some() { "The error from the log" } else { "Last lines of the log" };
    format!("{what} (full log attached below):\n```\n{}\n```\n", excerpt.join("\n"))
}

#[derive(Default)]
pub(crate) struct SystemInfo {
    pub os: String,
    pub cpu: String,
    pub gpu: String,
    pub ram: String,
}

#[cfg(target_os = "linux")]
pub(crate) fn system_info() -> SystemInfo {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let field = |text: &str, key: &str| {
        text.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim_start_matches([' ', '\t', ':', '=']).trim().trim_matches('"').to_string()).unwrap_or_default()
    };
    let mut os = field(&read("/etc/os-release"), "PRETTY_NAME");
    let kernel = read("/proc/sys/kernel/osrelease").trim().to_string();
    if !kernel.is_empty() {
        os = format!("{os} (kernel {kernel})").trim().to_string();
    }
    let ram = field(&read("/proc/meminfo"), "MemTotal")
        .trim_end_matches("kB")
        .trim()
        .parse::<f64>()
        .map(|kb| format!("{:.0} GB RAM", kb / 1024.0 / 1024.0))
        .unwrap_or_default();
    // lspci names the graphics card; it's optional, so leave the field empty without it.
    let gpu = std::process::Command::new("lspci")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("VGA compatible controller") || l.contains("3D controller") || l.contains("Display controller"))
        .filter_map(|l| l.splitn(3, ':').nth(2).map(|s| s.trim().to_string()))
        .collect::<Vec<_>>()
        .join("; ");
    SystemInfo { os, cpu: field(&read("/proc/cpuinfo"), "model name"), gpu, ram }
}

/// GPU names from `system_profiler SPDisplaysDataType` ("Chipset Model: Apple M2 Pro").
/// Plain text so it can be tested anywhere.
#[cfg(any(target_os = "macos", test))]
fn parse_chipsets(text: &str) -> String {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("Chipset Model:"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(target_os = "macos")]
pub(crate) fn system_info() -> SystemInfo {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let version = run("sw_vers", &["-productVersion"]);
    let kernel = run("sysctl", &["-n", "kern.osrelease"]);
    let mut os = format!("macOS {version}").trim().to_string();
    if !kernel.is_empty() {
        os = format!("{os} (kernel {kernel})");
    }
    let ram = run("sysctl", &["-n", "hw.memsize"])
        .parse::<f64>()
        .map(|bytes| format!("{:.0} GB RAM", bytes / 1024.0 / 1024.0 / 1024.0))
        .unwrap_or_default();
    SystemInfo {
        os,
        cpu: run("sysctl", &["-n", "machdep.cpu.brand_string"]),
        gpu: parse_chipsets(&run("system_profiler", &["SPDisplaysDataType"])),
        ram,
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn chipsets_from_system_profiler() {
        let text = "Graphics/Displays:\n\n    Apple M2 Pro:\n\n      Chipset Model: Apple M2 Pro\n      Type: GPU\n      Bus: Built-In\n";
        assert_eq!(parse_chipsets(text), "Apple M2 Pro");
        let two = "      Chipset Model: Intel UHD Graphics 630\n      Chipset Model: AMD Radeon Pro 5500M\n";
        assert_eq!(parse_chipsets(two), "Intel UHD Graphics 630; AMD Radeon Pro 5500M");
        assert_eq!(parse_chipsets(""), "");
    }
}
