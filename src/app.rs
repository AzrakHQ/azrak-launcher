//! Application state, navigation and event handling. All of it lives on the UI thread;
//! background threads hand results back through `slint::invoke_from_event_loop`.

use crate::audio::{self, Sound};
use crate::catalog::{self, CatalogFile, Game};
use crate::config::Config;
use crate::display::Monitor;
use crate::gamepad::Pad;
use crate::images;
use crate::library::{self, LocalGame};
use crate::psn::{self, Info};
use crate::sessions::{Session, Sessions};
use crate::util::{self, norm};
use crate::{AppWindow, ToastData};
use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Focus zones (mirrors `Zone` in ui/theme.slint).
pub const Z_TABS: i32 = 0;
pub const Z_TOP: i32 = 1;
pub const Z_ROW: i32 = 2;
pub const Z_ACTIONS: i32 = 3;
pub const Z_SEARCH: i32 = 4;
pub const Z_SORT: i32 = 5;
pub const Z_CHIPS: i32 = 6;
pub const Z_GRID: i32 = 7;
pub const Z_HUB: i32 = 8;
pub const Z_SHOTS: i32 = 9;
pub const Z_DESC: i32 = 10;
pub const Z_SETTINGS: i32 = 11;
pub const Z_MENU: i32 = 12;
pub const Z_DOWNLOADS: i32 = 13;
pub const Z_RELEASES: i32 = 14;
pub const Z_HUB_DETAILS: i32 = 15;
pub const Z_SORT_PICKER: i32 = 16;
pub const Z_DENSITY: i32 = 17;
pub const Z_TRAILER: i32 = 18;
pub const Z_CONTROLS: i32 = 19;
pub const Z_OS_INSTALL: i32 = 20;
pub const Z_WIFI_JOIN: i32 = 21;
pub const Z_POWER: i32 = 22;
pub const Z_SEARCH_KEYS: i32 = 23;
pub const Z_VERSIONS: i32 = 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    None = 0,
    Hub = 1,
    Settings = 2,
    Viewer = 3,
    Launch = 4,
    Menu = 5,
    Downloads = 7, // 6 is reserved by the startup splash.
    Sort = 8,
    Trailer = 9,
    Controls = 10,
    /// Install AzrakOS from the USB stick (os/install.rs).
    OsInstall = 11,
    /// The password for a Wi-Fi network (os/wifi_join.rs).
    WifiJoin = 12,
    /// "Restart now?" / "Shut down now?" (os/power.rs).
    Power = 13,
    /// The Library's search on a controller (search_keys.rs).
    SearchKeys = 14,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    Trailer,
    Search,
    Options,
    Settings,
    TabPrev,
    TabNext,
    PageUp,
    PageDown,
    First,
    Last,
}

pub const GENRES: [(&str, &str); 16] = [
    ("Action", r"(?i)action|hack|slash|beat.?.?em|brawler|souls"),
    ("Adventure", r"(?i)adventure|narrative|exploration|open.world|story"),
    ("RPG", r"(?i)rpg|role"),
    ("Shooter", r"(?i)shoot|fps|gun"),
    ("Horror", r"(?i)horror"),
    ("Survival", r"(?i)survival"),
    ("Racing", r"(?i)racing|driving|kart|motor"),
    ("Sports", r"(?i)sport|football|soccer|basketball|golf|tennis|wrestling|boxing|skate|baseball|hockey|cricket|ufc"),
    ("Fighting", r"(?i)fight|martial"),
    ("Platformer", r"(?i)platform|metroidvania"),
    ("Puzzle", r"(?i)puzzle"),
    ("Simulation", r"(?i)simulat|management|farming|sandbox|builder|life"),
    ("Strategy", r"(?i)strateg|tactic|tower|4x"),
    ("Stealth", r"(?i)stealth"),
    ("Family", r"(?i)party|family|casual|kids|music|rhythm"),
    ("VR", r"(?i)\bvr\b|virtual reality"),
];


pub const SORTS: [&str; 7] = ["Newest uploads", "Name (A–Z)", "Release date", "Top rated", "Size (largest)", "Size (smallest)", "Compatibility"];

/// A catalog game plus derived data.
pub struct GameV {
    pub g: Game,
    pub info: Option<Info>,
    pub name: String,
    pub buckets: Vec<&'static str>,
    pub search: crate::search::Doc,
    pub rel_day: i64,
    pub is_new: bool,
    pub local: Option<usize>,
}

pub struct LocalV {
    pub l: LocalGame,
    pub info: Option<Info>,
    pub cat: Option<usize>,
    pub name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowItem {
    Local(usize),
    #[allow(dead_code)] // catalog games no longer appear on the Games tab
    Cat(usize),
    All,
}

/// What the game hub / options menu is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub game: Option<usize>,
    pub local: Option<usize>,
}

/// See App::catalog_anchor.
pub struct CatalogAnchor {
    hub: Option<(Option<i64>, Option<usize>)>,
    menu: Option<(Option<i64>, Option<usize>)>,
    grid: Option<i64>,
    grid_index: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct ActionDef {
    pub id: &'static str,
    pub label: String,
    pub icon: &'static str,
    pub primary: bool,
    pub danger: bool,
    pub round: bool,
}

pub struct App {
    pub ui: slint::Weak<AppWindow>,
    pub cfg: Arc<Mutex<Config>>,
    pub games: Vec<GameV>,
    pub groups: crate::game_groups::Groups,
    /// Games the Library shows before a search (see `browsable`); the rest come up when searched for.
    pub library_total: usize,
    pub locals: Vec<LocalV>,
    pub library: Arc<Mutex<Vec<LocalGame>>>,
    pub art: psn::Shared,
    pub sessions: Sessions,
    pub downloads: crate::downloads::Manager,
    pub installer: crate::installer::Manager,
    pub install_pending: Option<String>,
    pub install_states: HashMap<String, crate::installer::State>,
    pub download_pending: Option<(i64, String, String)>,
    pub download_states: HashMap<String, crate::downloads::State>,
    pub download_model: Rc<VecModel<crate::DownloadData>>,
    pub live: Vec<Session>,
    pub images: images::Store,
    pub monitors: Vec<Monitor>,
    /// Design-canvas → window scale (see present.rs update_scale).
    pub scale: f32,

    pub view: i32,
    pub overlay: Overlay,
    pub zone: i32,
    pub idx: i32,
    pub stack: Vec<(Overlay, i32, i32)>,

    pub row: Vec<RowItem>,
    pub sel: usize,
    pub hero_actions: Vec<ActionDef>,
    pub bg_key: String,
    pub bg_show_b: bool,

    pub filtered: Vec<usize>,
    /// Nothing matched the search as typed: what it searched for instead.
    pub search_note: String,
    /// Every game's match for the search, from the last `apply_filter`.
    pub hits: crate::search::Hits,
    pub genre: String,
    pub status_filter: String,
    pub genre_list: Vec<(String, usize)>,
    pub sort: usize,
    pub query: String,
    pub search_editing: bool,
    pub cols: usize,
    pub card_w: f32,
    pub row_h: f32,
    pub grid_window: (usize, usize),
    pub grid_model: Rc<VecModel<crate::CardRow>>,
    pub tile_model: Rc<VecModel<crate::TileData>>,
    pub toast_model: Rc<VecModel<ToastData>>,

    pub hub: Option<Target>,
    pub hub_actions: Vec<ActionDef>,
    pub hub_shots: Vec<String>,
    pub viewer_i: usize,
    pub menu_target: Option<Target>,
    pub menu_actions: Vec<ActionDef>,

    pub settings_rows: Vec<crate::SettingData>,
    pub edit_index: i32,

    pub toasts: Vec<ToastData>,
    pub toast_seq: i32,
    /// A notification's action just opened something: clicks until then are the rest of a
    /// double-click, not choices on the screen that came up under the pointer.
    clicks_after: Option<std::time::Instant>,
    pub status: String,
    pub status_busy: bool,
    pub catalog_updated: f64,
    pub syncing: bool,
    pub enriching: bool,
    /// New artwork and names arrived while the Library was on screen: applied once it isn't, as
    /// they regroup releases and the cards and count would jump under the player.
    pub art_pending: bool,
    pub pad_hints: bool,
    pub clock: String,
    /// Updates the trailer player's progress and controls while it is open.
    pub trailer_timer: Option<slint::Timer>,
    pub trailer_controls_until: f64,
    pub genre_res: Vec<(&'static str, regex::Regex)>,

    // Keys of images currently on screen, so a finished load updates only what shows it.
    pub tile_keys: Vec<String>,
    /// Cover key → the cards (by index) showing it, for the rows currently in the grid model.
    pub grid_keys: std::collections::HashMap<String, Vec<usize>>,
    pub grid_scroll: f32,
    pub grid_trim: slint::Timer,
    pub hero_logo_key: String,
    pub hub_keys: std::collections::HashSet<String>,
    pub viewer_key: String,
    pub launch_keys: Vec<String>,
    pub launch_local: Option<usize>,
    pub bg_pending: Option<String>,
    pub bg_standin: Option<String>,
    pub bg_timer: slint::Timer,
    pub rest_timer: slint::Timer,
    pub hero_flip: bool,
    pub row_flip: bool,
    pub settings_ids: Vec<crate::settings::SId>,
    /// Settings' Advanced section is expanded (for this session).
    pub settings_advanced: bool,
    /// A VPN connect, disconnect or import in progress ("Connecting…"), shown in Settings.
    pub vpn_busy: String,
    /// The VPNs last shown in Settings, to refresh it when one connects or drops.
    pub vpn_seen: String,
    pub catalog_timer: slint::Timer,
    /// A download failed for lack of network; retried once it's back (see net_watch_start).
    pub net_down: bool,
    net_checking: bool,
    net_watch: slint::Timer,
    net_retry: NetRetry,
    /// Settings → SYSTEM on AzrakOS.
    pub sys: crate::os::system_ui::SysUi,
    pub osi: crate::os::install::OsInstall,
    /// On the AzrakOS USB stick the launcher's own start (first run?, catalog stale?) waits for
    /// "Try AzrakOS" (os/install.rs).
    pub held_start: Option<(bool, bool)>,
    pub wj: crate::os::wifi_join::WifiJoin,
    pub sk: crate::search_keys::SearchKeys,
    /// The laptop's battery as last read, and the lowest level already warned about.
    pub battery: Option<crate::battery::Battery>,
    pub battery_warned: u8,
    battery_read: f64,
    pub power: crate::os::power::PowerAsk,
    /// Notifications that arrived while a card (Join Wi-Fi, Restart now?) was open: they wait
    /// for it to close instead of covering it.
    held_toasts: Vec<ToastData>,
    /// This PC's processor, memory and graphics (read in the background at start).
    pub pc: Option<crate::pc::Specs>,
    pub pc_checking: bool,
    pub boot: crate::boot::Boot,
    pub kyty: crate::kyty_ui::KytyUi,
    pub shad: crate::shad_ui::ShadUi,
    pub warming: bool,
    pub upd: crate::update::AppUpdate,
    pub compat: crate::compat::Db,
    pub compat_checked: f64,
    /// Your own results from this PC, shared with KytyPS5 in batches.
    pub my_results: crate::results::Results,
    /// Saved logs of games that just crashed, by game ID, waiting to go with their rating.
    pub crash_logs: HashMap<String, std::path::PathBuf>,
    /// Library: show one console's games (None: every console).
    pub platform_filter: Option<crate::platform::Platform>,
    /// The Library catalog has games for more than one console (shows badges and a console chip).
    pub mixed_consoles: bool,
    /// The platforms the catalog has, when there are several: each gets a "… games only" chip,
    /// the last status chips, in this order.
    pub consoles: Vec<crate::platform::Platform>,
    /// The status filters lead `genre_list`: All, Installed, Playable (on Linux) and, with
    /// more than one console, "PS5 games only" and "PS4 games only". Genres follow.
    pub status_count: usize,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` with the app. If the app is busy (re-entrant callback), retry on the next loop turn.
pub fn with_app(f: impl FnOnce(&mut App) + 'static) {
    let f = RefCell::new(Some(f));
    let done = APP.with(|a| match a.try_borrow_mut() {
        Ok(mut guard) => {
            if let (Some(app), Some(f)) = (guard.as_mut(), f.borrow_mut().take()) {
                f(app);
            }
            true
        }
        Err(_) => false,
    });
    if !done {
        if let Some(f) = f.into_inner() {
            slint::Timer::single_shot(Duration::ZERO, move || with_app(f));
        }
    }
}

/// Same, for callbacks that must return a value synchronously.
fn with_app_ret<R: Default>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f).unwrap_or_default(),
        Err(_) => R::default(),
    })
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

// ====================================================================== quitting

/// Ctrl+Q or Ctrl+W: the usual desktop keys to close a window.
fn is_quit_key(text: &str, ctrl: bool) -> bool {
    ctrl && matches!(text, "q" | "Q" | "w" | "W")
}

/// Whether a key or button may close the launcher. On AzrakOS the launcher is the whole screen:
/// closing it leaves a black TV with no way back, so nothing in it may (Settings has Restart and
/// Shut down instead). A signal from the system still ends it (see `watch_exit_signals`).
fn may_quit(on_os: bool) -> bool {
    !on_os
}

/// Close the launcher, saying `why` in the log so a closed launcher is never a mystery.
/// Returns false (and logs that) on AzrakOS, where it stays open.
pub fn quit(why: &str) -> bool {
    if !may_quit(crate::os::system::on_os()) {
        crate::log!("not quitting ({why}): on AzrakOS the launcher is the whole screen");
        return false;
    }
    crate::log!("quitting: {why}");
    slint::quit_event_loop().ok();
    true
}

/// The write end of the pipe the exit-signal handler writes to, and who sent the last signal.
static EXIT_SIGNAL_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static EXIT_SIGNAL_FROM: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn on_exit_signal(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    // Only what's safe in a signal handler: remember the sender, write one byte.
    use std::sync::atomic::Ordering::Relaxed;
    unsafe {
        if !info.is_null() {
            EXIT_SIGNAL_FROM.store((*info).si_pid(), Relaxed);
        }
        let byte = sig as u8;
        libc::write(EXIT_SIGNAL_PIPE.load(Relaxed), &byte as *const u8 as *const libc::c_void, 1);
    }
}

/// A launcher ended by a signal (`pkill`, the session ending) used to vanish without a word in
/// the log. SIGTERM, SIGINT and SIGHUP now go to a handler that hands them to a thread of their
/// own, which logs the signal and who sent it, then closes the launcher the usual way (downloads
/// and installs stop cleanly). A second signal, or a launcher that hasn't closed 5 s later, ends
/// it at once. A handler rather than blocking the signals: a blocked signal stays blocked in every
/// program the launcher starts (games, emulators, the USB stick's installer), which then can't be
/// stopped (the installer hung at 96% in a VM), while a handler is reset in them.
pub fn watch_exit_signals() {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::sync::atomic::Ordering::Relaxed;
    unsafe {
        let mut fds = [0 as libc::c_int; 2];
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        EXIT_SIGNAL_PIPE.store(fds[1], Relaxed);
        let mut reader = std::fs::File::from_raw_fd(fds[0]);
        std::thread::spawn(move || {
            let mut asked = false;
            let mut byte = [0u8; 1];
            while reader.read_exact(&mut byte).is_ok() {
                let sig = byte[0] as libc::c_int;
                let pid = EXIT_SIGNAL_FROM.load(Relaxed);
                // Its command line, on one log line (`bash -c` scripts span several).
                let sender: String = std::fs::read(format!("/proc/{pid}/cmdline"))
                    .map(|c| String::from_utf8_lossy(&c).chars().map(|c| if c.is_control() { ' ' } else { c }).take(200).collect())
                    .unwrap_or_default();
                let sender = sender.trim();
                let name = match sig { libc::SIGTERM => "SIGTERM", libc::SIGINT => "SIGINT", _ => "SIGHUP" };
                crate::log!("quitting: {name} from process {pid} ({sender})");
                if asked {
                    std::process::exit(128 + sig);
                }
                asked = true;
                let _ = slint::invoke_from_event_loop(|| { slint::quit_event_loop().ok(); });
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(5));
                    crate::log!("the launcher didn't close in 5 s after {name}: ending it now");
                    std::process::exit(128 + sig);
                });
            }
        });
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_exit_signal as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
}

// ====================================================================== startup

pub fn run(ui: AppWindow, monitors: Vec<Monitor>, target_monitor: Option<Monitor>, windowed: bool) {
    let cfg = Arc::new(Mutex::new(Config::load()));
    audio::init();
    audio::set_enabled(cfg.lock().unwrap().sounds);

    let library = Arc::new(Mutex::new(library::scan(&cfg.lock().unwrap().game_dir_paths())));
    let art = psn::Store::load();
    let sessions = Sessions::start(library.clone(), cfg.clone(), || post(|app| app.on_sessions()));
    std::thread::spawn(images::trim_thumbs);
    std::thread::spawn(crate::update::refresh_menu_icon);
    let pool = images::Pool::new(4, 16, Arc::new(|key, buf| post(move |app| app.on_image(key, buf))));

    let genre_res = GENRES.iter().map(|(n, re)| (*n, regex::Regex::new(re).unwrap())).collect();
    let catalog = CatalogFile::load();
    let mut app = App {
        ui: ui.as_weak(),
        cfg: cfg.clone(),
        games: Vec::new(),
        groups: Default::default(),
        library_total: 0,
        locals: Vec::new(),
        library,
        art,
        sessions,
        downloads: { let c = cfg.lock().unwrap(); crate::downloads::Manager::load(c.seed_after_download, c.download_vpn()) },
        installer: crate::installer::Manager::load(),
        install_pending: None,
        install_states: HashMap::new(),
        download_pending: None,
        download_states: HashMap::new(),
        download_model: Rc::new(VecModel::default()),
        live: Vec::new(),
        images: images::Store::new(pool, 200),
        monitors,
        scale: 1.0,
        view: 0,
        overlay: Overlay::None,
        zone: Z_ROW,
        idx: 0,
        stack: Vec::new(),
        row: Vec::new(),
        sel: 0,
        hero_actions: Vec::new(),
        bg_key: String::new(),
        bg_show_b: false,
        filtered: Vec::new(),
        search_note: String::new(),
        hits: Default::default(),
        genre: "All".into(),
        status_filter: "All".into(),
        genre_list: Vec::new(),
        sort: 0,
        query: String::new(),
        search_editing: false,
        cols: 7,
        card_w: 214.0,
        row_h: 380.0,
        grid_window: (0, 0),
        grid_model: Rc::new(VecModel::default()),
        tile_model: Rc::new(VecModel::default()),
        toast_model: Rc::new(VecModel::default()),
        hub: None,
        hub_actions: Vec::new(),
        hub_shots: Vec::new(),
        viewer_i: 0,
        menu_target: None,
        menu_actions: Vec::new(),
        settings_rows: Vec::new(),
        edit_index: -1,
        toasts: Vec::new(),
        toast_seq: 0,
        clicks_after: None,
        status: String::new(),
        status_busy: false,
        catalog_updated: catalog.updated,
        syncing: false,
        enriching: false,
        art_pending: false,
        pad_hints: false,
        clock: String::new(),
        trailer_timer: None,
        trailer_controls_until: 0.0,
        genre_res,
        tile_keys: Vec::new(),
        grid_keys: Default::default(),
        grid_scroll: 0.0,
        grid_trim: slint::Timer::default(),
        hero_logo_key: String::new(),
        hub_keys: Default::default(),
        viewer_key: String::new(),
        launch_keys: Vec::new(),
        launch_local: None,
        bg_pending: None,
        bg_standin: None,
        bg_timer: slint::Timer::default(),
        rest_timer: slint::Timer::default(),
        hero_flip: false,
        row_flip: false,
        settings_ids: Vec::new(),
        settings_advanced: false,
        vpn_busy: String::new(),
        vpn_seen: String::new(),
        catalog_timer: slint::Timer::default(),
        net_down: false,
        net_checking: false,
        net_watch: slint::Timer::default(),
        net_retry: NetRetry::default(),
        sys: Default::default(),
        osi: Default::default(),
        held_start: None,
        wj: Default::default(),
        sk: Default::default(),
        battery: None,
        battery_warned: 100,
        battery_read: 0.0,
        power: Default::default(),
        held_toasts: Vec::new(),
        pc: None,
        pc_checking: false,
        boot: Default::default(),
        kyty: Default::default(),
        shad: Default::default(),
        warming: false,
        upd: Default::default(),
        compat: crate::compat::with_mine(crate::compat::load().0, &crate::results::load()),
        my_results: crate::results::load(),
        crash_logs: HashMap::new(),
        platform_filter: None,
        mixed_consoles: false,
        consoles: Vec::new(),
        status_count: 4,
        compat_checked: 0.0,
    };
    ui.set_grid_rows(ModelRc::from(app.grid_model.clone()));
    ui.set_tiles(ModelRc::from(app.tile_model.clone()));
    ui.set_toasts(ModelRc::from(app.toast_model.clone()));
    ui.set_downloads(ModelRc::from(app.download_model.clone()));
    app.set_catalog(catalog.games.clone());
    app.relayout();
    app.push_all();
    let stale = catalog.stale();
    let first_run = catalog.games.is_empty() || !Config::path().exists();
    if !Config::path().exists() {
        cfg.lock().unwrap().save(); // remember detected defaults
    }
    APP.with(|a| *a.borrow_mut() = Some(app));
    with_app(move |app| {
        // From the AzrakOS USB stick: first the choice to try AzrakOS or install it. The launcher
        // sets itself up (catalog, artwork, emulators) only when it's tried.
        if crate::os::system::on_usb() {
            app.held_start = Some((first_run, stale));
            app.os_install_welcome();
        } else {
            app.start_launcher(first_run, stale);
        }
    });

    wire_callbacks(&ui);
    crate::gamepad::spawn(|p| post(move |app| app.on_pad(p)));

    // Clock + session timers: one cheap tick per second (only changed text is pushed).
    let tick = slint::Timer::default();
    tick.start(slint::TimerMode::Repeated, Duration::from_secs(1), || with_app(|app| app.tick()));
    with_app(|app| app.tick());
    // Under Gamescope (AzrakOS) the launcher's window is only shown once it's tagged.
    for ms in [300, 1000, 3000] {
        slint::Timer::single_shot(Duration::from_millis(ms), || { std::thread::spawn(crate::sessions::claim_screen); });
    }

    // The window's close button (or Alt+F4) quits like Ctrl+Q, so never on AzrakOS.
    ui.window().on_close_requested(|| {
        if quit("window closed") { slint::CloseRequestResponse::HideWindow } else { slint::CloseRequestResponse::KeepWindowShown }
    });
    ui.show().expect("could not open window");
    // AzrakOS's session waits for this line to know something is on screen, whichever screen comes
    // first (its watchdog starts the launcher again otherwise; docs/AZRAKOS.md).
    crate::log!("window shown");
    crate::display::place_window(&ui, target_monitor.as_ref(), windowed);
    slint::run_event_loop().expect("event loop failed");
    drop(tick);
    with_app(|app| {
        app.installer.shutdown();
        app.downloads.shutdown();
        crate::trailer::close();
    });
}

impl App {
    /// Every 10 s: the battery next to the clock, and a notice when it runs low.
    fn battery_tick(&mut self) {
        let now = util::now_secs();
        if now - self.battery_read < 10.0 {
            return;
        }
        self.battery_read = now;
        let battery = crate::battery::read();
        // Warn only when two readings in a row (10 s apart) say it's on battery: a charger can
        // read as unplugged for a moment (seen on the test laptop), which isn't worth a notice.
        let steady = battery.is_some_and(|b| b.plugged) || self.battery.is_some_and(|b| !b.plugged);
        if battery != self.battery {
            let ui = self.ui();
            ui.set_battery_shown(battery.is_some());
            if let Some(b) = battery {
                ui.set_battery_percent(b.percent as i32);
                ui.set_battery_plugged(b.plugged);
            }
            self.battery = battery;
        }
        if let Some((title, text)) = battery.filter(|_| steady).and_then(|b| crate::battery::notice(b, &mut self.battery_warned)) {
            self.toast(title, &text, 2);
        }
    }

    /// The launcher's own start: the start screen (with its first-run setup), the catalog, the
    /// emulators, its own update check.
    pub fn start_launcher(&mut self, first_run: bool, stale: bool) {
        self.boot_start(first_run);
        self.kyty_start();
        self.shad_start();
        self.app_update_start();
        self.compat_start();
        self.net_watch_start();
        self.pc_check();
        if stale {
            self.start_sync(false);
        } else {
            self.start_enrich();
            self.check_published();
        }
        // New releases are published daily; a launcher left open for days still picks them up.
        // AZRAK_CATALOG_INTERVAL (seconds): for testing reloads while something is open.
        let every = std::env::var("AZRAK_CATALOG_INTERVAL").ok().and_then(|s| s.parse::<f64>().ok()).filter(|s| *s >= 1.0)
            .unwrap_or(3600.0);
        self.catalog_timer.start(slint::TimerMode::Repeated, Duration::from_secs_f64(every),
            || with_app(|app| app.check_published()));
    }
}

fn wire_callbacks(ui: &AppWindow) {
    ui.on_key(|text, ctrl, alt, repeat| with_app_ret(|app| app.on_key(&text, ctrl, alt, repeat)));
    ui.on_click(|zone, idx| with_app(move |app| app.on_click(zone, idx)));
    ui.on_hover(|zone, idx| with_app(move |app| app.on_hover(zone, idx)));
    ui.on_row_wheel(|d| {
        with_app(move |app| {
            if d.abs() >= 1.0 {
                app.select_tile(app.sel as i64 + if d > 0.0 { 1 } else { -1 });
            }
        })
    });
    ui.on_grid_scrolled(|| with_app(|app| app.push_grid_window()));
    ui.on_grid_wheel(|dy| with_app(move |app| app.grid_wheel(dy)));
    ui.on_resized(|| with_app(|app| {
        app.update_scale();
        app.relayout();
        app.push_all();
        app.set_grid_scroll(app.grid_scroll, 0);
        if app.view == 1 && app.zone == Z_GRID { app.ensure_grid_visible(); }
        if app.overlay == Overlay::Settings {
            app.scroll_settings();
        }
    }));
    ui.on_search_edited(|t| with_app(move |app| app.on_search(t.to_string())));
    ui.on_search_done(|_| with_app(|app| {
        app.stop_search_edit();
        if !app.filtered.is_empty() {
            app.set_focus(Z_GRID, 0);
            app.ensure_grid_visible();
        }
        audio::play(Sound::Select);
    }));
    ui.on_edit_done(|t, _| with_app(move |app| app.finish_edit(Some(t.to_string()))));
    ui.on_download_confirm(|| with_app(|app| app.confirm_download()));
    ui.on_download_action(|key, action| with_app(move |app| app.download_action(&key, &action)));
    ui.on_toast_clicked(|id, action| with_app(move |app| app.toast_clicked(id, &action)));
    ui.on_trailer_action(|action| with_app(move |app| app.trailer_action(&action)));
    ui.on_download_close(|| with_app(|app| app.back()));
    ui.on_hub_release(|delta| with_app(move |app| app.cycle_hub_release(delta)));
    ui.on_genres_scroll(|delta| with_app(move |app| app.scroll_genres(delta)));
}

// ====================================================================== data

impl App {
    pub fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window gone")
    }

    pub fn art_for_game(&self, g: &Game) -> Option<Info> {
        let store = self.art.lock().unwrap();
        let sony = std::iter::once(&g.title_id).chain(g.title_ids.iter())
            .filter_map(|id| store.get(id)).find(|info| !info.master.is_empty())
            .or_else(|| store.get(&g.title_id));
        pick_art(g.platform, g.steam.as_ref(), sony)
    }

    fn display_name(site: &str, info: Option<&Info>) -> String {
        static SUFFIX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\s+(PS4\s*(&|and)\s*)?PS5$").unwrap());
        let Some(n) = info.map(|i| i.name.as_str()).filter(|n| !n.is_empty()) else { return site.to_string() };
        if n.chars().any(|c| matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x9fff | 0xac00..=0xd7af)) {
            return site.to_string();
        }
        let cleaned: String = n.chars().filter(|c| !matches!(c, '™' | '®' | '©')).collect();
        let cleaned = SUFFIX.replace(cleaned.trim(), "").split_whitespace().collect::<Vec<_>>().join(" ");
        if cleaned.is_empty() { site.to_string() } else { cleaned }
    }

    /// Japan-region store entries come back in Japanese; drop those text fields so the
    /// English site data is used instead (artwork is kept).
    fn sanitize(info: Option<Info>) -> Option<Info> {
        let cjk = |s: &str| s.chars().any(|c| matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x9fff | 0xac00..=0xd7af));
        info.map(|mut i| {
            if i.genres.iter().any(|g| cjk(g)) {
                i.genres.clear();
            }
            if cjk(&i.short) {
                i.short.clear();
            }
            if cjk(&i.long) {
                i.long.clear();
            }
            if cjk(&i.publisher) {
                i.publisher.clear();
            }
            i
        })
    }

    pub fn set_catalog(&mut self, games: Vec<Game>) {
        // Reads the artwork as it is now: nothing left waiting.
        self.art_pending = false;
        let downloaded: HashSet<i64> = self.downloads.snapshot().iter().map(|job| job.topic).collect();
        let games: Vec<Game> = games.into_iter().filter(|g| listed(g, &downloaded)).collect();
        let today = (util::now_secs() / 86400.0) as i64;
        let mut out = Vec::with_capacity(games.len());
        for g in games {
            let info = Self::sanitize(self.art_for_game(&g));
            let name = Self::display_name(&g.name, info.as_ref());
            let genre_text = g.genres.iter().chain(info.iter().flat_map(|i| i.genres.iter())).cloned().collect::<Vec<_>>().join(" | ");
            let mut buckets: Vec<&'static str> = self.genre_res.iter().filter(|(_, re)| re.is_match(&genre_text)).map(|(n, _)| *n).collect();
            if buckets.is_empty() {
                buckets.push("Other");
            }
            let rel = util::parse_long_date(&g.release).or_else(|| info.as_ref().and_then(|i| util::parse_iso_date(&i.release)));
            let added = util::parse_iso_date(&g.date).map(|(y, m, d)| util::days_from_civil(y, m, d));
            let publisher = info.as_ref().map(|i| i.publisher.clone()).unwrap_or_default();
            out.push(GameV {
                search: crate::search::Doc::new(&name, &g.name,
                    &format!("{} {} {} {} {}", g.title_ids.join(" "), g.title_id, publisher, g.region, g.version),
                    &format!("{genre_text} {}", buckets.join(" "))),
                name,
                buckets,
                rel_day: rel.map(|(y, m, d)| util::days_from_civil(y, m, d)).unwrap_or(0),
                is_new: added.is_some_and(|added| (0..14).contains(&(today - added))),
                local: None,
                info,
                g,
            });
        }
        borrow_art(&mut out);
        self.games = out;
        // The Library's list pointed into the old catalog.
        self.filtered.clear();
        let years: Vec<Option<i32>> = (0..self.games.len()).map(|i| self.release_year(i)).collect();
        self.groups = crate::game_groups::Groups::with_years(self.games.iter().zip(years).map(|(game, year)| (&game.g, game.name.as_str(), year)));
        borrow_group_art(&mut self.games, &self.groups);
        self.refresh_locals();
    }

    /// Rebuild installed games and link them to catalog entries.
    pub fn refresh_locals(&mut self) {
        let hub_id = self.hub.and_then(|t| t.local).and_then(|i| self.locals.get(i)).map(|l| l.l.id.clone());
        let menu_id = self.menu_target.and_then(|t| t.local).and_then(|i| self.locals.get(i)).map(|l| l.l.id.clone());
        let libs = self.library.lock().unwrap().clone();
        let index = CatalogIndex::new(self.games.iter().map(|g| (&g.g, g.name.as_str())));
        let mut locals = Vec::new();
        let store = self.art.lock().unwrap();
        for l in libs {
            let cat = index.find(&l);
            let info = Self::sanitize(store.get(&l.title_id).cloned()).or_else(|| cat.and_then(|c| self.games[c].info.clone()));
            let name = Self::display_name(&l.name, info.as_ref());
            locals.push(LocalV { l, info, cat, name });
        }
        drop(store);
        for g in &mut self.games {
            g.local = None;
        }
        for (i, l) in locals.iter().enumerate() {
            if let Some(c) = l.cat {
                self.games[c].local = Some(i);
            }
            // An installed title can match multiple release topics, including aliases.
            if !l.l.title_id.is_empty() {
                for game in &mut self.games {
                    if game.g.title_id == l.l.title_id || game.g.title_ids.contains(&l.l.title_id) {
                        game.local = Some(i);
                    }
                }
            }
        }
        self.locals = locals;
        for (target, previous_id) in [(&mut self.hub, hub_id), (&mut self.menu_target, menu_id)] {
            if let Some(target) = target {
                target.local = target.game.and_then(|i| self.games.get(i)).and_then(|g| g.local)
                    .or_else(|| previous_id.as_ref().and_then(|id| self.locals.iter().position(|l| &l.l.id == id)));
            }
        }
        self.build_row();
        self.build_genres();
        self.apply_filter();
    }

    fn playtime_key(l: &LocalGame) -> String {
        if l.title_id.is_empty() { l.path.to_string_lossy().into_owned() } else { l.title_id.clone() }
    }

    pub fn playtime(&self, l: &LocalGame) -> crate::sessions::PlayStats {
        self.sessions.playtime(&Self::playtime_key(l))
    }

    pub fn build_row(&mut self) {
        let prev = self.row.get(self.sel).copied();
        let mut locals: Vec<usize> = (0..self.locals.len()).collect();
        locals.sort_by(|a, b| {
            let (pa, pb) = (self.playtime(&self.locals[*a].l).last, self.playtime(&self.locals[*b].l).last);
            pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| self.locals[*a].name.cmp(&self.locals[*b].name))
        });
        let mut seen = std::collections::HashSet::new();
        let row: Vec<RowItem> = locals.into_iter().filter(|i| {
            let local = &self.locals[*i];
            // A game's catalog card spans platforms; an installed PS4 and PC copy are two tiles.
            let key = if let Some(catalog) = local.cat {
                format!("group:{:?}:{}", self.groups.releases(catalog).first(), local.l.platform.label())
            } else if !local.l.title_id.is_empty() { format!("id:{}", local.l.title_id) }
            else {
                // The PC and the PS5 version of a game are two games.
                let name = norm(&local.name);
                if name.is_empty() { format!("path:{}", local.l.path.display()) } else { format!("name:{}:{name}", local.l.platform.label()) }
            };
            seen.insert(key)
        }).map(RowItem::Local).collect();
        // Games tab = your installed games only; the whole catalog is the Library tab.
        self.row = row;
        self.sel = prev.and_then(|p| self.row.iter().position(|r| *r == p)).unwrap_or(self.sel.min(self.row.len().saturating_sub(1)));
    }

    /// Whether each game's card shows in the Library before a search: it has a picture, or it's
    /// yours (installed or downloading). A card with only a name looks broken among covers, so
    /// those games come up only when searched for.
    pub fn browsable(&self) -> Vec<bool> {
        let downloading: std::collections::HashSet<i64> = self.downloads.snapshot().iter().map(|job| job.topic).collect();
        self.groups.members.iter().map(|members| members.iter().any(|i| {
            let game = &self.games[*i];
            game.local.is_some() || downloading.contains(&game.g.id) || self.card_req(*i).is_some()
        })).collect()
    }

    pub fn build_genres(&mut self) {
        let shown = self.browsable();
        let members: Vec<&Vec<usize>> = self.groups.members.iter().zip(&shown).filter(|(_, shown)| **shown).map(|(m, _)| m).collect();
        self.library_total = members.len();
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for members in &members {
            let buckets: std::collections::HashSet<_> = members.iter().flat_map(|i| self.games[*i].buckets.iter().copied()).collect();
            for b in buckets {
                *counts.entry(b).or_default() += 1;
            }
        }
        let mut list: Vec<(String, usize)> = vec![("All".into(), members.len())];
        let installed = members.iter().filter(|members| members.iter().any(|i| self.games[*i].local.is_some())).count();
        list.push(("Installed".into(), installed));
        let count = |f: &dyn Fn(&crate::compat::Entry) -> bool| members.iter()
            .filter(|members| members.iter().any(|i| self.game_compat(&self.games[*i]).is_some_and(f))).count();
        // Linux results only: the launcher runs on Linux.
        list.push(("Playable".into(), count(&|e| e.in_game())));
        use crate::platform::Platform;
        self.consoles = [Platform::Ps5, Platform::Ps4, Platform::Pc].into_iter()
            .filter(|p| self.games.iter().any(|g| g.g.platform == *p)).collect();
        self.mixed_consoles = self.consoles.len() > 1;
        if !self.mixed_consoles {
            self.consoles.clear();
            self.platform_filter = None;
        } else {
            for p in self.consoles.clone() {
                let n = members.iter().filter(|m| m.iter().any(|i| self.games[*i].g.platform == p)).count();
                list.push((format!("{} games only", p.label()), n));
            }
        }
        self.status_count = list.len();
        list.push(("All genres".into(), members.len()));
        let mut rest: Vec<(&str, usize)> = counts.into_iter().collect();
        rest.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        list.extend(rest.into_iter().map(|(n, c)| (n.to_string(), c)));
        if !list.iter().any(|(n, _)| *n == self.genre) {
            self.genre = "All".into();
        }
        self.genre_list = list;
    }

    /// Every game's match for the search (search.rs); before one, every game matches. The
    /// Library keeps the last in `hits`.
    pub fn search_hits(&self) -> crate::search::Hits {
        crate::search::run(&crate::search::Query::new(&self.query), self.games.iter().map(|g| &g.search))
    }

    pub fn searching(&self) -> bool {
        !crate::search::Query::new(&self.query).is_empty()
    }

    pub fn apply_filter(&mut self) {
        // The Library's highlighted game (on the grid, or saved under an open card) stays the
        // one highlighted when the list changes behind it: the PC's specs or the compatibility
        // list arriving re-pick cards' releases and re-sort them. By release ID, which outlives
        // a catalog reload.
        let card_release = |app: &App, card: i32| usize::try_from(card).ok().and_then(|c| app.filtered.get(c)).and_then(|&g| app.games.get(g)).map(|g| g.g.id);
        let focused = (self.zone == Z_GRID).then(|| card_release(self, self.idx)).flatten();
        let saved: Vec<Option<i64>> = self.stack.iter().map(|&(_, zone, idx)| (zone == Z_GRID).then(|| card_release(self, idx)).flatten()).collect();
        let query = crate::search::Query::new(&self.query);
        let hits = self.search_hits();
        let genre = self.genre.as_str();
        let matches = |i: usize| {
                let g = &self.games[i];
                (genre == "All" || g.buckets.contains(&genre))
                    && self.matches_status(i)
                    && hits.scores[i].is_some()
            };
        // Each card shows its best release among those the filters let through (versions.rs).
        let rank = |i: usize| self.release_rank(i);
        // Before a search, only games with a picture or yours; a search finds every game.
        let shown = if query.is_empty() { self.browsable() } else { vec![true; self.groups.members.len()] };
        // How well each game shown matches: its best release.
        let mut matched = HashMap::new();
        let mut list: Vec<usize> = self.groups.members.iter().zip(shown).filter(|(_, shown)| *shown).filter_map(|(members, _)| {
            let shown = members.iter().copied().filter(|i| matches(*i)).max_by_key(|i| rank(*i))?;
            matched.insert(shown, members.iter().filter(|i| matches(**i)).filter_map(|i| hits.scores[*i]).max().unwrap_or(0));
            Some(shown)
        }).collect();
        let gs = &self.games;
        let score = |i: usize| gs[i].info.as_ref().and_then(|x| x.rating.as_ref()).map(|r| (r.score, r.total)).unwrap_or((-1.0, 0));
        match self.sort {
            // By the name the card shows (a game on several platforms: its plainest name).
            1 => list.sort_by_cached_key(|i| self.game_title(*i).to_lowercase()),
            2 => list.sort_by(|a, b| gs[*b].rel_day.cmp(&gs[*a].rel_day)),
            3 => list.sort_by(|a, b| score(*b).partial_cmp(&score(*a)).unwrap_or(std::cmp::Ordering::Equal)),
            4 => list.sort_by(|a, b| gs[*b].g.size_gb.unwrap_or(-1.0).partial_cmp(&gs[*a].g.size_gb.unwrap_or(-1.0)).unwrap_or(std::cmp::Ordering::Equal)),
            5 => list.sort_by(|a, b| gs[*a].g.size_gb.unwrap_or(1e9).partial_cmp(&gs[*b].g.size_gb.unwrap_or(1e9)).unwrap_or(std::cmp::Ordering::Equal)),
            6 => {
                // Linux results best first, then results from other OSes, untested last; ties by name.
                let rank = |i: usize| self.game_compat(&gs[i]).map(|e| e.status as u8 + if e.on_linux { 0 } else { 4 }).unwrap_or(9);
                list.sort_by_cached_key(|i| (rank(*i), self.game_title(*i).to_lowercase()))
            }
            _ => {
                let newest = |i: usize| self.groups.releases(i).iter().map(|index| gs[*index].g.id).max().unwrap_or(gs[i].g.id);
                list.sort_by(|a, b| newest(*b).cmp(&newest(*a)));
            }
        }
        // A search shows the best matches first; the chosen sort orders equal matches.
        if !query.is_empty() {
            list.sort_by_key(|i| std::cmp::Reverse(matched.get(i).copied().unwrap_or(0)));
        }
        self.search_note = match list.first() {
            Some(&top) if hits.near => format!("Showing results for “{}”", query.corrected(&self.games[top].search)),
            _ => String::new(),
        };
        self.filtered = list;
        self.hits = hits;
        let card_of = |app: &App, release: i64| app.filtered.iter()
            .position(|&g| app.groups.releases(g).iter().any(|&m| app.games[m].g.id == release));
        for (k, release) in saved.into_iter().enumerate() {
            if let Some(at) = release.and_then(|r| card_of(self, r)) {
                self.stack[k].2 = at as i32;
            }
        }
        if let Some((release, at)) = focused.and_then(|r| card_of(self, r).map(|at| (r, at))) {
            self.set_focus(Z_GRID, at as i32);
            if self.view == 1 && !self.boot.active {
                self.ensure_grid_visible();
                // Its card shows another release now: its backdrop follows.
                if self.games[self.filtered[at]].g.id != release {
                    self.focus_card_changed();
                }
            }
        }
    }

    // ------------------------------------------------------------------ background jobs

    /// Downloads that failed because the network wasn't up yet (on AzrakOS the launcher
    /// starts with the PC, often before the network) are retried as soon as it is, instead of
    /// waiting hours for the next scheduled check.
    pub fn net_watch_start(&mut self) {
        self.net_watch.start(slint::TimerMode::Repeated, Duration::from_secs(20), || {
            with_app(|app| {
                if !app.net_retry.due(crate::util::now_secs(), app.net_down) || app.net_checking {
                    return;
                }
                app.net_checking = true;
                std::thread::spawn(|| {
                    let up = crate::util::network_up();
                    post(move |app| {
                        app.net_checking = false;
                        if !up {
                            app.net_retry.offline();
                        } else if app.net_down {
                            app.net_down = false;
                            app.net_retry.retrying(crate::util::now_secs());
                            app.network_back();
                        }
                    });
                });
            })
        });
    }

    fn network_back(&mut self) {
        crate::log!("network is up: retrying the downloads that failed");
        // Covers that failed offline load again when the catalog reload below redraws the Library.
        self.images.clear_failures();
        let (auto, missing) = { let c = self.cfg.lock().unwrap(); (c.kyty_auto_update, !c.emulator_ok()) };
        if auto && missing {
            // Back on the welcome screen's list, if it's still up.
            if self.boot.active && self.boot.kyty_needed {
                self.boot.waiting_kyty = true;
                self.boot.f_kyty = 0.0;
                self.boot_render();
            }
            self.kyty.force_install = true;
        }
        self.kyty_check(false);
        self.shad_tick();
        self.start_sync(false);
    }

    /// What's open or focused, by topic ID, so a catalog reload can find it again.
    fn catalog_anchor(&self) -> CatalogAnchor {
        let id = |t: Option<Target>| t.map(|t| (t.game.and_then(|g| self.games.get(g)).map(|g| g.g.id), t.local));
        // The Library card in focus: now, or saved under an overlay to return to.
        let grid = if self.view == 1 && self.zone == Z_GRID && self.overlay == Overlay::None { Some(self.idx) }
            else { self.stack.iter().find(|(_, zone, _)| *zone == Z_GRID).map(|(_, _, idx)| *idx) };
        CatalogAnchor {
            hub: id(self.hub),
            menu: id(self.menu_target),
            grid: grid.and_then(|i| self.filtered.get(i as usize)).and_then(|g| self.games.get(*g)).map(|g| g.g.id),
            grid_index: grid,
        }
    }

    /// After a reload: point the Hub and the Options menu at the same games again, or close them
    /// if their game is gone (and isn't installed).
    fn restore_targets(&mut self, anchor: &CatalogAnchor) {
        let find = |app: &App, t: Option<(Option<i64>, Option<usize>)>| t.map(|(id, local)| Target {
            game: id.and_then(|id| app.games.iter().position(|g| g.g.id == id)),
            local: local.filter(|l| *l < app.locals.len()),
        });
        self.hub = find(self, anchor.hub);
        self.menu_target = find(self, anchor.menu);
        let gone = |t: Option<Target>| t.is_some_and(|t| t.game.is_none() && t.local.is_none());
        if gone(self.hub) || gone(self.menu_target) {
            self.hub = None;
            self.menu_target = None;
            while self.overlay != Overlay::None && self.overlay != Overlay::Settings && self.overlay != Overlay::Downloads {
                self.back();
            }
            // Settings and Downloads stay open, but a Hub or menu waiting under them would come
            // back pointing at nothing (a Download button that does nothing): drop them, so Back
            // returns to what was under them.
            self.stack.retain(|(overlay, _, _)| !matches!(overlay, Overlay::Hub | Overlay::Menu));
        }
    }

    /// After a reload and push_all: keep the same Library card focused, or the nearest one if it's
    /// gone (the list may now be shorter than where the focus was).
    fn restore_grid_focus(&mut self, anchor: &CatalogAnchor) {
        let Some(old) = anchor.grid_index else { return };
        let found = anchor.grid.and_then(|id| self.filtered.iter().position(|g| self.games[*g].g.id == id)
            .or_else(|| self.filtered.iter().position(|g| self.groups.releases(*g).iter().any(|m| self.games[*m].g.id == id))));
        let pos = found.unwrap_or_else(|| (old.max(0) as usize).min(self.filtered.len().saturating_sub(1)));
        // The scroll position belongs to the old list: keep the card in view (and within the list).
        if self.view == 1 && self.zone == Z_GRID && self.overlay == Overlay::None {
            self.set_focus(Z_GRID, pos as i32);
            self.set_grid_scroll(self.grid_scroll, 0);
            self.ensure_grid_visible();
        } else if let Some(entry) = self.stack.iter_mut().find(|(_, zone, _)| *zone == Z_GRID) {
            entry.2 = pos as i32;
            let top = (pos / self.cols.max(1)) as f32 * self.row_h;
            if top < self.grid_scroll || top > self.grid_scroll + self.row_h * 2.0 {
                self.set_grid_scroll(top, 0);
            }
        }
    }

    /// Fetch the published catalog and reload. `by_hand` (Settings) always reports back; the
    /// automatic checks only speak up when there are new releases.
    /// Sync when a newer catalog is published (a small check; see catalog::published_changed).
    pub fn check_published(&mut self) {
        if self.syncing {
            return;
        }
        std::thread::spawn(|| {
            match catalog::published_changed() {
                Ok(true) => post(|app| app.start_sync(false)),
                Ok(false) => {}
                Err(error) => crate::log!("Published catalog not checked: {error}"),
            }
        });
    }

    pub fn start_sync(&mut self, by_hand: bool) {
        if self.syncing {
            return;
        }
        self.syncing = true;
        let known: HashSet<i64> = self.games.iter().map(|g| g.g.id).collect();
        self.set_status("Checking for new releases…", true);
        std::thread::spawn(move || {
            let res = catalog::sync(&|p| post(move |app| app.set_status(&p, true)));
            post(move |app| {
                app.syncing = false;
                match res {
                    Ok((file, offline)) => {
                        if offline {
                            app.net_down = true;
                        }
                        let first = known.is_empty();
                        let new = file.games.iter().filter(|g| !known.contains(&g.id)).count();
                        app.catalog_updated = file.updated;
                        // Games are held by position; a reload adds and removes some, so what's
                        // open or focused is found again by topic ID.
                        let anchor = app.catalog_anchor();
                        app.set_catalog(file.games);
                        app.restore_targets(&anchor);
                        app.push_all();
                        app.restore_grid_focus(&anchor);
                        if !first && new > 0 {
                            app.toast(&format!("{new} new release{} in the Library", if new == 1 { "" } else { "s" }),
                                &format!("{} games · {} releases", app.library_total, app.games.len()), 1);
                        } else if !first && by_hand {
                            app.toast("No new releases", &format!("{} games · {} releases · seeders updated", app.library_total, app.games.len()), 1);
                        }
                        app.set_status("", false);
                        app.boot_catalog_done();
                        app.start_enrich();
                    }
                    Err(e) => {
                        app.set_status("", false);
                        if by_hand || app.boot.active {
                            app.toast("Couldn't refresh the catalog", &e, 2);
                        }
                        if app.boot.active {
                            // Offline: don't hold the welcome screen for the catalog or artwork.
                            app.boot.waiting_art = false;
                            app.boot.cat_failed = true;
                            app.boot.text_main = "Catalog reload failed · keeping the offline snapshot".into();
                            app.boot_catalog_done();
                        }
                        app.start_enrich();
                    }
                }
            });
        });
    }

    pub fn start_enrich(&mut self) {
        if self.enriching {
            return;
        }
        let titles: Vec<(String, &'static str)> = self
            .games
            .iter()
            .flat_map(|g| {
                let region = psn::region_from_label(&g.g.region);
                std::iter::once(&g.g.title_id).chain(g.g.title_ids.iter()).map(move |id| (id.clone(), region))
            })
            .chain(self.locals.iter().map(|l| (l.l.title_id.clone(), psn::region_from_content_id(&l.l.content_id))))
            .collect();
        let store = self.art.clone();
        self.enriching = true;
        std::thread::spawn(move || {
            let last = Mutex::new(std::time::Instant::now() - Duration::from_secs(1));
            // Games Sony has no art for get Steam's from the catalog (the daily update looks it up).
            let changed = psn::enrich(&store, titles, &|p| {
                let mut l = last.lock().unwrap();
                if l.elapsed() > Duration::from_millis(120) {
                    *l = std::time::Instant::now();
                    post(move |app| app.set_status(&p, true));
                }
            });
            post(move |app| {
                app.enriching = false;
                app.set_status("", false);
                if changed {
                    app.images.clear_failures();
                    if app.view == 1 && !app.boot.active {
                        app.art_pending = true;
                    } else {
                        app.apply_art();
                    }
                }
                app.boot_art_done();
            });
        });
    }

    /// Take in the artwork and names that arrived: they can regroup releases into fewer cards, so
    /// whatever is open or focused is found again by topic ID.
    pub fn apply_art(&mut self) {
        let anchor = self.catalog_anchor();
        let games: Vec<Game> = self.games.iter().map(|g| g.g.clone()).collect();
        self.set_catalog(games);
        self.restore_targets(&anchor);
        if !self.boot.active {
            self.push_all();
            self.restore_grid_focus(&anchor);
        }
    }

    /// Download every Library cover into the disk cache (skips cached ones instantly).
    /// Download artwork before it's needed, in two stages:
    /// 1. what the first screens show — the Home row's tiles, backgrounds and logos, and every
    ///    Library cover (the welcome screen waits for this);
    /// 2. every other game's Game Hub background and logo, newest first, in the background.
    /// Sony's image server takes ~1 s per image, so anything fetched on demand is visibly slow.
    pub fn warm_covers(&mut self) {
        if self.warming {
            return;
        }
        let url = |r: Option<crate::present::ImgReq>| match r.map(|r| r.src) {
            Some(images::Src::Url(u)) => Some(u),
            _ => None,
        };
        let mut first: Vec<String> = Vec::new();
        for i in 0..self.row.len() {
            first.extend(url(self.tile_req(self.row[i])));
            first.extend(url(self.hero_bg_url(i)));
            if self.row[i] != RowItem::All {
                let info = self.target_info(self.row_target(i));
                first.extend(url(info.as_ref().and_then(|x| crate::present::req_url(&x.logo, 960, 0.0))));
            }
        }
        first.extend((0..self.games.len()).filter_map(|gi| url(self.card_req(gi))));
        let mut rest: Vec<String> = Vec::new();
        // Screenshots of the games on the Home row (the ones most likely to be opened).
        for i in 0..self.row.len() {
            if self.row[i] != RowItem::All {
                if let Some(info) = self.target_info(self.row_target(i)) {
                    rest.extend(info.shots.iter().take(8).filter_map(|u| url(crate::present::req_url(u, 480, 0.0))));
                }
            }
        }
        let mut order: Vec<usize> = (0..self.games.len()).collect();
        order.sort_by(|a, b| self.games[*b].g.date.cmp(&self.games[*a].g.date));
        for gi in order {
            let t = Target { game: Some(gi), local: self.games[gi].local };
            rest.extend(url(self.target_bg_url(t)));
            let info = self.games[gi].info.as_ref();
            rest.extend(url(info.and_then(|x| crate::present::req_url(&x.logo, 960, 0.0))));
        }
        let seen: std::collections::HashSet<String> = first.iter().cloned().collect();
        rest.retain(|u| !seen.contains(u));
        self.warming = true;
        std::thread::spawn(move || {
            let throttle = |f: &dyn Fn(usize, usize), done: usize, total: usize, last: &Mutex<std::time::Instant>| {
                let mut l = last.lock().unwrap();
                if l.elapsed() > Duration::from_millis(150) || done == total {
                    *l = std::time::Instant::now();
                    f(done, total);
                }
            };
            let last = Mutex::new(std::time::Instant::now() - Duration::from_secs(1));
            images::warm(first, 24, &|done, total| {
                throttle(&|d, t| post(move |app| app.boot_covers(d, t)), done, total, &last)
            });
            post(|app| app.boot_covers_done());
            // Stage 2: quietly, with fewer connections so on-demand images still get through.
            let last = Mutex::new(std::time::Instant::now() - Duration::from_secs(1));
            images::warm(rest, 10, &|done, total| {
                throttle(&|d, t| post(move |app| {
                    if !app.boot.active && !app.syncing && !app.enriching && !app.kyty.busy && !app.upd.busy {
                        app.set_status(&format!("Downloading game art {d}/{t}"), true);
                    }
                }), done, total, &last)
            });
            post(|app| {
                app.warming = false;
                if app.status.starts_with("Downloading game art") {
                    app.set_status("", false);
                }
            });
        });
    }

    // ------------------------------------------------------------------ KytyPS5 compatibility

    /// Which console a game is for: the installed copy's, else its catalog title ID / tag.
    pub fn target_platform(&self, t: Target) -> crate::platform::Platform {
        if let Some(l) = t.local {
            return self.locals[l].l.platform;
        }
        t.game.map(|g| self.games[g].g.platform).unwrap_or_default()
    }

    /// A PC game that runs through Proton or Wine: a Wine-packed release, or an installed .exe.
    pub fn pc_windows(&self, t: Target) -> bool {
        let installed = t.local.and_then(|l| self.locals[l].l.pc.as_ref()).is_some_and(|m| m.kind == crate::pcgame::Kind::Windows);
        installed || t.game.is_some_and(|g| self.games[g].g.wine)
    }

    fn local_platform(&self, game_id: &str) -> crate::platform::Platform {
        self.locals.iter().find(|l| l.l.id == game_id).map(|l| l.l.platform).unwrap_or_default()
    }

    pub fn game_compat(&self, g: &GameV) -> Option<&crate::compat::Entry> {
        (!g.g.title_id.is_empty()).then(|| self.compat.get(&g.g.title_id)).flatten()
    }

    pub fn target_compat(&self, t: Target) -> Option<&crate::compat::Entry> {
        let tid = t.local.map(|l| self.locals[l].l.title_id.clone()).filter(|s| !s.is_empty())
            .or_else(|| t.game.map(|g| self.games[g].g.title_id.clone()))?;
        self.compat.get(&tid.to_uppercase())
    }

    /// Will it play here, from the emulator's results and this PC's verdict. `None` for PC
    /// games, which have no emulator list (their Hub shows ProtonDB or "Native game").
    pub fn outlook(&self, t: Target) -> Option<crate::compat::Outlook> {
        let platform = self.target_platform(t);
        (platform != crate::platform::Platform::Pc)
            .then(|| crate::compat::Outlook::of(self.target_compat(t), self.pc_verdict(platform)))
    }

    /// A game's releases for the same platform, oldest version first, so "Version 3 of 3" is
    /// the newest one. (Other platforms are the Game Hub's "Play it on" row.)
    pub fn release_order(&self, index: usize) -> Vec<usize> {
        let platform = self.games[index].g.platform;
        let mut members: Vec<usize> = self.groups.releases(index).iter().copied().filter(|&i| self.games[i].g.platform == platform).collect();
        members.sort_by_key(|i| (crate::game_groups::version_key(&self.games[*i].g.version), self.games[*i].g.id));
        members
    }

    /// Refresh the community list in the background (at start and every 6 hours).
    pub fn compat_start(&mut self) {
        // Now and then, remind about results that haven't been shared yet.
        slint::Timer::single_shot(Duration::from_secs(20), || with_app(|app| app.remind_to_share()));
        let (_, age) = crate::compat::load();
        if age > crate::compat::REFRESH || self.compat.is_empty() {
            self.compat_fetch();
        }
        let t = slint::Timer::default();
        t.start(slint::TimerMode::Repeated, Duration::from_secs(30 * 60), || {
            with_app(|app| {
                if util::now_secs() - app.compat_checked > crate::compat::REFRESH {
                    app.compat_fetch();
                }
            })
        });
        std::mem::forget(t);
    }

    fn remind_to_share(&mut self) {
        // Only results from earlier days: don't nag about a game rated a minute ago.
        let day_ago = util::now_secs() - 24.0 * 3600.0;
        let n = self.my_results.unshared().iter().filter(|(_, r)| r.rated < day_ago).count();
        if n == 0 || self.boot.active || util::now_secs() - self.my_results.reminded < crate::results::REMIND_EVERY {
            return;
        }
        self.my_results.reminded = util::now_secs();
        self.my_results.save();
        self.toast_action(&format!("You've rated {n} game{} you played", if n == 1 { "" } else { "s" }),
            "Share your results with the emulator communities.", 0, "share", "Share");
    }

    /// "How far did it get?": saves your result on this PC. `after_play` adds "Not now".
    pub fn open_rating(&mut self, l: usize, after_play: bool) {
        if self.locals[l].l.title_id.is_empty() {
            self.toast("Can't rate this game", "Its folder has no title ID.", 2);
            return;
        }
        let mk = |id: &'static str, label: &str, icon: &'static str| ActionDef { id, label: label.into(), icon, primary: false, danger: false, round: false };
        let mut items = vec![
            mk("rate_ingame", "In game: reaches gameplay", "play"),
            mk("rate_menu", "Main menu, but not gameplay", "grid"),
            mk("rate_logo", "Intro logos, then stops", "film"),
            mk("rate_noboot", "Doesn't start", "stop"),
        ];
        if after_play {
            items.push(mk("rate_skip", "Not now", ""));
        }
        let name = self.locals[l].name.clone();
        self.menu_target = Some(Target { game: self.locals[l].cat, local: Some(l) });
        self.menu_actions = items;
        audio::play(Sound::Select);
        self.push_overlay(Overlay::Menu, Z_MENU, 0);
        self.push_menu(&format!("How far did {name} get?"));
    }

    /// Save a rating (`None`: skipped) with the KytyPS5 build it was played on.
    fn rate(&mut self, l: usize, status: Option<crate::compat::Status>) {
        let (tid, name) = (self.locals[l].l.title_id.clone(), self.locals[l].name.clone());
        // The log that goes with this result: the saved crash log if the game just crashed,
        // otherwise a copy of its latest log (the next launch would overwrite the original).
        let log = match self.crash_logs.remove(&self.locals[l].l.id) {
            Some(saved) => Some(saved),
            None if status.is_some() => crate::results::save_log(&util::cache_dir().join("logs").join(format!("{tid}.log")), &tid, false),
            None => None,
        }.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let (emulator, platform) = (self.cfg.lock().unwrap().emulator_path(), self.locals[l].l.platform);
        std::thread::spawn(move || {
            let kyty = emulator_version(platform, &emulator);
            post(move |app| {
                let Some(status) = status else {
                    app.my_results.skip(&tid, &kyty);
                    app.my_results.save();
                    return;
                };
                app.my_results.rate(&tid, &name, status, &kyty, util::now_secs(), &log);
                app.my_results.save();
                app.compat = crate::compat::with_mine(std::mem::take(&mut app.compat), &app.my_results);
                app.build_genres();
                app.apply_filter();
                app.push_all();
                if app.overlay == Overlay::Hub {
                    app.push_hub();
                }
                app.toast(&format!("Saved: {} on Linux", status.label()),
                    &format!("It's shared with {} the next time you choose Settings → Share your game ratings.", crate::platform::Platform::of_title_id(&tid).unwrap_or_default().emulator()), 1);
            });
        });
    }

    /// After a game closes, ask how far it got, unless it was rated or skipped on this build.
    fn ask_rating(&mut self, game_id: &str) {
        let Some(l) = self.locals.iter().position(|g| g.l.id == game_id) else { return };
        let tid = self.locals[l].l.title_id.clone();
        let (emulator, platform) = (self.cfg.lock().unwrap().emulator_path(), self.locals[l].l.platform);
        let id = game_id.to_string();
        std::thread::spawn(move || {
            let kyty = emulator_version(platform, &emulator);
            post(move |app| {
                let free = matches!(app.overlay, Overlay::None | Overlay::Hub) && app.live.is_empty() && !app.boot.active;
                if free && app.my_results.should_ask(&tid, &kyty) {
                    if let Some(l) = app.locals.iter().position(|g| g.l.id == id) {
                        app.open_rating(l, true);
                    }
                }
            });
        });
    }

    /// Open the pre-filled KytyPS5 report for every result not shared yet (up to a batch).
    pub fn share_results(&mut self) {
        let batch: Vec<(String, crate::results::MyResult)> = self.my_results.unshared().into_iter()
            .take(crate::results::BATCH).map(|(k, v)| (k.clone(), v.clone())).collect();
        if batch.is_empty() {
            self.toast("Nothing new to share", "Rate a game from its Options menu after playing it.", 0);
            return;
        }
        let now = util::now_secs();
        // The logs to attach, gathered in one folder named after each game, opened next to the forms.
        let folder = util::data_dir().join("share");
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::create_dir_all(&folder);
        let mut reports = Vec::new();
        for (tid, r) in &batch {
            let saved = Some(std::path::PathBuf::from(&r.log)).filter(|p| p.is_file()).unwrap_or_else(|| util::cache_dir().join("logs").join(format!("{tid}.log")));
            let log = std::fs::read(&saved).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            if !log.is_empty() {
                let name: String = r.name.chars().filter(|c| c.is_alphanumeric() || " -_".contains(*c)).collect();
                let _ = std::fs::write(folder.join(format!("{tid} {}.log", name.trim())), &log);
            }
            if let (Some(status), Some(saved)) = (r.status(), self.my_results.games.get_mut(tid)) {
                saved.shared = now;
                let game_version = self.locals.iter().find(|g| &g.l.title_id == tid).map(|g| g.l.version.clone()).unwrap_or_default();
                reports.push((r.name.clone(), tid.clone(), r.kyty.clone(), game_version, status, log));
            }
        }
        self.my_results.save();
        // Building the links reads system details: do it, and open the tabs, off the UI thread.
        std::thread::spawn(move || {
            for (i, (name, tid, kyty, game_version, status, log)) in reports.iter().enumerate() {
                if i > 0 {
                    std::thread::sleep(Duration::from_millis(800));
                }
                open_url(&crate::compat::report_url_for(name, tid, kyty, game_version, *status, log));
            }
        });
        if std::fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_some()) {
            open_url(&folder.to_string_lossy());
        }
        let n = batch.len();
        let left = self.my_results.unshared().len();
        let mut more = if left > 0 { format!(" {left} more next time.") } else { String::new() };
        if batch.iter().any(|(tid, _)| crate::platform::Platform::of_title_id(tid) == Some(crate::platform::Platform::Ps4)) {
            more.push_str(" shadPS4 only accepts reports for games dumped from a copy you own, unmodified.");
        }
        self.toast(&format!("Opened {n} report{} in your browser", if n == 1 { "" } else { "s" }),
            &format!("Check each one, drag in its log from the folder that opened (named after the game), and submit.{more}"), 1);
        self.refresh_settings();
    }

    fn compat_fetch(&mut self) {
        self.compat_checked = util::now_secs();
        std::thread::spawn(|| {
            let res = crate::compat::fetch();
            post(move |app| match res {
                Ok(db) => {
                    let db = crate::compat::with_mine(db, &app.my_results);
                    let changed = db.len() != app.compat.len()
                        || db.iter().any(|(k, v)| app.compat.get(k).map(|o| o.status != v.status).unwrap_or(true));
                    app.compat = db;
                    if changed {
                        app.build_genres();
                        app.apply_filter();
                        if !app.boot.active {
                            app.push_all();
                        }
                    }
                }
                Err(e) => crate::log!("compatibility list refresh failed: {e}"),
            });
        });
    }

    pub fn rescan_library(&mut self) {
        let dirs = self.cfg.lock().unwrap().game_dir_paths();
        *self.library.lock().unwrap() = library::scan(&dirs);
        self.refresh_locals();
        self.push_all();
    }

    // ------------------------------------------------------------------ events

    pub fn on_image(&mut self, key: String, buf: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>) {
        self.images.insert(key.clone(), buf);
        self.refresh_images(&key);
    }

    pub fn on_sessions(&mut self) {
        let before: Vec<u32> = self.live.iter().map(|s| s.pid).collect();
        self.live = self.sessions.live();
        let after: Vec<u32> = self.live.iter().map(|s| s.pid).collect();
        for e in self.sessions.take_ended() {
            let played = util::fmt_duration(e.played);
            let crashed = e.exit_code.filter(|c| *c != 0 && !e.stopped);
            // Exited cleanly but almost at once: most likely it couldn't start.
            let failed_to_start = crashed.is_none() && e.played < 5.0 && !e.stopped;
            if crashed.is_some() || failed_to_start {
                // Keep the log: the next launch of this game would overwrite it. It goes to KytyPS5
                // with this game's rating, so they can see what went wrong.
                let tid = self.locals.iter().find(|l| l.l.id == e.game_id).map(|l| l.l.title_id.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| e.game_id.clone());
                let saved = (!e.log.as_os_str().is_empty()).then(|| crate::results::save_log(&e.log, &tid, true)).flatten();
                if let Some(saved) = &saved {
                    self.crash_logs.insert(e.game_id.clone(), saved.clone());
                }
                let action = saved.as_ref().map(|p| format!("log:{}", p.display())).unwrap_or_default();
                let platform = self.local_platform(&e.game_id);
                let pc = platform == crate::platform::Platform::Pc;
                let emulator = platform.emulator();
                let what = match crashed {
                    Some(code) => {
                        crate::log!("{} crashed (exit {code}), log: {}", e.name, e.log.display());
                        audio::play(Sound::Error);
                        (format!("{} stopped unexpectedly", e.name), if pc { format!("The game exited with code {code}.") } else { format!("{emulator} exited with code {code}.") })
                    }
                    None if pc => (format!("{} closed right after starting", e.name), "It may need something this PC doesn't have (Wine, a library).".into()),
                    None => (format!("{} closed right after starting", e.name), format!("{emulator} may not support it yet.")),
                };
                let more = match (&saved, pc) {
                    (Some(_), false) => format!(" Its log is saved for your next report to {emulator}."),
                    _ => String::new(),
                };
                let button = if saved.is_some() { "See the log" } else { "" };
                self.toast_game_action(&what.0, &format!("{}{more}", what.1), 2, &e.game_id, &action, button);
            } else {
                self.toast_game(&e.name, &format!("Played for {played}"), 1, &e.game_id);
            }
            // Ratings go to the emulators' community lists: not for PC games.
            if self.local_platform(&e.game_id) != crate::platform::Platform::Pc {
                self.ask_rating(&e.game_id);
            }
        }
        if before != after {
            // Play ⇄ Resume/Stop changes the buttons; keep focus on the first one.
            if self.zone == Z_ACTIONS || self.zone == Z_HUB {
                self.set_focus(self.zone, 0);
            }
            if self.zone == Z_TOP && self.live.is_empty() && self.idx < 2 {
                self.set_focus(Z_TOP, 3);
            }
            self.build_row();
            self.push_row();
            self.push_hero();
            if self.overlay == Overlay::Hub {
                self.push_hub();
            }
            self.tick();
            if self.live.is_empty() {
                self.kyty_games_closed();
                self.shad_games_closed();
            }
        }
    }

    pub fn tick(&mut self) {
        self.push_installs();
        self.push_downloads();
        if self.overlay == Overlay::Settings && self.edit_index < 0 && self.vpn_seen != crate::settings::vpn_signature() {
            self.refresh_settings();
        }
        if self.overlay == Overlay::Settings {
            self.sys_poll();
        }
        let tm = util::local_time();
        let (h, m) = (tm.tm_hour, tm.tm_min);
        let clock = format!("{}:{:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, m, if h < 12 { "AM" } else { "PM" });
        let ui = self.ui();
        if clock != self.clock {
            ui.set_clock(clock.clone().into());
            self.clock = clock;
        }
        self.battery_tick();
        let running = match self.live.first() {
            Some(s) => crate::RunningData { active: true, name: s.name.clone().into(), time: util::fmt_clock(util::now_secs() - s.since).into() },
            None => crate::RunningData::default(),
        };
        if running.active || ui.get_running().active {
            ui.set_running(running);
            // The kicker / row label show the live session time too.
            if self.view == 0 && self.overlay == Overlay::None && self.current_session().is_some() {
                self.push_row_text();
                self.push_hero_kicker();
            }
        }
    }

    pub fn on_pad(&mut self, p: Pad) {
        if p == Pad::Ps {
            self.sessions.toggle_focus();
            return;
        }
        if !crate::display::window_has_focus(&self.ui()) {
            return;
        }
        if !self.pad_hints {
            self.pad_hints = true;
            self.ui().set_pad_hints(true);
            // Settings names keyboard shortcuts only to keyboard players.
            if self.overlay == Overlay::Settings { self.refresh_settings(); }
        }
        let act = match p {
            Pad::Up => Act::Up,
            Pad::Down => Act::Down,
            Pad::Left => Act::Left,
            Pad::Right => Act::Right,
            Pad::Confirm => Act::Confirm,
            Pad::Back => Act::Back,
            Pad::Square => Act::Trailer,
            Pad::Triangle => Act::Search,
            Pad::Options => Act::Options,
            Pad::L1 => Act::TabPrev,
            Pad::R1 => Act::TabNext,
            Pad::L2 => Act::PageUp,
            Pad::R2 => Act::PageDown,
            Pad::Ps => return,
        };
        if self.search_editing || self.edit_index >= 0 {
            // Controller input ends text editing.
            if self.search_editing {
                self.stop_search_edit();
            } else {
                self.finish_edit(None);
            }
            if act == Act::Back {
                return;
            }
        }
        if self.overlay == Overlay::Downloads && self.ui().get_download_editing() {
            self.ui().invoke_focus_root();
            if act == Act::Confirm { return; }
        }
        self.act(act);
    }

    pub fn on_key(&mut self, text: &str, ctrl: bool, alt: bool, repeat: bool) -> bool {
        if self.pad_hints {
            self.pad_hints = false;
            self.ui().set_pad_hints(false);
            if self.overlay == Overlay::Settings { self.refresh_settings(); }
        }
        let k = |key: Key| SharedString::from(key) == text;
        let dir = if k(Key::UpArrow) {
            Some(Act::Up)
        } else if k(Key::DownArrow) {
            Some(Act::Down)
        } else if k(Key::LeftArrow) {
            Some(Act::Left)
        } else if k(Key::RightArrow) {
            Some(Act::Right)
        } else {
            None
        };

        if self.overlay == Overlay::WifiJoin {
            return self.wifi_join_key(text, ctrl, alt);
        }
        if self.overlay == Overlay::SearchKeys {
            return self.search_keys_key(text, ctrl, alt);
        }
        // Text editing: the TextInput handles typing; we get what it doesn't use.
        if self.overlay == Overlay::Downloads && self.ui().get_download_consent()
            && !self.ui().get_download_editing() && !ctrl && !alt && k(Key::Tab) {
            self.ui().set_download_focus_path(true);
            return true;
        }
        if self.overlay == Overlay::Downloads && self.ui().get_download_consent() && self.ui().get_download_editing() {
            if k(Key::Escape) {
                self.act_downloads(Act::Back);
                return true;
            }
            return false;
        }
        if self.search_editing {
            if k(Key::Escape) {
                self.stop_search_edit();
                return true;
            }
            if let Some(d) = dir {
                self.stop_search_edit();
                self.act(d);
                return true;
            }
            return false;
        }
        if self.edit_index >= 0 {
            if k(Key::Escape) {
                self.finish_edit(None);
                return true;
            }
            if matches!(dir, Some(Act::Up | Act::Down)) {
                let cur = self.ui().get_edit_text().to_string();
                self.finish_edit(Some(cur));
                self.act(dir.unwrap());
                return true;
            }
            return false;
        }
        if is_quit_key(text, ctrl) {
            // Not on AzrakOS (see `quit`): there a Ctrl held a moment too long while typing
            // "w" would leave a black TV.
            quit(&format!("Ctrl+{} pressed", text.to_uppercase()));
            return true;
        }
        if ctrl && text == "d" && !self.boot.active {
            self.open_downloads(None);
            return true;
        }
        if ctrl || alt {
            return false;
        }
        if let Some(d) = dir {
            self.act(d);
            return true;
        }
        let act = if text == "\n" || text == "\r" || k(Key::Return) || text == " " {
            if repeat {
                return true;
            }
            Act::Confirm
        } else if k(Key::Escape) || k(Key::Backspace) {
            Act::Back
        } else if k(Key::PageUp) {
            Act::PageUp
        } else if k(Key::PageDown) {
            Act::PageDown
        } else if k(Key::Home) {
            Act::First
        } else if k(Key::End) {
            Act::Last
        } else if k(Key::Menu) || text == "o" || text == "O" {
            Act::Options
        } else if text == "t" || text == "T" {
            Act::Trailer
        } else if text == "/" {
            Act::Search
        } else if (text == "s" || text == "S") && self.overlay == Overlay::None && self.view == 0 {
            Act::Settings
        } else if k(Key::Tab) {
            Act::TabNext
        } else if self.overlay == Overlay::None && self.view == 1 && text.chars().count() == 1 && text.chars().all(|c| c.is_alphanumeric()) {
            // Type-to-search in the library. The letter goes after what's there, and typing carries
            // on after it: focusing the field selects everything, so the cursor moves to the end
            // (otherwise "wolverine" came out as "olverinew").
            let q = format!("{}{}", self.query, text);
            self.ui().set_query(q.clone().into());
            self.on_search(q);
            self.start_search_edit();
            self.ui().invoke_search_cursor_to_end();
            return true;
        } else if (text == "f" || text == "s") && self.overlay == Overlay::None {
            Act::Search
        } else {
            return false;
        };
        self.act(act);
        true
    }

    /// The mouse moved onto something: where the controller's highlight follows it.
    pub fn on_hover(&mut self, zone: i32, idx: i32) {
        if zone == Z_OS_INSTALL && self.overlay == Overlay::OsInstall {
            self.hover_os_install(idx);
        }
    }

    pub fn on_click(&mut self, zone: i32, idx: i32) {
        if self.boot.active {
            return self.boot_confirm();
        }
        if self.clicks_after.is_some_and(|t| std::time::Instant::now() < t) {
            return;
        }
        if self.search_editing && zone != Z_SEARCH {
            self.stop_search_edit();
        }
        if self.edit_index >= 0 && !(zone == Z_SETTINGS && idx == self.edit_index) {
            let cur = self.ui().get_edit_text().to_string();
            self.finish_edit(Some(cur));
        }
        if zone == Z_WIFI_JOIN && self.overlay == Overlay::WifiJoin {
            return self.click_wifi_join(idx);
        }
        if zone == Z_SEARCH_KEYS && self.overlay == Overlay::SearchKeys {
            return self.click_search_keys(idx);
        }
        if zone == Z_POWER && self.overlay == Overlay::Power {
            return self.click_power(idx);
        }
        if zone == Z_OS_INSTALL && self.overlay == Overlay::OsInstall {
            if idx < 0 {
                self.act_os_install(Act::Back);
            } else {
                self.click_os_install(idx);
            }
            return;
        }
        // Clicks on an overlay backdrop close it.
        if idx < 0 {
            self.back();
            return;
        }
        if zone == Z_CONTROLS && self.overlay == Overlay::Controls {
            self.ui().set_controls_ps4(idx == 1);
            return;
        }
        if zone == Z_ROW && self.overlay == Overlay::None {
            if self.zone == Z_ROW && self.sel == idx as usize {
                self.activate_row();
            } else {
                self.set_focus(self.home_zone(), 0);
                self.select_tile(idx as i64);
            }
            return;
        }
        if zone == Z_SEARCH {
            self.set_focus(Z_SEARCH, 0);
            self.open_search();
            return;
        }
        self.set_focus(zone, idx);
        if zone == Z_GRID {
            self.ensure_grid_visible();
        }
        self.act(Act::Confirm);
    }

    pub fn on_search(&mut self, q: String) {
        if q == self.query {
            return;
        }
        self.query = q;
        self.apply_filter();
        self.set_grid_scroll(0.0, 0);
        self.push_grid();
        self.push_genres();
    }

    pub fn start_search_edit(&mut self) {
        if self.view != 1 {
            self.switch_view(1, false);
        }
        self.search_editing = true;
        self.set_focus(Z_SEARCH, 0);
        self.ui().invoke_focus_search();
    }

    pub fn stop_search_edit(&mut self) {
        self.search_editing = false;
        self.ui().invoke_focus_root();
    }

    // ------------------------------------------------------------------ navigation

    /// Where selection rests on the Home screen: the tile row, or its buttons when there are no tiles.
    pub fn home_zone(&self) -> i32 {
        if self.row.is_empty() { Z_ACTIONS } else { Z_ROW }
    }

    pub fn set_focus(&mut self, zone: i32, idx: i32) {
        self.zone = zone;
        self.idx = idx;
        let ui = self.ui();
        ui.set_zone(zone);
        ui.set_idx(idx);
    }

    pub fn move_focus(&mut self, zone: i32, idx: i32) {
        self.set_focus(zone, idx);
        audio::play(Sound::Move);
    }

    /// The top bar's buttons: 0–1 a running game's Resume and Stop, 3 Settings, 4 Downloads. (2 was
    /// a search button, which only led to the Library's own search: △ or / still gets there.)
    fn top_items(&self) -> Vec<i32> {
        if self.live.is_empty() { vec![3, 4] } else { vec![0, 1, 3, 4] }
    }

    pub fn act(&mut self, a: Act) {
        if self.boot.active {
            if a == Act::Confirm && (self.boot.first || self.boot.ready) {
                self.boot_confirm();
            }
            return;
        }
        match self.overlay {
            Overlay::Launch => {
                if a == Act::Back {
                    self.hide_launch_splash();
                }
            }
            Overlay::Viewer => self.act_viewer(a),
            Overlay::Trailer => self.act_trailer(a),
            Overlay::Controls => match a {
                Act::Back | Act::Confirm => self.back(),
                Act::Left | Act::Right => {
                    let ui = self.ui();
                    ui.set_controls_ps4(!ui.get_controls_ps4());
                    audio::play(Sound::Move);
                }
                _ => {}
            },
            Overlay::Menu => self.act_menu(a),
            Overlay::OsInstall => self.act_os_install(a),
            Overlay::WifiJoin => self.act_wifi_join(a),
            Overlay::SearchKeys => self.act_search_keys(a),
            Overlay::Power => self.act_power(a),
            Overlay::Settings => self.act_settings(a),
            Overlay::Hub => self.act_hub(a),
            Overlay::Downloads => self.act_downloads(a),
            Overlay::Sort => self.act_sort_picker(a),
            Overlay::None => self.act_main(a),
        }
    }

    fn act_main(&mut self, a: Act) {
        match a {
            Act::Back => return self.back(),
            Act::Search => return self.open_search(),
            Act::Settings => return self.open_settings(),
            Act::TabPrev | Act::TabNext => return self.switch_view(1 - self.view, true),
            Act::Trailer => {
                if let Some(t) = self.focused_target() {
                    self.play_trailer(t);
                }
                return;
            }
            Act::Options => {
                match self.focused_target() {
                    Some(t) => self.open_menu(t),
                    None => self.open_settings(),
                }
                return;
            }
            _ => {}
        }
        match self.zone {
            Z_TABS => match a {
                Act::Left if self.idx > 0 => self.move_focus(Z_TABS, self.idx - 1),
                Act::Right if self.idx < 1 => self.move_focus(Z_TABS, self.idx + 1),
                Act::Right => self.move_focus(Z_TOP, self.top_items()[0]),
                Act::Down => {
                    if self.view == 0 { self.move_focus(self.home_zone(), 0) } else { self.move_focus(Z_SEARCH, 0) }
                }
                Act::Confirm => self.switch_view(self.idx, true),
                _ => {}
            },
            Z_TOP => {
                let items = self.top_items();
                let pos = items.iter().position(|i| *i == self.idx).unwrap_or(0);
                match a {
                    Act::Left if pos > 0 => self.move_focus(Z_TOP, items[pos - 1]),
                    Act::Left => self.move_focus(Z_TABS, 1),
                    Act::Right if pos + 1 < items.len() => self.move_focus(Z_TOP, items[pos + 1]),
                    Act::Down => {
                        if self.view == 0 { self.move_focus(self.home_zone(), 0) } else { self.move_focus(Z_SEARCH, 0) }
                    }
                    Act::Confirm => match self.idx {
                        0 => self.resume_game(),
                        1 => self.stop_game(None),
                        4 => self.open_downloads(None),
                        _ => self.open_settings(),
                    },
                    _ => {}
                }
            }
            Z_ROW => match a {
                Act::Left => self.select_tile(self.sel as i64 - 1),
                Act::Right => self.select_tile(self.sel as i64 + 1),
                Act::First | Act::PageUp => self.select_tile(0),
                Act::Last | Act::PageDown => self.select_tile(self.row.len() as i64 - 1),
                Act::Up => self.move_focus(Z_TABS, 0),
                Act::Down if !self.hero_actions.is_empty() => self.move_focus(Z_ACTIONS, 0),
                Act::Confirm => self.activate_row(),
                _ => {}
            },
            Z_ACTIONS => match a {
                Act::Left if self.idx > 0 => self.move_focus(Z_ACTIONS, self.idx - 1),
                Act::Right if (self.idx as usize) + 1 < self.hero_actions.len() => self.move_focus(Z_ACTIONS, self.idx + 1),
                Act::Up if self.row.is_empty() => self.move_focus(Z_TABS, 0),
                Act::Up => self.move_focus(self.home_zone(), 0),
                Act::Confirm => {
                    if let Some(act) = self.hero_actions.get(self.idx as usize).map(|a| a.id) {
                        let t = self.row_target(self.sel);
                        self.run_action(act, t);
                    }
                }
                _ => {}
            },
            Z_SEARCH => match a {
                Act::Right => self.move_focus(Z_SORT, 0),
                Act::Up => self.move_focus(Z_TABS, 1),
                Act::Down => self.focus_status(),
                Act::Confirm => self.open_search(),
                _ => {}
            },
            Z_SORT => match a {
                Act::Left => self.move_focus(Z_SEARCH, 0),
                Act::PageUp => self.cycle_sort(-1),
                Act::PageDown => self.cycle_sort(1),
                Act::Up => self.move_focus(Z_TABS, 1),
                Act::Down => self.focus_status(),
                Act::Confirm => self.open_sort_picker(),
                Act::Right => self.move_focus(Z_DENSITY, 0),
                _ => {}
            },
            Z_DENSITY => match a {
                Act::Left if self.idx > 0 => self.move_focus(Z_DENSITY, 0),
                Act::Left => self.move_focus(Z_SORT, 0),
                Act::Right => self.move_focus(Z_DENSITY, 1),
                Act::Up => self.move_focus(Z_TABS, 1),
                Act::Down => self.focus_status(),
                Act::Confirm => self.set_library_density(self.idx == 1),
                _ => {}
            },
            Z_CHIPS => { let n: i32 = self.status_count as i32; match a {
                Act::Left if self.idx > if self.idx < n { 0 } else { n } => {
                    self.move_focus(Z_CHIPS, self.idx - 1);
                    self.push_genres();
                }
                Act::Right if (self.idx as usize) + 1 < if self.idx < n { n as usize } else { self.genre_list.len() } => {
                    self.move_focus(Z_CHIPS, self.idx + 1);
                    self.push_genres();
                }
                Act::Up if self.idx >= n => self.focus_status(),
                Act::Up => self.move_focus(Z_SEARCH, 0),
                Act::Down if self.idx < n => self.focus_chip(),
                Act::Down if !self.filtered.is_empty() => {
                    let first = self.first_visible_card();
                    self.move_focus(Z_GRID, first as i32);
                    self.focus_card_changed();
                }
                Act::Confirm => {
                    if let Some((g, _)) = self.genre_list.get(self.idx as usize).cloned() {
                        let i = self.idx as usize;
                        let chosen = if i == 0 { self.status_filter == "All" && self.platform_filter.is_none() }
                            else if i < n as usize { self.console_chip(i).is_none() && self.status_filter == g }
                            else { self.genre == g || (i == n as usize && self.genre == "All") };
                        // The chip that's already chosen: "All" (or "All genres") takes you to the
                        // games; any other one goes back to All, like pressing a toggle again.
                        if chosen && (i == 0 || i == n as usize) {
                            if !self.filtered.is_empty() {
                                let first = self.first_visible_card();
                                self.move_focus(Z_GRID, first as i32);
                                self.focus_card_changed();
                            }
                            return;
                        }
                        let g = if chosen { if i < n as usize { "All".to_string() } else { "All genres".to_string() } } else { g };
                        if let Some(p) = self.console_chip(self.idx as usize) {
                            // A console chip toggles: press it again to see every console.
                            self.platform_filter = if self.platform_filter == Some(p) { None } else { Some(p) };
                        } else if self.idx == 0 {
                            // "All" means everything: no status filter and every console.
                            self.status_filter = g;
                            self.platform_filter = None;
                        }
                        else if self.idx < n { self.status_filter = g; }
                        else { self.genre = if g == "All genres" { "All".into() } else { g }; }
                        self.apply_filter();
                        self.set_grid_scroll(0.0, 0);
                        self.push_library();
                        audio::play(Sound::Select);
                    }
                }
                _ => {}
            }},
            Z_GRID => self.act_grid(a),
            _ => {}
        }
    }

    fn focus_chip(&mut self) {
        let i = self.genre_list.iter().enumerate().skip(self.status_count).find(|(_, (g, _))| *g == self.genre).map(|(i, _)| i).unwrap_or(self.status_count);
        self.move_focus(Z_CHIPS, i as i32);
        self.push_genres();
    }

    fn focus_status(&mut self) {
        let i = self.genre_list.iter().take(self.status_count).position(|(g, _)| *g == self.status_filter).unwrap_or(0);
        self.move_focus(Z_CHIPS, i as i32);
        self.push_genres();
    }

    /// The platform a status-row chip stands for ("PS5 games only", "PC games only"…).
    pub fn console_chip(&self, i: usize) -> Option<crate::platform::Platform> {
        let first = self.status_count.checked_sub(self.consoles.len())?;
        if i < first || i >= self.status_count {
            return None;
        }
        self.consoles.get(i - first).copied()
    }

    /// The game is for the console the Library shows (any, unless one is chosen).
    pub fn platform_ok(&self, game: &GameV) -> bool {
        self.platform_filter.is_none_or(|p| game.g.platform == p)
    }

    pub fn matches_status(&self, index: usize) -> bool {
        let game = &self.games[index];
        self.platform_ok(game) && self.status_filter_ok(game)
    }

    /// The status filter alone (Installed, Playable, …), whatever the console.
    pub fn status_filter_ok(&self, game: &GameV) -> bool {
        self.status_filter == "All" || (self.status_filter == "Installed" && game.local.is_some())
            || (self.status_filter == "Playable" && self.game_compat(game).is_some_and(|entry| entry.in_game()))
    }

    pub fn open_sort_picker(&mut self) {
        if self.overlay != Overlay::None || self.view != 1 { return; }
        self.push_overlay(Overlay::Sort, Z_SORT_PICKER, self.sort as i32);
    }

    fn act_sort_picker(&mut self, action: Act) {
        match action {
            Act::Back => self.back(),
            Act::Up => self.move_focus(Z_SORT_PICKER, (self.idx - 1).rem_euclid(SORTS.len() as i32)),
            Act::Down => self.move_focus(Z_SORT_PICKER, (self.idx + 1).rem_euclid(SORTS.len() as i32)),
            Act::First => self.move_focus(Z_SORT_PICKER, 0),
            Act::Last => self.move_focus(Z_SORT_PICKER, SORTS.len() as i32 - 1),
            Act::Confirm if (0..SORTS.len() as i32).contains(&self.idx) => {
                self.sort = self.idx as usize;
                self.back();
                self.apply_filter();
                self.set_grid_scroll(0.0, 0);
                self.push_library();
                audio::play(Sound::Select);
            }
            _ => {}
        }
    }

    pub fn set_library_density(&mut self, compact: bool) {
        if self.cfg.lock().unwrap().library_compact == compact { return; }
        let anchor = if self.zone == Z_GRID { self.idx.max(0) as usize } else { self.first_visible_card() };
        { let mut cfg = self.cfg.lock().unwrap(); cfg.library_compact = compact; cfg.save(); }
        self.relayout();
        self.push_library();
        self.set_grid_scroll((anchor / self.cols) as f32 * self.row_h, 0);
    }

    pub fn scroll_genres(&mut self, delta: i32) {
        if self.overlay != Overlay::None || self.view != 1 { return; }
        let i = if self.zone == Z_CHIPS && self.idx >= self.status_count as i32 { self.idx } else {
            self.genre_list.iter().enumerate().skip(self.status_count).find(|(_, (g, _))| *g == self.genre).map(|(i, _)| i).unwrap_or(self.status_count) as i32
        };
        let last = self.genre_list.len().saturating_sub(1).max(3) as i32;
        self.set_focus(Z_CHIPS, (i + delta).clamp(3, last));
        self.push_genres();
    }

    fn act_grid(&mut self, a: Act) {
        let n = self.filtered.len() as i64;
        if n == 0 {
            if a == Act::Up {
                self.focus_chip();
            }
            return;
        }
        let cols = self.cols as i64;
        let i = self.idx as i64;
        let page = ((self.logical_size().1 - crate::library_layout::TOP) / self.row_h).floor().max(1.0) as i64 * cols;
        let j = match a {
            Act::Left => i - 1,
            Act::Right => i + 1,
            Act::Up if i < cols => return self.focus_chip(),
            Act::Up => i - cols,
            Act::Down if i + cols >= n => {
                if i / cols < (n - 1) / cols { n - 1 } else { return }
            }
            Act::Down => i + cols,
            Act::PageUp => (i - page).max(i % cols),
            Act::PageDown => (i + page).min(n - 1),
            Act::First => 0,
            Act::Last => n - 1,
            Act::Confirm => {
                if let Some(&gi) = self.filtered.get(i as usize) {
                    audio::play(Sound::Select);
                    let local = self.games[gi].local;
                    self.open_hub(Target { game: Some(gi), local });
                }
                return;
            }
            _ => return,
        };
        if j < 0 || j >= n || j == i {
            return;
        }
        self.move_focus(Z_GRID, j as i32);
        self.ensure_grid_visible();
        self.focus_card_changed();
    }

    fn act_hub(&mut self, a: Act) {
        match a {
            Act::Back => return self.back(),
            Act::Trailer => {
                if let Some(t) = self.hub {
                    self.play_trailer(t);
                }
                return;
            }
            Act::Options => {
                if let Some(t) = self.hub {
                    self.open_menu(t);
                }
                return;
            }
            _ => {}
        }
        let shots = self.hub_shots.len() as i32;
        match (self.zone, a) {
            (Z_HUB, Act::Left) if self.idx > 0 => self.move_focus(Z_HUB, self.idx - 1),
            (Z_HUB, Act::Right) if (self.idx as usize) + 1 < self.hub_actions.len() => self.move_focus(Z_HUB, self.idx + 1),
            (Z_HUB | Z_VERSIONS | Z_RELEASES, Act::Down) => {
                self.hub_step(1);
                self.scroll_hub();
            }
            (Z_VERSIONS, Act::Left) if self.idx > 0 => self.move_focus(Z_VERSIONS, self.idx - 1),
            (Z_VERSIONS, Act::Right) if (self.idx as usize) + 1 < self.hub_version_count() => self.move_focus(Z_VERSIONS, self.idx + 1),
            (Z_VERSIONS, Act::Confirm) => self.switch_version(self.idx as usize),
            (Z_VERSIONS | Z_RELEASES, Act::Up) => {
                self.hub_step(-1);
                self.scroll_hub();
            }
            (Z_RELEASES, Act::Left) => self.cycle_hub_release(-1),
            (Z_RELEASES, Act::Right | Act::Confirm) => self.cycle_hub_release(1),
            (Z_HUB_DETAILS, Act::Confirm) => {
                let ui = self.ui(); ui.set_hub_details_open(!ui.get_hub_details_open());
            }
            (Z_HUB_DETAILS, Act::Up) => {
                self.hub_step(-1);
                self.scroll_hub();
            }
            (Z_HUB_DETAILS, Act::Down) => {
                if shots > 0 { self.move_focus(Z_SHOTS, 0); self.prefetch_viewer(); } else { self.move_focus(Z_DESC, 0); }
                self.scroll_hub();
            }
            (Z_HUB, Act::Confirm) => {
                if let (Some(act), Some(t)) = (self.hub_actions.get(self.idx as usize).map(|a| a.id), self.hub) {
                    self.run_action(act, t);
                }
            }
            (Z_SHOTS, Act::Left) if self.idx > 0 => {
                self.move_focus(Z_SHOTS, self.idx - 1);
                self.scroll_shots();
            }
            (Z_SHOTS, Act::Right) if self.idx + 1 < shots => {
                self.move_focus(Z_SHOTS, self.idx + 1);
                self.scroll_shots();
            }
            (Z_SHOTS, Act::Up) => {
                self.move_focus(Z_HUB_DETAILS, 0);
                self.scroll_hub();
            }
            (Z_SHOTS, Act::Down) => {
                self.move_focus(Z_DESC, 0);
                self.scroll_hub();
            }
            (Z_SHOTS, Act::Confirm) => self.open_viewer(self.idx as usize),
            (Z_DESC, Act::Up) => {
                if self.idx > 0 {
                    self.idx -= 1;
                    self.scroll_hub();
                } else if shots > 0 {
                    self.move_focus(Z_SHOTS, 0);
                    self.scroll_hub();
                } else {
                    self.move_focus(Z_HUB_DETAILS, 0);
                    self.scroll_hub();
                }
            }
            (Z_DESC, Act::Down | Act::PageDown) => {
                self.idx += 1;
                self.scroll_hub();
            }
            _ => {}
        }
    }

    fn act_viewer(&mut self, a: Act) {
        let n = self.hub_shots.len().max(1);
        match a {
            Act::Left => {
                self.viewer_i = (self.viewer_i + n - 1) % n;
                self.push_viewer();
                audio::play(Sound::Move);
            }
            Act::Right => {
                self.viewer_i = (self.viewer_i + 1) % n;
                self.push_viewer();
                audio::play(Sound::Move);
            }
            Act::Back | Act::Confirm => self.back(),
            _ => {}
        }
    }

    fn act_menu(&mut self, a: Act) {
        let n = self.menu_actions.len() as i32;
        match a {
            Act::Up if self.idx > 0 => self.move_focus(Z_MENU, self.idx - 1),
            Act::Down if self.idx + 1 < n => self.move_focus(Z_MENU, self.idx + 1),
            Act::Back | Act::Options => self.back(),
            Act::Confirm => {
                let act = self.menu_actions.get(self.idx as usize).map(|a| a.id);
                let t = self.menu_target;
                self.back();
                if let (Some(act), Some(t)) = (act, t) {
                    self.run_action(act, t);
                }
            }
            _ => {}
        }
    }

    pub fn back(&mut self) {
        if self.overlay == Overlay::Trailer {
            crate::trailer::close();
            self.trailer_timer = None;
            self.ui().set_trailer_frame(slint::Image::default());
        }
        if self.overlay != Overlay::None {
            if self.overlay == Overlay::Downloads {
                self.download_pending = None;
                self.install_pending = None;
                self.ui().set_download_consent(false);
                self.ui().invoke_focus_root();
            }
            audio::play(Sound::Back);
            if self.overlay == Overlay::Settings && self.edit_index >= 0 {
                self.finish_edit(None);
            }
            let (ov, zone, idx) = self.stack.pop().unwrap_or((Overlay::None, if self.view == 0 { Z_ROW } else { Z_GRID }, 0));
            self.overlay = ov;
            self.ui().set_overlay(ov as i32);
            self.set_focus(zone, idx);
            self.release_toasts();
            if ov == Overlay::None {
                self.ui().set_hub_open(false);
                self.hub = None;
                self.push_hero();
                self.push_row();
                if self.view == 0 {
                    self.show_row_background(self.sel, true);
                } else {
                    self.focus_card_changed();
                }
            } else if ov == Overlay::Hub {
                self.push_hub();
            }
            return;
        }
        if self.view == 1 {
            if !self.query.is_empty() && self.zone == Z_SEARCH {
                self.ui().set_query("".into());
                self.on_search(String::new());
                return;
            }
            self.switch_view(0, true);
            audio::play(Sound::Back);
            return;
        }
        if self.zone != Z_ROW {
            self.move_focus(self.home_zone(), 0);
        } else if self.sel != 0 {
            self.select_tile(0);
        }
    }

    pub fn push_overlay(&mut self, ov: Overlay, zone: i32, idx: i32) {
        self.stack.push((self.overlay, self.zone, self.idx));
        self.overlay = ov;
        let ui = self.ui();
        ui.set_overlay(ov as i32);
        ui.set_hub_open(self.hub.is_some() && (ov == Overlay::Hub || self.stack.iter().any(|s| s.0 == Overlay::Hub)));
        self.set_focus(zone, idx);
    }

    pub fn switch_view(&mut self, v: i32, focus: bool) {
        if self.view != v {
            audio::play(Sound::Select);
        }
        self.view = v;
        let ui = self.ui();
        ui.set_view(v);
        ui.set_bg_dim(v == 1);
        if v == 1 {
            self.push_library();
            if focus {
                if self.filtered.is_empty() {
                    self.set_focus(Z_SEARCH, 0);
                } else {
                    let first = self.first_visible_card();
                    self.set_focus(Z_GRID, first as i32);
                    self.focus_card_changed();
                }
            }
        } else {
            if self.art_pending {
                self.apply_art();
            }
            self.show_row_background(self.sel, true);
            if focus {
                self.set_focus(self.home_zone(), 0);
            }
        }
    }

    pub fn select_tile(&mut self, i: i64) {
        if self.row.is_empty() {
            return;
        }
        let i = i.clamp(0, self.row.len() as i64 - 1) as usize;
        if i == self.sel {
            return;
        }
        self.sel = i;
        audio::play(Sound::Move);
        self.ui().set_sel(i as i32);
        self.row_flip = true;
        self.push_row_text();
        self.hero_flip = true;
        self.push_hero();
        self.show_row_background(i, false);
        self.prefetch_neighbors();
        self.schedule_rest_prefetch();
    }

    pub fn row_target(&self, i: usize) -> Target {
        match self.row.get(i) {
            Some(RowItem::Local(l)) => Target { game: self.locals[*l].cat, local: Some(*l) },
            Some(RowItem::Cat(g)) => Target { game: Some(*g), local: self.games[*g].local },
            _ => Target { game: None, local: None },
        }
    }

    fn focused_target(&self) -> Option<Target> {
        let t = match (self.view, self.zone) {
            (0, Z_ROW | Z_ACTIONS) => self.row_target(self.sel),
            (1, Z_GRID) => {
                let g = *self.filtered.get(self.idx as usize)?;
                Target { game: Some(g), local: self.games[g].local }
            }
            _ => return None,
        };
        (t.game.is_some() || t.local.is_some()).then_some(t)
    }

    fn activate_row(&mut self) {
        match self.row.get(self.sel).copied() {
            Some(RowItem::All) => self.switch_view(1, true),
            Some(RowItem::Local(l)) => {
                if self.session_for_local(l).is_some() {
                    audio::play(Sound::Select);
                    self.resume_game();
                } else {
                    self.launch(l);
                }
            }
            Some(RowItem::Cat(g)) => {
                audio::play(Sound::Select);
                self.open_hub(Target { game: Some(g), local: self.games[g].local });
            }
            None => {}
        }
    }

    pub fn session_for_local(&self, l: usize) -> Option<&Session> {
        let lv = &self.locals[l];
        self.live.iter().find(|s| s.game_id == lv.l.id || (!lv.l.title_id.is_empty() && s.title_id == lv.l.title_id))
    }

    pub fn current_session(&self) -> Option<&Session> {
        match self.row.get(self.sel) {
            Some(RowItem::Local(l)) => self.session_for_local(*l),
            Some(RowItem::Cat(g)) => self.games[*g].local.and_then(|l| self.session_for_local(l)),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ actions

    pub fn actions_for(&self, t: Target, in_hub: bool) -> Vec<ActionDef> {
        let mut a = Vec::new();
        let mk = |id, label: &str, icon, primary| ActionDef { id, label: label.into(), icon, primary, danger: false, round: false };
        // Its trailer is the game's; its store page stays this version's (game_info keeps both so).
        let info = self.game_info(t);
        if let Some(l) = t.local {
            if self.session_for_local(l).is_some() {
                a.push(mk("resume", "Resume", "play", true));
                a.push(ActionDef { danger: true, ..mk("stop", "Stop", "stop", false) });
            } else {
                a.push(mk("play", "Play", "play", true));
            }
        }
        if !in_hub && (t.game.is_some() || t.local.is_some()) {
            a.push(mk("hub", if t.local.is_some() { "Game Hub" } else { "View Game" }, "info", t.local.is_none()));
        }
        let pkg = t.game.is_some_and(|g| self.games[g].g.is_pkg());
        if t.local.is_none() && pkg && self.download_for_target(t).is_none() {
            a.push(mk("pkg_info", "PKG release", "info", false));
        } else if t.local.is_none() && t.game.is_some_and(|g| !self.games[g].g.magnet.is_empty()) {
            let job = self.download_for_target(t);
            let installing = job.as_ref().is_some_and(|job| self.installer.snapshot().iter().any(|record| record.key == job.key && record.state.active()));
            // A game that doesn't start (or can't on this PC) can still be downloaded, but it
            // isn't offered as the obvious next step.
            let discouraged = job.is_none() && self.outlook(t).is_some_and(|o| o.discourages_download());
            let (id, label) = if installing { ("download", "Installing…") }
                else if job.as_ref().is_some_and(|job| job.state == crate::downloads::State::Complete) { ("install", "Install") }
                else if job.is_some() { ("download", "View download") }
                else if discouraged { ("download", "Download anyway") }
                else { ("download", "Download") };
            a.push(mk(id, label, "disk", a.is_empty() && !discouraged));
        }
        let discouraged = a.iter().any(|x| x.label == "Download anyway");
        if crate::trailer::available() && info.as_ref().is_some_and(|i| !i.video.is_empty()) {
            // Ahead of "Download anyway", so the Hub doesn't open with the download selected.
            let at = if discouraged { a.iter().position(|x| x.id == "download").unwrap_or(a.len()) } else { a.len() };
            a.insert(at, mk("trailer", "Trailer", "film", false));
        }
        // Its store page: Steam for PC games, the PlayStation Store for console ones. (The forum
        // topic said nothing the Game Hub doesn't.)
        if in_hub && info.as_ref().is_some_and(|i| !i.store.is_empty()) {
            let label = if self.target_platform(t) == crate::platform::Platform::Pc { "Steam page" } else { "PlayStation Store" };
            a.push(ActionDef { round: true, ..mk("store", label, "bag", false) });
        }
        if !a.iter().any(|x| x.primary) && !discouraged {
            if let Some(f) = a.first_mut() {
                f.primary = true;
            }
        }
        a
    }

    pub fn target_info(&self, t: Target) -> Option<Info> {
        t.local.and_then(|l| self.locals[l].info.clone()).or_else(|| t.game.and_then(|g| self.games[g].info.clone()))
    }

    pub fn run_action(&mut self, id: &str, t: Target) {
        match id {
            "play" => {
                if let Some(l) = t.local {
                    self.launch(l);
                }
            }
            "resume" => {
                audio::play(Sound::Select);
                self.resume_game();
            }
            "stop" => self.stop_game(t.local),
            "hub" => {
                audio::play(Sound::Select);
                self.open_hub(t);
            }
            "trailer" => self.play_trailer(t),
            "download" => self.prepare_download(t),
            "pkg_info" => {
                let others = t.game.map(|g| self.release_order(g).iter().filter(|i| !self.games[**i].g.is_pkg()).count()).unwrap_or(0);
                let tip = if others > 0 { "This game has another release that isn't a PKG: switch to it with the release selector." } else { "Look for a release of this game that is a folder, an archive or an exFAT image." };
                self.toast("This release is a PS5 PKG package", &format!("PS5 games install from extracted releases only, not PKG packages. {tip}"), 0);
            }
            "install" => {
                if let Some(job) = self.download_for_target(t) { self.prepare_install(&job.key); }
            }
            "store" => {
                if let Some(i) = self.target_info(t).filter(|i| !i.store.is_empty()) {
                    open_url(&i.store);
                    let page = if self.target_platform(t) == crate::platform::Platform::Pc { "The Steam page" } else { "The PlayStation Store page" };
                    self.toast("Opened in your browser", &format!("{page} is in your browser."), 1);
                }
            }
            "folder" => {
                if let Some(l) = t.local {
                    open_url(&self.locals[l].l.path.to_string_lossy());
                }
            }
            "log" => {
                if let Some(l) = t.local {
                    let lg = &self.locals[l].l;
                    let p = util::cache_dir().join("logs").join(format!("{}.log", if lg.title_id.is_empty() { &lg.id } else { &lg.title_id }));
                    if p.is_file() {
                        open_url(&p.to_string_lossy());
                    } else {
                        self.toast("No emulator log yet", "Play the game once to create one.", 0);
                    }
                }
            }
            "settings" => self.open_settings(),
            // Straight to the row that adds it, not the top of Settings.
            "add_folder" => self.open_setting(crate::settings::SId::Dirs),
            "library" => self.switch_view(1, true),
            "rate" => {
                if let Some(l) = t.local {
                    self.open_rating(l, false);
                }
            }
            "rate_ingame" | "rate_menu" | "rate_logo" | "rate_noboot" | "rate_skip" => {
                if let Some(l) = t.local {
                    use crate::compat::Status;
                    let status = match id {
                        "rate_ingame" => Some(Status::InGame),
                        "rate_menu" => Some(Status::MainMenu),
                        "rate_logo" => Some(Status::Logo),
                        "rate_noboot" => Some(Status::DoesntBoot),
                        _ => None,
                    };
                    self.rate(l, status);
                }
            }
            "compat" => {
                let platform = self.target_platform(t);
                if platform == crate::platform::Platform::Pc {
                    let appid = t.game.and_then(|g| self.games[g].g.steam.as_ref()).map(|s| s.appid);
                    open_url(&appid.map(|a| format!("https://www.protondb.com/app/{a}")).unwrap_or_else(|| crate::compat::list_page(platform).into()));
                    self.toast("Opened in your browser", "ProtonDB: how well it runs on Linux, reported by players.", 1);
                } else {
                    open_url(crate::compat::list_page(platform));
                    self.toast("Opened in your browser", &format!("The {} compatibility list is in your browser.", platform.emulator()), 1);
                }
            }
            "start_with" => {
                let Some(l) = t.local else { return };
                let game = self.locals[l].l.clone();
                match crate::pcgame::rotate_launch(&game.path) {
                    Ok(m) => {
                        self.toast("Play now starts", &crate::pcgame::short_path(&m.launch, &m.alternatives.last().cloned().unwrap_or_default()), 1);
                        self.rescan_library();
                    }
                    Err(e) => self.toast("Couldn't change it", &e, 2),
                }
            }
            _ => {}
        }
    }

    pub fn launch(&mut self, l: usize) {
        if let Some(s) = self.live.first() {
            let name = s.name.clone();
            self.toast(&format!("{name} is already running"), "Stop it first, or choose Resume to go back to it.", 2);
            audio::play(Sound::Error);
            return;
        }
        let game = self.locals[l].l.clone();
        // PS4 games run on shadPS4, which is installed the first time one is played.
        if game.platform == crate::platform::Platform::Ps4 && !crate::shad::installed() {
            self.shad_install_then_launch(game.id.clone());
            return;
        }
        match self.sessions.launch(&game) {
            Ok(()) => {
                audio::play(Sound::Start);
                self.live = self.sessions.live();
                self.show_launch_splash(l);
                self.build_row();
                self.push_row();
                self.push_hero();
                if self.overlay == Overlay::Hub {
                    self.push_hub();
                }
            }
            Err(e) => {
                audio::play(Sound::Error);
                self.toast("Couldn't start the game", &e, 2);
            }
        }
    }

    pub fn stop_game(&mut self, local: Option<usize>) {
        let s = match local {
            Some(l) => self.session_for_local(l).cloned(),
            None => self.live.first().cloned(),
        };
        if let Some(s) = s {
            audio::play(Sound::Back);
            let platform = self.local_platform(&s.game_id);
            let sub = if platform == crate::platform::Platform::Pc { "Closing the game.".to_string() }
                else { format!("Closing the game and saving {}'s caches.", platform.emulator()) };
            self.toast_game(&format!("Stopping {}…", s.name), &sub, 0, &s.game_id.clone());
            self.sessions.stop(s.pid);
        }
    }

    pub fn resume_game(&mut self) {
        if !self.sessions.resume() {
            self.toast("Couldn't switch to the game", "Its window wasn't found. Resume needs xdotool installed.", 2);
        }
    }

    /// Play a game's official trailer inside the launcher.
    pub fn play_trailer(&mut self, t: Target) {
        let Some(url) = self.game_info(t).map(|i| i.video.clone()).filter(|v| !v.is_empty()) else { return };
        if !crate::trailer::available() {
            return;
        }
        let weak = self.ui().as_weak();
        let opened = crate::trailer::open(&url, move |frame| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                // A frame can arrive just after the player closed: ignore it then.
                if let Some(ui) = weak.upgrade().filter(|ui| ui.get_overlay() == Overlay::Trailer as i32) {
                    ui.set_trailer_frame(slint::Image::from_rgba8(frame));
                }
            });
        });
        if let Err(e) = opened {
            self.toast("Can't play the trailer", &e, 2);
            return;
        }
        audio::play(Sound::Select);
        let ui = self.ui();
        ui.set_trailer_title(target_name(self, t).into());
        ui.set_trailer_progress(0.0);
        ui.set_trailer_time("".into());
        ui.set_trailer_paused(false);
        ui.set_trailer_loading(true);
        ui.set_trailer_frame(slint::Image::default());
        self.show_trailer_controls();
        self.push_overlay(Overlay::Trailer, Z_TRAILER, 0);
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), || with_app(|app| app.trailer_tick()));
        self.trailer_timer = Some(timer);
    }

    fn show_trailer_controls(&mut self) {
        self.trailer_controls_until = util::now_secs() + 3.0;
        self.ui().set_trailer_controls(true);
    }

    fn trailer_tick(&mut self) {
        let Some(status) = crate::trailer::poll() else {
            if self.overlay == Overlay::Trailer { self.back(); }
            return;
        };
        if let Some(ended) = status.ended {
            if self.overlay == Overlay::Trailer { self.back(); }
            if let Err(e) = ended {
                self.toast("Can't play the trailer", &e, 2);
            }
            return;
        }
        let ui = self.ui();
        let clock = |s: f64| { let s = s.max(0.0) as u64; format!("{}:{:02}", s / 60, s % 60) };
        ui.set_trailer_progress(if status.duration > 0.0 { (status.position / status.duration) as f32 } else { 0.0 });
        ui.set_trailer_time(if status.duration > 0.0 { format!("{} / {}", clock(status.position), clock(status.duration)) } else { String::new() }.into());
        ui.set_trailer_paused(status.paused);
        ui.set_trailer_loading(status.loading);
        // Controls stay up while paused or loading, and fade a few seconds after the last input.
        ui.set_trailer_controls(status.paused || status.loading || util::now_secs() < self.trailer_controls_until);
    }

    fn act_trailer(&mut self, a: Act) {
        match a {
            Act::Confirm => crate::trailer::toggle_pause(),
            Act::Left => crate::trailer::seek(-10.0, false),
            Act::Right => crate::trailer::seek(10.0, false),
            Act::Back | Act::Trailer => return self.back(),
            _ => {}
        }
        self.show_trailer_controls();
        self.trailer_tick();
    }

    /// Mouse input on the trailer player.
    pub fn trailer_action(&mut self, action: &str) {
        if self.overlay != Overlay::Trailer {
            return;
        }
        match action {
            "toggle" => crate::trailer::toggle_pause(),
            "close" => return self.back(),
            _ => {
                if let Some(fraction) = action.strip_prefix("seek:").and_then(|f| f.parse::<f64>().ok()) {
                    if let Some(status) = crate::trailer::poll().filter(|s| s.duration > 0.0) {
                        crate::trailer::seek(fraction.clamp(0.0, 1.0) * status.duration, true);
                    }
                }
            }
        }
        self.show_trailer_controls();
        self.trailer_tick();
    }

    pub fn open_hub(&mut self, t: Target) {
        self.ui().set_hub_details_open(false);
        if *crate::images::DEBUG {
            let ui = self.ui();
            slint::Timer::single_shot(Duration::from_millis(1500), move || {
                with_app(|app| {
                    let ui = app.ui();
                    crate::log!("hub content-h {} col-h {} view-h {}", ui.get_hub_content_h(), ui.get_hub_col_h(), ui.get_hub_view_h());
                })
            });
            let _ = ui;
        }
        if *crate::images::DEBUG {
            crate::log!("t={:>6} open hub", crate::images::START.elapsed().as_millis());
        }
        self.hub = Some(t);
        self.hub_actions = self.actions_for(t, true);
        self.hub_shots = self.game_info(t).map(|i| i.shots.clone()).unwrap_or_default();
        self.ui().set_hub_y(0.0);
        self.ui().set_shots_x(0.0);
        if self.overlay == Overlay::Hub {
            self.set_focus(Z_HUB, 0);
        } else {
            self.push_overlay(Overlay::Hub, Z_HUB, 0);
        }
        // The hub uses the game's hub art as its background.
        let (bg, cover) = (self.target_bg_url(t), self.cover_req(t));
        self.want_background_or(bg, true, cover);
        self.push_hub();
    }

    pub fn cycle_hub_release(&mut self, delta: i32) {
        if self.overlay != Overlay::Hub { return; }
        let Some(t) = self.hub else { return; };
        let Some(current) = t.game else { return; };
        let members = self.release_order(current);
        if members.len() < 2 { return; }
        let position = members.iter().position(|i| *i == current).unwrap_or(0) as i32;
        let next = members[(position + delta).rem_euclid(members.len() as i32) as usize];
        self.open_hub(Target { game: Some(next), local: self.games[next].local.or(t.local) });
        self.set_focus(Z_RELEASES, 0);
    }

    fn open_viewer(&mut self, i: usize) {
        if self.hub_shots.is_empty() {
            return;
        }
        self.viewer_i = i;
        audio::play(Sound::Select);
        self.push_overlay(Overlay::Viewer, Z_SHOTS, i as i32);
        self.push_viewer();
    }

    pub fn open_menu(&mut self, t: Target) {
        let mut items: Vec<ActionDef> = Vec::new();
        let mk = |id: &'static str, label: &str, icon: &'static str| ActionDef { id, label: label.into(), icon, primary: false, danger: false, round: false };
        for a in self.actions_for(t, true) {
            let label = match a.id {
                "store" if self.target_platform(t) != crate::platform::Platform::Pc => "PlayStation Store page".to_string(),
                "trailer" => "Watch trailer".to_string(),
                _ => a.label.clone(),
            };
            items.push(ActionDef { label, ..a });
        }
        if self.overlay != Overlay::Hub {
            items.insert(items.iter().position(|a| a.id == "trailer").unwrap_or(items.len()).min(if t.local.is_some() { 1 } else { 0 }), mk("hub", "Game Hub", "info"));
        }
        let pc = self.target_platform(t) == crate::platform::Platform::Pc;
        if let Some(l) = t.local {
            items.push(mk("folder", "Open game folder", "folder"));
            items.push(mk("log", if pc { "View game log" } else { "View emulator log" }, "log"));
            if pc {
                // When the launcher picked the wrong thing to start: the next candidate.
                if let Some(m) = self.locals[l].l.pc.as_ref().filter(|m| !m.alternatives.is_empty()) {
                    items.push(mk("start_with", &format!("Start with {}", crate::pcgame::short_path(&m.alternatives[0], &m.launch)), "play"));
                }
            } else {
                items.push(mk("rate", "Rate how it runs", "star"));
            }
        }
        let compat = match self.target_platform(t) {
            crate::platform::Platform::Ps4 => "shadPS4 compatibility list",
            crate::platform::Platform::Ps5 => "KytyPS5 compatibility list",
            crate::platform::Platform::Pc => "ProtonDB page",
        };
        items.push(mk("compat", compat, "web"));
        items.push(mk("settings", "Settings", "gear"));
        let title = t.local.map(|l| self.locals[l].name.clone()).or_else(|| t.game.map(|g| self.games[g].name.clone())).unwrap_or_default();
        self.menu_target = Some(t);
        self.menu_actions = items;
        audio::play(Sound::Select);
        self.push_overlay(Overlay::Menu, Z_MENU, 0);
        self.push_menu(&title);
    }

    /// "Starting…" splash that stays up until the game's window actually appears (or the game
    /// exits), instead of for a fixed time while KytyPS5 is still booting in the background.
    /// Which controller games will get: shown when a game starts and on the controls screen.
    pub fn push_controller_status(&mut self) {
        let ui = self.ui();
        ui.set_launch_keyboard(crate::gamepad::connected().is_empty());
        ui.set_launch_controller(crate::gamepad::status().into());
    }

    /// Settings → Controls: the keyboard layout games use without a controller.
    pub fn open_controls(&mut self) {
        self.push_controller_status();
        audio::play(Sound::Select);
        self.push_overlay(Overlay::Controls, Z_CONTROLS, 0);
    }

    fn show_launch_splash(&mut self, l: usize) {
        self.push_launch(l);
        let platform = self.locals[l].l.platform;
        self.ui().set_launch_sub(if platform == crate::platform::Platform::Pc { "Starting…".into() } else { format!("Starting with {}…", platform.emulator()).into() });
        self.ui().set_launch_ps4(platform == crate::platform::Platform::Ps4);
        self.push_controller_status();
        self.push_overlay(Overlay::Launch, self.zone, self.idx);
        let Some(pid) = self.session_for_local(l).map(|s| s.pid) else {
            slint::Timer::single_shot(Duration::from_millis(2000), || with_app(|app| app.hide_launch_splash()));
            return;
        };
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut noted = false;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let el = start.elapsed();
                let gone = !std::path::Path::new(&format!("/proc/{pid}")).exists();
                let shown = el > Duration::from_millis(1200) && !crate::sessions::windows_of_pid(pid).is_empty();
                if gone || shown || el > Duration::from_secs(90) {
                    // A moment more once the window exists, so the game's first frame is ready.
                    if shown {
                        std::thread::sleep(Duration::from_millis(400));
                        // On a desktop the window manager already brought it up; Gamescope needs telling.
                        crate::sessions::activate_pid_window(pid);
                    }
                    post(|app| app.hide_launch_splash());
                    return;
                }
                if !noted && el > Duration::from_secs(8) {
                    noted = true;
                    post(|app| app.ui().set_launch_sub("Still starting… a game's first launch can take a while".into()));
                }
            }
        });
    }

    pub fn hide_launch_splash(&mut self) {
        if self.overlay == Overlay::Launch {
            let (ov, z, i) = self.stack.pop().unwrap_or((Overlay::None, Z_ROW, 0));
            self.overlay = ov;
            self.ui().set_overlay(ov as i32);
            self.set_focus(z, i);
        }
    }

    // ------------------------------------------------------------------ settings

    pub fn open_settings(&mut self) {
        audio::play(Sound::Select);
        self.sys_poll();
        self.build_settings();
        let first = self.settings_rows.iter().position(|r| r.kind != 0).unwrap_or(0) as i32;
        self.ui().set_settings_y(0.0);
        self.push_overlay(Overlay::Settings, Z_SETTINGS, first);
        self.push_settings();
    }

    fn act_settings(&mut self, a: Act) {
        let n = self.settings_rows.len() as i32;
        let focusable = |rows: &Vec<crate::SettingData>, i: i32| i >= 0 && i < rows.len() as i32 && rows[i as usize].kind != 0;
        match a {
            Act::Back => self.back(),
            Act::Up | Act::Down => {
                let step = if a == Act::Up { -1 } else { 1 };
                let mut j = self.idx + step;
                while j >= 0 && j < n && !focusable(&self.settings_rows, j) {
                    j += step;
                }
                if focusable(&self.settings_rows, j) {
                    self.move_focus(Z_SETTINGS, j);
                    self.scroll_settings();
                }
            }
            Act::Left | Act::Right => self.settings_change(self.idx as usize, if a == Act::Left { -1 } else { 1 }),
            Act::Confirm => self.settings_activate(self.idx as usize),
            _ => {}
        }
    }

    pub fn finish_edit(&mut self, text: Option<String>) {
        let i = self.edit_index;
        if i < 0 {
            return;
        }
        self.edit_index = -1;
        let ui = self.ui();
        ui.set_edit_index(-1);
        ui.invoke_focus_root();
        if let Some(t) = text {
            self.settings_commit_text(i as usize, t);
        }
        self.build_settings();
        self.push_settings();
    }

    // ------------------------------------------------------------------ toasts / status

    pub fn toast(&mut self, text: &str, sub: &str, kind: i32) {
        self.toast_full(text, sub, kind, None, false);
    }

    /// Notification from the launcher itself (updates): its icon, and its name above the title.
    pub fn toast_app(&mut self, text: &str, sub: &str, kind: i32) {
        let mut t = toast_data(text, sub, kind);
        (t.app, t.kicker) = (true, "AZRAK LAUNCHER".into());
        self.notify(t);
    }

    /// Notification with a game's tile as its icon.
    pub fn toast_game(&mut self, text: &str, sub: &str, kind: i32, game_id: &str) {
        let img = self.game_tile(game_id);
        self.toast_full(text, sub, kind, img, false);
    }

    fn game_tile(&mut self, game_id: &str) -> Option<slint::Image> {
        self.locals.iter().position(|l| l.l.id == game_id)
            .and_then(|l| self.tile_req(RowItem::Local(l)))
            .and_then(|r| self.images.get(&r.key))
    }

    /// A notification that does `action` when clicked (see `toast_clicked`), with a button saying
    /// what that is.
    pub fn toast_action(&mut self, text: &str, sub: &str, kind: i32, action: &str, button: &str) {
        self.toast_with(text, sub, kind, None, false, action, button);
    }

    pub fn toast_game_action(&mut self, text: &str, sub: &str, kind: i32, game_id: &str, action: &str, button: &str) {
        let img = self.game_tile(game_id);
        self.toast_with(text, sub, kind, img, false, action, button);
    }

    pub fn toast_full(&mut self, text: &str, sub: &str, kind: i32, image: Option<slint::Image>, app: bool) {
        self.toast_with(text, sub, kind, image, app, "", "");
    }

    #[allow(clippy::too_many_arguments)]
    pub fn toast_with(&mut self, text: &str, sub: &str, kind: i32, image: Option<slint::Image>, app: bool, action: &str, button: &str) {
        let mut t = toast_data(text, sub, kind);
        (t.has_image, t.image, t.app, t.action, t.button) = (image.is_some(), image.unwrap_or_default(), app, action.into(), button.into());
        self.notify(t);
    }

    /// News about the launcher's own update; clicking it goes to Settings → Updates → Azrak
    /// Launcher, where it's installed or restarted into.
    pub fn toast_app_update(&mut self, text: &str, sub: &str, kind: i32, button: &str) -> ToastData {
        let mut t = toast_data(text, sub, kind);
        (t.app, t.kicker, t.action, t.button) = (true, "AZRAK LAUNCHER".into(), "app-update".into(), button.into());
        t
    }

    /// The new version is installed: one click from the button that starts it. (Ready while the
    /// start screen is up, it's told when the Home screen shows: see `boot_finish`.)
    pub fn toast_update_ready(&mut self, version: &str) {
        if self.boot.active { return; }
        let mut t = self.toast_app_update(&format!("Version {version} is ready"), "Restart the launcher to start using it.", 1, "Restart in Settings");
        // Worth more than a glance: it stays a while.
        t.life = 20_000;
        self.notify(t);
    }

    /// Clicking a notification does its action (if it has one) and closes it; once, even if
    /// it's clicked again while it glides away.
    pub fn toast_clicked(&mut self, id: i32, action: &str) {
        if !self.toasts.iter().any(|t| t.id == id && !t.leaving) {
            return;
        }
        self.toast_dismiss(id);
        if !action.is_empty() {
            self.clicks_after = Some(std::time::Instant::now() + Duration::from_millis(500));
        }
        if let Some(path) = action.strip_prefix("log:") {
            open_url(path);
            return;
        }
        match action {
            "downloads" => self.open_downloads(None),
            "share" => self.open_share_setting(),
            "this-pc" => self.open_setting(crate::settings::SId::ThisPc),
            "app-update" => self.open_setting(crate::settings::SId::AppUpdate),
            _ => {}
        }
    }

    /// A card that asks something (Join Wi-Fi, Restart now?) is open.
    pub fn modal_open(&self) -> bool {
        matches!(self.overlay, Overlay::WifiJoin | Overlay::Power)
    }

    /// Show the notifications held while a card was open.
    fn release_toasts(&mut self) {
        if !self.modal_open() {
            for t in std::mem::take(&mut self.held_toasts) {
                self.notify(t);
            }
        }
    }

    pub fn notify(&mut self, mut t: ToastData) {
        // A card is a question being answered: news can wait until it closes, errors can't.
        if self.modal_open() && t.kind != 2 {
            self.held_toasts.push(t);
            return;
        }
        self.toast_seq += 1;
        let id = self.toast_seq;
        t.id = id;
        if t.life == 0 {
            // Long enough to read: ~4 s plus reading time for the message; errors stay longer.
            let chars = (t.text.chars().count() + t.sub.chars().count()) as i32;
            t.life = (3500 + chars * 45).clamp(4000, 10000).max(if t.kind == 2 { 8000 } else { 0 });
        }
        let life = t.life as u64;
        self.toasts.push(t);
        // Three at most: the oldest makes room.
        if let Some(oldest) = self.toasts.iter().filter(|t| !t.leaving).nth(3).and(self.toasts.iter().find(|t| !t.leaving)).map(|t| t.id) {
            self.toast_dismiss(oldest);
        }
        self.push_toasts();
        slint::Timer::single_shot(Duration::from_millis(life), move || with_app(move |app| app.toast_dismiss(id)));
    }

    /// Glide a notification away, then drop it.
    pub fn toast_dismiss(&mut self, id: i32) {
        let Some(t) = self.toasts.iter_mut().find(|t| t.id == id && !t.leaving) else { return };
        t.leaving = true;
        self.push_toasts();
        slint::Timer::single_shot(Duration::from_millis(320), move || {
            with_app(move |app| {
                app.toasts.retain(|t| t.id != id);
                app.push_toasts();
            })
        });
    }

    pub fn set_status(&mut self, text: &str, busy: bool) {
        self.boot_status(text);
        if self.status != text || self.status_busy != busy {
            self.status = text.to_string();
            self.status_busy = busy;
            let ui = self.ui();
            // The welcome screen reads the raw progress; the Home screen's line says what it's for.
            ui.set_status(crate::words::status(text).into());
            ui.set_status_busy(busy);
        }
    }
}

pub fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

pub fn target_name(app: &App, t: Target) -> String {
    t.local.map(|l| app.locals[l].name.clone()).or_else(|| t.game.map(|g| app.games[g].name.clone())).unwrap_or_default()
}

/// A notification's text, with nothing else yet (see `App::notify`).
pub fn toast_data(text: &str, sub: &str, kind: i32) -> ToastData {
    ToastData { text: text.into(), sub: sub.into(), kind, ..Default::default() }
}

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}


/// The emulator build a game runs on, recorded with its rating: KytyPS5's for PS5 games,
/// shadPS4's release for PS4 games. Can take a moment for a custom KytyPS5: call off the UI thread.
fn emulator_version(platform: crate::platform::Platform, kyty: &std::path::Path) -> String {
    match platform {
        crate::platform::Platform::Ps4 => crate::shad::pretty(&crate::shad::load_state().installed),
        crate::platform::Platform::Ps5 => crate::kyty::version_for(kyty),
        crate::platform::Platform::Pc => String::new(),
    }
}

/// Whether the Library lists a release. PS5 PKG packages can't be installed (KytyPS5 runs
/// extracted games), so they aren't, except one you're downloading or have downloaded.
fn listed(g: &Game, downloaded: &HashSet<i64>) -> bool {
    !g.is_pkg() || downloaded.contains(&g.id)
}

/// Finds the catalog release an installed game is: a PC game by the topic it was installed from,
/// a PlayStation game by title ID, else by name, on the same platform only (a PC and a PS5
/// game of the same name are different games).
struct CatalogIndex {
    by_topic: HashMap<i64, usize>,
    by_tid: HashMap<String, usize>,
    by_name: HashMap<(crate::platform::Platform, String), usize>,
}

impl CatalogIndex {
    /// `games`: each release with its display name.
    fn new<'a>(games: impl Iterator<Item = (&'a crate::catalog::Game, &'a str)>) -> Self {
        let mut index = Self { by_topic: HashMap::new(), by_tid: HashMap::new(), by_name: HashMap::new() };
        for (i, (g, display)) in games.enumerate() {
            index.by_topic.insert(g.id, i);
            for id in std::iter::once(&g.title_id).chain(g.title_ids.iter()).filter(|id| !id.is_empty()) {
                index.by_tid.entry(id.clone()).or_insert(i);
            }
            index.by_name.entry((g.platform, norm(display))).or_insert(i);
            index.by_name.entry((g.platform, norm(&g.name))).or_insert(i);
        }
        index
    }

    fn find(&self, l: &LocalGame) -> Option<usize> {
        let topic = l.pc.as_ref().map(|m| m.topic).filter(|t| *t > 0);
        topic.and_then(|t| self.by_topic.get(&t).copied())
            .or_else(|| (!l.title_id.is_empty()).then(|| self.by_tid.get(&l.title_id).copied()).flatten())
            .or_else(|| self.by_name.get(&(l.platform, norm(&l.name))).copied())
    }
}

#[cfg(test)]
mod quit_tests {
    use super::*;

    #[test]
    fn only_ctrl_q_and_ctrl_w_are_quit_keys() {
        assert!(is_quit_key("q", true) && is_quit_key("w", true) && is_quit_key("Q", true));
        // Typing "wolverine" in the search box, or Ctrl+A to select it, is not quitting.
        assert!(!is_quit_key("w", false) && !is_quit_key("q", false));
        assert!(!is_quit_key("a", true) && !is_quit_key("\u{11}", true));
    }

    #[test]
    fn nothing_quits_on_azrakos() {
        assert!(!may_quit(true));
        assert!(may_quit(false));
    }
}

#[cfg(test)]
mod catalog_index_tests {
    use super::*;
    use crate::platform::Platform;

    #[test]
    fn ps5_pkg_releases_are_not_listed_unless_downloaded() {
        let release = |id: i64, title: &str, platform: Platform| crate::catalog::Game { id, title: title.into(), platform, ..Default::default() };
        let none = HashSet::new();
        assert!(!listed(&release(1, "[PS5] Diablo II: Resurrected [EUR / v01.039.000 / Retail / FPKG] [35.21 GB]", Platform::Ps5), &none));
        assert!(listed(&release(2, "[PS5] Stray [EUR / folder]", Platform::Ps5), &none));
        assert!(listed(&release(3, "[PS4] Firewatch [EUR / PKG]", Platform::Ps4), &none), "PS4 PKGs install");
        assert!(listed(&release(1, "[PS5] Diablo II: Resurrected [FPKG]", Platform::Ps5), &HashSet::from([1])), "already downloaded");
    }

    #[test]
    fn installed_games_match_releases_of_their_own_platform() {
        let release = |id: i64, name: &str, platform: Platform| crate::catalog::Game { id, name: name.into(), platform, ..Default::default() };
        let games = [release(1, "Hollow Knight", Platform::Ps5), release(2, "Hollow Knight", Platform::Pc), release(3, "Celeste", Platform::Ps5)];
        let index = CatalogIndex::new(games.iter().map(|g| (g, g.name.as_str())));
        let local = |name: &str, platform: Platform| LocalGame { name: name.into(), platform, ..Default::default() };
        assert_eq!(index.find(&local("Hollow Knight", Platform::Pc)), Some(1));
        assert_eq!(index.find(&local("Hollow Knight", Platform::Ps5)), Some(0));
        assert_eq!(index.find(&local("Celeste", Platform::Pc)), None, "no PC release of it: not the PS5 one");
    }
}

/// When to retry downloads that failed for lack of network. A network that answers GitHub's name
/// but not its downloads mustn't be retried every 20 s: 20 s, 40 s, 80 s … up to 30 min while
/// retries keep failing. Once the network is seen down, or downloads have worked for 5 minutes
/// since the last retry, the next outage is a new one, retried after 20 s again.
#[derive(Default)]
pub struct NetRetry {
    retries: u32,
    next: f64,
    last: f64,
}

impl NetRetry {
    /// Called every 20 s: whether to check the network now (`down`: a download failed for lack of it).
    pub fn due(&mut self, now: f64, down: bool) -> bool {
        if !down && self.retries > 0 && now - self.last >= 300.0 {
            *self = Self::default();
        }
        down && now >= self.next
    }
    /// The network is down (not just the downloads failing).
    pub fn offline(&mut self) {
        *self = Self::default();
    }
    /// It's up: the failed downloads are being retried now.
    pub fn retrying(&mut self, now: f64) {
        self.next = now + (20.0 * 2f64.powi(self.retries.min(7) as i32)).min(1800.0);
        self.retries += 1;
        self.last = now;
    }
}

/// Which artwork a game shows. PC games: Steam's (from the catalog). PlayStation games: Sony's, by
/// title ID; a release without one (new releases from the daily refresh) or one Sony has no art
/// for gets Steam's, when the daily update found the game there, keeping whatever Sony does know
/// (rating, store page). Nothing at all: the Library shows a placeholder with the game's name.
pub fn pick_art(platform: crate::platform::Platform, steam: Option<&crate::catalog::Steam>, sony: Option<&Info>) -> Option<Info> {
    if platform == crate::platform::Platform::Pc {
        return steam.map(|steam| steam.info());
    }
    if sony.is_some_and(|i| !i.master.is_empty()) {
        return sony.cloned();
    }
    match (steam, sony) {
        (Some(steam), Some(s)) => {
            let mut m = steam.info();
            m.rating = s.rating.clone();
            m.age = s.age.clone();
            if !s.store.is_empty() {
                m.store = s.store.clone();
            }
            Some(m)
        }
        (Some(steam), None) => Some(steam.info()),
        (None, sony) => sony.cloned(),
    }
}

#[cfg(test)]
mod net_retry_tests {
    use super::NetRetry;

    #[test]
    fn retries_back_off_while_failing_and_start_over_after_working() {
        let mut n = NetRetry::default();
        assert!(!n.due(0.0, false));
        assert!(n.due(0.0, true));
        // Up, but the retried downloads keep failing: 20 s, 40 s, 80 s.
        n.retrying(0.0);
        assert!(!n.due(19.0, true) && n.due(20.0, true));
        n.retrying(20.0);
        assert!(!n.due(59.0, true) && n.due(60.0, true));
        n.retrying(60.0);
        assert!(!n.due(139.0, true));
        // The last retry worked; ten minutes later the network drops: retried in 20 s, not 160.
        for t in (160..=660).step_by(20) {
            assert!(!n.due(t as f64, false));
        }
        assert!(n.due(680.0, true));
        // Seen down: back to the start too.
        n.retrying(680.0);
        n.offline();
        assert!(n.due(700.0, true));
    }
}

/// Releases with no artwork at all (a new upload whose details aren't read yet, a PS4 game packed
/// for PC) show another release's of the same game, from any console. Only the pictures: the text
/// may be about another edition.
pub fn borrow_art(games: &mut [GameV]) {
    let has_art = has_official_art;
    let mut donors = HashMap::<String, usize>::new();
    for (i, g) in games.iter().enumerate() {
        if !has_art(g) && g.g.cover.is_empty() {
            continue;
        }
        for key in [crate::game_groups::art_key(&g.name), crate::game_groups::art_key(&g.g.name)] {
            if key.is_empty() {
                continue;
            }
            // Official art beats a forum cover.
            let better = donors.get(&key).is_none_or(|&d| !has_art(&games[d]) && has_art(g));
            if better {
                donors.insert(key, i);
            }
        }
    }
    for i in 0..games.len() {
        if games[i].info.is_some() || !games[i].g.cover.is_empty() {
            continue;
        }
        let Some(&d) = [&games[i].name, &games[i].g.name].iter().find_map(|n| donors.get(&crate::game_groups::art_key(n))) else { continue };
        lend_art(games, d, i);
    }
}

fn has_official_art(g: &GameV) -> bool {
    g.info.as_ref().is_some_and(|i| !i.portrait.is_empty() || !i.master.is_empty())
}

/// Release `to` shows `from`'s pictures.
fn lend_art(games: &mut [GameV], from: usize, to: usize) {
    let (donor, platform) = (&games[from], games[to].g.platform);
    let info = donor.info.as_ref().filter(|_| has_official_art(donor)).map(|i| Info {
        source: i.source.clone(), master: i.master.clone(), hub: i.hub.clone(), logo: i.logo.clone(), portrait: i.portrait.clone(),
        bg: i.bg.clone(), banner: i.banner.clone(), ..Default::default()
    });
    // PS4 box art has a blue "PS4" band, cut only from PS4 cards.
    let cover = (info.is_none() && (donor.g.platform != crate::platform::Platform::Ps4 || platform == crate::platform::Platform::Ps4))
        .then(|| donor.g.cover.clone());
    match (games[to].info.as_mut(), info) {
        // Its own store details stay; only the pictures come from the other release.
        (Some(own), Some(art)) => {
            (own.source, own.master, own.hub, own.logo, own.portrait, own.bg, own.banner) =
                (art.source, art.master, art.hub, art.logo, art.portrait, art.bg, art.banner);
        }
        (own, art) => if own.is_none() { games[to].info = art; },
    }
    if let Some(cover) = cover {
        games[to].g.cover = cover;
    }
}

/// A release without official pictures shows another of its game's (by another name: "Hades +
/// Original Soundtrack Bundle" on PC is Hades): official art beats a forum cover, which often
/// doesn't load; a forum cover is only lent to a release with no picture at all.
pub fn borrow_group_art(games: &mut [GameV], groups: &crate::game_groups::Groups) {
    for members in &groups.members {
        // A portrait (the card's shape) first, then any official art, then a forum cover.
        let portrait = |g: &GameV| g.info.as_ref().is_some_and(|i| !i.portrait.is_empty());
        let Some(donor) = members.iter().copied().filter(|&i| has_official_art(&games[i]) || !games[i].g.cover.is_empty())
            .max_by_key(|&i| (portrait(&games[i]), has_official_art(&games[i]), std::cmp::Reverse(i))) else { continue };
        let official = has_official_art(&games[donor]);
        for &i in members {
            if !has_official_art(&games[i]) && (official || games[i].g.cover.is_empty()) {
                lend_art(games, donor, i);
            }
        }
    }
}

#[cfg(test)]
mod art_tests {
    use super::*;
    use crate::platform::Platform;

    #[test]
    fn playstation_art_comes_from_sony_then_steam_pc_art_from_steam() {
        let steam = crate::catalog::Steam { appid: 686060, name: "Mewgenics".into(), assets: crate::catalog::SteamAssets {
            portrait: Some("https://steam/686060/library_600x900_2x.jpg".into()), ..Default::default() }, ..Default::default() };
        let sony = Info { name: "Stray".into(), master: "https://sony/master.png".into(), portrait: "https://sony/portrait.png".into(), ..Default::default() };
        let sony_bare = Info { name: "Mewgenics".into(), store: "https://store.playstation.com/x".into(),
            rating: Some(crate::psn::Rating { score: 4.5, total: 10 }), ..Default::default() };
        // Sony's art wins for PlayStation games.
        assert_eq!(pick_art(Platform::Ps5, Some(&steam), Some(&sony)).unwrap().portrait, "https://sony/portrait.png");
        // No Sony art: Steam's, with Sony's rating and store page kept.
        let mixed = pick_art(Platform::Ps5, Some(&steam), Some(&sony_bare)).unwrap();
        assert_eq!((mixed.portrait.as_str(), mixed.store.as_str()), ("https://steam/686060/library_600x900_2x.jpg", "https://store.playstation.com/x"));
        assert!(mixed.rating.is_some());
        assert_eq!(pick_art(Platform::Ps4, Some(&steam), None).unwrap().source.as_deref(), Some("steam"));
        // Neither: nothing (the Library shows the name instead).
        assert!(pick_art(Platform::Ps5, None, None).is_none());
        // PC games: Steam only, even if a title ID happened to match something.
        assert_eq!(pick_art(Platform::Pc, Some(&steam), Some(&sony)).unwrap().source.as_deref(), Some("steam"));
        assert!(pick_art(Platform::Pc, None, Some(&sony)).is_none());
    }

    #[test]
    fn releases_without_art_borrow_another_releases_pictures() {
        let game = |name: &str, platform: Platform, cover: &str, info: Option<Info>| GameV {
            g: Game { name: name.into(), platform, cover: cover.into(), ..Default::default() }, info, name: name.into(),
            buckets: vec![], search: Default::default(), rel_day: 0, is_new: false, local: None,
        };
        let sony = Info { master: "https://sony/bb.png".into(), short: "About Bloodborne.".into(), ..Default::default() };
        let mut games = vec![
            game("Bloodborne Game of the Year Edition (Complete Edition)", Platform::Ps4, "https://forum/bb-ps4.png", Some(sony)),
            game("Bloodborne: Game of the Year", Platform::Pc, "", None),
            game("Toy Story 3", Platform::Ps4, "https://forum/ts3-ps4.png", None),
            game("Toy Story 3", Platform::Pc, "", None),
            game("Hollow Knight", Platform::Pc, "https://forum/hk.jpg", None),
            game("Hollow Knight", Platform::Ps5, "", None),
            game("Unknown Game", Platform::Ps5, "", None),
        ];
        borrow_art(&mut games);
        let bb = games[1].info.as_ref().unwrap();
        assert_eq!((bb.master.as_str(), bb.short.as_str()), ("https://sony/bb.png", ""), "pictures only");
        assert_eq!(games[3].g.cover, "", "a PS4 band never lands on a PC card");
        assert_eq!(games[5].g.cover, "https://forum/hk.jpg");
        assert!(games[6].info.is_none() && games[6].g.cover.is_empty());
    }
}
