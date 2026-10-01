//! KytyPS5 compatibility reports from the community list (kytyps5.github.io), by title ID.
//! Same data KytyPS5's own launcher uses. Cached in ~/.cache/ps5-launcher/compatibility.json.

use crate::util::{atomic_write, cache_dir, http_get};
use std::collections::HashMap;

const URL: &str = "https://kytyps5.github.io/data/compatibility.json";
pub const LIST_PAGE: &str = "https://kytyps5.github.io/";
pub const REFRESH: f64 = 6.0 * 3600.0;
/// KytyPS5's "Game Emulation Status Report" form; reports there feed the list above.
const REPORT_FORM: &str = "https://github.com/KytyPS5/KytyPS5/issues/new?template=kytyps5-game-emulation.yaml";
/// shadPS4's community list for PS4 games, published as one file per update.
const SHAD_URL: &str = "https://github.com/shadps4-compatibility/shadps4-game-compatibility/releases/latest/download/compatibility_data.json";
pub const SHAD_LIST_PAGE: &str = "https://github.com/shadps4-compatibility/shadps4-game-compatibility/issues";
/// The system the report comes from, as the shadPS4 form's "Operating System" list spells it.
const THIS_OS: &str = if cfg!(target_os = "macos") { "macOS" } else { "Linux" };
const SHAD_REPORT_FORM: &str = "https://github.com/shadps4-compatibility/shadps4-game-compatibility/issues/new?template=game_compatibility.yml";

/// The community list page for a console's emulator.
pub fn list_page(platform: crate::platform::Platform) -> &'static str {
    match platform {
        crate::platform::Platform::Ps5 => LIST_PAGE,
        crate::platform::Platform::Ps4 => SHAD_LIST_PAGE,
        // How Windows games run through Proton/Wine, reported by players.
        crate::platform::Platform::Pc => "https://www.protondb.com/",
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
    fn form_label(self) -> &'static str {
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

fn path() -> std::path::PathBuf {
    cache_dir().join("compatibility.json")
}

fn shad_path() -> std::path::PathBuf {
    cache_dir().join("compatibility-shadps4.json")
}

fn shad_status(s: &str) -> Option<Status> {
    Some(match s {
        "status-playable" | "status-ingame" => Status::InGame,
        "status-menus" => Status::MainMenu,
        "status-boots" => Status::Logo,
        "status-nothing" => Status::DoesntBoot,
        _ => return None,
    })
}

/// shadPS4's list: { "CUSA04118": { "os-linux": { "status": "status-ingame", "last_tested": … }, … } }.
fn parse_shad(bytes: &[u8]) -> Option<Db> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut db = Db::new();
    for (tid, per_os) in v.as_object()? {
        let Some(per_os) = per_os.as_object() else { continue };
        let on = |os: &str| per_os.get(os).and_then(|e| e["status"].as_str()).and_then(shad_status);
        let Some(best) = per_os.values().filter_map(|e| e["status"].as_str().and_then(shad_status)).min() else { continue };
        let linux = on("os-linux");
        let mut platforms: Vec<String> = per_os.keys().filter_map(|k| match k.as_str() {
            "os-linux" => Some("Linux".to_string()),
            "os-windows" => Some("Windows".to_string()),
            "os-macOS" => Some("macOS".to_string()),
            _ => None,
        }).collect();
        platforms.sort();
        // The Linux report's test date, else the newest one.
        let tested = per_os.get("os-linux").and_then(|e| e["last_tested"].as_str())
            .or_else(|| per_os.values().filter_map(|e| e["last_tested"].as_str()).max())
            .unwrap_or_default().to_string();
        db.insert(tid.to_uppercase(), Entry {
            status: linux.unwrap_or(best),
            on_linux: linux.is_some(),
            mine: false,
            linux,
            windows: on("os-windows"),
            reports: per_os.len() as u64,
            version: tested,
            platforms,
        });
    }
    Some(db)
}

fn parse(bytes: &[u8]) -> Option<Db> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut db = Db::new();
    for (tid, e) in v.as_object()? {
        let Some(best) = e["status"].as_str().and_then(Status::parse) else { continue };
        let plat = e["platforms"].as_object();
        let mut platforms: Vec<String> = plat
            .map(|p| {
                p.keys()
                    .map(|k| match k.as_str() {
                        "linux" => "Linux".into(),
                        "windows" => "Windows".into(),
                        "macos" => "macOS".into(),
                        o => o.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        platforms.sort();
        let on = |os: &str| plat.and_then(|p| p.get(os)).and_then(|l| l["status"].as_str()).and_then(Status::parse);
        let linux = on("linux");
        db.insert(
            tid.to_uppercase(),
            Entry {
                status: linux.unwrap_or(best),
                on_linux: linux.is_some(),
                mine: false,
                linux,
                windows: on("windows"),
                reports: e["reports"].as_u64().unwrap_or(1),
                version: plat
                    .and_then(|p| p.values().filter_map(|x| x["version"].as_str()).max_by_key(|s| s.len()).map(String::from))
                    .unwrap_or_default(),
                platforms,
            },
        );
    }
    Some(db)
}

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

/// Cached copy (instant) and its age in seconds.
/// Both lists (KytyPS5 for PS5, shadPS4 for PS4) and the age of the older cached one.
pub fn load() -> (Db, f64) {
    let age_of = |p: std::path::PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok()
        .and_then(|t| t.elapsed().ok()).map(|d| d.as_secs_f64()).unwrap_or(f64::MAX);
    let age = age_of(path()).max(age_of(shad_path()));
    let mut db = std::fs::read(path()).ok().and_then(|b| parse(&b)).unwrap_or_default();
    db.extend(std::fs::read(shad_path()).ok().and_then(|b| parse_shad(&b)).unwrap_or_default());
    (db, age)
}

/// Download both lists; a list that can't be fetched keeps its cached copy.
pub fn fetch() -> Result<Db, String> {
    let mut errors = Vec::new();
    for (url, file, check) in [(URL, path(), parse as fn(&[u8]) -> Option<Db>), (SHAD_URL, shad_path(), parse_shad)] {
        match http_get(url).and_then(|b| check(&b).map(|_| b).ok_or_else(|| "unexpected data".to_string())) {
            Ok(bytes) => { let _ = atomic_write(&file, &bytes); }
            Err(e) => {
                crate::log!("compatibility list {url} unavailable: {e}");
                errors.push(e);
            }
        }
    }
    if errors.len() == 2 {
        return Err(errors.join("; "));
    }
    Ok(load().0)
}

/// The report form, pre-filled with the game, your result, this PC and the end of the emulator
/// log. Nothing is sent until the player reviews it and submits it on GitHub.
/// The report form for the game's console: KytyPS5's for PS5 games, shadPS4's for PS4 games.
pub fn report_url_for(name: &str, title_id: &str, emulator_version: &str, game_version: &str, status: Status, log: &str) -> String {
    let q = crate::util::query_escape;
    if crate::platform::Platform::of_title_id(title_id) == Some(crate::platform::Platform::Ps4) {
        let sys = system_info();
        let label = match status {
            Status::InGame => "Ingame",
            Status::MainMenu => "Menus",
            Status::Logo => "Boots",
            Status::DoesntBoot => "Nothing",
        };
        let details = format!("{} on {THIS_OS}.", status.meaning());
        let log = log_excerpt(log);
        let mut url = format!("{SHAD_REPORT_FORM}&title={}", q(&format!("{title_id} - {name}")));
        for (field, value) in [
            ("game-name", name), ("game-serial", title_id), ("game-version", game_version),
            ("emulator-version", emulator_version), ("emulation-status", label), ("Operating-System", THIS_OS),
            ("processor", &sys.cpu), ("graphics-card", &sys.gpu), ("emulation-description", &details), ("log", &log),
        ] {
            if !value.is_empty() {
                url.push_str(&format!("&{field}={}", q(value)));
            }
        }
        return url;
    }
    let kyty_version = emulator_version;
    let mut url = format!("{REPORT_FORM}&title={}", q(&format!("[GAME STATUS]: {name} ({})", THIS_OS.to_lowercase())));
    let sys = system_info();
    let log = log_excerpt(log);
    let details = format!("{} on {THIS_OS}.", status.meaning());
    for (field, value) in [
        ("game-title", name),
        ("game-id", title_id),
        ("kyty-version", kyty_version),
        ("compatibility-status", status.form_label()),
        ("what-happened", &details),
        ("steps-to-reproduce", "Start the game with KytyPS5."),
        ("expected-behavior", "The game plays normally."),
        ("os", &sys.os),
        ("cpu", &sys.cpu),
        ("gpu", &sys.gpu),
        ("ram-vram", &sys.ram),
        ("log-file", &log),
    ] {
        if !value.is_empty() {
            url.push_str(&format!("&{field}={}", q(value)));
        }
    }
    url
}

/// The last lines of a log, sized to keep the link well under GitHub's URL limit.
fn log_excerpt(log: &str) -> String {
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
struct SystemInfo {
    os: String,
    cpu: String,
    gpu: String,
    ram: String,
}

#[cfg(target_os = "linux")]
fn system_info() -> SystemInfo {
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
fn system_info() -> SystemInfo {
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
    #[test]
    fn parses_feed() {
        let json = br#"{"PPSA02929":{"status":"InGame","reports":2,"platforms":{"windows":{"status":"InGame","version":"0.2.2 (KytyPS5-2026-08-18-6bf6929)"},"linux":{"status":"MainMenu","version":"v0.3.0"}}},"PPSA1":{"status":"Weird"}}"#;
        let db = parse(json).unwrap();
        assert_eq!(db.len(), 1);
        let e = &db["PPSA02929"];
        // Linux wins over a better Windows result.
        assert_eq!(e.status, Status::MainMenu);
        assert!(e.on_linux);
        assert_eq!(e.windows, Some(Status::InGame));
        assert_eq!(e.tag(), "Menus");
        assert_eq!(e.platforms, vec!["Linux", "Windows"]);
        assert_eq!(e.tested_date(), "2026-08-18");
        assert!(Status::InGame < Status::DoesntBoot);
    }

    #[test]
    fn windows_only_results_are_marked() {
        let json = br#"{"PPSA1":{"status":"InGame","platforms":{"windows":{"status":"InGame"}}},"PPSA2":{"status":"Logo"}}"#;
        let db = parse(json).unwrap();
        let w = &db["PPSA1"];
        assert!(!w.on_linux);
        assert_eq!(w.tag(), "Win · In-game");
        assert_eq!(w.chip_text(), "In-game on Windows · untested on Linux");
        assert_eq!(db["PPSA2"].tag(), "Other OS · Boots");
    }

    #[test]
    fn shadps4_list_and_ps4_report() {
        let json = br#"{"CUSA04118":{"os-linux":{"status":"status-ingame","last_tested":"2026-10-01T22:30:38Z"},"os-windows":{"status":"status-playable","last_tested":"2026-09-01T00:00:00Z"}},
            "CUSA00001":{"os-windows":{"status":"status-boots","last_tested":"2026-09-02T00:00:00Z"}},"CUSA00002":{"os-linux":{"status":"status-unknown"}}}"#;
        let db = parse_shad(json).unwrap();
        let fw = &db["CUSA04118"];
        assert_eq!((fw.status, fw.on_linux, fw.windows), (Status::InGame, true, Some(Status::InGame)));
        assert_eq!(fw.tested_date(), "2026-10-01");
        assert_eq!(fw.platforms, vec!["Linux", "Windows"]);
        assert_eq!(db["CUSA00001"].tag(), "Win · Boots");
        assert!(!db.contains_key("CUSA00002"), "unknown statuses are skipped");
        let url = report_url_for("Firewatch", "CUSA04118", "0.18.0", "01.08", Status::MainMenu, "");
        assert!(url.starts_with(SHAD_REPORT_FORM));
        assert!(url.contains("&game-serial=CUSA04118") && url.contains("&emulation-status=Menus") && url.contains("&game-version=01%2E08"));
        assert!(url.contains(&format!("&Operating-System={THIS_OS}")), "the report names the system it comes from");
        assert_eq!(list_page(crate::platform::Platform::Ps4), SHAD_LIST_PAGE);
    }

    #[test]
    fn your_result_wins_on_this_pc() {
        let json = br#"{"PPSA1":{"status":"InGame","platforms":{"windows":{"status":"InGame"}}}}"#;
        let mut mine = crate::results::Results::default();
        mine.rate("PPSA1", "One", Status::Logo, "b", 1.0, "");
        mine.rate("PPSA9", "Nine", Status::InGame, "b", 1.0, "");
        let db = with_mine(parse(json).unwrap(), &mine);
        let e = &db["PPSA1"];
        assert!(e.mine && e.on_linux);
        assert_eq!(e.status, Status::Logo);
        assert_eq!(e.windows, Some(Status::InGame), "the community's results stay visible");
        assert_eq!(e.chip_text(), "Boots on Linux · your result");
        assert_eq!(db["PPSA9"].tag(), "In-game");
        assert!(e.in_game_anywhere(), "Windows says in-game, even though your result is Boots");
    }

    #[test]
    fn in_game_anywhere_checks_every_os() {
        let json = br#"{"A":{"status":"InGame","platforms":{"windows":{"status":"InGame"},"linux":{"status":"Logo"}}},"B":{"status":"MainMenu","platforms":{"linux":{"status":"MainMenu"}}},"C":{"status":"InGame"}}"#;
        let db = parse(json).unwrap();
        assert!(db["A"].in_game_anywhere() && db["A"].status == Status::Logo);
        assert!(!db["B"].in_game_anywhere());
        assert!(db["C"].in_game_anywhere());
    }

    #[test]
    fn report_url_prefills_the_form() {
        let url = report_url_for("Dreaming Sarah", "PPSA02929", "KytyPS5-2026-09-30-b7a1fac", "", Status::DoesntBoot, "boot\n\nCould not find suitable device\n");
        assert!(url.starts_with(REPORT_FORM));
        assert!(url.contains("&game-id=PPSA02929"));
        assert!(url.contains("&game-title=Dreaming%20Sarah"));
        assert!(url.contains("&kyty-version=KytyPS5%2D2026%2D09%2D30%2Db7a1fac"));
        assert!(url.contains("&compatibility-status=Doesn%27t%20boot"));
        assert!(url.contains("&log-file="));
        assert!(url.contains("Could%20not%20find%20suitable%20device"));
        assert!(report_url_for("A", "B", "C", "", Status::InGame, "").find("log-file").is_none());
        let long = "x".repeat(200) + "\n";
        assert!(log_excerpt(&long.repeat(100)).len() < 2600);
        let crash = format!("{}--- Build ---\nOfficial build X\n--- Error ---\nCould not find suitable device\n{}", "boot line\n".repeat(500), "after\n".repeat(3));
        let e = log_excerpt(&crash);
        assert!(e.starts_with("The error from the log") && e.contains("--- Error ---\nCould not find suitable device"));
        assert!(!e.contains("boot line"));
    }
}
