//! PS5 Launcher self-update from its own GitHub releases.
//!
//! The launcher checks at start and every 6 hours. A newer release is downloaded in the
//! background, verified (SHA-256 from GitHub), test-run, and swapped in place of the running
//! binary, or on macOS the whole app bundle (both keep the running program alive until it exits). The new version then starts
//! on the next launch, or right away with **Restart now** (never while a game is running).

use crate::app::*;
use crate::util::{http_json, now_secs};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Where updates come from. A fork can build with `PS5_LAUNCHER_REPO=owner/name` to follow its own releases.
pub const REPO: &str = match option_env!("PS5_LAUNCHER_REPO") {
    Some(repo) => repo,
    None => "MohamedAliRashad/ps5-launcher",
};
#[cfg(target_os = "linux")]
const ASSET: &str = "ps5-launcher-linux-x86_64.tar.gz";
#[cfg(target_os = "macos")]
const ASSET: &str = "ps5-launcher-macos-universal.zip";
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
    let asset = assets.iter().find(|a| a["name"] == ASSET).ok_or_else(|| format!("this release has no {} build yet", if cfg!(target_os = "macos") { "macOS" } else { "Linux" }))?;
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

/// The `.app` bundle that holds this executable, when it runs from one.
#[cfg(any(target_os = "macos", test))]
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    // <name>.app/Contents/MacOS/<exe>
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
}

fn writable(dir: &Path) -> bool {
    std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
}

/// What an update replaces, if the launcher may replace it itself: the installed binary on Linux,
/// the whole app bundle on macOS (the returned path is the executable inside it).
pub fn replaceable_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    if exe.components().any(|c| c.as_os_str() == "target") && exe.parent().is_some_and(|p| p.ends_with("release") || p.ends_with("debug") || p.ends_with("ci")) {
        return Err("running from a source build (update with git pull + ./install.sh)".into());
    }
    #[cfg(target_os = "macos")]
    {
        let app = bundle_of(&exe).ok_or("PS5 Launcher isn't running from its app bundle; download the new version from the releases page")?;
        let dir = app.parent().ok_or("no install folder")?;
        if !writable(dir) {
            return Err(format!("{} is not writable; download the new version from the releases page", dir.display()));
        }
        return Ok(exe);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let dir = exe.parent().ok_or("no install folder")?;
        if !writable(dir) {
            return Err(format!("{} is not writable (installed system-wide? re-run the install command with sudo)", dir.display()));
        }
        Ok(exe)
    }
}

/// Put `new` where `old` is, keeping `old` until the move has worked. Both are directories.
#[cfg(any(target_os = "macos", test))]
fn swap_dirs(old: &Path, new: &Path) -> Result<(), String> {
    let name = old.file_name().ok_or("bad install path")?.to_string_lossy().into_owned();
    let aside = old.with_file_name(format!(".{name}.replaced"));
    let _ = std::fs::remove_dir_all(&aside);
    std::fs::rename(old, &aside).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(new, old) {
        let _ = std::fs::rename(&aside, old);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

/// Download, verify, test and swap in the new binary. Returns the installed version.
pub fn install(rel: &Release, exe: &Path, progress: &dyn Fn(String, f32)) -> Result<(), String> {
    // The folder holding what gets replaced: the binary's, or on macOS the bundle's.
    #[cfg(target_os = "macos")]
    let app = bundle_of(exe).ok_or("not running from an app bundle")?;
    #[cfg(target_os = "macos")]
    let dir = app.parent().ok_or("no install folder")?.to_path_buf();
    #[cfg(not(target_os = "macos"))]
    let dir = exe.parent().ok_or("no install folder")?.to_path_buf();
    // Work next to the install so the final rename stays on one filesystem.
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
    #[cfg(target_os = "macos")]
    let (ok, new_item, new_bin) = {
        // ditto keeps the bundle's permissions and structure; the zip holds <dir>/PS5 Launcher.app.
        let ok = Command::new("ditto").args(["-x", "-k"]).arg(&archive).arg(&work).status().is_ok_and(|s| s.success());
        let new_app = work.join("ps5-launcher-macos-universal").join("PS5 Launcher.app");
        let bin = new_app.join("Contents/MacOS/ps5-launcher");
        (ok, new_app, bin)
    };
    #[cfg(not(target_os = "macos"))]
    let (ok, new_item, new_bin) = {
        let ok = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&work).status().is_ok_and(|s| s.success());
        let bin = work.join("ps5-launcher-linux-x86_64").join("ps5-launcher");
        (ok, bin.clone(), bin)
    };
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
    #[cfg(target_os = "macos")]
    let r = swap_dirs(&app, &new_item);
    #[cfg(not(target_os = "macos"))]
    let r = {
        // Keep the old binary for a manual rollback, then swap atomically.
        let _ = std::fs::copy(exe, dir.join("ps5-launcher.previous"));
        let staged = dir.join(".ps5-launcher.new");
        std::fs::rename(&new_item, &staged).map_err(|e| e.to_string()).and_then(|_| std::fs::rename(&staged, exe).map_err(|e| e.to_string()))
    };
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

    #[test]
    fn finds_the_bundle_around_an_executable() {
        let exe = Path::new("/Applications/PS5 Launcher.app/Contents/MacOS/ps5-launcher");
        assert_eq!(bundle_of(exe), Some(PathBuf::from("/Applications/PS5 Launcher.app")));
        assert_eq!(bundle_of(Path::new("/usr/local/bin/ps5-launcher")), None);
        assert_eq!(bundle_of(Path::new("/x/Foo.app/Contents/Resources/ps5-launcher")), None);
        assert_eq!(bundle_of(Path::new("/x/Foo/Contents/MacOS/ps5-launcher")), None);
    }

    #[test]
    fn swap_dirs_replaces_and_leaves_no_leftovers() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("App.app");
        let new = t.path().join("stage").join("App.app");
        std::fs::create_dir_all(old.join("Contents")).unwrap();
        std::fs::write(old.join("Contents/v"), "1").unwrap();
        std::fs::create_dir_all(new.join("Contents")).unwrap();
        std::fs::write(new.join("Contents/v"), "2").unwrap();
        swap_dirs(&old, &new).unwrap();
        assert_eq!(std::fs::read_to_string(old.join("Contents/v")).unwrap(), "2");
        assert!(!new.exists());
        let names: Vec<_> = std::fs::read_dir(t.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(!names.iter().any(|n| n.ends_with(".replaced")), "{names:?}");
    }

    #[test]
    fn failed_swap_keeps_the_old_install() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("App.app");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("v"), "1").unwrap();
        assert!(swap_dirs(&old, &t.path().join("missing")).is_err());
        assert_eq!(std::fs::read_to_string(old.join("v")).unwrap(), "1");
    }
}
