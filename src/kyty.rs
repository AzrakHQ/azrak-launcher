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

use crate::util::{atomic_write, http_json, now_secs};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "KytyPS5/KytyPS5";
/// Folders Kyty writes relative to its working directory.
pub const DATA_DIRS: [&str; 6] = ["_SaveData", "_PipelineCache", "_DownloadData", "_TempData", "_Textures", "_Patches"];
pub const CHECK_INTERVAL: f64 = 6.0 * 3600.0;

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct State {
    pub installed: String,
    pub previous: String,
    pub last_check: f64,
    pub latest: String,
    /// A build the user rolled back from: not re-installed automatically.
    pub skip: String,
}

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

pub fn load_state() -> State {
    std::fs::read(root().join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_state(s: &State) {
    if let Ok(j) = serde_json::to_vec_pretty(s) {
        let _ = atomic_write(&root().join("state.json"), &j);
    }
}

pub fn mark_checked(latest: &str) {
    let mut s = load_state();
    s.last_check = now_secs();
    s.latest = latest.to_string();
    save_state(&s);
}

/// Short, human version of a tag: "KytyPS5-2026-09-29-59a1760" -> "2026-09-29 · 59a1760".
pub fn pretty(tag: &str) -> String {
    let t = tag.trim_start_matches("KytyPS5-");
    match t.rsplit_once('-') {
        Some((date, sha)) if sha.len() >= 7 && date.len() == 10 => format!("{date} · {sha}"),
        _ => t.to_string(),
    }
}

/// Commit id of a tag ("…-59a1760" -> "59a1760").
pub fn tag_commit(tag: &str) -> &str {
    tag.rsplit('-').next().unwrap_or("")
}

/// Latest Linux x86_64 build on GitHub: the newest release that has one. A release is published
/// before its builds finish uploading, and one platform's build can fail, so the newest release
/// may have no Linux build yet (on 2026-10-14 it had only macOS's).
pub fn latest_release() -> Result<Release, String> {
    let v = http_json(&format!("https://api.github.com/repos/{REPO}/releases?per_page=10"))?;
    pick_release(&v)
}

fn pick_release(releases: &serde_json::Value) -> Result<Release, String> {
    let mut list: Vec<&serde_json::Value> = releases.as_array().ok_or("no releases found")?.iter()
        .filter(|r| r["draft"].as_bool() != Some(true) && r["prerelease"].as_bool() != Some(true))
        .collect();
    if list.is_empty() {
        return Err("no releases found".into());
    }
    list.sort_by(|a, b| b["published_at"].as_str().unwrap_or("").cmp(a["published_at"].as_str().unwrap_or("")));
    list.into_iter().find_map(|r| {
        let asset = r["assets"].as_array()?.iter()
            .find(|a| a["name"].as_str().is_some_and(|n| n.contains("Linux") && n.contains("x86_64") && n.ends_with(".tar.gz")))?;
        Some(Release {
            tag: r["tag_name"].as_str()?.to_string(),
            url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
            size: asset["size"].as_u64().unwrap_or(0),
            sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).unwrap_or("").to_string(),
        })
    }).ok_or_else(|| "no recent release has a Linux build yet".to_string())
}

/// The KytyPS5 build a game runs on: the managed build's tag, or a custom build's
/// "2026-09-30 (b7a1fac)". Can take up to 3 s for a custom build: call it off the UI thread.
pub fn version_for(emulator: &Path) -> String {
    let installed = load_state().installed;
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

/// Large downloads can take minutes: no overall deadline, only a stall timeout per read.
static DOWNLOADER: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| {
    ureq::AgentBuilder::new()
        .user_agent(crate::util::USER_AGENT)
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(60))
        .build()
});

fn download(rel: &Release, dest: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    download_file(&rel.url, rel.size, &rel.sha256, dest, progress)
}

/// Stream `url` to `dest`, verifying its SHA-256 when one is given.
pub fn download_file(url: &str, size: u64, sha256: &str, dest: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let resp = DOWNLOADER.get(url).call().map_err(|e| format!("download failed: {e}"))?;
    let total = resp.header("Content-Length").and_then(|s| s.parse().ok()).unwrap_or(size);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    if !sha256.is_empty() {
        let got = format!("{:x}", hasher.finalize());
        if !got.eq_ignore_ascii_case(sha256) {
            return Err("download is corrupted (checksum mismatch)".into());
        }
    }
    Ok(())
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

fn switch_current(tag: &str) -> Result<(), String> {
    let r = root();
    let tmp = r.join("current.new");
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(Path::new("versions").join(tag), &tmp).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, r.join("current")).map_err(|e| e.to_string())
}

/// Download, verify, extract and activate a release. Keeps the previous version for rollback.
pub fn install(rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String> {
    let r = root();
    let versions = r.join("versions");
    std::fs::create_dir_all(&versions).map_err(|e| e.to_string())?;
    let archive = r.join(format!("{}.tar.gz.part", rel.tag));
    let last = std::cell::Cell::new(0u64);
    download(rel, &archive, &|done, total| {
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
    let staging = versions.join(format!("{}.tmp", rel.tag));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let ok = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&staging).status().is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&archive);
    if !ok {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("could not extract the archive (is `tar` installed?)".into());
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
        return Err("the downloaded emulator does not run on this system".into());
    }
    link_data_dirs(&dir)?;
    let final_dir = versions.join(&rel.tag);
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&dir, &final_dir).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&staging);

    let mut st = load_state();
    if st.installed != rel.tag {
        st.previous = std::mem::take(&mut st.installed);
    }
    st.installed = rel.tag.clone();
    st.latest = rel.tag.clone();
    if st.skip == rel.tag {
        st.skip.clear();
    }
    st.last_check = now_secs();
    switch_current(&rel.tag)?;
    save_state(&st);
    cleanup(&st);
    crate::log!("KytyPS5 installed: {}", rel.tag);
    Ok(())
}

/// Switch back to the previously installed version.
pub fn rollback() -> Result<String, String> {
    let mut st = load_state();
    if st.previous.is_empty() || !root().join("versions").join(&st.previous).is_dir() {
        return Err("no previous version to roll back to".into());
    }
    switch_current(&st.previous)?;
    st.skip = st.installed.clone();
    std::mem::swap(&mut st.installed, &mut st.previous);
    save_state(&st);
    Ok(st.installed)
}

/// Keep only the installed and previous versions.
fn cleanup(st: &State) {
    let Ok(rd) = std::fs::read_dir(root().join("versions")) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name != st.installed && name != st.previous {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
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
        let r = pick_release(&list).unwrap();
        assert_eq!(r.tag, "KytyPS5-2026-10-12-df5b9b9", "newest by date with a Linux build, never a pre-release");
        assert_eq!(r.sha256, "ab");
        assert!(pick_release(&serde_json::Value::Array(vec![rel("only-mac", "2026-10-13T00:00:00Z", vec![asset("x-macOS.zip")])])).is_err());
    }

    use super::*;
    #[test]
    fn tags() {
        assert_eq!(pretty("KytyPS5-2026-09-29-59a1760"), "2026-09-29 · 59a1760");
        assert_eq!(tag_commit("KytyPS5-2026-09-29-59a1760"), "59a1760");
    }
}
