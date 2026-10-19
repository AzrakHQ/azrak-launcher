//! Joining a secured Wi-Fi network (Settings → Internet): a card with the network, its password
//! and an on-screen keyboard, so a controller is all it takes; a keyboard types straight in.
//! Controller: ✕ presses the key, □ deletes, △ adds a space, L1 shift, R1 symbols, R2 or Options
//! joins, ○ cancels. Keyboard: typing leaves the highlight where it is; Enter right after typing
//! joins, Enter after moving with the arrows presses the highlighted key. Esc cancels, Tab shows
//! the password.

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
    /// A wrong password (red).
    pub error: String,
    /// Not tried yet: nothing typed, or too short to be a Wi-Fi password.
    pub note: String,
    /// WEP takes passwords from 5 characters; everything newer from 8.
    pub wep: bool,
    /// The last input was typed on a keyboard (a character or Backspace), so Enter means "done".
    /// After moving with the arrows it presses the highlighted key instead.
    pub typed: bool,
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
        self.note.clear();
    }

    fn delete(&mut self) {
        self.text.pop();
        self.error.clear();
        self.note.clear();
    }

    /// A character typed on a keyboard: the highlight stays, so a typo is never "joined".
    fn key_typed(&mut self, c: char) {
        self.type_char(c);
        self.typed = true;
    }

    fn key_backspace(&mut self) {
        self.delete();
        self.typed = true;
    }

    /// Move the highlight. False at the keyboard's edge.
    fn step(&mut self, a: Act) -> bool {
        self.typed = false;
        match a {
            Act::Up if self.row > 0 => {
                if self.row == 4 {
                    self.col = action_to_col(self.col);
                }
                self.row -= 1;
            }
            Act::Down if self.row < 4 => {
                self.row += 1;
                if self.row == 4 {
                    self.col = col_to_action(self.col);
                }
            }
            Act::Left if self.col > 0 => self.col -= 1,
            Act::Right if self.col + 1 < if self.row == 4 { ACTIONS.len() } else { COLS } => self.col += 1,
            _ => return false,
        }
        true
    }

    /// Press the highlighted key. True when it's Join.
    fn press(&mut self) -> bool {
        self.typed = false;
        if self.row < 4 {
            if let Some(c) = self.char_at(self.row, self.col) {
                self.type_char(c);
                // Shift is for one letter, as on a phone.
                if c.is_alphabetic() {
                    self.shift = false;
                }
            }
            return false;
        }
        match ACTIONS[self.col].0 {
            Action::Shift => self.shift = !self.shift,
            Action::Layer => self.symbols = !self.symbols,
            Action::Space => self.type_char(' '),
            Action::Delete => self.delete(),
            Action::Show => self.show = !self.show,
            Action::Join => return true,
        }
        false
    }

    /// Enter on a keyboard: right after typing it joins; otherwise it presses the highlighted
    /// key. True when it joins.
    fn enter(&mut self) -> bool {
        self.typed || self.press()
    }

    /// Enter on a keyboard would join (for its hint).
    fn enter_joins(&self) -> bool {
        self.typed || (self.row == 4 && ACTIONS[self.col].0 == Action::Join)
    }

    /// Why the password isn't worth trying yet: nothing typed, or shorter than any Wi-Fi password
    /// (a sure "wrong password" after a long wait).
    fn not_ready(&self) -> Option<String> {
        let (n, min) = (self.text.chars().count(), if self.wep { 5 } else { 8 });
        if n == 0 {
            Some("Type the password first".into())
        } else if n < min {
            Some(format!("Wi-Fi passwords have at least {min} characters"))
        } else {
            None
        }
    }
}

impl App {
    /// Settings → Internet: a secured network that isn't saved yet asks for its password here.
    pub fn wifi_join_open(&mut self, ssid: &str, signal: &str, wep: bool) {
        audio::play(Sound::Select);
        self.wj = WifiJoin { ssid: ssid.into(), signal: signal.into(), wep, row: 1, ..Default::default() };
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
            if self.wj.enter() {
                self.wj_join();
            } else {
                audio::play(Sound::Move);
                self.wj_push();
            }
        } else if k(Key::Backspace) {
            self.wj.key_backspace();
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
                    self.wj.key_typed(c);
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
            Act::Up | Act::Down | Act::Left | Act::Right => {
                if !w.step(a) {
                    return;
                }
            }
            Act::Confirm => return self.wj_press(),
            Act::Back => return self.wj_close(),
            Act::Trailer => w.delete(),
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
        if self.wj.press() {
            return self.wj_join();
        }
        audio::play(Sound::Move);
        self.wj_push();
    }

    fn wj_join(&mut self) {
        if let Some(note) = self.wj.not_ready() {
            self.wj.note = note;
            audio::play(Sound::Back);
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
        ui.set_wj_note(w.note.clone().into());
        ui.set_wj_enter_joins(w.enter_joins());
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

    #[test]
    fn enter_after_typing_joins_after_arrows_presses() {
        let mut w = WifiJoin { row: 1, ..Default::default() };
        // Typing leaves the highlight on q; Enter right after it means "done".
        w.key_typed('a');
        assert_eq!((w.row, w.col, w.text.as_str()), (1, 0, "a"));
        assert!(w.enter_joins() && w.enter());
        // After the arrows, Enter presses the highlighted key (w), and again (w).
        assert!(w.step(Act::Right));
        assert!(!w.enter_joins());
        assert!(!w.enter() && !w.enter());
        assert_eq!(w.text, "aww");
        // Backspace counts as typing.
        w.key_backspace();
        assert!(w.enter());
        assert_eq!(w.text, "aw");
        // On Join, Enter joins whatever came last.
        assert!(w.step(Act::Down) && w.step(Act::Down) && w.step(Act::Down));
        while w.step(Act::Right) {}
        assert!(w.enter_joins() && w.enter());
    }

    #[test]
    fn too_short_to_try() {
        let mut w = WifiJoin::default();
        assert_eq!(w.not_ready().as_deref(), Some("Type the password first"));
        w.text = "letmein".into();
        assert_eq!(w.not_ready().as_deref(), Some("Wi-Fi passwords have at least 8 characters"));
        w.text.push('1');
        assert_eq!(w.not_ready(), None);
        // WEP keys can be 5 characters.
        let w = WifiJoin { wep: true, text: "abcde".into(), ..Default::default() };
        assert_eq!(w.not_ready(), None);
        // Typing again clears the note.
        let mut w = WifiJoin { note: "Type the password first".into(), ..Default::default() };
        w.key_typed('x');
        assert!(w.note.is_empty());
    }
}
