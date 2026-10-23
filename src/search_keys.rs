//! The Library's search with a controller: an on-screen keyboard docked under the results, so a
//! controller is all it takes (with only a keyboard, the search field takes typing as before).
//! Results update with every letter, and the row above the keys holds the best matches to
//! open straight away, or recent searches before anything is typed.
//! Controller: ✕ presses the key, □ deletes, △ adds a space, R2 or Options shows the results,
//! ○ closes. A keyboard types straight in, as on the Wi-Fi card.

use crate::app::*;
use crate::audio::{self, Sound};

/// Character rows, centered under each other like a keyboard's.
const ROWS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm"];
/// As wide as the widest row, in keys.
const WIDTH: f32 = 10.0;
/// Rows: the suggestions, the characters, then the actions.
const SUGGESTIONS: usize = 0;
const ACTION_ROW: usize = ROWS.len() + 1;
const MAX_SUGGESTIONS: usize = 3;
const MAX_RECENT: usize = 8;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    Space,
    Delete,
    Clear,
    Done,
}
const ACTIONS: [(Action, f32); 4] = [(Action::Space, 3.0), (Action::Delete, 2.0), (Action::Clear, 2.0), (Action::Done, 3.0)];

#[derive(Default)]
pub struct SearchKeys {
    pub row: usize,
    pub col: usize,
    /// The last input was typed on a keyboard, so Enter means "show the results" (as on the
    /// Wi-Fi card).
    pub typed: bool,
}

/// How many keys `row` has, with `suggestions` in the first.
fn row_len(row: usize, suggestions: usize) -> usize {
    match row {
        SUGGESTIONS => suggestions,
        ACTION_ROW => ACTIONS.len(),
        r => ROWS[r - 1].len(),
    }
}

/// The middle of a key, in keys from the keyboard's left edge.
fn center(row: usize, col: usize, suggestions: usize) -> f32 {
    match row {
        SUGGESTIONS => (col as f32 + 0.5) * WIDTH / suggestions.max(1) as f32,
        ACTION_ROW => ACTIONS[..col].iter().map(|(_, w)| w).sum::<f32>() + ACTIONS[col].1 / 2.0,
        r => (WIDTH - ROWS[r - 1].len() as f32) / 2.0 + col as f32 + 0.5,
    }
}

/// The key of `row` nearest `x`, so moving up and down keeps to the same place.
fn nearest(row: usize, x: f32, suggestions: usize) -> usize {
    (0..row_len(row, suggestions))
        .min_by(|a, b| (center(row, *a, suggestions) - x).abs().total_cmp(&(center(row, *b, suggestions) - x).abs()))
        .unwrap_or(0)
}

impl SearchKeys {
    /// Move the highlight. False at the keyboard's edge.
    fn step(&mut self, a: Act, suggestions: usize) -> bool {
        self.typed = false;
        let x = center(self.row, self.col, suggestions);
        let to = match a {
            Act::Up if self.row == 1 && suggestions == 0 => return false,
            Act::Up if self.row > 0 => self.row - 1,
            Act::Down if self.row < ACTION_ROW => self.row + 1,
            Act::Left if self.col > 0 => {
                self.col -= 1;
                return true;
            }
            Act::Right if self.col + 1 < row_len(self.row, suggestions) => {
                self.col += 1;
                return true;
            }
            _ => return false,
        };
        self.row = to;
        self.col = nearest(to, x, suggestions);
        true
    }

    /// The suggestions changed (letters went in): keep the highlight on one.
    fn fit(&mut self, suggestions: usize) {
        if self.row == SUGGESTIONS {
            if suggestions == 0 {
                (self.row, self.col) = (1, nearest(1, center(SUGGESTIONS, self.col, MAX_SUGGESTIONS), 0));
            } else {
                self.col = self.col.min(suggestions - 1);
            }
        }
    }
}

impl App {
    /// Starting a search: the on-screen keyboard with a controller connected, else the field.
    pub fn open_search(&mut self) {
        if crate::gamepad::connected().is_empty() {
            return self.start_search_edit();
        }
        if self.view != 1 {
            self.switch_view(1, false);
        }
        // Closing it comes back to the search field, even when it opened from Home.
        self.set_focus(Z_SEARCH, 0);
        audio::play(Sound::Select);
        self.sk = SearchKeys { row: 2, ..Default::default() };
        self.push_overlay(Overlay::SearchKeys, Z_SEARCH_KEYS, 0);
        self.ui().invoke_focus_root();
        self.sk_push();
    }

    /// What the row above the keys offers: recent searches before anything is typed, then
    /// the best matches.
    fn sk_suggestions(&self) -> Vec<String> {
        if self.query.trim().is_empty() {
            self.cfg.lock().unwrap().recent_searches.iter().take(MAX_SUGGESTIONS).cloned().collect()
        } else {
            // The same game for two consoles (or a PC) says which is which.
            let games: Vec<&GameV> = self.filtered.iter().take(MAX_SUGGESTIONS).map(|&i| &self.games[i]).collect();
            games.iter().map(|g| if games.iter().filter(|o| o.name == g.name).count() > 1 {
                format!("{} · {}", g.name, g.g.platform.label())
            } else {
                g.name.clone()
            }).collect()
        }
    }

    fn sk_set_query(&mut self, q: String) {
        self.ui().set_query(q.clone().into());
        self.on_search(q);
        let n = self.sk_suggestions().len();
        self.sk.fit(n);
    }

    fn sk_type(&mut self, c: char) {
        let q = format!("{}{}", self.query, c);
        self.sk_set_query(q);
    }

    fn sk_delete(&mut self) {
        let mut q = self.query.clone();
        q.pop();
        self.sk_set_query(q);
    }

    /// Remember a search that found something, newest first.
    fn remember_search(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() || self.filtered.is_empty() {
            return;
        }
        let mut cfg = self.cfg.lock().unwrap();
        cfg.recent_searches.retain(|r| !r.eq_ignore_ascii_case(&q));
        cfg.recent_searches.insert(0, q);
        cfg.recent_searches.truncate(MAX_RECENT);
        cfg.save();
    }

    /// Done typing: the keyboard goes and the highlight goes to the first result.
    fn sk_done(&mut self) {
        self.remember_search();
        self.back();
        if !self.filtered.is_empty() {
            self.set_focus(Z_GRID, 0);
            self.set_grid_scroll(0.0, 0);
            self.focus_card_changed();
        }
    }

    /// A suggestion: a recent search searches again; a match opens its Game Hub.
    fn sk_pick(&mut self, i: usize) {
        if self.query.trim().is_empty() {
            let Some(q) = self.cfg.lock().unwrap().recent_searches.get(i).cloned() else { return };
            self.sk_set_query(q);
            return self.sk_done();
        }
        if i >= self.filtered.len() {
            return;
        }
        self.remember_search();
        self.back();
        self.set_focus(Z_GRID, i as i32);
        self.ensure_grid_visible();
        self.act(Act::Confirm);
    }

    /// Press the highlighted key.
    fn sk_press(&mut self) {
        self.sk.typed = false;
        let (row, col) = (self.sk.row, self.sk.col);
        match row {
            SUGGESTIONS => return self.sk_pick(col),
            ACTION_ROW => match ACTIONS[col].0 {
                Action::Space => self.sk_type(' '),
                Action::Delete => self.sk_delete(),
                Action::Clear => self.sk_set_query(String::new()),
                Action::Done => return self.sk_done(),
            },
            r => {
                if let Some(c) = ROWS[r - 1].chars().nth(col) {
                    self.sk_type(c);
                }
            }
        }
        audio::play(Sound::Move);
        self.sk_push();
    }

    pub fn act_search_keys(&mut self, a: Act) {
        match a {
            Act::Up | Act::Down | Act::Left | Act::Right => {
                let n = self.sk_suggestions().len();
                if !self.sk.step(a, n) {
                    return;
                }
            }
            Act::Confirm => return self.sk_press(),
            Act::Back => return self.back(),
            Act::Trailer => self.sk_delete(),
            Act::Search => self.sk_type(' '),
            Act::PageDown | Act::Options => return self.sk_done(),
            _ => return,
        }
        audio::play(Sound::Move);
        self.sk_push();
    }

    /// Keys while the keyboard is up; all of them are used, so nothing reaches the Library.
    pub fn search_keys_key(&mut self, text: &str, ctrl: bool, alt: bool) -> bool {
        use slint::platform::Key;
        let k = |key: Key| slint::SharedString::from(key) == text;
        if k(Key::Escape) {
            self.back();
        } else if k(Key::Return) {
            if self.sk.typed {
                self.sk_done();
            } else {
                self.sk_press();
            }
        } else if k(Key::Backspace) {
            self.sk_delete();
            self.sk.typed = true;
            self.sk_push();
        } else if k(Key::UpArrow) {
            self.act_search_keys(Act::Up);
        } else if k(Key::DownArrow) {
            self.act_search_keys(Act::Down);
        } else if k(Key::LeftArrow) {
            self.act_search_keys(Act::Left);
        } else if k(Key::RightArrow) {
            self.act_search_keys(Act::Right);
        } else if !ctrl && !alt {
            let mut chars = text.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if !c.is_control() {
                    self.sk_type(c);
                    self.sk.typed = true;
                    self.sk_push();
                }
            }
        }
        true
    }

    /// A key clicked with the mouse: row * 100 + column (negative: above the keyboard, which
    /// closes it).
    pub fn click_search_keys(&mut self, idx: i32) {
        if idx < 0 {
            return self.back();
        }
        let (row, col) = ((idx / 100) as usize, (idx % 100) as usize);
        if row <= ACTION_ROW && col < row_len(row, self.sk_suggestions().len()) {
            (self.sk.row, self.sk.col) = (row, col);
            self.sk_press();
        }
    }

    pub fn sk_push(&self) {
        let ui = self.ui();
        let s = &self.sk;
        let key = |label: &str, width: f32, focused: bool, accent: bool|
            crate::WjKey { label: label.into(), w: width, focused, accent, on: false, icon: "".into() };
        let m = |v: Vec<crate::WjKey>| -> slint::ModelRc<crate::WjKey> { std::rc::Rc::new(slint::VecModel::from(v)).into() };
        let suggestions = self.sk_suggestions();
        let width = WIDTH / suggestions.len().max(1) as f32;
        ui.set_sk_suggestions(m(suggestions.iter().enumerate()
            .map(|(c, label)| key(label, width, s.row == SUGGESTIONS && s.col == c, false)).collect()));
        ui.set_sk_suggestions_label(if suggestions.is_empty() { "" } else if self.query.trim().is_empty() { "Recent searches" } else { "Best matches" }.into());
        let rows: Vec<slint::ModelRc<crate::WjKey>> = ROWS.iter().enumerate().map(|(r, chars)| m(chars.chars().enumerate()
            .map(|(c, ch)| key(&ch.to_string(), 1.0, s.row == r + 1 && s.col == c, false)).collect())).collect();
        let mut rows = rows.into_iter();
        ui.set_sk_row1(rows.next().unwrap_or_default());
        ui.set_sk_row2(rows.next().unwrap_or_default());
        ui.set_sk_row3(rows.next().unwrap_or_default());
        ui.set_sk_row4(rows.next().unwrap_or_default());
        ui.set_sk_actions(m(ACTIONS.iter().enumerate().map(|(c, (a, width))| {
            let label = match a {
                Action::Space => "Space",
                Action::Delete => "Delete",
                Action::Clear => "Clear",
                Action::Done => "Show results",
            };
            key(label, *width, s.row == ACTION_ROW && s.col == c, *a == Action::Done)
        }).collect()));
        let n = self.filtered.len();
        ui.set_sk_count(if self.query.trim().is_empty() {
            String::new()
        } else if !self.search_note.is_empty() {
            self.search_note.clone()
        } else if n == 0 {
            "No matches".into()
        } else if n == 1 {
            "1 game".into()
        } else {
            format!("{n} games")
        }.into());
        ui.set_sk_count_warn(!self.search_note.is_empty() || (n == 0 && !self.query.trim().is_empty()));
        ui.set_sk_enter_done(s.typed || (s.row == ACTION_ROW && ACTIONS[s.col].0 == Action::Done));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let total: f32 = ACTIONS.iter().map(|(_, w)| w).sum();
        assert_eq!(total, WIDTH);
        assert!(ROWS.iter().all(|r| r.len() as f32 <= WIDTH));
    }

    #[test]
    fn up_and_down_keep_to_the_same_place() {
        let mut k = SearchKeys { row: 2, col: 0, typed: false };
        // q, down to a, z; then Space; then back up to z.
        assert!(k.step(Act::Down, 4) && (k.row, k.col) == (3, 0));
        assert!(k.step(Act::Down, 4) && (k.row, k.col) == (4, 0));
        assert!(k.step(Act::Down, 4) && (k.row, k.col) == (ACTION_ROW, 0));
        assert!(!k.step(Act::Down, 4));
        assert!(k.step(Act::Up, 4) && k.row == 4);
        // The last letter of the top row, up to the last of four suggestions, and down again.
        let mut k = SearchKeys { row: 1, col: 9, typed: false };
        assert!(k.step(Act::Up, 4) && (k.row, k.col) == (SUGGESTIONS, 3));
        assert!(k.step(Act::Down, 4) && k.row == 1 && k.col >= 8);
        // No suggestions: the top row is the top.
        let mut k = SearchKeys { row: 1, col: 3, typed: false };
        assert!(!k.step(Act::Up, 0));
        // Show results is the last action, under the right of the keyboard.
        let mut k = SearchKeys { row: 4, col: 6, typed: false };
        assert!(k.step(Act::Down, 0) && ACTIONS[k.col].0 == Action::Done);
    }

    #[test]
    fn suggestions_that_go_away_move_the_highlight_to_the_keys() {
        let mut k = SearchKeys { row: SUGGESTIONS, col: 3, typed: false };
        k.fit(2);
        assert_eq!((k.row, k.col), (SUGGESTIONS, 1));
        k.fit(0);
        assert_eq!(k.row, 1);
    }
}
