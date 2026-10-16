//! KytyPS5 auto-updater.
//!
//! KytyPS5 publishes prebuilt Linux builds on GitHub Releases. The launcher can manage its own
//! copy under ~/.local/share/ps5-launcher/kyty:
//!
//!   versions/<tag>/     one extracted release per folder
//!   current -> versions/<tag>
//!   data/               _SaveData, _PipelineCache, ... shared by every version (symlinked in)
//!   state.json          installed / previous tag
//!
//! Kyty writes its data relative to its working directory, so each version folder gets symlinks
//! to `data/`: saves and shader caches survive updates and rollbacks.

use crate::emulator::store::ManagedStore;
pub use crate::emulator::store::Release;
use crate::util::http_json;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "KytyPS5/KytyPS5";
/// Folders Kyty writes relative to its working directory.
pub const DATA_DIRS: [&str; 6] = ["_SaveData", "_PipelineCache", "_DownloadData", "_TempData", "_Textures", "_Patches"];

pub fn root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::util::expand_home("~/.local/share"))
        .join(crate::util::APP_NAME)
        .join("kyty")
}

/// Stable emulator path for the managed install (survives updates).
pub fn managed_emulator() -> PathBuf {
    root().join("current").join("kyty_emulator")
}

pub fn is_managed(emulator: &Path) -> bool {
    emulator.starts_with(root())
}

pub fn store() -> ManagedStore {
    ManagedStore::new(root())
}

/// Short, human version of a tag: "KytyPS5-2026-09-29-59a1760" -> "2026-09-29 · 59a1760".
pub fn pretty(tag: &str) -> String {
    let t = tag.trim_start_matches("KytyPS5-");
    match t.rsplit_once('-') {
        Some((date, sha)) if sha.len() >= 7 && date.len() == 10 => format!("{date} · {sha}"),
        _ => t.to_string(),
    }
}

/// Whether a release asset is the KytyPS5 build for `os` ("linux" or "macos").
///
/// Linux builds are named like "...Linux...x86_64....tar.gz". KytyPS5 ships macOS as an
/// x86-64 build that runs under Rosetta 2, so on macOS the architecture isn't checked and any
/// archive whose name says macOS (or Darwin/OSX) matches.
fn asset_matches(name: &str, os: &str) -> bool {
    if os == "macos" {
        let n = name.to_ascii_lowercase();
        return ["macos", "darwin", "osx"].iter().any(|k| n.contains(k)) && [".tar.gz", ".tgz", ".zip"].iter().any(|e| n.ends_with(e));
    }
    name.contains("Linux") && name.contains("x86_64") && name.ends_with(".tar.gz")
}

/// Zip files start with "PK" and a record type. Sniffed from the content because the download
/// is saved under one fixed name whatever its format.
fn is_zip(path: &Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && matches!(&magic, b"PK\x03\x04" | b"PK\x05\x06")
}

/// Unpack a downloaded emulator archive into `dest`: KytyPS5 ships Linux as .tar.gz and macOS
/// as .zip. On macOS `ditto` is used for zips since it keeps permissions and symlinks.
fn extract(archive: &Path, dest: &Path) -> bool {
    let mut cmd;
    if is_zip(archive) {
        if cfg!(target_os = "macos") {
            cmd = Command::new("ditto");
            cmd.args(["-x", "-k"]).arg(archive).arg(dest);
        } else {
            cmd = Command::new("unzip");
            cmd.args(["-q", "-o"]).arg(archive).arg("-d").arg(dest);
        }
    } else {
        cmd = Command::new("tar");
        cmd.arg(if cfg!(target_os = "macos") { "-xf" } else { "-xzf" }).arg(archive).arg("-C").arg(dest);
    }
    cmd.status().is_ok_and(|s| s.success())
}

fn os_label() -> &'static str {
    if std::env::consts::OS == "macos" { "macOS" } else { "Linux" }
}

/// Latest build for this OS on GitHub: the newest release that has one. A release is published
/// before its builds finish uploading, and one platform's build can fail, so the newest release
/// may have no build for this OS yet (on 2026-10-14 it had only macOS's).
pub fn latest_release() -> Result<Release, String> {
    let v = http_json(&format!("https://api.github.com/repos/{REPO}/releases?per_page=10"))?;
    pick_release(&v, std::env::consts::OS)
}

fn pick_release(releases: &serde_json::Value, os: &str) -> Result<Release, String> {
    let mut list: Vec<&serde_json::Value> = releases.as_array().ok_or("no releases found")?.iter()
        .filter(|r| r["draft"].as_bool() != Some(true) && r["prerelease"].as_bool() != Some(true))
        .collect();
    if list.is_empty() {
        return Err("no releases found".into());
    }
    list.sort_by(|a, b| b["published_at"].as_str().unwrap_or("").cmp(a["published_at"].as_str().unwrap_or("")));
    list.into_iter().find_map(|r| {
        let asset = r["assets"].as_array()?.iter()
            .find(|a| a["name"].as_str().is_some_and(|n| asset_matches(n, os)))?;
        Some(Release {
            tag: r["tag_name"].as_str()?.to_string(),
            url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
            size: asset["size"].as_u64().unwrap_or(0),
            sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).unwrap_or("").to_string(),
        })
    }).ok_or_else(|| format!("no recent release has a {} build yet", os_label()))
}

/// The KytyPS5 build a game runs on: the managed build's tag, or a custom build's
/// "2026-09-30 (b7a1fac)". Can take up to 3 s for a custom build: call it off the UI thread.
pub fn version_for(emulator: &Path) -> String {
    let installed = store().load_state().installed;
    if is_managed(emulator) && !installed.is_empty() {
        return installed;
    }
    binary_version(emulator).map(|(git, date)| format!("{date} ({git})").trim().to_string()).unwrap_or_default()
}

/// Version info of any emulator binary, from its `--help` banner ("git = 6799ecb, date = 2026.09.29").
pub fn binary_version(emulator: &Path) -> Option<(String, String)> {
    // Never hang on an odd binary: give it 3 s to print its banner.
    use std::io::Read as _;
    let mut child = Command::new(emulator)
        .arg("--help")
        .current_dir(emulator.parent()?)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    while child.try_wait().ok()?.is_none() {
        if start.elapsed() > std::time::Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    let line = text.lines().next()?;
    let field = |k: &str| line.split(&format!("{k} = ")).nth(1).map(|s| s.split(',').next().unwrap_or("").trim().to_string());
    Some((field("git")?, field("date").unwrap_or_default().replace('.', "-")))
}

fn link_data_dirs(version_dir: &Path) -> Result<(), String> {
    let data = root().join("data");
    for d in DATA_DIRS {
        let shared = data.join(d);
        std::fs::create_dir_all(&shared).map_err(|e| e.to_string())?;
        let link = version_dir.join(d);
        if link.symlink_metadata().is_ok() {
            if link.is_symlink() {
                std::fs::remove_file(&link).map_err(|e| e.to_string())?;
            } else {
                // A real folder shipped in the archive: fold its contents into the shared one.
                let _ = Command::new("cp").args(["-an"]).arg(link.join(".")).arg(&shared).status();
                std::fs::remove_dir_all(&link).map_err(|e| e.to_string())?;
            }
        }
        std::os::unix::fs::symlink(&shared, &link).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Copy saves and caches from a self-built/custom Kyty folder into the shared data folder
/// (only into folders that are still empty; the source is never modified).
pub fn import_data_from(emulator_dir: &Path) -> usize {
    let data = root().join("data");
    let mut copied = 0;
    for d in DATA_DIRS {
        let src = emulator_dir.join(d);
        let dst = data.join(d);
        if !src.is_dir() || src.is_symlink() {
            continue;
        }
        let empty = std::fs::read_dir(&dst).map(|mut r| r.next().is_none()).unwrap_or(true);
        if !empty {
            continue;
        }
        let _ = std::fs::create_dir_all(&dst);
        if Command::new("cp").args(["-a"]).arg(src.join(".")).arg(&dst).status().is_ok_and(|s| s.success()) {
            copied += 1;
        }
    }
    copied
}

/// Download, verify, extract and activate a release. Keeps the previous version for rollback.
pub fn install(rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String> {
    let store = store();
    std::fs::create_dir_all(store.versions_dir()).map_err(|e| e.to_string())?;
    let archive = store.download_path(&rel.tag, "tar.gz");
    let last = std::cell::Cell::new(0u64);
    crate::emulator::download::file(&rel.url, rel.size, &rel.sha256, &archive, &|done, total| {
        let pct = if total > 0 { done * 100 / total } else { 0 };
        if pct != last.get() {
            last.set(pct);
            progress(format!("Downloading KytyPS5 {} · {pct}%", pretty(&rel.tag)), pct as f32 / 100.0 * 0.9);
        }
    })
    .inspect_err(|_| {
        let _ = std::fs::remove_file(&archive);
    })?;

    progress("Installing KytyPS5…".into(), 0.95);
    let staging = store.staging_dir(&rel.tag);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let ok = extract(&archive, &staging);
    let _ = std::fs::remove_file(&archive);
    if !ok {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("could not extract the archive (is `tar` installed, and `unzip` for zip files?)".into());
    }
    // Some archives wrap everything in one top-level folder.
    let mut dir = staging.clone();
    if !dir.join("kyty_emulator").is_file() {
        if let Some(inner) = std::fs::read_dir(&staging).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.join("kyty_emulator").is_file())) {
            dir = inner;
        }
    }
    let emu = dir.join("kyty_emulator");
    if binary_version(&emu).is_none() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(if cfg!(target_os = "macos") {
            "the downloaded emulator does not run (on Apple Silicon it needs Rosetta: run `softwareupdate --install-rosetta`)".into()
        } else {
            "the downloaded emulator does not run on this system".into()
        });
    }
    link_data_dirs(&dir)?;
    store.activate(&rel.tag, &dir)?;
    let _ = std::fs::remove_dir_all(&staging);
    crate::log!("KytyPS5 installed: {}", rel.tag);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn takes_the_newest_release_with_a_linux_build() {
        let asset = |n: &str| serde_json::json!({ "name": n, "browser_download_url": format!("https://x/{n}"), "size": 1, "digest": "sha256:ab" });
        let rel = |tag: &str, at: &str, assets: Vec<serde_json::Value>| serde_json::json!({ "tag_name": tag, "published_at": at, "assets": assets });
        let mut beta = rel("KytyPS5-2026-10-14-beta", "2026-10-14T00:00:00Z", vec![asset("KytyPS5-beta-Linux-x86_64.tar.gz")]);
        beta["prerelease"] = true.into();
        let list = serde_json::Value::Array(vec![
            rel("KytyPS5-2026-10-13-9e41d83", "2026-10-13T21:52:01Z", vec![asset("KytyPS5-2026-10-13-9e41d83-macOS-x86_64.zip")]),
            rel("KytyPS5-2026-10-12-dad900c", "2026-10-12T03:14:05Z", vec![asset("KytyPS5-2026-10-12-dad900c-Linux-x86_64.tar.gz")]),
            rel("KytyPS5-2026-10-12-df5b9b9", "2026-10-12T13:17:34Z", vec![asset("KytyPS5-2026-10-12-df5b9b9-Linux-x86_64.tar.gz")]),
            beta,
        ]);
        let r = pick_release(&list, "linux").unwrap();
        assert_eq!(r.tag, "KytyPS5-2026-10-12-df5b9b9", "newest by date with a Linux build, never a pre-release");
        assert_eq!(r.sha256, "ab");
        assert!(pick_release(&serde_json::Value::Array(vec![rel("only-mac", "2026-10-13T00:00:00Z", vec![asset("x-macOS.zip")])]), "linux").is_err());
    }

    use super::*;
    #[test]
    fn tags() {
        assert_eq!(pretty("KytyPS5-2026-09-29-59a1760"), "2026-09-29 · 59a1760");
    }
}

#[cfg(test)]
mod asset_tests {
    use super::asset_matches;

    #[test]
    fn linux_wants_the_x86_64_tarball() {
        assert!(asset_matches("KytyPS5-2026-10-01-4479808-Linux-x86_64.tar.gz", "linux"));
        assert!(!asset_matches("KytyPS5-2026-10-01-4479808-Linux-aarch64.tar.gz", "linux"));
        assert!(!asset_matches("KytyPS5-2026-10-01-4479808-Windows-x86_64.zip", "linux"));
        assert!(!asset_matches("KytyPS5-2026-10-01-4479808-macOS-x86_64.tar.gz", "linux"));
    }

    #[test]
    fn macos_takes_a_macos_archive_of_either_kind() {
        assert!(asset_matches("KytyPS5-2026-10-01-4479808-macOS-x86_64.tar.gz", "macos"));
        assert!(asset_matches("KytyPS5-macos.zip", "macos"));
        assert!(asset_matches("kyty-darwin-x86_64.tgz", "macos"));
        assert!(!asset_matches("KytyPS5-2026-10-01-4479808-Linux-x86_64.tar.gz", "macos"));
        assert!(!asset_matches("KytyPS5-2026-10-01-4479808-Windows-x86_64.zip", "macos"));
        assert!(!asset_matches("KytyPS5-macOS.dmg", "macos"));
    }
}

#[cfg(test)]
mod extract_tests {
    use super::{extract, is_zip};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn have(tool: &str) -> bool {
        Command::new(tool).arg("--help").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok()
    }

    /// A folder shaped like KytyPS5's macOS zip: a flat executable, a library and a symlink.
    fn payload(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("kyty_emulator"), b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(dir.join("kyty_emulator"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("libMoltenVK.dylib"), b"lib").unwrap();
        std::os::unix::fs::symlink("kyty_emulator", dir.join("emu-link")).unwrap();
    }

    fn check(dest: &std::path::Path) {
        assert!(dest.join("kyty_emulator").is_file());
        assert_eq!(std::fs::metadata(dest.join("kyty_emulator")).unwrap().permissions().mode() & 0o111, 0o111, "executable bit kept");
        assert_eq!(std::fs::read(dest.join("libMoltenVK.dylib")).unwrap(), b"lib");
        assert_eq!(std::fs::read_link(dest.join("emu-link")).unwrap(), std::path::Path::new("kyty_emulator"));
    }

    #[test]
    fn zip_is_told_apart_from_gzip_by_content() {
        let t = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let p = t.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        assert!(is_zip(&write("a.part", b"PK\x03\x04rest")));
        assert!(is_zip(&write("empty.zip", b"PK\x05\x06")));
        assert!(!is_zip(&write("b.part", &[0x1f, 0x8b, 8, 0])));
        assert!(!is_zip(&write("short", b"PK")));
        assert!(!is_zip(&t.path().join("missing")));
    }

    #[test]
    fn a_zip_extracts_with_permissions_and_symlinks() {
        if !have("zip") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        payload(&src);
        // Named like the download: the extension says nothing about the format.
        let archive = t.path().join("KytyPS5-tag.tar.gz.part");
        assert!(Command::new("zip").current_dir(&src).args(["-qry"]).arg(&archive).arg(".").status().unwrap().success());
        let dest = t.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        assert!(extract(&archive, &dest));
        check(&dest);
    }

    #[test]
    fn a_tar_gz_extracts_too() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        payload(&src);
        let archive = t.path().join("a.tar.gz.part");
        assert!(Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(&src).arg(".").status().unwrap().success());
        let dest = t.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        assert!(extract(&archive, &dest));
        check(&dest);
    }

    #[test]
    fn garbage_does_not_extract() {
        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("bad");
        std::fs::write(&archive, b"not an archive at all").unwrap();
        let dest = t.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        assert!(!extract(&archive, &dest));
    }
}
