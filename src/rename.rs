//! Moving from "PS5 Launcher" to Azrak Launcher (v1.17). The only place that still knows the old
//! names: it runs once at start and moves an older install over, leaving nothing behind.
//!
//! - ~/.config, ~/.local/share and ~/.cache `ps5-launcher` folders become `azrak`.
//! - The default game and download folders, ~/Games/PS5 and ~/Downloads/PS5, become
//!   ~/Games/Azrak and ~/Downloads/Azrak (folders someone chose themselves stay where they are).
//! - Every path saved into them is rewritten: settings, transfers, installs, play time, results,
//!   the emulators' state, and the links inside the emulators' folders.
//! - Ownership markers in download folders and PC game manifests get their new names.
//! - An install.sh install gets its new binary name, menu entry and icon.
//!
//! Each step looks for what's old, so running again does nothing.

use crate::util::{atomic_write, cache_dir, config_dir, data_dir, expand_home};
use std::path::{Path, PathBuf};

const OLD: &str = "ps5-launcher";
const OLD_OWNED: &str = ".ps5-launcher-owned";
const OLD_MANIFEST: &str = ".ps5-launcher-game.json";
/// PS5 Launcher OS images from before AzrakOS start the launcher by its old name.
pub const OLD_OS_MARKER: &str = "/usr/lib/ps5-launcher-os/release";
/// The old default folders and their new names.
const OLD_DEFAULTS: [(&str, &str); 2] = [("~/Games/PS5", "~/Games/Azrak"), ("~/Downloads/PS5", "~/Downloads/Azrak")];

pub fn migrate() {
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for new in [config_dir(), data_dir(), cache_dir()] {
        let old = new.with_file_name(OLD);
        if move_folder(&old, &new) {
            moved.push((old, new));
        }
    }
    moved.extend(default_folders());
    if !moved.is_empty() {
        rewrite_saved_paths(&moved);
        relink(&data_dir(), &moved);
        crate::log!("moved from PS5 Launcher: {}", moved.iter().map(|(o, n)| format!("{} → {}", o.display(), n.display())).collect::<Vec<_>>().join(", "));
    }
    rename_markers();
    install();
}

/// Move the launcher's folder `old` to `new` (the same parent folder, so the same drive: instant).
/// When both exist, what's only in the old one moves over and the old one goes once it's empty.
fn move_folder(old: &Path, new: &Path) -> bool {
    if std::fs::symlink_metadata(old).map(|m| !m.is_dir()).unwrap_or(true) {
        return false;
    }
    if !new.exists() {
        return match std::fs::rename(old, new) {
            Ok(()) => true,
            Err(e) => {
                crate::log!("couldn't move {} to {}: {e}", old.display(), new.display());
                false
            }
        };
    }
    let Ok(entries) = std::fs::read_dir(old) else { return false };
    for entry in entries.flatten() {
        let to = new.join(entry.file_name());
        if !to.exists() {
            let _ = std::fs::rename(entry.path(), to);
        }
    }
    if std::fs::remove_dir(old).is_err() {
        crate::log!("{} still holds files that are also in {}; left as it is", old.display(), new.display());
    }
    true
}

/// ~/Games/PS5 and ~/Downloads/PS5, when the settings still use them, become ~/Games/Azrak and
/// ~/Downloads/Azrak, and the settings say so. A new folder that already exists isn't merged into:
/// then the settings keep the old one.
fn default_folders() -> Vec<(PathBuf, PathBuf)> {
    let path = config_dir().join("config.json");
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let mut moved = Vec::new();
    let mut changed = false;
    for (old_name, new_name) in OLD_DEFAULTS {
        let (old, new) = (expand_home(old_name), expand_home(new_name));
        let is_old = |v: &serde_json::Value| v.as_str().is_some_and(|v| v.trim_end_matches('/') == old_name || Path::new(v) == old);
        let keys: Vec<&str> = ["download_dir", "install_dir"].into_iter()
            // A setting from before the field existed meant the old default.
            .filter(|k| config.get(*k).is_none_or(&is_old) && (*k == "download_dir") == old_name.contains("Downloads"))
            .collect();
        let listed = config["game_dirs"].as_array().is_some_and(|d| d.iter().any(&is_old));
        if keys.is_empty() && !listed {
            continue;
        }
        if old.exists() {
            if new.exists() || std::fs::rename(&old, &new).is_err() {
                crate::log!("{} stays: {} already exists", old.display(), new.display());
                continue;
            }
            moved.push((old.clone(), new.clone()));
        }
        for key in keys {
            config[key] = new_name.into();
        }
        if let Some(dirs) = config.get_mut("game_dirs").and_then(|d| d.as_array_mut()) {
            for dir in dirs.iter_mut().filter(|d| is_old(d)) {
                *dir = new_name.into();
            }
        }
        changed = true;
    }
    if changed {
        if let Ok(bytes) = serde_json::to_vec_pretty(&config) {
            let _ = atomic_write(&path, &bytes);
        }
    }
    moved
}

/// Paths saved as text in the launcher's own files: settings, transfers, installs, play time,
/// results, and the emulators' state.
fn rewrite_saved_paths(moved: &[(PathBuf, PathBuf)]) {
    let mut files = Vec::new();
    for dir in [config_dir(), config_dir().join("transfers"), config_dir().join("installs"), data_dir().join("kyty"), data_dir().join("shadps4")] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            files.extend(entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")));
        }
    }
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let new = rewrite(&text, moved);
        if new != text {
            let _ = atomic_write(&file, new.as_bytes());
        }
    }
}

/// `text` with every path into a moved folder pointing into its new place. Only whole folder
/// names change: "/Games/PS5/x" does, "/Games/PS5-backup" doesn't.
pub fn rewrite(text: &str, moved: &[(PathBuf, PathBuf)]) -> String {
    let mut text = text.to_string();
    for (old, new) in moved {
        let (old, new) = (old.to_string_lossy(), new.to_string_lossy());
        for end in ["/", "\""] {
            text = text.replace(&format!("{old}{end}"), &format!("{new}{end}"));
        }
    }
    text
}

/// Links inside the launcher's data folder that pointed into a moved folder (KytyPS5's save data,
/// shader caches…) point into the new place.
fn relink(dir: &Path, moved: &[(PathBuf, PathBuf)]) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() {
            let Ok(target) = std::fs::read_link(&path) else { continue };
            for (old, new) in moved {
                if let Ok(rest) = target.strip_prefix(old) {
                    let _ = std::fs::remove_file(&path);
                    let _ = std::os::unix::fs::symlink(new.join(rest), &path);
                    break;
                }
            }
        } else if meta.is_dir() {
            relink(&path, moved);
        }
    }
}

/// Download folders' ownership markers and PC game manifests, by their new names.
fn rename_markers() {
    let transfers = config_dir().join("transfers").join("jobs.json");
    if let Some(jobs) = std::fs::read(&transfers).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
        for folder in jobs["jobs"].as_array().into_iter().flatten().filter_map(|j| j["folder"].as_str()) {
            rename_marker(&Path::new(folder).join(OLD_OWNED), crate::downloads::MARKER);
        }
    }
    for dir in crate::config::Config::load().game_dir_paths() {
        adopt_manifests(&dir, 0);
    }
}

fn adopt_manifests(dir: &Path, depth: usize) {
    adopt_manifest(dir);
    if depth >= 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            adopt_manifests(&entry.path(), depth + 1);
        }
    }
}

/// A PC game installed by PS5 Launcher: its manifest by the new name (the Library calls this for
/// game folders found later, e.g. on another drive).
pub fn adopt_manifest(dir: &Path) {
    rename_marker(&dir.join(OLD_MANIFEST), crate::pcgame::MANIFEST);
}

/// A download folder from PS5 Launcher: its ownership marker by the new name.
pub fn adopt_marker(folder: &Path) {
    rename_marker(&folder.join(OLD_OWNED), crate::downloads::MARKER);
}

fn rename_marker(old: &Path, new_name: &str) {
    if std::fs::symlink_metadata(old).is_ok_and(|m| m.is_file()) {
        let new = old.with_file_name(new_name);
        if !new.exists() {
            let _ = std::fs::rename(old, new);
        }
    }
}

/// An install.sh install: the binary becomes `azrak`, with a new menu entry and icon (the old ones
/// go). On an older PS5 Launcher OS image, whose session starts ~/.local/bin/ps5-launcher, the old
/// name stays as a link until the OS updates to AzrakOS.
fn install() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(bin) = exe.parent() else { return };
    let old_exe = bin.join(OLD);
    let new_exe = bin.join(crate::util::APP_NAME);
    if exe.file_name().is_some_and(|n| n == OLD) && !new_exe.exists() && std::fs::rename(&exe, &new_exe).is_ok() {
        crate::log!("binary renamed to {}", new_exe.display());
    }
    if new_exe.exists() {
        let _ = std::fs::rename(bin.join(format!("{OLD}.previous")), bin.join(format!("{}.previous", crate::util::APP_NAME)));
        let old_os = Path::new(OLD_OS_MARKER).exists() && !Path::new(crate::os::system::MARKER).exists();
        if old_os && !old_exe.exists() {
            let _ = std::os::unix::fs::symlink(crate::util::APP_NAME, &old_exe);
        } else if !old_os && std::fs::symlink_metadata(&old_exe).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = std::fs::remove_file(&old_exe);
        }
    }
    let data = data_dir();
    let Some(share) = data.parent() else { return };
    let config_base = config_dir();
    let Some(config_base) = config_base.parent() else { return };
    let mut menu_changed = false;
    for dir in [share.join("applications"), config_base.join("autostart")] {
        let old = dir.join(format!("{OLD}.desktop"));
        if old.is_file() {
            let exec = if new_exe.exists() { new_exe.clone() } else { exe.clone() };
            let _ = atomic_write(&dir.join(format!("{}.desktop", crate::util::APP_NAME)), desktop_entry(&exec).as_bytes());
            let _ = std::fs::remove_file(&old);
            menu_changed = true;
        }
    }
    let icons = share.join("icons").join("hicolor");
    let old_icon = icons.join("scalable").join("apps").join(format!("{OLD}.svg"));
    if old_icon.is_file() {
        let _ = atomic_write(&old_icon.with_file_name(format!("{}.svg", crate::util::APP_NAME)), crate::update::ICON);
        let _ = std::fs::remove_file(&old_icon);
        let _ = std::process::Command::new("gtk-update-icon-cache").args(["-q", "-f", "-t"]).arg(&icons)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        menu_changed = true;
    }
    if menu_changed {
        let _ = std::process::Command::new("update-desktop-database").arg(share.join("applications"))
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        crate::log!("menu entry and icon renamed to Azrak Launcher");
    }
}

/// The menu entry install.sh writes (kept the same as there).
pub fn desktop_entry(exe: &Path) -> String {
    format!("[Desktop Entry]\nType=Application\nName=Azrak Launcher\nGenericName=Game Launcher\nComment=Console-style game launcher for Linux\n\
Exec={}\nIcon={}\nTerminal=false\nCategories=Game;\nKeywords=azrak;ps5;ps4;playstation;kyty;shadps4;emulator;games;\nStartupWMClass={}\n",
        exe.display(), crate::util::APP_NAME, crate::util::APP_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_whole_folder_names_only() {
        let moved = vec![(PathBuf::from("/home/u/Games/PS5"), PathBuf::from("/home/u/Games/Azrak")),
            (PathBuf::from("/home/u/.local/share/ps5-launcher"), PathBuf::from("/home/u/.local/share/azrak"))];
        let text = r#"{"path":"/home/u/Games/PS5/PPSA02929-443f","other":"/home/u/Games/PS5-backup/x","root":"/home/u/Games/PS5","emulator":"/home/u/.local/share/ps5-launcher/kyty/current/kyty_emulator"}"#;
        assert_eq!(rewrite(text, &moved), r#"{"path":"/home/u/Games/Azrak/PPSA02929-443f","other":"/home/u/Games/PS5-backup/x","root":"/home/u/Games/Azrak","emulator":"/home/u/.local/share/azrak/kyty/current/kyty_emulator"}"#);
    }

    #[test]
    fn relinks_into_the_new_folder() {
        let t = tempfile::tempdir().unwrap();
        let (old, new) = (t.path().join("ps5-launcher"), t.path().join("azrak"));
        std::fs::create_dir_all(new.join("kyty/data/_SaveData")).unwrap();
        std::fs::create_dir_all(new.join("kyty/versions/v1")).unwrap();
        std::os::unix::fs::symlink(old.join("kyty/data/_SaveData"), new.join("kyty/versions/v1/_SaveData")).unwrap();
        std::os::unix::fs::symlink("versions/v1", new.join("kyty/current")).unwrap();
        relink(&new, &[(old, new.clone())]);
        assert_eq!(std::fs::read_link(new.join("kyty/versions/v1/_SaveData")).unwrap(), new.join("kyty/data/_SaveData"));
        assert_eq!(std::fs::read_link(new.join("kyty/current")).unwrap(), PathBuf::from("versions/v1"), "relative links stay");
        assert!(new.join("kyty/versions/v1/_SaveData").is_dir());
    }
}
