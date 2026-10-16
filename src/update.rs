//! PS5 Launcher self-update from its own GitHub releases.
//!
//! The launcher checks at start and every 6 hours. A newer release is downloaded in the
//! background, verified (SHA-256 from GitHub), test-run, and swapped in place of the running
//! binary (Linux keeps the running program alive until it exits). The new version then starts
//! on the next launch, or right away with **Restart now** (never while a game is running).

use crate::app::*;
use crate::util::{http_json, now_secs};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const REPO: &str = "AzrakHQ/azrak";
const ASSET: &str = "ps5-launcher-linux-x86_64.tar.gz";
const CHECK_INTERVAL: f64 = 6.0 * 3600.0;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// Version of the running binary. `PS5_LAUNCHER_PRETEND_VERSION` exists for testing updates.
/// Put the current menu icon in place of an older one. Automatic updates replace only the
/// binary, so without this an updated launcher keeps the icon of the version first installed.
/// Only an icon that install.sh put in the user's data folder is touched.
pub fn refresh_menu_icon() {
    const ICON: &[u8] = include_bytes!("../assets/ps5-launcher.svg");
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| crate::util::expand_home("~/.local/share"));
    let hicolor = data.join("icons").join("hicolor");
    let path = hicolor.join("scalable").join("apps").join(format!("{}.svg", crate::util::APP_NAME));
    match std::fs::read(&path) {
        Ok(old) if old != ICON => {}
        _ => return,
    }
    if let Err(e) = crate::util::atomic_write(&path, ICON) {
        crate::log!("menu icon update failed: {e}");
        return;
    }
    crate::log!("menu icon updated");
    // Desktops read icons through GTK's cache for that folder; rebuild it so the new one shows.
    let _ = Command::new("gtk-update-icon-cache")
        .args(["-q", "-f", "-t"])
        .arg(&hicolor)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

pub fn current_version() -> String {
    std::env::var("PS5_LAUNCHER_PRETEND_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

fn parse(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches('v').split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse(candidate) > parse(current)
}

pub fn latest_release() -> Result<Release, String> {
    let v = http_json(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?;
    let tag = v["tag_name"].as_str().ok_or("no releases found")?;
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let asset = assets.iter().find(|a| a["name"] == ASSET).ok_or("this release has no Linux build yet")?;
    let mut sha256 = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).unwrap_or("").to_string();
    if sha256.is_empty() {
        // Older releases: a separate .sha256 file.
        if let Some(u) = assets.iter().find(|a| a["name"] == format!("{ASSET}.sha256")).and_then(|a| a["browser_download_url"].as_str()) {
            if let Ok(b) = crate::util::http_get(u) {
                sha256 = String::from_utf8_lossy(&b).split_whitespace().next().unwrap_or("").to_string();
            }
        }
    }
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
        sha256,
    })
}

/// The installed binary, if the launcher may replace it itself.
pub fn replaceable_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    if exe.components().any(|c| c.as_os_str() == "target") && exe.parent().is_some_and(|p| p.ends_with("release") || p.ends_with("debug")) {
        return Err("running from a source build (update with git pull + ./install.sh)".into());
    }
    let dir = exe.parent().ok_or("no install folder")?;
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    if unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0 {
        return Err(format!("{} is not writable (installed system-wide? re-run the install command with sudo)", dir.display()));
    }
    Ok(exe)
}

/// Download, verify, test and swap in the new binary. Returns the installed version.
pub fn install(rel: &Release, exe: &Path, progress: &dyn Fn(String, f32)) -> Result<(), String> {
    let dir = exe.parent().ok_or("no install folder")?;
    // Work next to the binary so the final rename stays on one filesystem.
    let work = dir.join(".ps5-launcher-update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&work);
    };
    let archive = work.join(ASSET);
    let last = std::cell::Cell::new(101u64);
    let res = crate::kyty::download_file(&rel.url, rel.size, &rel.sha256, &archive, &|done, total| {
        let pct = if total > 0 { done * 100 / total } else { 0 };
        if pct != last.get() {
            last.set(pct);
            progress(format!("Downloading PS5 Launcher {} · {pct}%", rel.version), pct as f32 / 100.0);
        }
    });
    if let Err(e) = res {
        cleanup();
        return Err(e);
    }
    progress(format!("Installing PS5 Launcher {}…", rel.version), 1.0);
    let ok = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&work).status().is_ok_and(|s| s.success());
    let new_bin = work.join("ps5-launcher-linux-x86_64").join("ps5-launcher");
    if !ok || !new_bin.is_file() {
        cleanup();
        return Err("could not unpack the update".into());
    }
    // The new binary must run here and report the expected version.
    let out = Command::new(&new_bin).arg("--version").env_remove("PS5_LAUNCHER_PRETEND_VERSION").output();
    let reported = out.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if !reported.ends_with(&rel.version) {
        cleanup();
        return Err(format!("the new version does not run on this system ({})", if reported.is_empty() { "no output" } else { &reported }));
    }
    let _ = std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755));
    // Keep the old binary for a manual rollback, then swap atomically.
    let _ = std::fs::copy(exe, dir.join("ps5-launcher.previous"));
    let staged = dir.join(".ps5-launcher.new");
    std::fs::rename(&new_bin, &staged).map_err(|e| e.to_string())?;
    let r = std::fs::rename(&staged, exe).map_err(|e| e.to_string());
    cleanup();
    r?;
    crate::log!("PS5 Launcher updated to {}", rel.version);
    Ok(())
}

// ---------------------------------------------------------------------------- in the app

#[derive(Default)]
pub struct AppUpdate {
    pub latest: Option<Release>,
    pub checking: bool,
    pub busy: bool,
    pub progress: String,
    pub error: String,
    /// Installed on disk, waiting for a restart.
    pub installed: Option<String>,
    pub last_check: f64,
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

impl App {
    pub fn app_update_start(&mut self) {
        self.app_update_check(false);
        let t = slint::Timer::default();
        t.start(slint::TimerMode::Repeated, Duration::from_secs(30 * 60), || {
            with_app(|app| {
                if now_secs() - app.upd.last_check > CHECK_INTERVAL {
                    app.app_update_check(false);
                }
            })
        });
        std::mem::forget(t);
    }

    pub fn app_update_available(&self) -> bool {
        self.upd.installed.is_none() && self.upd.latest.as_ref().is_some_and(|r| is_newer(&r.version, &current_version()))
    }

    pub fn app_update_check(&mut self, manual: bool) {
        if self.upd.checking || self.upd.busy {
            return;
        }
        self.upd.checking = true;
        self.upd.error.clear();
        self.kyty_refresh_settings();
        std::thread::spawn(move || {
            let res = latest_release();
            post(move |app| {
                app.upd.checking = false;
                app.upd.last_check = now_secs();
                match res {
                    Ok(rel) => {
                        app.upd.latest = Some(rel.clone());
                        if app.app_update_available() {
                            let auto = app.cfg.lock().unwrap().app_auto_update;
                            if auto || manual {
                                app.app_update_install(rel);
                            } else {
                                app.toast_app("Update available", &format!("PS5 Launcher {} can be installed from Settings.", rel.version), 0);
                            }
                        } else if manual {
                            app.toast_app("PS5 Launcher is up to date", &format!("You have the latest version, {}.", current_version()), 1);
                        }
                    }
                    Err(e) => {
                        crate::log!("launcher update check failed: {e}");
                        // A background check that fails (offline) stays in the log; Settings
                        // shows errors only for a check you asked for.
                        if manual {
                            app.upd.error = format!("Update check failed: {e}");
                            app.toast_app("Couldn't check for updates", &e, 2);
                        }
                    }
                }
                app.kyty_refresh_settings();
            });
        });
    }

    pub fn app_update_install(&mut self, rel: Release) {
        if self.upd.busy {
            return;
        }
        let exe = match replaceable_exe() {
            Ok(p) => p,
            Err(e) => {
                crate::log!("launcher update {} available, not installed: {e}", rel.version);
                self.upd.error = format!("Can't update automatically: {e}");
                self.toast_app("Update available", &format!("PS5 Launcher {} can't install automatically: {e}.", rel.version), 0);
                self.kyty_refresh_settings();
                return;
            }
        };
        self.upd.busy = true;
        self.upd.error.clear();
        self.kyty_refresh_settings();
        std::thread::spawn(move || {
            let last = std::sync::Mutex::new(String::new());
            let res = install(&rel, &exe, &|p, _| {
                let mut l = last.lock().unwrap();
                if *l != p {
                    *l = p.clone();
                    post(move |app| {
                        app.upd.progress = p.clone();
                        if !app.boot.active {
                            app.set_status(&p, true);
                        }
                        app.kyty_refresh_settings();
                    });
                }
            });
            post(move |app| {
                app.upd.busy = false;
                app.upd.progress.clear();
                app.set_status("", false);
                match res {
                    Ok(()) => {
                        app.upd.installed = Some(rel.version.clone());
                        app.toast_app("Update ready", &format!("PS5 Launcher {} starts the next time you open it. To switch now, choose Settings → Updates → PS5 Launcher.", rel.version), 1);
                    }
                    Err(e) => {
                        crate::log!("launcher update failed: {e}");
                        app.upd.error = format!("Update failed: {e}");
                        app.toast_app("Update failed", &e, 2);
                    }
                }
                app.kyty_refresh_settings();
            });
        });
    }

    /// Start the new version in place of this one (keeps the same window position/flags).
    pub fn app_restart(&mut self) {
        if !self.live.is_empty() {
            self.toast_app("Finish your game first", "The launcher can restart once no game is running.", 0);
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        // current_exe() of a replaced binary reads "…/ps5-launcher (deleted)": use the path itself.
        let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
        // Drop --monitor: after a display change the saved setting must win.
        let mut args: Vec<String> = Vec::new();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            if a == "--monitor" {
                it.next();
            } else {
                args.push(a);
            }
        }
        crate::log!("restarting into the new version");
        self.installer.shutdown();
        self.downloads.shutdown();
        use std::os::unix::process::CommandExt;
        let err = Command::new(&exe).args(&args).env_remove("PS5_LAUNCHER_PRETEND_VERSION").exec();
        let (seed, vpn) = { let c = self.cfg.lock().unwrap(); (c.seed_after_download, c.download_vpn()) };
        self.downloads = crate::downloads::Manager::load(seed, vpn);
        self.installer = crate::installer::Manager::load();
        self.push_downloads();
        self.toast_app("Couldn't restart", &err.to_string(), 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert!(is_newer("1.2.0", "1.1.9"));
        assert!(is_newer("v1.10.0", "1.9.3"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.0", "1.2.0"));
    }
}
