//! Game sessions: launch, detect, stop and switch between games and the launcher (Steam-style).
//! Any kyty_emulator process is detected, including ones started outside the launcher.

use crate::config::Config;
use crate::library::LocalGame;
use crate::util::{atomic_write, cache_dir, config_dir, now_secs};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const EMULATOR_NAMES: [&str; 3] = ["kyty_emulator", "kyty_emulator.exe", "shadps4"];

// ------------------------------------------------------------------ X11 helpers (xdotool)

/// Empty without an X display (as on macOS, where only Gamescope's helpers reach this).
fn xdotool(args: &[&str]) -> String {
    if std::env::var_os("DISPLAY").is_none() {
        return String::new();
    }
    Command::new("xdotool")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Visible windows of a session: of its process and of every process in its group (a PC game's
/// start script hands over to the game, often through Wine; games the launcher starts lead their
/// own process group).
#[cfg(target_os = "linux")]
pub fn windows_of_pid(pid: u32) -> Vec<String> {
    let mut pids = group_members(pid);
    if !pids.contains(&pid) {
        pids.insert(0, pid);
    }
    pids.iter().flat_map(|p| xdotool(&["search", "--onlyvisible", "--pid", &p.to_string()]).split_whitespace().map(String::from).collect::<Vec<_>>()).collect()
}

/// Running processes whose process group is `pgid`.
#[cfg(target_os = "macos")]
pub fn group_members(pgid: u32) -> Vec<u32> {
    // No /proc on macOS: pgrep lists a process group's members.
    let out = Command::new("pgrep").args(["-g", &pgid.to_string()]).stderr(Stdio::null()).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().filter_map(|p| p.parse().ok()).collect()).unwrap_or_default()
}

/// Running processes whose process group is `pgid` (not zombies).
#[cfg(target_os = "linux")]
pub fn group_members(pgid: u32) -> Vec<u32> {
    let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
    rd.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).filter(|pid| {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|stat| {
            let rest = stat.rsplit_once(')')?.1.split_whitespace().collect::<Vec<_>>();
            // state, ppid, pgrp
            Some(rest.first() != Some(&"Z") && rest.get(2)?.parse::<u32>().ok()? == pgid)
        }).unwrap_or(false)
    }).collect()
}

#[cfg(target_os = "linux")]
pub fn activate_pid_window(pid: u32) -> bool {
    if gamescope() {
        return gamescope_show(pid);
    }
    match windows_of_pid(pid).last() {
        Some(w) => {
            xdotool(&["windowactivate", w]);
            true
        }
        None => false,
    }
}

pub fn show_launcher() {
    activate_pid_window(std::process::id());
}

#[cfg(target_os = "linux")]
fn active_window_pid() -> u32 {
    if gamescope() {
        let focused = xprop(&["-root", "GAMESCOPE_FOCUSED_WINDOW"]);
        let Some(window) = focused.rsplit('=').next().map(str::trim).filter(|w| w.parse::<u64>().is_ok()) else { return 0 };
        return xdotool(&["getwindowpid", window]).parse().unwrap_or(0);
    }
    xdotool(&["getactivewindow", "getwindowpid"]).parse().unwrap_or(0)
}

// ------------------------------------------------------------------ Gamescope (PS5 Launcher OS)
//
// Gamescope has no window manager to activate windows. In its Steam mode (`gamescope -e`, as on
// SteamOS) each window carries a STEAM_GAME id, and the root window's
// GAMESCOPECTRL_BASELAYER_APPID lists the ids to show, first match wins. A window without an id
// is never shown, so the launcher tags its own window as soon as it exists (claim_screen).

/// The launcher's own id; games use their process id.
const LAUNCHER_APPID: u32 = 7;

/// Running inside Gamescope (it sets this for the programs it starts).
fn gamescope() -> bool {
    std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
}

fn xprop(args: &[&str]) -> String {
    Command::new("xprop").args(args).stderr(Stdio::null()).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn appid(pid: u32) -> u32 {
    if pid == std::process::id() { LAUNCHER_APPID } else { pid }
}

/// Give a process's windows its id. Returns false when it has no window yet.
fn gamescope_tag(pid: u32) -> bool {
    // A game's windows can belong to any process of its group: tag them all with the session's id.
    let windows = if pid == std::process::id() { xdotool(&["search", "--onlyvisible", "--pid", &pid.to_string()]).split_whitespace().map(String::from).collect() } else { windows_of_pid(pid) };
    for w in &windows {
        xprop(&["-id", w, "-f", "STEAM_GAME", "32c", "-set", "STEAM_GAME", &appid(pid).to_string()]);
    }
    !windows.is_empty()
}

/// Show this process's window; the launcher stays behind it as the fallback.
fn gamescope_show(pid: u32) -> bool {
    if !gamescope_tag(pid) {
        return false;
    }
    let order = if appid(pid) == LAUNCHER_APPID { LAUNCHER_APPID.to_string() } else { format!("{},{LAUNCHER_APPID}", appid(pid)) };
    xprop(&["-root", "-f", "GAMESCOPECTRL_BASELAYER_APPID", "32c", "-set", "GAMESCOPECTRL_BASELAYER_APPID", &order]);
    true
}

/// Under Gamescope, make the launcher's window showable (and shown if nothing else is).
/// Call once its window exists; harmless anywhere else.
pub fn claim_screen() {
    if gamescope() && gamescope_tag(std::process::id())
        && !xprop(&["-root", "GAMESCOPECTRL_BASELAYER_APPID"]).contains('=') {
        gamescope_show(std::process::id());
    }
}

// ------------------------------------------------------------------ PC games

/// How a PC game starts, from its manifest: its script, AppImage or program, run in its folder;
/// a Windows game through umu-launcher (Proton) or Wine, in its own prefix.
fn pc_command(game: &LocalGame) -> Result<Command, String> {
    use crate::pcgame::Kind;
    let m = game.pc.as_ref().ok_or("This game's manifest is missing; install it again")?;
    let target = game.path.join(&m.launch);
    if !target.is_file() {
        return Err(format!("{} is missing from the game's folder", m.launch));
    }
    let dir = target.parent().unwrap_or(&game.path).to_path_buf();
    let executable = || {
        use std::os::unix::fs::PermissionsExt;
        if target.metadata().is_ok_and(|md| md.permissions().mode() & 0o100 == 0) {
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
        }
    };
    let mut command = match m.kind {
        Kind::Script => {
            // A script without "#!" (some repacks' "start") runs with sh.
            if crate::pcgame::probe_file(&target) == crate::pcgame::Probe::Script {
                executable();
                Command::new(&target)
            } else {
                let mut c = Command::new("sh");
                c.arg(&target);
                c
            }
        }
        Kind::AppImage => {
            executable();
            let mut c = Command::new(&target);
            // Without FUSE (common on minimal systems), AppImages can still unpack and run.
            if !Path::new("/dev/fuse").exists() {
                c.env("APPIMAGE_EXTRACT_AND_RUN", "1");
            }
            c
        }
        Kind::Native => {
            executable();
            Command::new(&target)
        }
        Kind::Windows => {
            let (runner, label) = crate::pcgame::windows_runner()
                .ok_or("This is a Windows game: install umu-launcher (runs it with Proton) or Wine to play it")?;
            let prefix = crate::util::data_dir().join("prefixes").join(&game.id);
            let _ = std::fs::create_dir_all(&prefix);
            let mut c = Command::new(runner);
            c.arg(&target).env("WINEPREFIX", &prefix);
            if label == "Proton" {
                // umu downloads and updates Proton (GE-Proton) by itself.
                c.env("GAMEID", "umu-default").env("PROTONPATH", "GE-Proton");
            }
            c
        }
    };
    command.current_dir(dir);
    Ok(command)
}

// ------------------------------------------------------------------ macOS window helpers (AppleScript)
// These drive System Events, so macOS asks once for Accessibility permission. Without it every
// helper answers "no window", and the launcher falls back to timeouts and signals.

/// Output of the script, or None when it failed (e.g. Accessibility permission not granted).
#[cfg(target_os = "macos")]
fn osascript(script: &str) -> Option<String> {
    let out = Command::new("osascript").args(["-e", script]).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// AppleScript for "the process with this unix id", as System Events sees it.
#[cfg(target_os = "macos")]
fn process_with_pid(pid: u32) -> String {
    format!("(first process whose unix id is {pid})")
}

/// One placeholder per window of the process (callers only count them).
#[cfg(target_os = "macos")]
pub fn windows_of_pid(pid: u32) -> Vec<String> {
    let n: usize = osascript(&format!(r#"tell application "System Events" to count windows of {}"#, process_with_pid(pid))).and_then(|v| v.parse().ok()).unwrap_or(0);
    (0..n).map(|i| i.to_string()).collect()
}

#[cfg(target_os = "macos")]
pub fn activate_pid_window(pid: u32) -> bool {
    osascript(&format!(r#"tell application "System Events" to set frontmost of {} to true"#, process_with_pid(pid))).is_some()
}

#[cfg(target_os = "macos")]
fn active_window_pid() -> u32 {
    osascript(r#"tell application "System Events" to get unix id of first process whose frontmost is true"#).and_then(|v| v.parse().ok()).unwrap_or(0)
}

// ------------------------------------------------------------------ /proc helpers

#[cfg(target_os = "linux")]
fn boot_time() -> f64 {
    std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("btime")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0.0)
}

#[cfg(target_os = "linux")]
fn proc_start(pid: u32) -> f64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(19)?.parse::<f64>().ok()))
        .map(|t| boot_time() + t / ticks)
        .unwrap_or_else(now_secs)
}

#[cfg(target_os = "linux")]
fn is_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(')').map(|(_, r)| r.trim_start().chars().next() != Some('Z')))
        .unwrap_or(false)
}

/// The folder a game lives in: a path to a file inside it (eboot.bin) means its folder, but a
/// `.zar` archive is the game itself.
fn game_folder(game: String) -> String {
    let gp = Path::new(&game);
    if gp.is_file() && !game.ends_with(".zar") {
        return gp.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or(game);
    }
    game
}

/// Elapsed time as `ps -o etime=` prints it: "05:03", "1:02:03" or "2-01:00:00" (days-h:m:s).
#[cfg(any(target_os = "macos", test))]
fn parse_etime(s: &str) -> Option<f64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, s),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs = 0.0;
    for part in parts {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86400.0 + secs)
}

/// One line of `ps -axww -o pid=,etime=,command=` as (pid, seconds running, command line).
#[cfg(any(target_os = "macos", test))]
fn parse_ps_line(line: &str) -> Option<(u32, f64, &str)> {
    let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
    let (etime, command) = rest.trim_start().split_once(char::is_whitespace)?;
    Some((pid.parse().ok()?, parse_etime(etime)?, command.trim_start()))
}

/// The game path from an emulator's command line; None unless it is the emulator running a game.
/// `ps` joins the arguments with spaces, so a path containing spaces ends at the next " --" option.
#[cfg(any(target_os = "macos", test))]
fn emulator_game(command: &str, extra_name: &str) -> Option<String> {
    let names = EMULATOR_NAMES.iter().copied().chain((!extra_name.is_empty()).then_some(extra_name));
    let is_emulator = names.into_iter().any(|name| {
        command.match_indices(name).any(|(i, _)| {
            // The program's own name: after a "/" (or at the start), and ending the program word.
            let after = &command[i + name.len()..];
            (i == 0 || command[..i].ends_with('/')) && (after.is_empty() || after.starts_with(' '))
        })
    });
    if !is_emulator {
        return None;
    }
    let after = command.split_once(" --game ")?.1;
    let game = after.split(" --").next().unwrap_or(after).trim();
    (!game.is_empty()).then(|| game.to_string())
}

/// pid -> (game path, start time) for every running emulator.
#[cfg(target_os = "linux")]
fn scan_emulators(extra_name: &str) -> HashMap<u32, (String, f64)> {
    let mut found = HashMap::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return found };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let args: Vec<String> = raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let Some(argv0) = args.first() else { continue };
        let base = Path::new(argv0).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !(EMULATOR_NAMES.contains(&base.as_str()) || (!extra_name.is_empty() && base == extra_name)) {
            continue;
        }
        // Only an emulator that is running a game counts (not e.g. `kyty_emulator --help`).
        // KytyPS5 takes `--game <dir>`, shadPS4 `-g <dir>`.
        let Some(mut game) = args.iter().position(|a| a == "--game" || a == "-g").and_then(|i| args.get(i + 1)).cloned() else { continue };
        if !game.is_empty() && !game.starts_with('/') {
            if let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
                game = cwd.join(&game).to_string_lossy().into_owned();
            }
        }
        found.insert(pid, (game_folder(game), proc_start(pid)));
    }
    found
}

// macOS has no /proc: emulators are found by reading `ps` instead.

/// Alive means running: a zombie (exited, not yet reaped) is gone for our purposes, as on Linux.
#[cfg(target_os = "macos")]
fn is_alive(pid: u32) -> bool {
    if !crate::hostos::pid_exists(pid) {
        return false;
    }
    let state = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).stderr(Stdio::null()).output().ok();
    state.is_none_or(|o| !String::from_utf8_lossy(&o.stdout).trim_start().starts_with('Z'))
}

#[cfg(target_os = "macos")]
fn scan_emulators(extra_name: &str) -> HashMap<u32, (String, f64)> {
    let mut found = HashMap::new();
    let Ok(out) = Command::new("ps").args(["-axww", "-o", "pid=,etime=,command="]).stderr(Stdio::null()).output() else { return found };
    let now = now_secs();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((pid, elapsed, command)) = parse_ps_line(line) else { continue };
        let Some(game) = emulator_game(command, extra_name) else { continue };
        found.insert(pid, (game_folder(game), now - elapsed));
    }
    found
}

// ------------------------------------------------------------------ playtime

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug)]
#[serde(default)]
pub struct PlayStats {
    pub total: f64,
    pub count: u64,
    pub last: f64,
}

fn playtime_path() -> PathBuf {
    config_dir().join("playtime.json")
}

pub fn load_playtime() -> HashMap<String, PlayStats> {
    std::fs::read(playtime_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

// ------------------------------------------------------------------ sessions

#[derive(Clone, Debug)]
pub struct Session {
    pub pid: u32,
    pub game_id: String,
    pub title_id: String,
    pub name: String,
    pub path: String,
    pub since: f64,
    pub own: bool,
    pub log: PathBuf,
    pub stopping: bool,
    /// Runs until its whole process group has ended (PC games: the start script may exit first).
    pub group: bool,
}

#[derive(Clone, Debug)]
pub struct Ended {
    pub game_id: String,
    pub name: String,
    pub exit_code: Option<i32>,
    pub stopped: bool,
    pub played: f64,
    pub log: PathBuf,
}

struct Inner {
    live: Vec<Session>,
    children: HashMap<u32, Child>,
    ended: Vec<Ended>,
    playtime: HashMap<String, PlayStats>,
}

#[derive(Clone)]
pub struct Sessions {
    inner: Arc<Mutex<Inner>>,
    library: Arc<Mutex<Vec<LocalGame>>>,
    config: Arc<Mutex<Config>>,
}

impl Sessions {
    /// `notify` is called (from a background thread) whenever sessions start or end.
    pub fn start(library: Arc<Mutex<Vec<LocalGame>>>, config: Arc<Mutex<Config>>, notify: impl Fn() + Send + 'static) -> Sessions {
        let s = Sessions {
            inner: Arc::new(Mutex::new(Inner { live: Vec::new(), children: HashMap::new(), ended: Vec::new(), playtime: load_playtime() })),
            library,
            config,
        };
        let me = s.clone();
        std::thread::Builder::new()
            .name("sessions".into())
            .spawn(move || loop {
                if me.tick() {
                    notify();
                }
                std::thread::sleep(Duration::from_millis(1000));
            })
            .ok();
        s
    }

    pub fn live(&self) -> Vec<Session> {
        self.inner.lock().unwrap().live.clone()
    }

    pub fn playtime(&self, key: &str) -> PlayStats {
        self.inner.lock().unwrap().playtime.get(key).copied().unwrap_or_default()
    }

    pub fn take_ended(&self) -> Vec<Ended> {
        std::mem::take(&mut self.inner.lock().unwrap().ended)
    }

    pub fn launch(&self, game: &LocalGame) -> Result<(), String> {
        let cfg = self.config.lock().unwrap().clone();
        let mut inner = self.inner.lock().unwrap();
        if let Some(s) = inner.live.first() {
            return Err(format!("{} is already running. Stop it first.", s.name));
        }
        if game.platform == crate::platform::Platform::Pc {
            let command = pc_command(game)?;
            return self.spawn(&mut inner, game, command, true);
        }
        let ps4 = game.platform == crate::platform::Platform::Ps4;
        if ps4 && !crate::shad::emulator().is_file() {
            return Err("shadPS4 isn't installed yet".into());
        }
        if !ps4 && !cfg.emulator_ok() {
            return Err(format!("Emulator not found or not executable:\n{}", cfg.emulator_path().display()));
        }
        let emu = if ps4 { crate::shad::emulator() } else { cfg.emulator_path() };
        let mut args: Vec<String> = vec![
            "--game".into(),
            game.path.to_string_lossy().into_owned(),
            "--screen-width".into(),
            cfg.width.to_string(),
            "--screen-height".into(),
            cfg.height.to_string(),
            "--present-mode".into(),
            cfg.present_mode.clone(),
        ];
        if cfg.fullscreen {
            args.push("--fullscreen".into());
        }
        if cfg.amd_cpu {
            args.push("--amd-cpu".into());
        }
        args.extend(shell_split(&cfg.extra_args));
        if ps4 {
            // shadPS4: game folder and fullscreen; its other settings live in its own config.
            args = vec!["-g".into(), game.path.to_string_lossy().into_owned(), "-f".into(), cfg.fullscreen.to_string()];
        }
        let mut command = Command::new(&emu);
        command.args(&args).current_dir(emu.parent().unwrap_or(Path::new("/")));
        self.spawn(&mut inner, game, command, false)
    }

    /// Start a game's process in its own process group, logging to the game's log file.
    fn spawn(&self, inner: &mut Inner, game: &LocalGame, mut command: Command, group: bool) -> Result<(), String> {
        let log_dir = cache_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log = log_dir.join(format!("{}.log", if game.title_id.is_empty() { &game.id } else { &game.title_id }));
        let logf = std::fs::File::create(&log).map_err(|e| e.to_string())?;
        let logf2 = logf.try_clone().map_err(|e| e.to_string())?;
        crate::log!("launch: {command:?}");
        use std::os::unix::process::CommandExt;
        let child = command
            .stdin(Stdio::null())
            .stdout(logf)
            .stderr(logf2)
            .process_group(0)
            .spawn()
            .map_err(|e| e.to_string())?;
        let pid = child.id();
        inner.children.insert(pid, child);
        inner.live.push(Session {
            pid,
            game_id: game.id.clone(),
            title_id: game.title_id.clone(),
            name: game.name.clone(),
            path: game.path.to_string_lossy().into_owned(),
            since: now_secs(),
            own: true,
            log,
            stopping: false,
            group,
        });
        Ok(())
    }

    pub fn stop(&self, pid: u32) {
        let (own, group) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(s) = inner.live.iter_mut().find(|s| s.pid == pid) else { return };
            s.stopping = true;
            (s.own, s.group)
        };
        std::thread::spawn(move || {
            // 1) Close the window like Alt+F4 so the emulator shuts down cleanly (saves caches).
            if close_window_gracefully(pid, group) {
                return;
            }
            // 2) Terminate, then 3) kill.
            terminate(pid, own, group);
        });
    }

    /// Bring the running game's window to the front.
    pub fn resume(&self) -> bool {
        let pids: Vec<u32> = self.inner.lock().unwrap().live.iter().map(|s| s.pid).collect();
        pids.into_iter().any(activate_pid_window)
    }

    /// PS button: toggle between the game and the launcher.
    pub fn toggle_focus(&self) {
        let pids: Vec<u32> = self.inner.lock().unwrap().live.iter().map(|s| s.pid).collect();
        let active = active_window_pid();
        let in_game = active != 0 && pids.iter().any(|p| *p == active || group_members(*p).contains(&active));
        if pids.is_empty() || in_game {
            show_launcher();
        } else {
            self.resume();
        }
    }

    /// Returns true if anything changed.
    fn tick(&self) -> bool {
        if gamescope() {
            for pid in self.inner.lock().unwrap().live.iter().map(|s| s.pid).collect::<Vec<_>>() {
                gamescope_tag(pid);
            }
        }
        let extra = self.config.lock().unwrap().emulator_path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let procs = scan_emulators(&extra);
        let mut changed = false;
        let mut finished = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            // Newly detected external sessions.
            for (pid, (path, started)) in &procs {
                if inner.live.iter().any(|s| s.pid == *pid) {
                    continue;
                }
                let lib = self.library.lock().unwrap();
                let canon = std::fs::canonicalize(path).ok();
                let g = lib
                    .iter()
                    .find(|g| std::fs::canonicalize(&g.path).ok() == canon && canon.is_some())
                    .cloned()
                    .or_else(|| crate::library::read_param(Path::new(path)));
                drop(lib);
                let (game_id, title_id, name) = match g {
                    Some(g) => (g.id, g.title_id, g.name),
                    None => (String::new(), String::new(), Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown game".into())),
                };
                crate::log!("detected running game: {name} (pid {pid})");
                inner.live.push(Session { pid: *pid, game_id, title_id, name, path: path.clone(), since: *started, own: false, log: PathBuf::new(), stopping: false, group: false });
                changed = true;
            }
            // Finished sessions.
            let mut i = 0;
            while i < inner.live.len() {
                let pid = inner.live[i].pid;
                let group = inner.live[i].group;
                let (mut alive, code) = match inner.children.get_mut(&pid) {
                    Some(child) => match child.try_wait() {
                        Ok(Some(st)) => (false, st.code().or_else(|| {
                            use std::os::unix::process::ExitStatusExt;
                            st.signal().map(|s| -s)
                        })),
                        Ok(None) => (true, None),
                        Err(_) => (false, None),
                    },
                    None => (procs.contains_key(&pid), None),
                };
                if group && !alive && !group_members(pid).is_empty() {
                    alive = true;
                }
                if alive {
                    i += 1;
                    continue;
                }
                let s = inner.live.remove(i);
                inner.children.remove(&pid);
                finished.push((s, code));
                changed = true;
            }
            let now = now_secs();
            for (s, code) in &finished {
                let played = now - s.since;
                let key = if s.title_id.is_empty() { s.path.clone() } else { s.title_id.clone() };
                if played >= 3.0 && !key.is_empty() {
                    let e = inner.playtime.entry(key).or_default();
                    e.total = (e.total + played).floor();
                    e.count += 1;
                    e.last = now.floor();
                }
                crate::log!("game ended: {} exit {:?}", s.name, code);
                inner.ended.push(Ended { game_id: s.game_id.clone(), name: s.name.clone(), exit_code: *code, stopped: s.stopping, played, log: s.log.clone() });
            }
            if !finished.is_empty() {
                if let Ok(json) = serde_json::to_vec_pretty(&inner.playtime) {
                    let _ = atomic_write(&playtime_path(), &json);
                }
            }
        }
        if !finished.is_empty() && self.config.lock().unwrap().return_on_exit {
            show_launcher();
        }
        changed
    }
}

/// Ask the game's window to close (like Alt+F4) so the emulator saves its caches.
/// True once the game is gone.
#[cfg(target_os = "linux")]
fn close_window_gracefully(pid: u32, group: bool) -> bool {
    // Gamescope has no window manager to act on Alt+F4.
    let wins = if gamescope() { vec![] } else { windows_of_pid(pid) };
    let Some(w) = wins.last() else { return false };
    xdotool(&["windowactivate", w]);
    std::thread::sleep(Duration::from_millis(250));
    xdotool(&["key", "--clearmodifiers", "alt+F4"]);
    wait_gone(pid, group, 4.0)
}

/// macOS goes straight to SIGTERM: there is no reliable way to close another app's window.
#[cfg(target_os = "macos")]
fn close_window_gracefully(_pid: u32, _group: bool) -> bool {
    false
}

/// Terminate, then kill, a game's process (and its process group, if the launcher started it).
fn terminate(pid: u32, own: bool, group: bool) {
    for (sig, wait) in [(libc::SIGTERM, 4.0), (libc::SIGKILL, 2.0)] {
        unsafe {
            if own {
                libc::killpg(pid as i32, sig);
            } else {
                libc::kill(pid as i32, sig);
            }
        }
        if wait_gone(pid, group, wait) {
            return;
        }
    }
}

/// Wait for the process to end; for a `group` session (a PC game, whose start script may already
/// have exited), for every process in its group.
fn wait_gone(pid: u32, group: bool, secs: f64) -> bool {
    let end = now_secs() + secs;
    loop {
        if !is_alive(pid) && (!group || group_members(pid).is_empty()) {
            return true;
        }
        if now_secs() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Minimal shell-like splitting with quotes (for "extra emulator arguments").
pub fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => {
                quote = Some(c);
                has = true;
            }
            (_, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    has = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (_, c) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// The reason KytyPS5 gave for exiting, from its log: the text under its `--- Error ---` heading,
/// without the source-file location it appends. `None` when it printed no such section.
pub fn emulator_error(log: &str) -> Option<String> {
    let after = log.split("--- Error ---").nth(1)?;
    let text: Vec<&str> = after.lines().map(str::trim).skip_while(|l| l.is_empty()).take_while(|l| !l.is_empty()).collect();
    let mut joined = text.join(" ");
    // "... in /path/to/file.cpp:933" is for the emulator's developers, not for the player.
    if let Some(i) = joined.rfind(" in /") {
        if joined[i..].contains(".cpp:") || joined[i..].contains(".h:") {
            joined.truncate(i);
        }
    }
    let joined = joined.trim().to_string();
    (!joined.is_empty()).then(|| joined.chars().take(300).collect())
}

/// The last part of a log file, as text (logs can be large; the error is at the end).
pub fn log_tail(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(64 * 1024)));
    let mut bytes = Vec::new();
    let _ = f.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{emulator_error, emulator_game, game_folder, parse_etime, parse_ps_line};

    #[test]
    fn emulator_error_reads_the_reason_without_the_source_location() {
        let log = "Initialized: Audio\n--- Build ---\nOfficial build KytyPS5-2026-10-01-b3e419f\n--- Error ---\nCould not find suitable device:\n  Apple M4 Max: image view minLod is not supported; shaderCullDistance is not supported in /Users/runner/work/KytyPS5/src/vulkanWindow.cpp:933\n\n";
        assert_eq!(emulator_error(log).unwrap(), "Could not find suitable device: Apple M4 Max: image view minLod is not supported; shaderCullDistance is not supported");
        assert_eq!(emulator_error("all fine\nno error section"), None);
        assert_eq!(emulator_error("--- Error ---\n\n"), None);
        assert_eq!(emulator_error("--- Error ---\nout of memory\n").unwrap(), "out of memory");
    }

    #[test]
    fn stop_kills_what_a_start_script_left_behind() {
        use std::os::unix::process::CommandExt;
        // The start script exits at once; the "game" it started ignores SIGTERM.
        let mut script = std::process::Command::new("sh")
            .args(["-c", "sh -c 'trap \"\" TERM; exec sleep 60' & sleep 0.3; exit 0"])
            .process_group(0).spawn().unwrap();
        let pid = script.id();
        script.wait().unwrap();
        assert!(!super::group_members(pid).is_empty(), "the game outlives its start script");
        super::terminate(pid, true, true);
        assert!(super::group_members(pid).is_empty(), "the game was killed");
    }

    #[test]
    fn split() {
        assert_eq!(super::shell_split(r#"--a "b c" 'd' e\ f"#), vec!["--a", "b c", "d", "e f"]);
    }

    #[test]
    fn etime_formats_from_ps() {
        assert_eq!(parse_etime("05:03"), Some(303.0));
        assert_eq!(parse_etime("1:02:03"), Some(3723.0));
        assert_eq!(parse_etime("2-01:00:00"), Some(2.0 * 86400.0 + 3600.0));
        assert_eq!(parse_etime("00:07"), Some(7.0));
        assert_eq!(parse_etime("nonsense"), None);
        assert_eq!(parse_etime("1:2:3:4"), None);
        assert_eq!(parse_etime(""), None);
    }

    #[test]
    fn ps_lines_split_into_pid_elapsed_and_command() {
        assert_eq!(parse_ps_line("  4242       12:34 /opt/kyty/kyty_emulator --game /g/x"), Some((4242, 754.0, "/opt/kyty/kyty_emulator --game /g/x")));
        assert_eq!(parse_ps_line("1 1-00:00:01 /sbin/launchd"), Some((1, 86401.0, "/sbin/launchd")));
        assert_eq!(parse_ps_line("not a ps line"), None);
        assert_eq!(parse_ps_line(""), None);
    }

    #[test]
    fn emulator_command_lines() {
        let game = |c: &str| emulator_game(c, "");
        assert_eq!(game("/opt/kyty/kyty_emulator --game /games/CUSA1"), Some("/games/CUSA1".into()));
        // ps joins arguments with spaces: the path ends at the next option.
        assert_eq!(game("/opt/kyty/kyty_emulator --game /Users/me/My Games/CUSA 1 --fullscreen --amd-cpu"), Some("/Users/me/My Games/CUSA 1".into()));
        // The emulator itself may live in a folder with spaces.
        assert_eq!(game("/Users/me/Library/Application Support/ps5/kyty/current/kyty_emulator --game /g/x"), Some("/g/x".into()));
        assert_eq!(game("kyty_emulator --game /g/x"), Some("/g/x".into()));
        // Not running a game, or not the emulator.
        assert_eq!(game("/opt/kyty/kyty_emulator --help"), None);
        assert_eq!(game("/opt/kyty/kyty_emulator --game"), None);
        assert_eq!(game("/usr/bin/vim --game /g/x"), None);
        // The .exe name is a known emulator name too (as on Linux).
        assert_eq!(game("/opt/kyty/kyty_emulator.exe --game /g/x"), Some("/g/x".into()));
        assert_eq!(game("/opt/kyty/kyty_emulator.exe.bak --game /g/x"), None);
        assert_eq!(game("/opt/my_kyty_emulator --game /g/x"), None);
    }

    #[test]
    fn a_custom_emulator_name_counts_too() {
        assert_eq!(emulator_game("/opt/custom/my-kyty --game /g/x", "my-kyty"), Some("/g/x".into()));
        assert_eq!(emulator_game("/opt/custom/my-kyty --game /g/x", ""), None);
    }

    #[test]
    fn a_game_file_means_its_folder_except_for_zar_archives() {
        let t = tempfile::tempdir().unwrap();
        let folder = t.path().join("CUSA1");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("eboot.bin"), b"").unwrap();
        std::fs::write(t.path().join("game.zar"), b"").unwrap();
        let s = |p: std::path::PathBuf| p.to_string_lossy().into_owned();
        assert_eq!(game_folder(s(folder.join("eboot.bin"))), s(folder.clone()));
        assert_eq!(game_folder(s(folder.clone())), s(folder));
        assert_eq!(game_folder(s(t.path().join("game.zar"))), s(t.path().join("game.zar")));
        assert_eq!(game_folder("/does/not/exist".into()), "/does/not/exist");
    }
}
