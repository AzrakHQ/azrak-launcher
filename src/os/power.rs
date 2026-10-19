//! Restart and shut down (Settings → SYSTEM, and its "Restart to update" row), asked first: a
//! controller's ✕ is pressed by habit, and turning the TV's PC off by mistake isn't something
//! to undo with ○. The card opens on Cancel; once confirmed it shows what's happening and takes
//! no more input until the PC goes.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::os::system;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Power {
    Restart,
    ShutDown,
    /// Restart into the downloaded system update.
    Update,
}

#[derive(Default)]
pub struct PowerAsk {
    pub what: Option<Power>,
    /// The version the update brings (Power::Update).
    pub next: String,
    /// 0: restart / shut down, 1: Cancel.
    pub sel: usize,
    /// Confirmed: waiting for the PC to restart or shut down.
    pub busy: bool,
}

/// The card's two buttons, in order.
const CANCEL: usize = 1;

impl App {
    /// Ask before restarting or shutting down. A running game is closed first, so its progress
    /// is saved: the card doesn't open while one runs.
    pub fn power_ask(&mut self, what: Power) {
        if !self.live.is_empty() {
            self.toast("A game is running", "Close it first, so its progress is saved.", 2);
            return;
        }
        audio::play(Sound::Select);
        let next = self.sys.update.as_ref().and_then(|u| u.staged.clone()).unwrap_or_default();
        self.power = PowerAsk { what: Some(what), next, sel: CANCEL, busy: false };
        self.push_overlay(Overlay::Power, Z_POWER, 0);
        self.power_push();
    }

    pub fn act_power(&mut self, a: Act) {
        if self.power.busy {
            return;
        }
        match a {
            Act::Left | Act::Up if self.power.sel > 0 => self.power.sel -= 1,
            Act::Right | Act::Down if self.power.sel < CANCEL => self.power.sel += 1,
            Act::Confirm => return self.power_choose(self.power.sel),
            Act::Back => return self.back(),
            _ => return,
        }
        audio::play(Sound::Move);
        self.power_push();
    }

    /// A button clicked with the mouse (negative: the backdrop, which cancels).
    pub fn click_power(&mut self, idx: i32) {
        if self.power.busy {
            return;
        }
        if idx < 0 {
            return self.back();
        }
        self.power.sel = (idx as usize).min(CANCEL);
        self.power_choose(self.power.sel);
    }

    fn power_choose(&mut self, sel: usize) {
        let Some(what) = self.power.what else { return self.back() };
        if sel == CANCEL {
            return self.back();
        }
        audio::play(Sound::Select);
        self.power.busy = true;
        self.power_push();
        let restart = what != Power::ShutDown;
        std::thread::spawn(move || {
            let result = if restart { system::restart() } else { system::shut_down() };
            let _ = slint::invoke_from_event_loop(move || with_app(move |app| match result {
                Err(e) => app.power_failed(restart, &e),
                // The session ends within moments. If the PC is still here long after, something
                // held it up: give the screen back rather than leave it waiting forever.
                Ok(()) => slint::Timer::single_shot(std::time::Duration::from_secs(30), move || with_app(move |app| {
                    if app.power.busy {
                        app.power_failed(restart, "It didn't happen. Try again in a moment.");
                    }
                })),
            }));
        });
    }

    fn power_failed(&mut self, restart: bool, why: &str) {
        self.power.busy = false;
        if self.overlay == Overlay::Power {
            self.back();
        }
        self.toast(if restart { "Couldn't restart" } else { "Couldn't shut down" }, why, 2);
    }

    fn power_push(&self) {
        let ui = self.ui();
        let p = &self.power;
        let (title, text, yes) = match (p.what, p.busy) {
            (Some(Power::ShutDown), false) => ("Shut down now?".to_string(), "The PC turns off. Its power button turns it on again.".to_string(), "Shut down"),
            (Some(Power::ShutDown), true) => ("Shutting down…".into(), "See you soon.".into(), ""),
            (Some(Power::Update), false) => ("Restart and update?".into(),
                format!("AzrakOS {} is downloaded. The PC restarts into it and comes back here.", p.next), "Restart and update"),
            (_, false) => ("Restart now?".into(), "The PC restarts and comes back here.".into(), "Restart"),
            (_, true) => ("Restarting…".into(), "The PC comes back here in a moment.".into(), ""),
        };
        ui.set_power_title(title.into());
        ui.set_power_text(text.into());
        ui.set_power_buttons(std::rc::Rc::new(slint::VecModel::from(vec![yes.into(), "Cancel".into()])).into());
        ui.set_power_danger(p.what == Some(Power::ShutDown));
        ui.set_power_sel(p.sel as i32);
        ui.set_power_busy(p.busy);
    }
}
