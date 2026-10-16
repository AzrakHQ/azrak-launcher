use launcher::{App, Controller};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

fn main() {
    let app = App::open(launcher::default_home()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(2)
    });
    let ctl = Controller::new(app, host::ctx()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(2)
    });
    let win = launcher_ui::AppWindow::new().expect("window");
    let ctl = Rc::new(RefCell::new(ctl));
    launcher_ui::bind::bind(&win, ctl);
    win.run().expect("run");
}
