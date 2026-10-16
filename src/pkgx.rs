//! PS4 PKG packages, extracted with the ShadPs4Plus PKG Extractor (GPL-2.0): the PKG code shadPS4
//! itself shipped until 0.8, as a standalone command-line tool. It is downloaded the first time a
//! PKG release is installed and checked against a pinned SHA-256, then kept under
//! ~/.local/share/azrak/pkg-extractor.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const VERSION: &str = "1.0";
const URL: &str = "https://github.com/AzaharPlus/shadPS4Plus/releases/download/PKG_EXTRACTOR_1_0/ShadPs4Plus-PkgExtractor-1.0-linux.zip";
const SIZE: u64 = 1_944_244;
const SHA256: &str = "2adb5760be588eef0db76a0b281467ad2fc8ce6885fe00778af143b51b3d4784";

/// src/pkgguard.c, built by build.rs: preloaded into the extractor so a failed or short write
/// stops it with a message, instead of leaving truncated files behind a "success" report.
const GUARD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pkgguard.so"));
/// What the guard prints from inside the extractor once it's loaded.
const GUARD_ACTIVE: &str = "PKG write guard active";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Game,
    Update,
    Dlc,
}

fn root() -> PathBuf {
    crate::util::data_dir().join("pkg-extractor")
}

fn tool() -> PathBuf {
    root().join(VERSION).join("squashfs-root").join("AppRun")
}

/// The extractor, downloaded and unpacked on first use.
pub fn ensure() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(tool) = TEST_TOOL.with(|t| t.borrow().clone()) {
        return Ok(tool);
    }
    if let Some(path) = std::env::var_os("AZRAK_PKG_EXTRACTOR").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let tool = tool();
    if tool.is_file() {
        return Ok(tool);
    }
    if cfg!(test) {
        return Err("no PKG extractor in unit tests (set AZRAK_PKG_EXTRACTOR)".into());
    }
    install()
}

fn install() -> Result<PathBuf, String> {
    let tool = tool();
    let r = root();
    std::fs::create_dir_all(&r).map_err(|e| e.to_string())?;
    let zip = r.join("download.zip.part");
    crate::kyty::download_file(URL, SIZE, SHA256, &zip, &|_, _| {}).inspect_err(|_| {
        let _ = std::fs::remove_file(&zip);
    })?;
    let staging = r.join(format!("{VERSION}.tmp"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let appimage = staging.join("pkg_extractor.AppImage");
    let unzipped = crate::shad::unzip_one(&zip, "pkg_extractor.AppImage", &appimage);
    let _ = std::fs::remove_file(&zip);
    unzipped?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&appimage, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    // Unpacked once, so it runs without FUSE.
    let unpacked = Command::new(&appimage).arg("--appimage-extract").current_dir(&staging)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&appimage);
    let unpacked_tool = staging.join("squashfs-root").join("AppRun");
    if !unpacked || !runs(&unpacked_tool) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("the PKG extractor does not run on this system".into());
    }
    let final_dir = r.join(VERSION);
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&staging, &final_dir).map_err(|e| e.to_string())?;
    crate::log!("PKG extractor {VERSION} installed");
    Ok(tool)
}

/// A throwaway working folder for one run. The extractor carries shadPS4's start-up code, which
/// sets up a user folder (a `user` folder in the working directory wins): this keeps that out of
/// the real shadPS4 folder and inside what the sandbox lets it write. Removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("azrak-pkg-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(dir.join("user")).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self(dir))
    }

    /// The guard library, kept next to the extractor: that folder evidently allows running
    /// programs, unlike a temporary folder that may be mounted noexec (the loader would then skip
    /// the guard). This folder is the fallback; the extraction checks the guard actually loaded.
    fn guard(&self, tool: &Path) -> Result<PathBuf, String> {
        #[cfg(test)]
        let bytes: &[u8] = if TEST_BROKEN_GUARD.with(|b| *b.borrow()) { b"not a shared library" } else { GUARD };
        #[cfg(not(test))]
        let bytes = GUARD;
        let place = |dir: &Path| -> std::io::Result<PathBuf> {
            let path = dir.join("pkgguard.so");
            if std::fs::read(&path).ok().as_deref() != Some(bytes) {
                let tmp = dir.join(format!(".pkgguard.so.{}", std::process::id()));
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &path)?;
            }
            Ok(path)
        };
        let beside_tool = std::fs::canonicalize(tool).ok().and_then(|t| t.parent().map(Path::to_path_buf));
        beside_tool.and_then(|dir| place(&dir).ok()).map_or_else(|| place(&self.0).map_err(|e| format!("{}: {e}", self.0.display())), Ok)
    }

    /// The extractor command, confined to writing in this folder and `writable`.
    fn command(&self, tool: &Path, writable: Option<&Path>) -> Result<(Command, crate::sandbox::Confinement), String> {
        let mut cmd = Command::new(tool);
        cmd.current_dir(&self.0).stdin(Stdio::null()).stderr(Stdio::null()).env("LD_PRELOAD", self.guard(tool)?);
        let mut dirs = vec![self.0.as_path()];
        dirs.extend(writable);
        let guard = crate::sandbox::confine(&mut cmd, &dirs)?;
        Ok((cmd, guard))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// It prints its banner for any input; a missing file is reported, not extracted.
fn runs(tool: &Path) -> bool {
    let Ok(scratch) = Scratch::new() else { return false };
    let Ok((mut cmd, _guard)) = scratch.command(tool, None) else { return false };
    cmd.arg(root().join("no-such.pkg")).arg("--check-type").output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("THE END"))
}

/// Base game, update or DLC (the tool's exit code: 101, 102, 103).
pub fn kind(tool: &Path, pkg: &Path) -> Result<Kind, String> {
    let scratch = Scratch::new()?;
    let (mut cmd, _guard) = scratch.command(tool, None)?;
    let out = cmd.arg(pkg).arg("--check-type").output().map_err(|e| format!("could not run the PKG extractor: {e}"))?;
    match out.status.code() {
        Some(101) => Ok(Kind::Game),
        Some(102) => Ok(Kind::Update),
        Some(103) => Ok(Kind::Dlc),
        _ => Err(problem(&String::from_utf8_lossy(&out.stdout)).unwrap_or_else(|| "not a PS4 PKG package".into())),
    }
}

/// Problems the tool reports (it exits 0 even then).
fn problem(output: &str) -> Option<String> {
    output.split(['\r', '\n']).map(str::trim)
        .find(|l| l.starts_with("Cannot") || l.starts_with("Could not") || l.contains("doesn't appear to be a valid PKG"))
        .map(|l| l.replace("doesn't appear to be a valid PKG file", "is not a PS4 PKG package"))
}

/// Extract `pkg` into the existing folder `dest` (the game lands in `dest/<TITLE_ID>`, an update
/// in `dest/<TITLE_ID>-patch`, DLC in `dest/<TITLE_ID>/<label>`). The extractor can write only
/// inside `dest`. Succeeds only if it ran to the end: a clean exit, its closing line, and every
/// file of the package written. `progress(files done, files)`.
pub fn extract(tool: &Path, pkg: &Path, dest: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let scratch = Scratch::new()?;
    let (mut cmd, guard) = scratch.command(tool, Some(dest))?;
    let mut child = cmd.arg(pkg).arg(dest).stdout(Stdio::piped()).spawn().map_err(|e| format!("could not run the PKG extractor: {e}"))?;
    drop(guard);
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut all = String::new();
        let mut line = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                if b == b'\r' || b == b'\n' {
                    let text = String::from_utf8_lossy(&line).into_owned();
                    let _ = tx.send(text.clone());
                    all.push_str(&text);
                    all.push('\n');
                    line.clear();
                } else {
                    line.push(b);
                }
            }
        }
        all.push_str(&String::from_utf8_lossy(&line));
        all
    });
    let files = regex::Regex::new(r"Extracting file (\d+) of (\d+)").unwrap();
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled".into());
        }
        while let Ok(line) = rx.try_recv() {
            if let Some(c) = files.captures(&line) {
                progress(c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(1));
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let output = reader.join().unwrap_or_default();
    finished(status, &output)
}

/// Whether an extraction ran to the end (see `extract`).
fn finished(status: std::process::ExitStatus, output: &str) -> Result<(), String> {
    if let Some(p) = problem(output) {
        return Err(p);
    }
    if !output.lines().any(|l| l.trim() == GUARD_ACTIVE) {
        return Err("the PKG extractor ran without its write guard, so its files can't be trusted; nothing was installed".into());
    }
    if !status.success() {
        return Err(format!("the PKG extractor stopped before finishing ({status})"));
    }
    let last = regex::Regex::new(r"Extracting file (\d+) of (\d+)").unwrap().captures_iter(output).last()
        .map(|c| (c[1].parse::<u64>().unwrap_or(0), c[2].parse::<u64>().unwrap_or(0)));
    match last {
        Some((done, files)) if files > 0 && done == files && output.contains("THE END") => Ok(()),
        Some((done, files)) => Err(format!("the PKG extractor stopped after {done} of {files} files")),
        None => Err("the PKG extractor wrote no files".into()),
    }
}

/// Where shadPS4 looks for DLC by default (its user folder's `addcont`).
pub fn addons_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = TEST_ADDONS.with(|t| t.borrow().clone()) {
        return dir;
    }
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| crate::util::expand_home("~/.local/share"))
        .join("shadPS4").join("addcont")
}

#[cfg(test)]
thread_local! {
    /// A stand-in extractor and add-on folder for the installer's tests (this thread only).
    pub static TEST_TOOL: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub static TEST_ADDONS: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    /// Write an unloadable guard, as when it sits on a noexec mount.
    pub static TEST_BROKEN_GUARD: std::cell::RefCell<bool> = const { std::cell::RefCell::new(false) };
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    /// A stand-in extractor with the real one's habits: it writes with stdio, ignores every
    /// result, and always prints its success lines. Named like the real binary so the guard
    /// applies. Each `.pkg` holds a mode (`ok`, `short`: a 512-byte file-size limit while
    /// writing 4,096 bytes, `shrink`: the file is cut behind its back) and a param.sfo path.
    pub fn careless_extractor(dir: &Path) -> PathBuf {
        let source = dir.join("careless.c");
        std::fs::write(&source, r#"
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc < 3) return 0;
    if (!strcmp(argv[2], "--check-type")) return 101;
    char mode[16] = {0}, sfo[4096] = {0}, dir[4096], path[4096], buf[4096];
    FILE *in = fopen(argv[1], "r");
    if (in) { if (fscanf(in, "%15s %4095s", mode, sfo) != 2) return 3; fclose(in); }
    snprintf(dir, sizeof dir, "%s/CUSA12345", argv[2]); mkdir(dir, 0755);
    snprintf(path, sizeof path, "%s/sce_sys", dir); mkdir(path, 0755);
    snprintf(path, sizeof path, "%s/sce_sys/param.sfo", dir);
    FILE *s = fopen(sfo, "rb"), *o = fopen(path, "wb");
    size_t n = fread(buf, 1, sizeof buf, s); fwrite(buf, 1, n, o); fclose(o); fclose(s);
    signal(SIGXFSZ, SIG_IGN);
    if (!strcmp(mode, "short")) { struct rlimit r = {512, 512}; setrlimit(RLIMIT_FSIZE, &r); }
    snprintf(path, sizeof path, "%s/eboot.bin", dir);
    FILE *f = fopen(path, "wb");
    memset(buf, 42, sizeof buf);
    fwrite(buf, 1, sizeof buf, f);
    if (!strcmp(mode, "shrink")) { fflush(f); if (truncate(path, 512)) return 4; }
    fclose(f);
    printf("Extracting file 1 of 1\nTHE END %s\n", argv[1]);
    return 0;
}
"#).unwrap();
        let tool = dir.join("pkg_extractor");
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        assert!(Command::new(cc).arg("-o").arg(&tool).arg(&source).status().unwrap().success());
        tool
    }

    pub fn careless_pkg(dir: &Path, name: &str, mode: &str) -> PathBuf {
        use crate::sfo::{build, Value};
        let sfo = dir.join("game.sfo");
        std::fs::write(&sfo, build(&[("CATEGORY", Value::Text("gd".into())), ("TITLE_ID", Value::Text("CUSA12345".into()))])).unwrap();
        let pkg = dir.join(name);
        std::fs::write(&pkg, format!("{mode} {}", sfo.display())).unwrap();
        pkg
    }

    #[test]
    fn a_failed_or_short_write_stops_the_extraction() {
        if !crate::sandbox::available() { return; }
        let t = tempfile::tempdir().unwrap();
        let tool = careless_extractor(t.path());
        for mode in ["short", "shrink"] {
            // Unguarded, it reports success over a truncated file: the reviewed failure.
            let free = t.path().join(format!("free-{mode}"));
            std::fs::create_dir(&free).unwrap();
            let out = Command::new(&tool).arg(careless_pkg(t.path(), "a.pkg", mode)).arg(&free).output().unwrap();
            assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("THE END"));
            assert_eq!(std::fs::metadata(free.join("CUSA12345/eboot.bin")).unwrap().len(), 512);
            // Through extract(), the guard stops it.
            let dest = t.path().join(format!("guarded-{mode}"));
            std::fs::create_dir(&dest).unwrap();
            let err = extract(&tool, &careless_pkg(t.path(), "a.pkg", mode), &dest, &AtomicBool::new(false), &|_, _| {}).unwrap_err();
            assert!(err.starts_with("Cannot write the extracted files"), "{mode}: {err}");
        }
        // A guard the loader skips (noexec mount, damaged file): the run is rejected.
        TEST_BROKEN_GUARD.with(|b| *b.borrow_mut() = true);
        let dest = t.path().join("unloaded");
        std::fs::create_dir(&dest).unwrap();
        let err = extract(&tool, &careless_pkg(t.path(), "a.pkg", "short"), &dest, &AtomicBool::new(false), &|_, _| {}).unwrap_err();
        assert!(err.contains("without its write guard"), "{err}");
        TEST_BROKEN_GUARD.with(|b| *b.borrow_mut() = false);
        let dest = t.path().join("guarded-ok");
        std::fs::create_dir(&dest).unwrap();
        extract(&tool, &careless_pkg(t.path(), "a.pkg", "ok"), &dest, &AtomicBool::new(false), &|_, _| {}).unwrap();
        assert_eq!(std::fs::metadata(dest.join("CUSA12345/eboot.bin")).unwrap().len(), 4096);
    }

    #[test]
    fn only_a_complete_clean_run_counts_as_extracted() {
        let ok = std::process::ExitStatus::from_raw(0);
        let full = "PKG write guard active\nExtracting pkg to x\n\r      Extracting file 1 of 2\r      Extracting file 2 of 2\nTHE END x\n";
        assert!(finished(ok, full).is_ok());
        // Crashed or killed (exit 42, SIGSEGV), stopped early, never started, or reported a problem.
        assert!(finished(std::process::ExitStatus::from_raw(42 << 8), full).unwrap_err().contains("stopped before finishing"));
        assert!(finished(std::process::ExitStatus::from_raw(libc::SIGSEGV), full).is_err());
        assert!(finished(ok, "PKG write guard active\n\r      Extracting file 1 of 2\nTHE END x").unwrap_err().contains("1 of 2"));
        // Everything else fine, but no sign the guard loaded.
        assert!(finished(ok, &full.replace("PKG write guard active\n", "")).unwrap_err().contains("without its write guard"));
        assert!(finished(ok, "Extracting file 2 of 2").is_err(), "no closing line");
        assert!(finished(ok, "THE END x").is_err());
        assert!(finished(ok, "Cannot extract PKG file : bad\nTHE END").unwrap_err().contains("Cannot extract"));
    }

    /// Downloads the real extractor (network): cargo test -- --ignored pkg_extractor_downloads
    #[test]
    #[ignore]
    fn pkg_extractor_downloads_verifies_and_classifies() {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_DATA_HOME", home.path());
        let tool = install().unwrap();
        assert!(tool.starts_with(home.path()) && tool.is_file());
        assert!(kind(&tool, &home.path().join("missing.pkg")).is_err());
        if let Ok(pkg) = std::env::var("PKG_FIXTURE") {
            assert_eq!(kind(&tool, Path::new(&pkg)).unwrap(), Kind::Game);
        }
    }
}
