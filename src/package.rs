//! Whether the launcher can install a release, judged from its file names: the Library hides PC
//! releases it can't install (from the file lists in the catalog), and a download stops before
//! any data when its torrent turns out to be one (from the torrent's own list). It follows what
//! installer.rs and pcgame.rs can do; where names can't tell (an archive's contents, whether a
//! file without an extension is a program), it assumes the release can be installed.

use crate::platform::Platform;

/// Why a release can't be installed, if it can't. `files` are its paths (`/`-separated, inside
/// the torrent) with their sizes.
pub fn unsupported(platform: Platform, files: &[(String, u64)]) -> Option<String> {
    if files.is_empty() { return None; }
    let names: Vec<(String, String, usize)> = files.iter().map(|(path, _)| {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        (path.to_ascii_lowercase(), name, path.matches('/').count())
    }).collect();
    match platform {
        Platform::Ps5 if names.iter().any(|(_, name, _)| name.ends_with(".pkg")) && !names.iter().any(|(_, name, _)| name == "eboot.bin") =>
            Some("it's a PS5 PKG package; PS5 games install from extracted folders, archives and exFAT images".into()),
        Platform::Pc => pc(&names),
        _ => None,
    }
}

const ARCHIVES: [&str; 12] = [".rar", ".zip", ".7z", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst"];
/// Programs by their usual Linux names.
const PROGRAMS: [&str; 7] = [".x86_64", ".x86", ".x64", ".bin", ".amd64", ".i386", ".i686"];
/// Files without an extension that are never the game's program.
const DOCUMENTS: [&str; 12] = ["readme", "license", "licence", "copying", "changelog", "changes", "authors", "credits",
    "news", "todo", "version", "install"];

fn pc(names: &[(String, String, usize)]) -> Option<String> {
    let archive = |name: &str| package(name).is_none()
        && (ARCHIVES.iter().any(|ext| name.ends_with(ext)) || is_volume(name));
    for (path, name, depth) in names {
        // Archives at the top (or in a folder at the top) are unpacked; what's inside isn't known.
        if *depth <= 1 && archive(name) { return None; }
        if package(name).is_some() { continue; }
        // "hades_v1.38177_wine_repack" has no extension either: a version's dot isn't one.
        let stem_only = name.rsplit_once('.').is_none_or(|(_, ext)| ext.is_empty() || ext.len() > 5
            || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) || ext.bytes().all(|b| b.is_ascii_digit()));
        let starts = name.ends_with(".sh") || name.ends_with(".run") || name.ends_with(".appimage")
            || ["start", "play", "run", "launch"].contains(&name.as_str())
            || PROGRAMS.iter().any(|ext| name.ends_with(ext))
            || (name.ends_with(".exe") && !crate::pcgame::helper(name))
            || (stem_only && !DOCUMENTS.iter().any(|d| name.starts_with(d)) && !bundled(path));
        if starts { return None; }
    }
    let has = |test: &dyn Fn(&str) -> bool| names.iter().any(|(_, name, _)| test(name));
    Some(if let Some(kind) = names.iter().find_map(|(_, name, _)| package(name)) {
        format!("it's {kind}, which Azrak doesn't install")
    } else if has(&|n| [".iso", ".mdf", ".mds", ".cue", ".nrg", ".ccd", ".img"].iter().any(|ext| n.ends_with(ext))) {
        "it's a disc image, which Azrak doesn't mount".into()
    } else if has(&|n| n.ends_with(".jar")) {
        "it's a Java .jar with nothing that starts it".into()
    } else if has(&|n| n.ends_with(".love")) {
        "it's a LÖVE game file without the LÖVE runtime".into()
    } else if has(&|n| n.ends_with(".exe")) {
        "it holds only a Windows installer".into()
    } else {
        "nothing in it starts a game".into()
    })
}

/// A system package format, by name.
fn package(name: &str) -> Option<&'static str> {
    if name.ends_with(".deb") { Some("a Debian package (.deb)") }
    else if name.ends_with(".rpm") { Some("an RPM package") }
    else if name.ends_with(".flatpak") || name.ends_with(".flatpakref") { Some("a Flatpak bundle") }
    else if name.ends_with(".snap") { Some("a Snap package") }
    else if name.contains(".pkg.tar") { Some("an Arch Linux package") }
    else if name.ends_with(".cxarchive") { Some("a CrossOver bottle (.cxarchive)") }
    else if name.ends_with(".polapp") { Some("a PlayOnLinux package (.polApp)") }
    else { None }
}

/// A RAR or 7z volume after the first: game.part2.rar, game.r00, game.7z.002.
fn is_volume(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("");
    (ext.len() == 3 && ext.starts_with('r') && ext[1..].bytes().all(|b| b.is_ascii_digit()))
        || (ext.len() == 3 && ext.bytes().all(|b| b.is_ascii_digit()) && name.contains(".7z."))
}

/// Inside a bundled runtime or library folder (Wine, Mono, Java…): its tools don't start the game.
fn bundled(path: &str) -> bool {
    ["lib/", "lib32/", "lib64/", "libs/", "wine/", "runtime/", "mono/", "jre/", "java/", "share/", "doc/", "docs/", "licenses/"]
        .iter().any(|d| path.starts_with(d) || path.contains(&format!("/{d}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn files(paths: &[&str]) -> Vec<(String, u64)> { paths.iter().map(|p| (p.to_string(), 1)).collect() }
    fn pc(paths: &[&str]) -> Option<String> { unsupported(Platform::Pc, &files(paths)) }

    #[test]
    fn pc_releases_that_install() {
        assert_eq!(pc(&["balatro_v1.0.1o.run"]), None, "a YAD Simple Installer");
        assert_eq!(pc(&["gog_celeste_1.4.sh"]), None);
        assert_eq!(pc(&["Game/game.part1.rar", "Game/game.part2.rar"]), None);
        assert_eq!(pc(&["Game.AppImage"]), None);
        assert_eq!(pc(&["Some Game/start.sh", "Some Game/game/data.pak"]), None);
        assert_eq!(pc(&["Some Game/SomeGame", "Some Game/README"]), None, "a program without an extension");
        assert_eq!(pc(&["Game/Game.exe", "Game/setup.exe"]), None);
        assert_eq!(pc(&["Game/bin/game.x86_64"]), None);
        assert_eq!(pc(&["hades_v1.38177_wine_repack", "OST/01. No Escape.mp3"]), None, "a YAD Simple Installer without .sh");
        assert_eq!(pc(&["Deus_Ex.txz"]), None);
    }

    #[test]
    fn pc_releases_that_dont() {
        assert!(pc(&["game_1.0_amd64.deb"]).unwrap().contains("Debian package"));
        assert!(pc(&["game-1.0-1.x86_64.rpm"]).unwrap().contains("RPM"));
        assert!(pc(&["game-1.0-1-x86_64.pkg.tar.zst"]).unwrap().contains("Arch Linux"), "an archive by name, a package really");
        assert!(pc(&["Game/game.iso", "Game/README"]).unwrap().contains("disc image"));
        assert!(pc(&["Game/game.jar", "Game/LICENSE"]).unwrap().contains("Java"));
        assert!(pc(&["StarCraft2.cxarchive"]).unwrap().contains("CrossOver"));
        assert!(pc(&["MR.polApp"]).unwrap().contains("PlayOnLinux"));
        assert!(pc(&["Game/setup.exe", "Game/setup-1.bin"]).is_none(), "a .bin may be the program");
        assert!(pc(&["Game/setup_game_1.0.exe"]).unwrap().contains("Windows installer"));
        assert!(pc(&["Game/data/level1.dat", "Game/lib/libSDL2.so"]).unwrap().contains("nothing"));
        assert!(pc(&["Game/lib/helper"]).is_some(), "a tool in a library folder");
    }

    /// What the rules make of a collected catalog: SURVEY=dist/rutracker/linux-topics.json.ru-backup.json
    /// cargo test -- --ignored survey --nocapture
    #[test]
    #[ignore]
    fn survey() {
        let Ok(path) = std::env::var("SURVEY") else { return };
        let catalog: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let (mut listed, mut hidden, mut unknown) = (0, std::collections::BTreeMap::<String, Vec<String>>::new(), 0);
        for topic in catalog["topics"].as_array().unwrap() {
            let files = &topic["files"];
            let Some(list) = files["list"].as_array().filter(|_| files["truncated"] != true && files["deeper_starters"] != true) else { unknown += 1; continue };
            let files: Vec<(String, u64)> = list.iter().map(|e| (e[0].as_str().unwrap().to_string(), e[1].as_u64().unwrap())).collect();
            match unsupported(Platform::Pc, &files) {
                None => listed += 1,
                Some(why) => hidden.entry(why).or_default().push(format!("{} {}: {:?}", topic["id"], topic["title"],
                    files.iter().take(4).map(|f| f.0.as_str()).collect::<Vec<_>>())),
            }
        }
        eprintln!("{listed} installable, {} not, {unknown} without a list", hidden.values().map(Vec::len).sum::<usize>());
        for (why, topics) in &hidden {
            eprintln!("{} × {why}", topics.len());
            for t in topics.iter().take(12) { eprintln!("    {t}"); }
        }
    }

    #[test]
    fn consoles() {
        assert!(unsupported(Platform::Ps5, &files(&["UP0001-PPSA01234_00-GAME.pkg"])).is_some());
        assert!(unsupported(Platform::Ps4, &files(&["UP0001-CUSA01234_00-GAME.pkg"])).is_none(), "PS4 PKGs install");
        assert!(unsupported(Platform::Ps5, &files(&["game/eboot.bin", "game/sce_sys/param.json"])).is_none());
    }
}
