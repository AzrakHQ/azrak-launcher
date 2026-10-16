//! Renders the real UI with the software renderer, no display needed, and writes PPM screenshots to
//! target/shots for review. One test only: a Slint platform can be installed once per process.
#![cfg(all(feature = "toy", feature = "native-pc"))]
use kernel::{CancelToken, RunnerId};
use launcher::{App, Controller};
use launcher_ui::{bind, AppWindow};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Model, PhysicalSize, Rgb8Pixel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use ui_model::{Action, SettingsPage};

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn home_with_games() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let mk = |name: &str, files: &[(&str, &str)]| {
        let p = d.path().join("src").join(name);
        for (f, body) in files {
            let t = p.join(f);
            std::fs::create_dir_all(t.parent().unwrap()).unwrap();
            std::fs::write(t, body).unwrap();
        }
        p
    };
    let app = App::open(d.path().to_path_buf()).unwrap();
    for (n, files) in [
        ("harbour-run", vec![(testkit::toy::MARKER, "Harbour Run")]),
        ("lantern-keep", vec![(testkit::toy::MARKER, "Lantern Keep")]),
        (
            "salt-meridian",
            vec![
                ("game.toml", "name = \"Salt Meridian\"\nexe = \"run\""),
                ("run", "x"),
            ],
        ),
        (
            "night-courier",
            vec![
                ("game.toml", "name = \"Night Courier\"\nexe = \"run\""),
                ("run", "x"),
            ],
        ),
    ] {
        let src = mk(n, &files);
        engines::install::install(
            &app.registry,
            &src,
            &d.path().join("games"),
            &Default::default(),
            &CancelToken::new(),
        )
        .unwrap();
    }
    d
}

fn shot(w: &MinimalSoftwareWindow, name: &str) -> Vec<Rgb8Pixel> {
    slint::platform::update_timers_and_animations();
    w.request_redraw();
    let (mut buf, mut drew) = (vec![Rgb8Pixel::default(); 1280 * 800], false);
    w.draw_if_needed(|r| {
        r.render(&mut buf, 1280);
        drew = true;
    });
    assert!(drew, "{name}: nothing drawn");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
    std::fs::create_dir_all(&dir).unwrap();
    let mut ppm = b"P6\n1280 800\n255\n".to_vec();
    for p in &buf {
        ppm.extend_from_slice(&[p.r, p.g, p.b]);
    }
    std::fs::write(dir.join(format!("{name}.ppm")), ppm).unwrap();
    let distinct: std::collections::HashSet<_> = buf.iter().map(|p| (p.r, p.g, p.b)).collect();
    assert!(
        distinct.len() > 8,
        "{name}: looks blank ({} colours)",
        distinct.len()
    );
    buf
}

#[test]
fn screens_render_from_registry_data() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(window.clone()))).unwrap();
    window.set_size(PhysicalSize::new(1280, 800));

    let d = home_with_games();
    let ctl = Controller::new(App::open(d.path().to_path_buf()).unwrap(), host::ctx()).unwrap();
    assert_eq!(ctl.games.len(), 4);
    let ctl = Rc::new(RefCell::new(ctl));
    let win = AppWindow::new().unwrap();
    win.show().unwrap();
    bind::bind(&win, ctl.clone());

    assert_eq!(
        win.get_chips().row_count(),
        3,
        "All + two systems, from descriptors"
    );
    assert_eq!(win.get_cards().row_count(), 4);
    shot(&window, "1-library");

    let g = ctl
        .borrow()
        .games
        .iter()
        .find(|g| g.name == "Harbour Run")
        .unwrap()
        .game
        .clone();
    ctl.borrow_mut().dispatch(Action::OpenGame(g));
    bind::render(&win, &ctl.borrow());
    assert_eq!(win.get_screen(), 1);
    assert_eq!(win.get_settings().row_count(), 3);
    shot(&window, "2-hub");

    ctl.borrow_mut().dispatch(Action::OpenSettings);
    bind::render(&win, &ctl.borrow());
    assert_eq!(win.get_screen(), 2);
    shot(&window, "3-settings-plugins");

    ctl.borrow_mut()
        .dispatch(Action::OpenSettingsPage(SettingsPage::Runner(
            RunnerId::new("toy-run"),
        )));
    bind::render(&win, &ctl.borrow());
    shot(&window, "4-settings-runner");

    // A fresh install: systems exist but no games yet.
    let empty_home = tempfile::tempdir().unwrap();
    let empty = Controller::new(
        App::open(empty_home.path().to_path_buf()).unwrap(),
        host::ctx(),
    )
    .unwrap();
    bind::render(&win, &empty);
    assert_eq!(win.get_empty_title().as_str(), "No games yet");
    shot(&window, "5-no-games");
}
