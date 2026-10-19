//! Joining a secured Wi-Fi network (Settings → Internet): a card with the network, its password
//! and an on-screen keyboard, so a controller is all it takes; a keyboard types straight in.
//! Controller: ✕ presses the key, □ deletes, △ adds a space, L1 shift, R1 symbols, R2 or Options
//! joins, ○ cancels. Keyboard: Enter presses the highlighted key, and typing moves the highlight
//! to Join (so typing then Enter joins); Esc cancels, Tab shows the password.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::os::system;

/// Character rows, ten keys each; the fifth row is ACTIONS, whose widths (in keys) add up to ten.
const LETTERS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm,.-"];
const SYMBOLS: [&str; 4] = ["!@#$%^&*()", "-_=+[]{}\\|", ";:\"<>/?~`€", "£¥§°•«»¿¡ "];
const COLS: usize = 10;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    Shift,
    Layer,
    Space,
    Delete,
    Show,
    Join,
}
const ACTIONS: [(Action, f32); 6] = [
    (Action::Shift, 1.5), (Action::Layer, 1.0), (Action::Space, 3.0), (Action::Delete, 1.5), (Action::Show, 1.5), (Action::Join, 1.5),
];

#[derive(Default)]
pub struct WifiJoin {
    pub ssid: String,
    pub signal: String,
    pub text: String,
    pub show: bool,
    pub shift: bool,
    pub symbols: bool,
    pub row: usize,
    pub col: usize,
    pub busy: bool,
    pub error: String,
}

impl WifiJoin {
    fn char_at(&self, row: usize, col: usize) -> Option<char> {
        let rows = if self.symbols { SYMBOLS } else { LETTERS };
        let c = rows.get(row)?.chars().nth(col)?;
        Some(if self.shift && !self.symbols { c.to_ascii_uppercase() } else { c })
    }

    fn type_char(&mut self, c: char) {
        if self.text.chars().count() < 63 {
            self.text.push(c);
        }
        self.error.clear();
    }
}

impl App {
    /// Settings → Internet: a secured network that isn't saved yet asks for its password here.
    pub fn wifi_join_open(&mut self, ssid: &str, signal: &str) {
        audio::play(Sound::Select);
        self.wj = WifiJoin { ssid: ssid.into(), signal: signal.into(), row: 1, ..Default::default() };
        self.push_overlay(Overlay::WifiJoin, Z_WIFI_JOIN, 0);
        self.wj_push();
    }

    fn wj_close(&mut self) {
        if !self.wj.busy {
            self.back();
        }
    }

    /// Keys while the card is open; true when used (everything is, so nothing reaches the screen
    /// behind it).
    pub fn wifi_join_key(&mut self, text: &str, ctrl: bool, alt: bool) -> bool {
        use slint::platform::Key;
        let k = |key: Key| slint::SharedString::from(key) == text;
        if self.wj.busy {
            return true;
        }
        if k(Key::Escape) {
            self.wj_close();
        } else if k(Key::Return) {
            self.wj_press();
        } else if k(Key::Backspace) {
            self.wj.text.pop();
            self.wj.error.clear();
            self.wj_push();
        } else if k(Key::Tab) {
            self.wj.show = !self.wj.show;
            self.wj_push();
        } else if k(Key::UpArrow) {
            self.act_wifi_join(Act::Up);
        } else if k(Key::DownArrow) {
            self.act_wifi_join(Act::Down);
        } else if k(Key::LeftArrow) {
            self.act_wifi_join(Act::Left);
        } else if k(Key::RightArrow) {
            self.act_wifi_join(Act::Right);
        } else if !ctrl && !alt {
            let mut chars = text.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if !c.is_control() {
                    self.wj.type_char(c);
                    (self.wj.row, self.wj.col) = (4, ACTIONS.len() - 1);
                    self.wj_push();
                }
            }
        }
        true
    }

    /// A key clicked with the mouse: row * 100 + column (negative: the backdrop, which cancels).
    pub fn click_wifi_join(&mut self, idx: i32) {
        if idx < 0 {
            return self.wj_close();
        }
        let (row, col) = ((idx / 100) as usize, (idx % 100) as usize);
        if row <= 4 && col < if row == 4 { ACTIONS.len() } else { COLS } && !self.wj.busy {
            (self.wj.row, self.wj.col) = (row, col);
            self.wj_press();
        }
    }

    pub fn act_wifi_join(&mut self, a: Act) {
        if self.wj.busy {
            return;
        }
        let w = &mut self.wj;
        match a {
            Act::Up if w.row > 0 => {
                if w.row == 4 {
                    w.col = action_to_col(w.col);
                }
                w.row -= 1;
            }
            Act::Down if w.row < 4 => {
                w.row += 1;
                if w.row == 4 {
                    w.col = col_to_action(w.col);
                }
            }
            Act::Left if w.col > 0 => w.col -= 1,
            Act::Right if w.col + 1 < if w.row == 4 { ACTIONS.len() } else { COLS } => w.col += 1,
            Act::Confirm => return self.wj_press(),
            Act::Back => return self.wj_close(),
            Act::Trailer => {
                w.text.pop();
                w.error.clear();
            }
            Act::Search => w.type_char(' '),
            Act::TabPrev => w.shift = !w.shift,
            Act::TabNext => w.symbols = !w.symbols,
            Act::PageDown | Act::Options => return self.wj_join(),
            _ => return,
        }
        audio::play(Sound::Move);
        self.wj_push();
    }

    fn wj_press(&mut self) {
        let w = &mut self.wj;
        if w.row < 4 {
            if let Some(c) = w.char_at(w.row, w.col) {
                w.type_char(c);
                // Shift is for one letter, as on a phone.
                if c.is_alphabetic() {
                    w.shift = false;
                }
            }
        } else {
            match ACTIONS[w.col].0 {
                Action::Shift => w.shift = !w.shift,
                Action::Layer => w.symbols = !w.symbols,
                Action::Space => w.type_char(' '),
                Action::Delete => {
                    w.text.pop();
                }
                Action::Show => w.show = !w.show,
                Action::Join => return self.wj_join(),
            }
        }
        audio::play(Sound::Move);
        self.wj_push();
    }

    fn wj_join(&mut self) {
        if self.wj.text.is_empty() {
            self.wj.error = "Type the password first".into();
            audio::play(Sound::Error);
            return self.wj_push();
        }
        audio::play(Sound::Select);
        self.wj.busy = true;
        self.wj.error.clear();
        self.wj_push();
        let (ssid, password) = (self.wj.ssid.clone(), self.wj.text.clone());
        std::thread::spawn(move || {
            let result = system::wifi_connect(&ssid, Some(&password));
            let _ = slint::invoke_from_event_loop(move || with_app(move |app| {
                app.wj.busy = false;
                match result {
                    Ok(()) => {
                        if app.overlay == Overlay::WifiJoin {
                            app.back();
                        }
                        app.wifi_joined(&ssid);
                    }
                    Err(e) => {
                        audio::play(Sound::Error);
                        app.wj.error = if e.contains("Secrets were required") || e.contains("password") || e.contains("802-11-wireless-security") {
                            "Wrong password. Check it and try again.".into()
                        } else {
                            e
                        };
                        app.wj_push();
                    }
                }
                app.sys_changed();
            }));
        });
    }

    pub fn wj_push(&self) {
        let ui = self.ui();
        let w = &self.wj;
        ui.set_wj_ssid(w.ssid.clone().into());
        ui.set_wj_sub(format!("Secured · {}", w.signal).into());
        ui.set_wj_field(if w.show { w.text.clone() } else { "•".repeat(w.text.chars().count()) }.into());
        ui.set_wj_show(w.show);
        ui.set_wj_error(w.error.clone().into());
        ui.set_wj_busy(w.busy);
        let key = |label: String, width: f32, focused: bool, accent: bool, on: bool| crate::WjKey { label: label.into(), w: width, focused, accent, on };
        let rows: Vec<Vec<crate::WjKey>> = (0..4).map(|r| (0..COLS).map(|c| {
            let label = w.char_at(r, c).map(|ch| if ch == ' ' { "␣".to_string() } else { ch.to_string() }).unwrap_or_default();
            key(label, 1.0, w.row == r && w.col == c, false, false)
        }).collect()).collect();
        let actions: Vec<crate::WjKey> = ACTIONS.iter().enumerate().map(|(i, (a, width))| {
            let (label, on) = match a {
                Action::Shift => ("Shift".to_string(), w.shift),
                Action::Layer => (if w.symbols { "abc" } else { "#+=" }.to_string(), w.symbols),
                Action::Space => ("Space".to_string(), false),
                Action::Delete => ("Delete".to_string(), false),
                Action::Show => (if w.show { "Hide" } else { "Show" }.to_string(), w.show),
                Action::Join => ("Join".to_string(), false),
            };
            key(label, *width, w.row == 4 && w.col == i, *a == Action::Join, on)
        }).collect();
        let m = |v: Vec<crate::WjKey>| -> slint::ModelRc<crate::WjKey> { std::rc::Rc::new(slint::VecModel::from(v)).into() };
        let mut rows = rows.into_iter();
        ui.set_wj_row0(m(rows.next().unwrap_or_default()));
        ui.set_wj_row1(m(rows.next().unwrap_or_default()));
        ui.set_wj_row2(m(rows.next().unwrap_or_default()));
        ui.set_wj_row3(m(rows.next().unwrap_or_default()));
        ui.set_wj_row4(m(actions));
    }
}

/// From a character row to the action row and back, keeping roughly the same place.
fn col_to_action(col: usize) -> usize {
    let mut x = 0.0;
    let target = col as f32 + 0.5;
    for (i, (_, w)) in ACTIONS.iter().enumerate() {
        x += w;
        if target <= x {
            return i;
        }
    }
    ACTIONS.len() - 1
}

fn action_to_col(action: usize) -> usize {
    let start: f32 = ACTIONS[..action].iter().map(|(_, w)| w).sum();
    ((start + ACTIONS[action].1 / 2.0) as usize).min(COLS - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_rows() {
        assert!(LETTERS.iter().chain(SYMBOLS.iter()).all(|r| r.chars().count() == COLS));
        let total: f32 = ACTIONS.iter().map(|(_, w)| w).sum();
        assert_eq!(total, COLS as f32);
        // Down from the first character goes to Shift, from the last to Join, and back up.
        assert_eq!((col_to_action(0), col_to_action(9)), (0, 5));
        // Up from any action key and down again comes back to the same key.
        assert!((0..ACTIONS.len()).all(|a| col_to_action(action_to_col(a)) == a));
        let w = WifiJoin { shift: true, ..Default::default() };
        assert_eq!((w.char_at(1, 0), w.char_at(0, 0)), (Some('Q'), Some('1')));
    }
}
