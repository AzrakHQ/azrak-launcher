//! Azrak Launcher: a fast, native console-style game launcher for Linux.

mod app;
mod audio;
mod boot;
mod catalog;
mod compat;
mod platform;
mod sfo;
mod shad;
mod shad_ui;
mod pkgx;
mod sandbox;
mod vpn;
mod pc;
mod pc_ui;
mod pcgame;
mod os;
mod exfat;
mod results;
mod trailer;
mod config;
mod display;
mod download_ui;
mod downloads;
mod gamepad;
mod game_groups;
mod images;
mod installer;
mod install_ui;
mod kyty;
mod kyty_ui;
mod update;
mod library;
mod library_layout;
mod present;
mod psn;
mod rename;
mod sessions;
mod settings;
mod util;
mod words;

slint::include_modules!();

const HELP: &str = "\
Azrak Launcher — a console-style game launcher for Linux

USAGE:
    azrak [OPTIONS]

OPTIONS:
    --windowed          Open in a normal window instead of fullscreen
    --monitor <NAME>    Display to use (e.g. DP-2); overrides Settings
    --sync              Reload the local RuTracker catalog on start
    --catalog <PATH>    Use an English RuTracker PS5 JSON snapshot
    --version           Print the version
    -h, --help          Show this help
";

fn main() {
    // `--supports FEATURE`: exit 0 when this launcher has it, so AzrakOS can tell (older releases
    // reject the option). gamescope: it tags its window, so Gamescope shows it; os-install: the
    // USB stick's install screens (the stick's build checks the launcher it bundles has them).
    // See docs/AZRAKOS.md.
    if std::env::args().nth(1).as_deref() == Some("--supports") {
        let known = ["gamescope", "system-settings", "os-install"];
        std::process::exit(if std::env::args().nth(2).is_some_and(|f| known.contains(&f.as_str())) { 0 } else { 1 });
    }
    if std::env::args().nth(1).as_deref() == Some("--probe-gpu") {
        pc::print_gpus();
        return;
    }
    let mut windowed = false;
    let mut monitor: Option<String> = None;
    let mut force_sync = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--windowed" => windowed = true,
            "--monitor" => monitor = args.next(),
            "--catalog" => {
                let Some(path) = args.next().filter(|value| !value.starts_with("--")) else {
                    eprintln!("--catalog requires a JSON file path");
                    std::process::exit(2);
                };
                std::env::set_var("AZRAK_CATALOG_PATH", path);
            }
            "--sync" => force_sync = true,
            "--version" | "-V" => {
                println!("azrak {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            other => {
                eprintln!("unknown option: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    // An install from before the rename (PS5 Launcher) moves over first.
    rename::migrate();
    if force_sync {
        // Keep the last-known-good cache if an external snapshot is invalid.
        if let Err(error) = catalog::sync(&|message| eprintln!("{message}")) {
            eprintln!("Could not reload RuTracker catalog: {error}");
        }
    }

    let cfg = config::Config::load();
    let mons = display::monitors();
    let target = display::pick(&mons, monitor.as_deref().unwrap_or(&cfg.monitor));
    // The UI scales itself to the window (see App::update_scale); keep the toolkit at 1:1 so
    // window sizes are never changed behind our back.
    if std::env::var_os("SLINT_SCALE_FACTOR").is_none() {
        std::env::set_var("SLINT_SCALE_FACTOR", "1");
    }
    // Only the winit backend is compiled in; make sure Slint picks the OpenGL renderer.
    if std::env::var_os("SLINT_BACKEND").is_none() {
        std::env::set_var("SLINT_BACKEND", "winit-femtovg");
    }

    let ui = AppWindow::new().expect("could not create the window (is a graphical session running?)");
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    app::run(ui, mons, target, windowed);
}
