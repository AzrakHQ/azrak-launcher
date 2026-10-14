//! shadPS4, the PS4 emulator, managed like KytyPS5: the official Linux build from GitHub
//! Releases, kept under ~/.local/share/ps5-launcher/shadps4:
//!
//!   versions/<tag>/squashfs-root/   one unpacked release per folder (AppImage contents)
//!   current -> versions/<tag>
//!   state.json                       installed / previous tag
//!
//! The release is a zip holding an AppImage. It is unpacked once (`--appimage-extract`), so
//! running it needs no FUSE. shadPS4 keeps saves and settings in ~/.local/share/shadPS4, which
//! every version shares.

use crate::util::{atomic_write, http_json, now_secs};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REPO: &str = "shadps4-emu/shadPS4";
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
    crate::util::data_dir().join("shadps4")
}

/// The managed emulator to launch (stable path across updates).
pub fn emulator() -> PathBuf {
    root().join("current").join("squashfs-root").join("AppRun")
}

pub fn is_managed(emulator: &Path) -> bool {
    emulator.starts_with(root())
}

pub fn installed() -> bool {
    emulator().is_file() && !load_state().installed.is_empty()
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

/// "v.0.18.0" -> "0.18.0".
pub fn pretty(tag: &str) -> String {
    tag.trim_start_matches('v').trim_start_matches('.').to_string()
}

/// The latest official (not pre-release) Linux build.
pub fn latest_release() -> Result<Release, String> {
    let v = http_json(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?;
    let tag = v["tag_name"].as_str().ok_or("no releases found")?.to_string();
    let asset = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.starts_with("shadps4-linux") && n.ends_with(".zip")))
        .ok_or("this release has no Linux build")?;
    Ok(Release {
        tag,
        url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
        sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).unwrap_or("").to_string(),
    })
}

/// Pull the one file whose name ends with `suffix` out of a zip (stored or deflated).
pub(crate) fn unzip_one(zip: &Path, suffix: &str, dest: &Path) -> Result<(), String> {
    let bytes = std::fs::read(zip).map_err(|e| e.to_string())?;
    let u16_at = |o: usize| bytes.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize).ok_or("damaged zip");
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).ok_or("damaged zip");
    let eocd = (0..bytes.len().saturating_sub(21)).rev().take(70_000)
        .find(|&i| bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or("not a zip file")?;
    let (count, mut at) = (u16_at(eocd + 10)?, u32_at(eocd + 16)?);
    for _ in 0..count {
        if u32_at(at)? != 0x0201_4b50 {
            return Err("damaged zip".into());
        }
        let (method, crc, packed, size) = (u16_at(at + 10)?, u32_at(at + 16)?, u32_at(at + 20)?, u32_at(at + 24)?);
        let (name_len, extra, comment, local) = (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?, u32_at(at + 42)?);
        let name = String::from_utf8_lossy(bytes.get(at + 46..at + 46 + name_len).ok_or("damaged zip")?).into_owned();
        at += 46 + name_len + extra + comment;
        if !name.ends_with(suffix) {
            continue;
        }
        if u32_at(local)? != 0x0403_4b50 {
            return Err("damaged zip".into());
        }
        let start = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        let raw = bytes.get(start..start + packed).ok_or("damaged zip")?;
        let data = match method {
            0 => raw.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(raw).read_to_end(&mut out).map_err(|e| e.to_string())?;
                out
            }
            _ => return Err("unsupported zip compression".into()),
        };
        let mut check = flate2::Crc::new();
        check.update(&data);
        if data.len() != size || check.sum() as usize != crc {
            return Err("download is corrupted (zip checksum mismatch)".into());
        }
        return std::fs::write(dest, &data).map_err(|e| e.to_string());
    }
    Err(format!("no {suffix} in the download"))
}

/// Whether a binary runs here (prints its CLI banner).
fn runs(emulator: &Path) -> bool {
    let Ok(mut child) = Command::new(emulator).arg("--help").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return false };
    let start = std::time::Instant::now();
    while child.try_wait().ok().flatten().is_none() {
        if start.elapsed() > std::time::Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut text = String::new();
    let _ = child.stdout.take().map(|mut o| o.read_to_string(&mut text));
    text.contains("shadPS4")
}

fn switch_current(tag: &str) -> Result<(), String> {
    let r = root();
    let tmp = r.join("current.new");
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(Path::new("versions").join(tag), &tmp).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, r.join("current")).map_err(|e| e.to_string())
}

/// Download, verify, unpack and activate a release. Keeps the previous version for rollback.
pub fn install(rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String> {
    let r = root();
    let versions = r.join("versions");
    std::fs::create_dir_all(&versions).map_err(|e| e.to_string())?;
    let zip = r.join(format!("{}.zip.part", rel.tag));
    let last = std::cell::Cell::new(u64::MAX);
    crate::kyty::download_file(&rel.url, rel.size, &rel.sha256, &zip, &|done, total| {
        let pct = if total > 0 { done * 100 / total } else { 0 };
        if pct != last.get() {
            last.set(pct);
            progress(format!("Downloading shadPS4 {} · {pct}%", pretty(&rel.tag)), pct as f32 / 100.0 * 0.85);
        }
    })
    .inspect_err(|_| {
        let _ = std::fs::remove_file(&zip);
    })?;

    progress("Installing shadPS4…".into(), 0.9);
    let staging = versions.join(format!("{}.tmp", rel.tag));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let fail = |e: String| {
        let _ = std::fs::remove_dir_all(&staging);
        e
    };
    let appimage = staging.join("shadps4.AppImage");
    let unzipped = unzip_one(&zip, ".AppImage", &appimage);
    let _ = std::fs::remove_file(&zip);
    unzipped.map_err(fail)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&appimage, std::fs::Permissions::from_mode(0o755)).map_err(|e| fail(e.to_string()))?;
    let unpacked = Command::new(&appimage).arg("--appimage-extract").current_dir(&staging)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&appimage);
    let emu = staging.join("squashfs-root").join("AppRun");
    if !unpacked || !runs(&emu) {
        return Err(fail("the downloaded shadPS4 does not run on this system".into()));
    }
    let final_dir = versions.join(&rel.tag);
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&staging, &final_dir).map_err(|e| e.to_string())?;

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
    crate::log!("shadPS4 installed: {}", rel.tag);
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
    use super::*;

    #[test]
    fn tags_and_zip() {
        assert_eq!(pretty("v.0.18.0"), "0.18.0");
        // A tiny stored + deflated zip, built by hand.
        let t = tempfile::tempdir().unwrap();
        let payload = b"#!/bin/sh\necho shadPS4\n".repeat(20);
        let mut deflated = Vec::new();
        {
            use std::io::Write;
            let mut e = flate2::write::DeflateEncoder::new(&mut deflated, flate2::Compression::default());
            e.write_all(&payload).unwrap();
        }
        let mut crc = flate2::Crc::new();
        crc.update(&payload);
        let name = b"Shadps4-sdl.AppImage";
        let mut zip = Vec::new();
        let local = zip.len() as u32;
        zip.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        zip.extend_from_slice(&[20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        zip.extend_from_slice(&crc.sum().to_le_bytes());
        zip.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(name);
        zip.extend_from_slice(&deflated);
        let central = zip.len() as u32;
        zip.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        zip.extend_from_slice(&[20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        zip.extend_from_slice(&crc.sum().to_le_bytes());
        zip.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&[0; 12]);
        zip.extend_from_slice(&local.to_le_bytes());
        zip.extend_from_slice(name);
        let central_len = zip.len() as u32 - central;
        zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        zip.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
        zip.extend_from_slice(&central_len.to_le_bytes());
        zip.extend_from_slice(&central.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        let (src, dest) = (t.path().join("a.zip"), t.path().join("out"));
        std::fs::write(&src, &zip).unwrap();
        unzip_one(&src, ".AppImage", &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert!(unzip_one(&src, ".exe", &dest).is_err());
        let mut bad = zip.clone();
        bad[60] ^= 0xff; // corrupt the compressed data
        std::fs::write(&src, &bad).unwrap();
        assert!(unzip_one(&src, ".AppImage", &dest).is_err());
    }
}

