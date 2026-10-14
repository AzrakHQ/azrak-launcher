//! PC games: how an installed release starts, and the manifest the installer leaves in its folder.
//!
//! RuTracker's PC releases come as folders, archives, GOG installers (`.sh`: a shell script with a
//! zip appended, unpacked without running the script) or AppImages. Once unpacked, a game starts
//! from one of: the uploader's `start`/`start.sh` script (repacks, Wine-packed games, GOG), an
//! AppImage, a native executable (often Unity's `Game.x86_64`), or, for a Windows game with no
//! script, its `.exe` through Proton (umu-launcher) or Wine. Files are recognised by their first
//! bytes, not their names, because torrents drop the executable flag.

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Written into an installed PC game's folder; the Library finds PC games by it.
pub const MANIFEST: &str = ".ps5-launcher-game.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A shell script (`start.sh`, GOG's `start.sh`, a repack's `start`).
    Script,
    AppImage,
    /// A native Linux executable.
    Native,
    /// A Windows executable, run through Proton or Wine.
    Windows,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub name: String,
    /// The RuTracker topic it was installed from (0: unknown).
    pub topic: i64,
    /// What Play starts, relative to the game's folder.
    pub launch: String,
    pub kind: Kind,
    /// Other things that could start it, best first (for choosing another).
    #[serde(default)]
    pub alternatives: Vec<String>,
}

impl Manifest {
    pub fn read(dir: &Path) -> Option<Self> {
        let file = std::fs::File::open(dir.join(MANIFEST)).ok()?;
        let mut bytes = Vec::new();
        file.take(64 * 1024).read_to_end(&mut bytes).ok()?;
        let m: Self = serde_json::from_slice(&bytes).ok()?;
        // Only paths inside the game's folder.
        (m.version == 1 && safe_relative(&m.launch)).then_some(m)
    }
}

/// Make the first alternative what Play starts (the current one goes last): for when the launcher
/// chose the wrong file. Returns the new manifest.
pub fn rotate_launch(dir: &Path) -> Result<Manifest, String> {
    let mut m = Manifest::read(dir).ok_or("This game's manifest is missing")?;
    if m.alternatives.is_empty() {
        return Err("There's nothing else to start".into());
    }
    let next = m.alternatives.remove(0);
    if !safe_relative(&next) || !dir.join(&next).is_file() {
        return Err(format!("{next} isn't in the game's folder"));
    }
    m.alternatives.push(std::mem::replace(&mut m.launch, next));
    m.kind = match probe_file(&dir.join(&m.launch)) {
        Probe::AppImage => Kind::AppImage,
        Probe::Elf { .. } => Kind::Native,
        Probe::Exe => Kind::Windows,
        _ => Kind::Script,
    };
    let bytes = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&dir.join(MANIFEST), &bytes).map_err(|e| e.to_string())?;
    Ok(m)
}

/// A relative path that stays inside its folder.
pub fn safe_relative(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

/// What a file is, from its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// An ELF program (it has an interpreter or is a static executable): x86-64 or not.
    Elf { x64: bool },
    AppImage,
    /// Starts with `#!`.
    Script,
    /// A Windows (PE) executable.
    Exe,
    Other,
}

impl Probe {
    /// Should be executable after installing (torrents drop the flag).
    pub fn runnable(self) -> bool {
        matches!(self, Probe::Elf { .. } | Probe::AppImage | Probe::Script)
    }
}

/// Recognise a file from its first bytes (4 KiB holds a program's headers).
pub fn probe(head: &[u8]) -> Probe {
    if head.starts_with(b"#!") {
        return Probe::Script;
    }
    if head.starts_with(b"MZ") && head.len() >= 0x40 {
        let pe = u32::from_le_bytes(head[0x3c..0x40].try_into().unwrap()) as usize;
        if head.get(pe..pe + 4) == Some(b"PE\0\0".as_slice()) {
            return Probe::Exe;
        }
        return Probe::Other;
    }
    if !head.starts_with(b"\x7fELF") || head.len() < 0x34 {
        return Probe::Other;
    }
    if head.get(8..11) == Some(b"AI\x02".as_slice()) {
        return Probe::AppImage;
    }
    let is64 = head[4] == 2;
    let le = head[5] == 1;
    if !le {
        return Probe::Other;
    }
    let u16_at = |o: usize| head.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |o: usize| head.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let u64_at = |o: usize| head.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()) as usize);
    let (Some(kind), Some(machine)) = (u16_at(16), u16_at(18)) else { return Probe::Other };
    let x64 = machine == 0x3e;
    if !(x64 || machine == 0x03) {
        return Probe::Other; // ARM etc.: not for this PC
    }
    // ET_EXEC is a program; ET_DYN is a program only with an interpreter (PIE), else a library.
    if kind == 2 {
        return Probe::Elf { x64 };
    }
    if kind != 3 {
        return Probe::Other;
    }
    let (phoff, phentsize, phnum) = if is64 { (u64_at(0x20), u16_at(0x36), u16_at(0x38)) } else { (u32_at(0x1c), u16_at(0x2a), u16_at(0x2c)) };
    let (Some(phoff), Some(size), Some(count)) = (phoff, phentsize, phnum) else { return Probe::Other };
    for i in 0..count.min(64) {
        match u32_at(phoff + i * size) {
            Some(3) => return Probe::Elf { x64 }, // PT_INTERP
            Some(_) => {}
            None => break, // headers beyond what was read: assume a library
        }
    }
    Probe::Other
}

pub fn probe_file(path: &Path) -> Probe {
    let Ok(file) = std::fs::File::open(path) else { return Probe::Other };
    let mut head = Vec::with_capacity(4096);
    if file.take(4096).read_to_end(&mut head).is_err() {
        return Probe::Other;
    }
    probe(&head)
}

/// Helpers that come with games but don't start them.
fn helper(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const WORDS: [&str; 22] = ["setup", "unins", "install", "redist", "vcredist", "dxsetup", "directx", "dotnet", "physx",
        "crashhandler", "crash_handler", "crashpad", "crashreport", "bugreport", "uploader", "desktop", "settings",
        "config", "uninstall", "update", "steam_api", "easyanticheat"];
    WORDS.iter().any(|w| n.contains(w)) || n.ends_with(".so") || n.contains(".so.") || n.ends_with(".dll")
}

/// Words of a game's name, to prefer the executable named after it.
fn words(name: &str) -> Vec<String> {
    name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(str::to_string).collect()
}

/// Choose what starts a game, from its files (relative paths) and what each is. Returns the
/// launch target with its kind, then the alternatives, best first.
pub fn choose(files: &[(String, Probe)], game_name: &str) -> Option<(String, Kind, Vec<String>)> {
    let name_words = words(game_name);
    let has_x64 = files.iter().any(|(_, p)| *p == Probe::Elf { x64: true });
    let mut scored: Vec<(i32, &str, Kind)> = Vec::new();
    for (path, probe) in files {
        let depth = path.matches('/').count() as i32;
        let file = path.rsplit('/').next().unwrap_or(path);
        let lower = file.to_ascii_lowercase();
        let dir_lower = path.to_ascii_lowercase();
        // Bundled runtimes are tools, not the game: Wine, Proton, Steam runtime, libraries.
        if ["lib/", "lib32/", "lib64/", "libs/", "/wine/", "wine/bin", "files/bin", "runtime/", "dist/", "mono/", "jre/", "java/"]
            .iter().any(|d| dir_lower.contains(d) || dir_lower.starts_with(d)) && !lower.starts_with("start") {
            continue;
        }
        let named = name_words.iter().any(|w| lower.contains(w.as_str()));
        let (score, kind) = match probe {
            Probe::Script | Probe::Other if ["start", "start.sh", "start-game.sh", "start_game.sh", "play.sh", "run.sh", "launch.sh", "game.sh"].contains(&lower.as_str()) => {
                (100 - depth * 5, Kind::Script)
            }
            Probe::Script if lower.ends_with(".sh") && !helper(&lower) && (named || depth == 0) => (60 - depth * 5 + i32::from(named) * 5, Kind::Script),
            Probe::AppImage => (90 - depth * 5, Kind::AppImage),
            Probe::Elf { x64 } if !helper(&lower) => {
                let mut s = 70 - depth * 5;
                if named { s += 10; }
                if lower.ends_with(".x86_64") || lower.ends_with(".x64") || lower.ends_with(".bin.x86_64") { s += 5; }
                if !x64 && has_x64 { s -= 20; }
                (s, Kind::Native)
            }
            Probe::Exe if !helper(&lower) => {
                let mut s = 30 - depth * 3;
                if named { s += 10; }
                if lower.contains("launcher") { s -= 5; }
                (s, Kind::Windows)
            }
            _ => continue,
        };
        scored.push((score, path, kind));
    }
    // Best first; ties: the shorter path.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
    let (_, best, kind) = *scored.first()?;
    // A Linux way to start it always wins over a Windows .exe.
    let alternatives = scored.iter().skip(1).take(8).map(|(_, p, _)| p.to_string()).collect();
    Some((best.to_string(), kind, alternatives))
}

/// The game's name from a release title: "[GOG] Celeste (v1.4) [x86, amd64] [Multi] [Native]" →
/// "Celeste".
pub fn clean_name(title: &str) -> String {
    use std::sync::LazyLock;
    static LEADING: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^(?:\s*\[[^\]]*\]\s*)+").unwrap());
    static TRAILING: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
        r"(?i)\s*(?:\((?:v\.?\s*)?[\d.,_\-/ ]+[a-z]?\)|\(v\.?[^)]*\)|\((?:build|rev|update|alpha|beta|early access|in development)[^)]*\)|\+\s*\d+\s*dlc\b.*|\+\s*(?:dlc|ost|soundtrack|bonus|update)s?\b.*)\s*$").unwrap());
    let name = LEADING.replace(title, "");
    let mut name = name.split(" [").next().unwrap_or("").trim().to_string();
    loop {
        let shorter = TRAILING.replace(&name, "").trim().to_string();
        if shorter == name || shorter.is_empty() { break; }
        name = shorter;
    }
    // "Russian / English" titles: the part with Latin letters.
    if let Some((a, b)) = name.split_once(" / ") {
        let latin = |s: &str| s.chars().filter(|c| c.is_ascii_alphabetic()).count();
        name = if latin(b) > latin(a) { b.trim().to_string() } else { a.trim().to_string() };
    }
    name
}

/// A folder name for a game: its name in safe characters.
pub fn folder_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s: String = s.trim_matches(|c| c == '.' || c == '-').chars().take(60).collect();
    if s.is_empty() { "game".into() } else { s }
}

/// How a Windows game is started: umu-launcher (Proton, downloaded and kept up to date by umu
/// itself) when installed, else the system's Wine. `None`: neither is installed.
pub fn windows_runner() -> Option<(PathBuf, &'static str)> {
    for (tool, label) in [("umu-run", "Proton"), ("wine", "Wine")] {
        if let Some(path) = which(tool) {
            return Some((path, label));
        }
    }
    None
}

pub fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(tool)).find(|p| {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(kind: u16, machine: u16, interp: bool) -> Vec<u8> {
        let mut h = vec![0u8; 0x200];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2; // 64-bit
        h[5] = 1; // little endian
        h[16..18].copy_from_slice(&kind.to_le_bytes());
        h[18..20].copy_from_slice(&machine.to_le_bytes());
        h[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        h[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        h[0x38..0x3a].copy_from_slice(&2u16.to_le_bytes());
        h[0x40..0x44].copy_from_slice(&6u32.to_le_bytes()); // PT_PHDR
        h[0x40 + 56..0x40 + 60].copy_from_slice(&(if interp { 3u32 } else { 1 }).to_le_bytes());
        h
    }

    #[test]
    fn recognises_files_by_their_first_bytes() {
        assert_eq!(probe(&elf(3, 0x3e, true)), Probe::Elf { x64: true }, "a PIE program");
        assert_eq!(probe(&elf(3, 0x3e, false)), Probe::Other, "a shared library");
        assert_eq!(probe(&elf(2, 0x03, false)), Probe::Elf { x64: false }, "a 32-bit program");
        assert_eq!(probe(&elf(2, 0xb7, false)), Probe::Other, "an ARM program");
        let mut appimage = elf(2, 0x3e, true);
        appimage[8..11].copy_from_slice(b"AI\x02");
        assert_eq!(probe(&appimage), Probe::AppImage);
        assert_eq!(probe(b"#!/bin/bash\n./game"), Probe::Script);
        let mut pe = vec![0u8; 0x100];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        assert_eq!(probe(&pe), Probe::Exe);
        assert_eq!(probe(b"PNG data"), Probe::Other);
        assert!(Probe::Script.runnable() && !Probe::Exe.runnable() && !Probe::Other.runnable());
    }

    fn files(list: &[(&str, Probe)]) -> Vec<(String, Probe)> {
        list.iter().map(|(p, k)| (p.to_string(), *k)).collect()
    }

    #[test]
    fn chooses_how_a_game_starts() {
        use Probe::*;
        // A Wine-packed repack: its start script, not the bundled Wine or the shortcut maker.
        let repack = files(&[("start", Script), ("desktop", Script), ("game_info/data/Game.exe", Exe),
            ("wine/bin/wine64", Elf { x64: true }), ("start-conf.sh", Script)]);
        let (launch, kind, alts) = choose(&repack, "Assassin's Creed Shadows").unwrap();
        assert_eq!((launch.as_str(), kind), ("start", Kind::Script));
        assert!(!alts.contains(&"desktop".to_string()) && !alts.iter().any(|a| a.starts_with("wine/")));
        // GOG, unpacked from its installer.
        let gog = files(&[("data/noarch/start.sh", Script), ("data/noarch/game/Celeste", Elf { x64: true }),
            ("data/noarch/support/gog-installer.sh", Script)]);
        assert_eq!(choose(&gog, "Celeste").unwrap().0, "data/noarch/start.sh");
        // Unity: the 64-bit program named after the game, not its crash handler or libraries.
        let unity = files(&[("Munin/Munin.x86", Elf { x64: false }), ("Munin/Munin.x86_64", Elf { x64: true }),
            ("Munin/UnityCrashHandler64", Elf { x64: true }), ("Munin/Munin_Data/Plugins/libsteam_api.so", Other)]);
        let (launch, kind, _) = choose(&unity, "Munin").unwrap();
        assert_eq!((launch.as_str(), kind), ("Munin/Munin.x86_64", Kind::Native));
        // A single AppImage.
        assert_eq!(choose(&files(&[("Neon-Drive.AppImage", AppImage), ("readme.txt", Other)]), "Neon Drive").unwrap().1, Kind::AppImage);
        // Windows only: the game's .exe, not its installer or redistributables.
        let windows = files(&[("Game/setup.exe", Exe), ("Game/_Redist/vcredist_x64.exe", Exe), ("Game/Ori.exe", Exe), ("Game/UnityCrashHandler64.exe", Exe)]);
        assert_eq!(choose(&windows, "Ori and the Blind Forest").unwrap(), ("Game/Ori.exe".into(), Kind::Windows, vec![]));
        // Linux beats Windows.
        let both = files(&[("Game.exe", Exe), ("Game.x86_64", Elf { x64: true })]);
        assert_eq!(choose(&both, "Game").unwrap().1, Kind::Native);
        assert_eq!(choose(&files(&[("readme.txt", Other), ("lib/libfoo.so", Elf { x64: true })]), "X"), None);
    }

    #[test]
    fn cleans_release_titles() {
        for (title, name) in [
            ("[GOG] Human: Fall Flat [x86] [ENG] [Unity3D]", "Human: Fall Flat"),
            ("88 Heroes [x86, amd64] [ENG / Multi9] [Unity3D] [GOG]", "88 Heroes"),
            ("Transport Fever 3: Deluxe Edition (2026) (v.40408) + 2 DLC [RUS + ENG + 11 / ENG + 1]", "Transport Fever 3: Deluxe Edition"),
            ("Age of Empires II Definitive Edition: The Viking Sagas (v.101.103.54800.0) + 16 DLC", "Age of Empires II Definitive Edition: The Viking Sagas"),
            ("Milkmaid of the Milky Way (23.03.2020) [amd64] [Multi] [Unity3D]", "Milkmaid of the Milky Way"),
            ("[In development] Ancient Warfare 3 (Alpha 0.33.12.2) [amd64] [ENG] [Unity3D]", "Ancient Warfare 3"),
            ("[Trilogy] Gothic / Gothic [x86] [RUS] [Wine]", "Gothic"),
            ("Marvel: Wolverine / Marvel's Wolverine - Deluxe Edition", "Marvel's Wolverine - Deluxe Edition"),
            ("Neon Drive Game + Soundtrack (1.6) [amd64] [Multi] [Unity3D]", "Neon Drive Game"),
        ] {
            assert_eq!(clean_name(title), name, "{title}");
        }
        assert_eq!(folder_name("Human: Fall Flat"), "Human-Fall-Flat");
        assert_eq!(folder_name("../.."), "game");
    }

    #[test]
    fn manifests_stay_inside_their_folder() {
        assert!(safe_relative("data/noarch/start.sh"));
        for bad in ["", "/usr/bin/sh", "../start.sh", "a/../b", "a//b", "./start"] {
            assert!(!safe_relative(bad), "{bad}");
        }
        let t = tempfile::tempdir().unwrap();
        let m = Manifest { version: 1, name: "Celeste".into(), topic: 5, launch: "start.sh".into(), kind: Kind::Script, alternatives: vec![] };
        std::fs::write(t.path().join(MANIFEST), serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(Manifest::read(t.path()), Some(m.clone()));
        std::fs::write(t.path().join(MANIFEST), serde_json::to_vec(&Manifest { launch: "../../bin/sh".into(), ..m }).unwrap()).unwrap();
        assert_eq!(Manifest::read(t.path()), None);
    }
}
