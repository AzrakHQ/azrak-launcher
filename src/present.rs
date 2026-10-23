//! Turning app state into Slint models, and image requests for what is on screen.

use crate::app::*;
use crate::images::{prio, Src};
use crate::psn::{sized, Info};
use crate::util;
use crate::{ActionData, CardData, CardRow, ChipData, Fact, FactRow, GenreChip, HeroData, HubData, MenuItem, Shot, TileData};
use slint::{ComponentHandle, Image, Model, SharedString};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct ImgReq {
    pub key: String,
    pub src: Src,
    pub w: u32,
    pub crop: f32,
}

pub fn req_url(url: &str, w: u32, crop: f32) -> Option<ImgReq> {
    (!url.is_empty()).then(|| ImgReq { key: format!("{url}#{w}#{crop}"), src: Src::Url(sized(url, w)), w, crop })
}

pub fn req_file(p: &std::path::Path, w: u32) -> ImgReq {
    ImgReq { key: format!("file:{}#{w}", p.display()), src: Src::File(p.to_path_buf()), w, crop: 0.0 }
}

const COVER_CROP: f32 = 0.0; // RuTracker artwork has no uniform banner to crop.
/// `bg_key` while the backdrop is plain (the game has no artwork).
const NO_BACKDROP: &str = "-";

fn yt_art(id: &str) -> Option<ImgReq> {
    (!id.is_empty()).then(|| req_url(&format!("https://i.ytimg.com/vi/{id}/maxresdefault.jpg"), 1920, 0.0)).flatten()
}

fn chip(icon: &str, text: impl Into<SharedString>, gold: bool) -> ChipData {
    ChipData { icon: icon.into(), text: text.into(), gold, dot: 0, stars: 0.0 }
}

/// "● In-game on Linux" / "● In-game on Windows · untested on Linux" / "● Untested on KytyPS5".
fn compat_chip(e: Option<&crate::compat::Entry>, platform: crate::platform::Platform) -> ChipData {
    match e {
        Some(e) => ChipData { icon: "".into(), text: e.chip_text().into(), gold: false, dot: e.status.level(), stars: 0.0 },
        None => ChipData { icon: "".into(), text: format!("Untested on {}", platform.emulator()).into(), gold: false, dot: 5, stars: 0.0 },
    }
}

fn action_data(a: &ActionDef) -> ActionData {
    ActionData { id: a.id.into(), label: a.label.clone().into(), icon: a.icon.into(), primary: a.primary, danger: a.danger, round: a.round }
}

fn logo_size(img: &Image, max_w: f32, max_h: f32) -> (f32, f32) {
    let s = img.size();
    if s.width == 0 || s.height == 0 {
        return (max_w, max_h);
    }
    let k = (max_w / s.width as f32).min(max_h / s.height as f32);
    (s.width as f32 * k, s.height as f32 * k)
}

/// Smallest text scale for Settings and dialogs (the Library uses the same floor).
pub const TEXT_SCALE_MIN: f32 = 0.75;

impl App {
    fn get_img(&mut self, r: &Option<ImgReq>, p: u8) -> Option<Image> {
        let r = r.as_ref()?;
        self.images.want(&r.key, r.src.clone(), r.w, r.crop, p)
    }

    // ------------------------------------------------------------------ artwork choices

    /// RuTracker covers: PS4 box art loses its blue "PS4" band, so cards stay clean like PS5's.
    fn cover_crop(&self, t: Target) -> f32 {
        if self.target_platform(t) == crate::platform::Platform::Ps4 { crate::images::CROP_PS4_BAND } else { COVER_CROP }
    }

    pub fn tile_req(&self, item: RowItem) -> Option<ImgReq> {
        let (info, local, cover) = match item {
            RowItem::Local(l) => (self.locals[l].info.as_ref(), Some(&self.locals[l].l), self.locals[l].cat.map(|c| self.games[c].g.cover.clone())),
            RowItem::Cat(g) => (self.games[g].info.as_ref(), None, Some(self.games[g].g.cover.clone())),
            RowItem::All => return None,
        };
        if let Some(r) = info.and_then(|i| req_url(&i.master, 256, 0.0)) {
            return Some(r);
        }
        if let Some(p) = local.and_then(|l| l.icon0.as_ref()) {
            return Some(req_file(p, 256));
        }
        let crop = self.cover_crop(match item { RowItem::Local(l) => Target { game: self.locals[l].cat, local: Some(l) }, RowItem::Cat(g) => Target { game: Some(g), local: None }, RowItem::All => Target { game: None, local: None } });
        if let Some(r) = cover.and_then(|c| req_url(&c, 256, crop)) {
            return Some(r);
        }
        info.and_then(|i| req_url(&i.hub, 512, 0.0))
    }

    pub fn target_bg_url(&self, t: Target) -> Option<ImgReq> {
        let info = self.target_info(t);
        if let Some(r) = info.as_ref().and_then(|i| req_url(&i.hub, 1920, 0.0)) {
            return Some(r);
        }
        if let Some(p) = t.local.and_then(|l| self.locals[l].l.pic0.clone()) {
            return Some(req_file(&p, 1920));
        }
        let g = t.game.map(|g| &self.games[g].g)?;
        yt_art(&g.trailer).or_else(|| req_url(&g.cover, 1280, self.cover_crop(t)))
    }

    /// The small cover shown while a big background downloads (usually already on disk).
    pub fn cover_req(&self, t: Target) -> Option<ImgReq> {
        t.game.and_then(|g| self.card_req(g))
            .or_else(|| t.local.and_then(|l| self.locals[l].l.icon0.clone()).map(|p| req_file(&p, 512)))
    }

    pub fn row_cover_req(&self, i: usize) -> Option<ImgReq> {
        match self.row.get(i)? {
            RowItem::All => None,
            _ => self.cover_req(self.row_target(i)),
        }
    }

    /// Show row item `i`'s background, with its cover standing in until it has downloaded.
    pub fn show_row_background(&mut self, i: usize, force: bool) {
        // No installed games: the plain Azrak backdrop. One game's key art there reads as an ad,
        // or as a game you already own.
        let (bg, cover) = if self.row.is_empty() { (None, None) } else { (self.hero_bg_url(i), self.row_cover_req(i)) };
        self.want_background_or(bg, force, cover);
    }

    pub fn hero_bg_url(&self, i: usize) -> Option<ImgReq> {
        match self.row.get(i)? {
            RowItem::All => {
                let g = self.games.iter().find(|g| g.info.as_ref().is_some_and(|i| !i.hub.is_empty()))?;
                req_url(&g.info.as_ref()?.hub, 1920, 0.0)
            }
            _ => self.target_bg_url(self.row_target(i)),
        }
    }

    fn logo_req(info: Option<&Info>) -> Option<ImgReq> {
        info.and_then(|i| req_url(&i.logo, 960, 0.0))
    }

    /// `card_req` would find a picture: the same sources, without building the request (the
    /// Library asks for every release on every keystroke).
    pub fn has_card_art(&self, gi: usize) -> bool {
        let g = &self.games[gi];
        !g.g.cover.is_empty() || g.info.as_ref().is_some_and(|i| !i.portrait.is_empty() || !i.master.is_empty() || !i.hub.is_empty())
    }

    pub fn card_req(&self, gi: usize) -> Option<ImgReq> {
        let g = &self.games[gi];
        let info = g.info.as_ref();
        // Steam's other art is wide: the release's own cover reads better on a 2:3 card.
        let wide = info.is_some_and(|i| i.source.as_deref() == Some("steam"));
        info.and_then(|i| req_url(&i.portrait, 440, 0.0))
            .or_else(|| info.filter(|_| !wide).and_then(|i| req_url(&i.master, 440, 0.0)))
            .or_else(|| req_url(&g.g.cover, 440, if g.g.platform == crate::platform::Platform::Ps4 { crate::images::CROP_PS4_BAND } else { COVER_CROP }))
            .or_else(|| info.and_then(|i| req_url(&i.master, 440, 0.0)))
            .or_else(|| info.and_then(|i| req_url(&i.hub, 640, 0.0)))
    }

    // ------------------------------------------------------------------ push everything

    pub fn push_all(&mut self) {
        self.push_row();
        self.push_hero();
        self.push_library();
        if self.overlay == Overlay::Hub {
            self.push_hub();
        }
        if self.view == 0 && self.overlay == Overlay::None {
            self.show_row_background(self.sel, true);
        }
    }

    // ------------------------------------------------------------------ home

    pub fn push_row(&mut self) {
        let items = self.row.clone();
        let mut tiles = Vec::with_capacity(items.len());
        let mut keys = Vec::with_capacity(items.len());
        for (i, it) in items.iter().enumerate() {
            let req = self.tile_req(*it);
            let p = if i.abs_diff(self.sel) < 12 { prio::TILE } else { prio::PREFETCH };
            let img = self.get_img(&req, p);
            keys.push(req.map(|r| r.key).unwrap_or_default());
            tiles.push(TileData { loaded: img.is_some(), image: img.unwrap_or_default(), special: *it == RowItem::All, badge: self.tile_badge(*it).into() });
        }
        self.tile_keys = keys;
        self.tile_model.set_vec(tiles);
        let ui = self.ui();
        ui.set_sel(self.sel as i32);
        self.push_row_text();
    }

    fn tile_badge(&self, it: RowItem) -> &'static str {
        match it {
            RowItem::Local(l) if self.session_for_local(l).is_some() => "Playing",
            // Everything on the Games tab is installed, so only a running game gets a badge.
            _ => "",
        }
    }

    pub fn push_row_text(&mut self) {
        let (name, sub) = match self.row.get(self.sel).copied() {
            None => (String::new(), String::new()),
            Some(RowItem::All) => ("Game Library".into(), format!("{} games · {} releases", self.library_total, self.games.len())),
            Some(RowItem::Local(l)) => {
                let lv = &self.locals[l];
                let sub = if let Some(s) = self.session_for_local(l) {
                    let _ = s;
                    "Playing now".to_string()
                } else {
                    let pt = self.playtime(&lv.l);
                    if pt.last > 0.0 {
                        format!("Last played {}", util::fmt_last_played(pt.last).to_lowercase())
                    } else {
                        "Not played yet".into()
                    }
                };
                (lv.name.clone(), sub)
            }
            Some(RowItem::Cat(g)) => {
                let gv = &self.games[g];
                let genre = gv.info.as_ref().and_then(|i| i.genres.first().cloned()).or_else(|| gv.g.genres.first().cloned()).unwrap_or_default();
                let status = self.game_compat(gv).map(|e| e.chip_text()).unwrap_or_default();
                (gv.name.clone(), [genre, gv.g.size.clone(), status].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "))
            }
        };
        let ui = self.ui();
        let front_b = ui.get_row_front_b();
        let flip = std::mem::take(&mut self.row_flip);
        // New selection: write the hidden slot and crossfade to it; otherwise update in place.
        if flip != front_b {
            ui.set_row_name_b(name.into());
            ui.set_row_sub_b(sub.into());
        } else {
            ui.set_row_name(name.into());
            ui.set_row_sub(sub.into());
        }
        if flip {
            ui.set_row_front_b(!front_b);
        }
    }

    /// The small line above a title. Only says things nothing else on screen already says:
    /// a running session, total playtime (Home), or when a game was added to the catalog.
    fn kicker_for(&self, t: Target, in_hub: bool) -> (String, String, bool) {
        if let Some(l) = t.local {
            if let Some(s) = self.session_for_local(l) {
                return (util::fmt_clock(util::now_secs() - s.since), "● PLAYING".into(), true);
            }
            let pt = self.playtime(&self.locals[l].l);
            // In the Game Hub, playtime is in the facts below.
            let text = if pt.total > 0.0 && !in_hub { format!("PLAYED {}", util::fmt_duration_words(pt.total).to_uppercase()) } else { String::new() };
            return (text, String::new(), false);
        }
        if let Some(g) = t.game {
            let gv = &self.games[g];
            let added = util::fmt_date(util::parse_iso_date(&gv.g.date)).to_uppercase();
            if added.is_empty() {
                // Nothing to say: where it comes from is in the Game Hub's technical details.
                return (String::new(), String::new(), false);
            }
            return (if gv.is_new { format!("NEW · ADDED {added}") } else { format!("ADDED {added}") }, String::new(), false);
        }
        (String::new(), String::new(), false)
    }

    fn chips_for(&self, t: Target) -> Vec<ChipData> {
        let info = self.target_info(t);
        let g = t.game.map(|g| &self.games[g]);
        let mut c: Vec<ChipData> = self.target_compat(t).map(|e| compat_chip(Some(e), self.target_platform(t))).into_iter().collect();
        if let Some(r) = info.as_ref().and_then(|i| i.rating.as_ref()) {
            c.push(ChipData { stars: r.score as f32, ..chip("", format!("{:.1} · {} ratings", r.score, thousands(r.total)), false) });
        }
        let genres: Vec<String> = info.as_ref().filter(|i| !i.genres.is_empty()).map(|i| i.genres.clone()).or_else(|| g.map(|g| g.g.genres.clone())).unwrap_or_default();
        if !genres.is_empty() {
            c.push(chip("tag", genres.iter().take(2).cloned().collect::<Vec<_>>().join(" · "), false));
        }
        if let Some(g) = g.filter(|g| !g.g.size.is_empty()) {
            c.push(chip("disk", g.g.size.clone(), false));
        }
        let rel = g.map(|g| g.g.release.clone()).filter(|r| !r.is_empty()).or_else(|| info.as_ref().map(|i| util::fmt_date(util::parse_iso_date(&i.release)))).unwrap_or_default();
        if !rel.is_empty() {
            c.push(chip("cal", rel, false));
        }
        if g.is_none() {
            // Not in the catalog: its title ID, or "PC" (PC games have none).
            if let Some(l) = t.local.map(|l| &self.locals[l].l) {
                c.push(chip("tag", if l.title_id.is_empty() { l.platform.label().to_string() } else { l.title_id.clone() }, false));
            }
        }
        c
    }

    pub fn push_hero_kicker(&mut self) {
        let ui = self.ui();
        let mut h = if ui.get_hero_front_b() { ui.get_hero_b() } else { ui.get_hero() };
        if let Some(it) = self.row.get(self.sel).copied() {
            if it != RowItem::All {
                let (k, a, live) = self.kicker_for(self.row_target(self.sel), false);
                h.kicker = k.into();
                h.kicker_accent = a.into();
                h.kicker_live = live;
                if ui.get_hero_front_b() { ui.set_hero_b(h) } else { ui.set_hero(h) }
            }
        }
    }

    pub fn push_hero(&mut self) {
        let it = self.row.get(self.sel).copied();
        let mut h = HeroData::default();
        self.hero_logo_key.clear();
        match it {
            None => {
                // No installed games: getting one from the Library comes first; a folder of games
                // already on this PC is the other way in.
                let mk = |id, label: &str, icon, primary| ActionDef { id, label: label.into(), icon, primary, danger: false, round: false };
                h.title = "No games yet".into();
                h.desc = "Pick a game in the Library, then Download and Install.".into();
                self.hero_actions = vec![mk("library", "Browse games", "grid", true), mk("add_folder", "Add game folder", "folder", false)];
            }
            Some(RowItem::All) => {
                let mk = |id, label: &str, icon, primary| ActionDef { id, label: label.into(), icon, primary, danger: false, round: false };
                if self.locals.is_empty() {
                    h.kicker = "GET STARTED".into();
                    h.title = "No games yet".into();
                    h.chips = model(vec![chip("grid", format!("{} games in the Library", self.groups.members.len()), false)]);
                    h.desc = "Pick a game in the Library, then Download and Install.".into();
                    self.hero_actions = vec![mk("library", "Browse games", "grid", true), mk("add_folder", "Add game folder", "folder", false)];
                } else {
                    h.kicker = "COLLECTION".into();
                    h.title = "Game Library".into();
                    h.chips = model(vec![chip("grid", format!("{} games", self.groups.members.len()), false)]);
                    h.desc = "Browse every PS5 game in the catalog. Search, filter by genre and sort by date, name, rating or size.".into();
                    self.hero_actions = vec![mk("library", "Open Library", "grid", true)];
                }
            }
            Some(_) => {
                let t = self.row_target(self.sel);
                let info = self.target_info(t);
                let (k, a, live) = self.kicker_for(t, false);
                h.kicker = k.into();
                h.kicker_accent = a.into();
                h.kicker_live = live;
                h.title = target_name(self, t).into();
                let lr = Self::logo_req(info.as_ref());
                if let Some(r) = &lr {
                    self.hero_logo_key = r.key.clone();
                    match self.get_img(&lr, prio::HERO) {
                        Some(img) => {
                            let (w, hh) = logo_size(&img, 620.0, 200.0);
                            h.logo = img;
                            h.has_logo = true;
                            h.logo_w = w * self.scale;
                            h.logo_h = hh * self.scale;
                        }
                        // On disk: it decodes in a few ms, so keep the space empty instead of flashing
                        // the text name. Still downloading: show the name as text meanwhile.
                        None if self.images.on_disk(&r.key, &r.src) => h.title = SharedString::default(),
                        None => {}
                    }
                }
                h.chips = model(self.chips_for(t));
                let g = t.game.map(|g| &self.games[g].g);
                let desc = info.as_ref().map(|i| i.short.clone()).filter(|s| s.chars().count() > 30 && s.chars().any(|c| c.is_lowercase()))
                    .or_else(|| g.and_then(|g| g.description.first().cloned()))
                    .or_else(|| g.map(|g| g.excerpt.clone()))
                    .unwrap_or_else(|| match self.target_platform(t) {
                        crate::platform::Platform::Pc if self.pc_windows(t) => "Installed locally. A Windows game, run with Proton or Wine.".to_string(),
                        crate::platform::Platform::Pc => "Installed locally. Runs on Linux by itself, no emulator needed.".to_string(),
                        platform => format!("Installed locally. Launches with the {} emulator.", platform.emulator()),
                    });
                h.desc = truncate(&desc, 260).into();
                self.hero_actions = self.actions_for(t, false);
            }
        }
        h.actions = model(self.hero_actions.iter().map(action_data).collect());
        // No tiles to select: selection lives on the buttons.
        if self.row.is_empty() && self.zone == Z_ROW && self.overlay == Overlay::None && !self.hero_actions.is_empty() {
            self.set_focus(Z_ACTIONS, 0);
        }
        if self.zone == Z_ACTIONS && self.idx as usize >= self.hero_actions.len() {
            self.set_focus(Z_ACTIONS, self.hero_actions.len().saturating_sub(1) as i32);
        }
        // A new selection goes into the hidden layer, which then crossfades in; anything else
        // (a logo arriving, the play timer) updates the visible layer in place.
        let ui = self.ui();
        ui.set_home_has_game(!self.row.is_empty());
        let front_b = ui.get_hero_front_b();
        if std::mem::take(&mut self.hero_flip) {
            if front_b { ui.set_hero(h) } else { ui.set_hero_b(h) }
            ui.set_hero_front_b(!front_b);
        } else if front_b {
            ui.set_hero_b(h)
        } else {
            ui.set_hero(h)
        }
    }

    /// Screenshots of a game you've paused on start downloading right away, so they're there
    /// when you open its Game Hub (~1 s each from Sony's servers, 16 in parallel).
    pub fn prefetch_shots(&mut self, t: Target) {
        let Some(info) = self.target_info(t) else { return };
        for u in info.shots.iter().take(8) {
            if let Some(r) = req_url(u, 480, 0.0) {
                self.images.want(&r.key, r.src, r.w, r.crop, prio::PREFETCH);
            }
        }
    }

    /// Called whenever the selection moves; acts once it has rested for 350 ms.
    pub fn schedule_rest_prefetch(&mut self) {
        self.rest_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(350), || {
            crate::app::with_app(|app| {
                let t = match (app.view, app.zone) {
                    (0, Z_ROW | Z_ACTIONS) => Some(app.row_target(app.sel)),
                    (1, Z_GRID) => app.filtered.get(app.idx as usize).map(|&g| Target { game: Some(g), local: app.games[g].local }),
                    _ => None,
                };
                if let Some(t) = t {
                    app.prefetch_shots(t);
                }
            })
        });
    }

    pub fn prefetch_neighbors(&mut self) {
        for d in [1i64, -1, 2] {
            let i = self.sel as i64 + d;
            if i < 0 || i as usize >= self.row.len() {
                continue;
            }
            let i = i as usize;
            if let Some(r) = self.hero_bg_url(i) {
                self.images.want(&r.key, r.src, r.w, r.crop, prio::PREFETCH);
            }
            let info = match self.row[i] {
                RowItem::All => None,
                _ => self.target_info(self.row_target(i)),
            };
            if let Some(r) = Self::logo_req(info.as_ref()) {
                self.images.want(&r.key, r.src, r.w, r.crop, prio::PREFETCH + 1);
            }
        }
    }

    // ------------------------------------------------------------------ background

    /// Crossfade the backdrop to `req`, with a stand-in image (usually already on disk) to show
    /// while `req` downloads.
    pub fn want_background_or(&mut self, req: Option<ImgReq>, force: bool, standin: Option<ImgReq>) {
        // No background art (or it can't be downloaded): its cover, else a plain backdrop, never
        // the previous game's art.
        let usable = |r: Option<ImgReq>| r.filter(|r| !self.images.has_failed(&r.key));
        let (req, standin) = match usable(req) { Some(r) => (Some(r), standin), None => (usable(standin), None) };
        let Some(r) = req else {
            if self.bg_key != NO_BACKDROP {
                self.images.cancel_unless(|j| j.prio != prio::HERO || j.max_w < 1000);
                self.bg_standin = None;
                self.apply_bg(NO_BACKDROP.into(), Image::default());
            }
            return;
        };
        if r.key == self.bg_key && !force {
            return;
        }
        if r.key == self.bg_key {
            return;
        }
        self.bg_pending = Some(r.key.clone());
        self.bg_standin = None;
        let keep = r.key.clone();
        // Moving quickly: drop queued backgrounds we already moved past.
        self.images.cancel_unless(move |j| j.prio != prio::HERO || j.key == keep || j.max_w < 1000);
        let downloading = !self.images.on_disk(&r.key, &r.src);
        if let Some(img) = self.images.want(&r.key, r.src, r.w, r.crop, prio::HERO) {
            self.apply_bg(r.key, img);
            return;
        }
        // Not downloaded yet (≈1 s from Sony's servers): show the game's cover meanwhile, so the
        // previous game's art never lingers behind this game's details.
        if downloading {
            if let Some(c) = standin {
                self.bg_standin = Some(c.key.clone());
                if let Some(img) = self.images.want(&c.key, c.src, c.w, c.crop, prio::HERO) {
                    self.apply_bg_standin(img);
                }
            }
        }
    }

    fn apply_bg_standin(&mut self, img: Image) {
        self.bg_standin = None;
        self.bg_key = String::new();
        self.bg_show_b = !self.bg_show_b;
        let ui = self.ui();
        if self.bg_show_b {
            ui.set_bg_b(img);
        } else {
            ui.set_bg_a(img);
        }
        ui.set_bg_show_b(self.bg_show_b);
    }

    fn apply_bg(&mut self, key: String, img: Image) {
        self.bg_pending = None;
        self.bg_key = key;
        self.bg_show_b = !self.bg_show_b;
        let ui = self.ui();
        if self.bg_show_b {
            ui.set_bg_b(img);
        } else {
            ui.set_bg_a(img);
        }
        ui.set_bg_show_b(self.bg_show_b);
    }

    /// An image finished loading: update whatever shows it.
    pub fn refresh_images(&mut self, key: &str) {
        if self.bg_standin.as_deref() == Some(key) && self.bg_pending.is_some() {
            if let Some(img) = self.images.get(key) {
                self.apply_bg_standin(img);
            }
        }
        if self.bg_pending.as_deref() == Some(key) {
            if let Some(img) = self.images.get(key) {
                self.apply_bg(key.to_string(), img);
            } else if self.images.has_failed(key) {
                // The background couldn't be downloaded: keep the cover standing in for it, or
                // go plain rather than leave the previous game's art.
                self.bg_pending = None;
                if !self.bg_key.is_empty() {
                    self.apply_bg(NO_BACKDROP.into(), Image::default());
                }
            }
        }
        if let Some(i) = self.tile_keys.iter().position(|k| k == key) {
            if let (Some(img), Some(mut t)) = (self.images.get(key), self.tile_model.row_data(i)) {
                t.image = img;
                t.loaded = true;
                self.tile_model.set_row_data(i, t);
            }
        }
        if key == self.hero_logo_key {
            self.push_hero();
        }
        self.grid_image_loaded(key);
        if self.hub_keys.contains(key) {
            self.push_hub();
        }
        if key == self.viewer_key {
            self.push_viewer();
        }
        if self.launch_keys.iter().any(|k| k == key) {
            if let Some(l) = self.launch_local {
                self.push_launch(l);
            }
        }
        self.boot_image_loaded(key);
        self.boot_cover_loaded(key);
    }

    // ------------------------------------------------------------------ library

    /// Recompute grid geometry from the window size.
    pub fn relayout(&mut self) {
        let (w, _) = self.logical_size();
        let compact = self.cfg.lock().unwrap().library_compact;
        let grid = crate::library_layout::Grid::new(w, self.scale, compact);
        self.cols = grid.cols;
        self.card_w = grid.card;
        self.row_h = grid.row;
        let (ui, k) = (self.ui(), self.scale);
        ui.set_card_w(self.card_w * k);
        ui.set_row_h(self.row_h * k);
        ui.set_grid_gap(grid.gap * k);
        ui.set_library_type_scale(grid.typography);
        ui.set_library_top(crate::library_layout::TOP * k);
        ui.set_library_compact(compact);
    }

    /// Window size in design units (the 1920×1080 canvas the UI is laid out on).
    pub fn logical_size(&self) -> (f32, f32) {
        let ui = self.ui();
        let s = ui.window().size();
        let sf = ui.window().scale_factor().max(0.1);
        if s.width == 0 {
            return (1920.0, 1080.0);
        }
        (s.width as f32 / sf / self.scale, s.height as f32 / sf / self.scale)
    }

    /// Map the design canvas onto the real window. Returns true if the scale changed.
    pub fn update_scale(&mut self) -> bool {
        let ui = self.ui();
        let s = ui.window().size();
        let sf = ui.window().scale_factor().max(0.1);
        if s.width < 100 || s.height < 100 {
            return false;
        }
        let (w, h) = (s.width as f32 / sf, s.height as f32 / sf);
        let user = std::env::var("AZRAK_SCALE").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        let k = ((w / 1920.0).min(h / 1080.0) * user).clamp(0.3, 4.0);
        if (k - self.scale).abs() < 0.001 {
            return false;
        }
        self.scale = k;
        ui.global::<crate::S>().set_k(k);
        ui.global::<crate::S>().set_t(k.max(TEXT_SCALE_MIN));
        true
    }

    pub fn push_library(&mut self) {
        let ui = self.ui();
        ui.set_sort_label(SORTS[self.sort].into());
        ui.set_sort_options(model(SORTS.iter().map(|label| SharedString::from(*label)).collect()));
        ui.set_sort_selected(self.sort as i32);
        // When the list (and its availability) was last counted: the newest count, else when the
        // launcher last loaded it. Each game's own date is in its Game Hub's technical details.
        let counted = self.games.iter().map(|game| game.g.peers_observed.as_str()).filter_map(util::parse_iso_date).max()
            .or_else(|| (self.catalog_updated > 0.0).then(|| util::civil_from_days((self.catalog_updated / 86400.0) as i64)));
        let updated = util::fmt_date(counted);
        ui.set_peer_snapshot_label(if updated.is_empty() { SharedString::default() } else { format!("List updated {updated}").into() });
        self.push_genres();
        self.push_grid();
    }

    pub fn push_genres(&mut self) {
        // As matched for the grid, unless the games changed since.
        let fresh;
        let hits = if self.hits.scores.len() == self.games.len() { &self.hits } else { fresh = self.search_hits(); &fresh };
        let n = self.status_count;
        let mut status_counts = vec![0usize; n];
        let mut genre_counts = std::collections::HashMap::<&str, usize>::new();
        let mut genre_total = 0;
        // As the grid: before a search, only the games it shows.
        let shown = if self.searching() { vec![true; self.groups.members.len()] } else { self.browsable() };
        for (members, _) in self.groups.members.iter().zip(shown).filter(|(_, shown)| *shown) {
            let mut status = vec![false; n];
            let mut buckets = HashSet::new();
            let mut any = false;
            for &index in members {
                let game = &self.games[index];
                if hits.scores[index].is_none() { continue; }
                let genre_ok = self.genre == "All" || game.buckets.contains(&self.genre.as_str());
                // Console chips count what pressing them would show, with the other filters.
                if genre_ok && self.mixed_consoles && self.status_filter_ok(game) {
                    if let Some(p) = self.consoles.iter().position(|p| *p == game.g.platform) {
                        status[n - self.consoles.len() + p] = true;
                    }
                }
                // "All" resets every filter, so it always counts everything.
                status[0] |= genre_ok;
                if genre_ok && self.platform_ok(game) {
                    status[1] |= game.local.is_some();
                    let compat = self.game_compat(game);
                    status[2] |= compat.is_some_and(|entry| entry.in_game());
                }
                if self.matches_status(index) { any = true; buckets.extend(game.buckets.iter().copied()); }
            }
            for (count, present) in status_counts.iter_mut().zip(status) { *count += usize::from(present); }
            genre_total += usize::from(any);
            for bucket in buckets { *genre_counts.entry(bucket).or_default() += 1; }
        }
        let mut x = 0.0f32;
        let mut chips = Vec::new();
        let mut statuses = Vec::new();
        let mut focus_x = 0.0;
        let typography = self.scale.max(0.75);
        let mut status_end = 0.0f32;
        for (i, (label, _)) in self.genre_list.iter().enumerate() {
            if i == n { status_end = x; x = 0.0; }
            let count = if i < n { status_counts[i] } else if i == n { genre_total } else { *genre_counts.get(label.as_str()).unwrap_or(&0) };
            let c = count.to_string();
            let w = 32.0 * self.scale + (label.chars().count() as f32 * 9.5 + 6.0 + c.len() as f32 * 8.6) * typography;
            if i >= n && ((self.zone == Z_CHIPS && i as i32 == self.idx) || ((self.zone != Z_CHIPS || self.idx < n as i32) && *label == self.genre)) {
                focus_x = x;
            }
            let on = if let Some(p) = self.console_chip(i) { self.platform_filter == Some(p) }
                else if i == 0 { self.status_filter == "All" && self.platform_filter.is_none() }
                else if i < n { *label == self.status_filter } else { *label == self.genre || (i == n && self.genre == "All") };
            let chip = GenreChip { index: i as i32, label: label.clone().into(), count: c.into(), on, x, w };
            if i < n { statuses.push(chip); } else { chips.push(chip); }
            x += w + 10.0 * self.scale;
        }
        let (win_w, _) = self.logical_size();
        let view_w = (win_w - 192.0 - 168.0) * self.scale;
        let max_scroll = (x - view_w).max(0.0);
        let scroll = (focus_x - view_w * 0.35).clamp(0.0, max_scroll);
        let ui = self.ui();
        ui.set_genres(model(chips));
        ui.set_status_filters(model(statuses));
        ui.set_status_filters_end(status_end);
        ui.set_genres_x(-scroll);
        ui.set_genres_more_right(x - scroll > view_w + 1.0);
    }

    pub fn push_grid(&mut self) {
        let rows = self.filtered.len().div_ceil(self.cols.max(1));
        let ui = self.ui();
        ui.set_grid_h((rows as f32 * self.row_h + 60.0) * self.scale);
        ui.set_count_label(if self.filtered.len() == self.library_total {
            format!("{} games · {} releases", self.filtered.len(), self.games.len())
        } else { format!("{} of {} games · {} releases", self.filtered.len(), self.library_total, self.games.len()) }.into());
        ui.set_grid_empty(if !self.filtered.is_empty() {
            "".into()
        } else if self.games.is_empty() {
            (if self.syncing { "Loading catalog…" } else { "Catalog is empty" }).into()
        } else if !self.searching() {
            let filtered = self.status_filter != "All" || self.platform_filter.is_some() || self.genre != "All";
            (if filtered { "No games with these filters" } else { "No games to show yet" }).into()
        } else if self.hits.scores.iter().any(Option::is_some) {
            "No matches with these filters. Choose All to see them".into()
        } else {
            format!("No games match “{}”", self.query.trim()).into()
        });
        ui.set_search_note(self.search_note.clone().into());
        if self.zone == Z_GRID && self.idx as usize >= self.filtered.len() {
            let i = self.filtered.len().saturating_sub(1) as i32;
            self.set_focus(if self.filtered.is_empty() { Z_CHIPS } else { Z_GRID }, i.max(0));
        }
        self.grid_window = (usize::MAX, 0);
        self.push_grid_window();
        // The search keyboard's best matches follow the results.
        if self.overlay == Overlay::SearchKeys {
            self.sk_push();
        }
    }

    /// Only rows around the viewport exist as UI elements; this keeps the grid instant with any catalog size.
    /// Scroll the Library to `pos` (design px from the top), gliding over `glide_ms`.
    pub fn set_grid_scroll(&mut self, pos: f32, glide_ms: i64) {
        let (_, h) = self.logical_size();
        let rows = self.filtered.len().div_ceil(self.cols.max(1));
        let max = (rows as f32 * self.row_h + 60.0 - (h - crate::library_layout::TOP)).max(0.0);
        let pos = pos.clamp(0.0, max);
        let from = self.grid_scroll;
        self.grid_scroll = pos;
        let ui = self.ui();
        ui.set_grid_glide(glide_ms.max(0));
        ui.set_grid_y(-pos * self.scale);
        // Cover the whole glide path so no row is missing mid-animation, then trim.
        self.push_grid_window_span(from.min(pos), from.max(pos));
        if (from - pos).abs() > 1.0 {
            self.grid_trim.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(glide_ms.max(0) as u64 + 60), || {
                crate::app::with_app(|app| app.push_grid_window())
            });
        }
    }

    /// Mouse wheel / touchpad over the Library. A wheel notch (60 px) moves about half a row.
    pub fn grid_wheel(&mut self, dy: f32) {
        if dy == 0.0 {
            return;
        }
        let notches = dy / 60.0;
        let is_wheel = dy.abs() >= 59.0 && (notches - notches.round()).abs() < 0.01;
        let step = if is_wheel { notches * self.row_h * 0.5 } else { dy / self.scale };
        let target = self.grid_scroll - step;
        self.set_grid_scroll(target, if is_wheel { 180 } else { 0 });
    }

    pub fn push_grid_window(&mut self) {
        let y = self.grid_scroll;
        self.push_grid_window_span(y, y);
    }

    /// Keep rows for scroll positions `lo..=hi` (plus 2 spare rows each side) in the model,
    /// adding and dropping rows at the edges instead of rebuilding the whole grid.
    fn push_grid_window_span(&mut self, lo: f32, hi: f32) {
        let (_, h) = self.logical_size();
        let view_h = h - crate::library_layout::TOP;
        let rows = self.filtered.len().div_ceil(self.cols.max(1));
        let first = ((lo / self.row_h).floor() as i64 - 2).max(0) as usize;
        let last = (((hi + view_h) / self.row_h).ceil() as usize + 2).min(rows);
        let (f0, l0) = self.grid_window;
        if (first, last) == (f0, l0) {
            return;
        }
        let m = self.grid_model.clone();
        let overlap = f0 < l0 && m.row_count() == l0 - f0 && first < l0 && last > f0;
        if !overlap {
            self.grid_window = (first, last);
            self.push_grid_rows();
        } else {
            for _ in f0..first.min(l0) {
                if first > f0 { m.remove(0); }
            }
            if last < l0 {
                for _ in last..l0 { m.remove(m.row_count() - 1); }
            }
            if first < f0 {
                for r in (first..f0).rev() {
                    let row = self.build_card_row(r);
                    m.insert(0, row);
                }
            }
            if last > l0 {
                for r in l0..last {
                    let row = self.build_card_row(r);
                    m.push(row);
                }
            }
            self.grid_window = (first, last);
            let (lo_k, hi_k) = (first * self.cols, last * self.cols);
            self.grid_keys.retain(|_, cards| {
                cards.retain(|k| *k >= lo_k && *k < hi_k);
                !cards.is_empty()
            });
            let keep: HashSet<String> = self.grid_keys.keys().cloned().collect();
            self.images.cancel_unless(move |j| j.prio != prio::CARD || keep.contains(&j.key));
        }
        // Look ahead so covers are ready before they scroll in.
        let ahead_to = ((last + 3) * self.cols).min(self.filtered.len());
        for k in (last * self.cols).min(ahead_to)..ahead_to {
            if let Some(r) = self.card_req(self.filtered[k]) {
                self.images.want(&r.key, r.src, r.w, r.crop, prio::PREFETCH);
            }
        }
    }

    fn build_card_row(&mut self, r: usize) -> CardRow {
        let mut cards = Vec::new();
        for c in 0..self.cols {
            let k = r * self.cols + c;
            let Some(&gi) = self.filtered.get(k) else { break };
            let req = self.card_req(gi);
            let img = self.get_img(&req, prio::CARD);
            if let Some(r) = &req {
                // Several cards can show the same cover (releases sharing Steam's art).
                let cards = self.grid_keys.entry(r.key.clone()).or_default();
                if !cards.contains(&k) {
                    cards.push(k);
                }
            }
            let g = &self.games[gi];
            let ce = self.game_compat(g);
            // Covers show Linux results only; the Game Hub has the rest.
            let (compat, compat_level) = ce.and_then(|e| Some((e.tag()?, e.status.level()))).unwrap_or_default();
            // Size is always visible. Version/region details belong to the selected Hub release.
            // This platform's releases, as the Game Hub numbers them ("Release 1 of 2").
            let releases = self.release_order(gi).len();
            let meta = size_label(&g.g).unwrap_or_else(|| "Size unknown".into());
            cards.push(CardData {
                index: k as i32,
                title: self.game_title(gi).into(),
                meta: meta.into(),
                releases: if releases > 1 { format!("{releases} releases") } else { String::new() }.into(),
                availability: crate::words::Availability::of(g.g.seeders).label().into(),
                availability_kind: crate::words::Availability::of(g.g.seeders).kind(),
                rating: g.info.as_ref().and_then(|i| i.rating.as_ref()).map(|r| format!("{:.1}", r.score)).unwrap_or_default().into(),
                badge: if g.local.is_some() { "Installed".into() } else if g.is_new { "New".into() } else { "".into() },
                console: if self.mixed_consoles { g.g.platform.label().into() } else { "".into() },
                // Every platform the game is on, in the usual order.
                consoles: model(self.game_platforms(gi).into_iter().filter(|_| self.mixed_consoles)
                    .map(|p| SharedString::from(p.label())).collect()),
                compat: compat.into(),
                compat_level,
                loaded: img.is_some(),
                no_art: img.is_none() && req.as_ref().is_none_or(|r| self.images.has_failed(&r.key)),
                image: img.unwrap_or_default(),
            });
        }
        CardRow { index: r as i32, cards: model(cards) }
    }

    pub fn push_grid_rows(&mut self) {
        let (first, last) = self.grid_window;
        self.grid_keys.clear();
        let rows: Vec<CardRow> = (first..last.max(first)).map(|r| self.build_card_row(r)).collect();
        let keep: HashSet<String> = self.grid_keys.keys().cloned().collect();
        self.images.cancel_unless(move |j| j.prio != prio::CARD || keep.contains(&j.key));
        self.grid_model.set_vec(rows);
    }

    /// A cover finished loading: update just the cards showing it, not the whole grid.
    fn grid_image_loaded(&mut self, key: &str) {
        let Some(cards) = self.grid_keys.get(key).cloned() else { return };
        let img = self.images.get(key);
        // Neither loaded nor failed (yet): nothing to show.
        if img.is_none() && !self.images.has_failed(key) {
            return;
        }
        let (first, _) = self.grid_window;
        for k in cards {
            let (r, c) = (k / self.cols.max(1), k % self.cols.max(1));
            if r < first {
                continue;
            }
            if let Some(row) = self.grid_model.row_data(r - first) {
                if let Some(mut card) = row.cards.row_data(c) {
                    // A cover that couldn't be downloaded: the game's name instead.
                    card.no_art = img.is_none();
                    card.loaded = img.is_some();
                    card.image = img.clone().unwrap_or_default();
                    row.cards.set_row_data(c, card);
                }
            }
        }
    }

    pub fn first_visible_card(&self) -> usize {
        let y = self.grid_scroll;
        // First row whose top is on screen (a row scrolled away by a few pixels still counts).
        let row = ((y - 20.0) / self.row_h).ceil().max(0.0) as usize;
        (row * self.cols).min(self.filtered.len().saturating_sub(1))
    }

    pub fn ensure_grid_visible(&mut self) {
        let (_, h) = self.logical_size();
        let view_h = h - crate::library_layout::TOP;
        let row = self.idx.max(0) as usize / self.cols.max(1);
        let top = row as f32 * self.row_h;
        let y = self.grid_scroll;
        // Rows sit on a grid of row_h (plus a fixed top margin in the UI), so scrolling to a
        // multiple of row_h leaves no part of the row above peeking out under the header.
        let new_y = if top < y {
            top
        } else if top + self.row_h > y + view_h {
            let min_y = top + self.row_h - view_h + 20.0;
            let snapped = (min_y / self.row_h).ceil() * self.row_h;
            // If the selected row doesn't fit below a whole row, put it at the top instead.
            if snapped <= top { snapped } else { top }
        } else {
            return;
        };
        self.set_grid_scroll(new_y, 260);
    }

    /// The Library's backdrop follows the selected game, once the selection rests for a moment
    /// (scrolling past twenty games shouldn't start twenty full-screen downloads).
    pub fn focus_card_changed(&mut self) {
        self.schedule_rest_prefetch();
        self.bg_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(160), || {
            crate::app::with_app(|app| {
                if app.view != 1 || app.zone != Z_GRID || app.overlay != Overlay::None {
                    return;
                }
                if let Some(&gi) = app.filtered.get(app.idx as usize) {
                    let t = Target { game: Some(gi), local: app.games[gi].local };
                    let (bg, cover) = (app.target_bg_url(t), app.cover_req(t));
                    app.want_background_or(bg, false, cover);
                }
            })
        });
    }

    pub fn cycle_sort(&mut self, d: i32) {
        self.sort = (self.sort as i32 + d).rem_euclid(SORTS.len() as i32) as usize;
        audio_move();
        self.apply_filter();
        self.set_grid_scroll(0.0, 0);
        self.push_library();
    }

    // ------------------------------------------------------------------ game hub

    pub fn push_hub(&mut self) {
        let Some(t) = self.hub else { return };
        let info = self.target_info(t);
        let mut keys = HashSet::new();
        let mut h = HubData::default();
        let (k, a, live) = self.kicker_for(t, true);
        h.kicker = k.into();
        h.kicker_accent = a.into();
        h.kicker_live = live;
        h.title = target_name(self, t).into();

        let lr = Self::logo_req(info.as_ref());
        if let Some(r) = &lr {
            keys.insert(r.key.clone());
            match self.get_img(&lr, prio::DETAIL) {
                Some(img) => {
                    let (w, hh) = logo_size(&img, 700.0, 220.0);
                    h.logo = img;
                    h.has_logo = true;
                    h.logo_w = w * self.scale;
                    h.logo_h = hh * self.scale;
                }
                None if self.images.on_disk(&r.key, &r.src) => h.title = SharedString::default(),
                None => {}
            }
        }
        // Same image as the Library card: it's already on disk, so the Hub opens with it instantly.
        let cover = t.game.and_then(|g| self.card_req(g))
            .or_else(|| info.as_ref().and_then(|i| req_url(&i.portrait, 440, 0.0)))
            .or_else(|| info.as_ref().and_then(|i| req_url(&i.master, 440, 0.0)))
            .or_else(|| t.local.and_then(|l| self.locals[l].l.icon0.clone()).map(|p| req_file(&p, 512)));
        if let Some(r) = &cover {
            keys.insert(r.key.clone());
        }
        if let Some(img) = self.get_img(&cover, prio::DETAIL) {
            h.cover = img;
            h.cover_loaded = true;
        }
        h.no_art = !h.cover_loaded && cover.as_ref().is_none_or(|r| self.images.has_failed(&r.key));
        h.console = self.target_platform(t).label().into();

        // KytyPS5 status is in the facts grid below, so it isn't repeated as a chip here.
        let mut chips: Vec<ChipData> = Vec::new();
        if let Some(r) = info.as_ref().and_then(|i| i.rating.as_ref()) {
            chips.push(ChipData { stars: r.score as f32, ..chip("", format!("{:.2} · {} ratings", r.score, thousands(r.total)), false) });
        }
        if let Some(a) = info.as_ref().and_then(|i| i.age.as_ref()).filter(|a| !a.name.is_empty()) {
            chips.push(chip("", a.name.clone(), false));
        }
        let genres: Vec<String> = info.as_ref().filter(|i| !i.genres.is_empty()).map(|i| i.genres.clone())
            .or_else(|| t.game.map(|g| self.games[g].buckets.iter().map(|s| s.to_string()).collect())).unwrap_or_default();
        for g in genres.into_iter().take(3) {
            chips.push(chip("", g, false));
        }
        h.chips = model(chips);

        self.hub_actions = self.actions_for(t, true);
        h.actions = model(self.hub_actions.iter().map(action_data).collect());

        let g_owned = t.game.map(|i| self.games[i].g.clone());
        let g = g_owned.as_ref();
        let region = g.map(|g| g.region.clone()).unwrap_or_default();
        let ce = self.target_compat(t).cloned();
        let ps4 = self.target_platform(t) == crate::platform::Platform::Ps4;
        let mut facts: Vec<(&str, String)> = vec![
            ("YOUR RESULT", ce.as_ref().filter(|e| e.mine).map(|e| e.status.meaning().to_string()).unwrap_or_default()),
            ("ON LINUX", ce.as_ref().and_then(|e| e.linux).map(|l| l.meaning().to_string()).unwrap_or_else(|| "Not tested yet".into())),
            ("ON WINDOWS", ce.as_ref().and_then(|e| e.windows).map(|w| w.meaning().to_string()).unwrap_or_default()),
            // Short values only: a fact box fits ~28 characters.
            ("LAST TESTED", ce.as_ref().map(|e| {
                let d = util::fmt_date(util::parse_iso_date(&e.tested_date()));
                let r = match e.reports { 0 => String::new(), 1 => "1 report".to_string(), n => format!("{n} reports") };
                [if d.is_empty() { e.tested_date() } else { d }, r].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
            }).unwrap_or_default()),
            ("TESTED ON", ce.as_ref().map(|e| e.platforms.join(", ")).unwrap_or_default()),
            ("PUBLISHER", info.as_ref().map(|i| i.publisher.clone()).filter(|publisher| !publisher.is_empty()).unwrap_or_else(|| g.map(|g| g.detail("publisher")).unwrap_or_default())),
            ("RELEASE", g.map(|g| g.release.clone()).filter(|s| !s.is_empty()).or_else(|| info.as_ref().map(|i| util::fmt_date(util::parse_iso_date(&i.release)))).unwrap_or_default()),
            ("SIZE", g.map(|g| g.size.clone()).unwrap_or_default()),
            ("MODE", g.map(|g| g.mode.clone()).filter(|s| !s.is_empty()).or_else(|| info.as_ref().and_then(|i| i.players.as_ref()).and_then(|p| p.as_i64()).map(|p| format!("{p} player{}", if p > 1 { "s" } else { "" }))).unwrap_or_default()),
            ("TITLE ID", g.map(|g| g.title_id.clone()).filter(|s| !s.is_empty()).or_else(|| t.local.map(|l| self.locals[l].l.title_id.clone())).unwrap_or_default()),
            ("REGION", region),
            ("UPDATE", g.map(|g| g.update.clone()).unwrap_or_default()),
        ];
        if let Some(game) = g {
            let observed = util::fmt_date(util::parse_iso_date(&game.peers_observed));
            facts.extend([
                // Behind "Availability": the catalog's counts, not live ones.
                ("SHARING IT (SEEDERS)", game.seeders.map(|count| count.to_string()).unwrap_or_else(|| "Unknown".into())),
                ("GETTING IT (LEECHERS)", game.leechers.map(|count| count.to_string()).unwrap_or_else(|| "Unknown".into())),
                ("COUNTED", if observed.is_empty() { "Date unknown".into() } else { format!("{observed} · not live") }),
            ]);
            facts.extend([
                ("RELEASE VERSION", game.version.clone()),
                ("DEVELOPER", game.detail("developer")),
                ("FORMAT", game.detail("format")),
                ("CONSOLE FIRMWARE NOTE", game.detail("minimum_firmware")),
                ("INTERFACE LANGUAGES", game.detail("interface_languages")),
                ("AUDIO LANGUAGES", game.detail("audio_languages")),
                ("SOURCE", "RuTracker".to_string()),
            ]);
        }
        if let Some(l) = t.local {
            let lv = &self.locals[l];
            let pt = self.playtime(&lv.l);
            facts.push(("PLAY TIME", if pt.total > 0.0 { util::fmt_duration(pt.total) } else { "Never played".into() }));
            facts.push(("LAST PLAYED", util::fmt_last_played(pt.last)));
            facts.push(("INSTALLED VERSION", lv.l.version.clone()));
            facts.push(("LOCATION", util::display_path(&lv.l.path.to_string_lossy())));
        }
        // A compact overview; technical provenance and installation data are expandable rows.
        let console = if ps4 || self.mixed_consoles { self.target_platform(t).label().to_string() } else { String::new() };
        h.subtitle = [
            console,
            info.as_ref().map(|i| i.publisher.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| g.map(|g| g.detail("publisher")).unwrap_or_default()),
            g.map(|g| g.release.clone()).unwrap_or_default(), g.map(|g| g.region.clone()).unwrap_or_default(),
        ].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ").into();
        // The game on other platforms: "Play it on" PS5 / PS4 / PC (versions.rs).
        h.versions = model(self.version_tiles(t));
        if let Some(index) = t.game {
            // "Version 3 of 3 (newest)": releases numbered oldest first; no topic numbers.
            let members = self.release_order(index);
            h.release_count = members.len() as i32;
            let position = members.iter().position(|i| *i == index).unwrap_or(0) + 1;
            let newest = if position == members.len() { " (newest)" } else { "" };
            h.release_label = format!("Release {position} of {}{newest}", members.len()).into();
            let game = &self.games[index].g;
            h.release_detail = [game.version.clone(), game.region.clone(), size_label(game).unwrap_or_default(), game.name.clone()].into_iter()
                .filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ").into();
        }

        // Will it play here? One headline, with the results behind it folded underneath.
        if let Some(outlook) = self.outlook(t) {
            h.verdict = outlook.headline().into();
            h.verdict_kind = outlook.kind();
            h.verdict_detail = self.verdict_detail(t).into();
        }
        if let Some((note, warn)) = self.download_note(t) {
            h.download_note = note.into();
            h.download_note_warn = warn;
        }

        // PC games: how it runs on Linux, natively or through Wine (with ProtonDB's rating).
        let pc_game = self.target_platform(t) == crate::platform::Platform::Pc;
        let steam = g.and_then(|g| g.steam.clone());
        // How it runs, said as on its "Play it on" tile (versions.rs): native, or Wine-packed,
        // or an installed .exe (catalogued or not) through Proton or Wine.
        let pc_runs: Option<(&str, String, i32)> = pc_game.then(|| self.plays_for(t).hub_fact());
        // Three overview boxes: how it runs (PC games), how easy it is to get, which version.
        // The size is in the line under Download.
        let availability = g.filter(|_| t.local.is_none()).map(|g| crate::words::Availability::of(g.seeders));
        let mode = g.map(|g| g.mode.clone()).filter(|s| !s.is_empty())
            .or_else(|| info.as_ref().and_then(|i| i.players.as_ref()).and_then(|p| p.as_i64()).map(|p| format!("{p} player{}", if p > 1 { "s" } else { "" })))
            .unwrap_or_default();
        let summary: Vec<Fact> = [
            pc_runs.map(|(k, v, kind)| (k, v, kind)),
            availability.map(|a| ("AVAILABILITY", a.label().to_string(), a.kind())),
            Some(if let Some(local) = t.local { ("INSTALLED VERSION", self.locals[local].l.version.clone(), 0) }
                else { ("VERSION", g.map(|g| g.version.clone()).unwrap_or_default(), 0) }),
            Some(("PLAYERS", mode, 0)),
            Some(("DOWNLOAD SIZE", g.and_then(size_label).unwrap_or_default(), 0)),
        ].into_iter().flatten().filter(|(_, v, _)| !v.is_empty()).take(3)
            .map(|(k, v, kind)| Fact { key: k.into(), value: v.into(), kind }).collect();
        h.facts = model(summary.chunks(3).map(|c| FactRow { items: model(c.to_vec()) }).collect());
        let mut facts = facts;
        if pc_game {
            // The emulator lists and console fields don't apply to PC games; Steam's do.
            facts.retain(|(k, _)| !matches!(*k, "YOUR RESULT" | "ON LINUX" | "ON WINDOWS" | "LAST TESTED" | "TESTED ON" | "TITLE ID" | "CONSOLE FIRMWARE NOTE" | "UPDATE"));
            if let Some(s) = &steam {
                facts.push(("METACRITIC", s.metacritic.map(|m| m.to_string()).unwrap_or_default()));
                facts.push(("STEAM", if s.linux { "Has a Linux version".into() } else { "Windows only on Steam".into() }));
                if !s.developers.is_empty() { facts.push(("DEVELOPER", s.developers.join(", "))); }
            }
            if let Some(m) = t.local.and_then(|l| self.locals[l].l.pc.as_ref()) {
                facts.push(("STARTS WITH", m.launch.clone()));
            }
        }
        let details: Vec<Fact> = facts.into_iter().filter(|(k, v)| !v.is_empty()
            && !matches!(*k, "SIZE" | "PUBLISHER" | "RELEASE" | "REGION"))
            .map(|(k, v)| Fact { key: k.into(), value: v.into(), kind: 0 }).collect();
        h.details = model(vec![FactRow { items: model(details) }]);

        let shots_urls = self.hub_shots.clone();
        let mut shots = Vec::new();
        for u in &shots_urls {
            let r = req_url(u, 480, 0.0);
            if let Some(r) = &r {
                keys.insert(r.key.clone());
            }
            let img = self.get_img(&r, prio::DETAIL - 1);
            shots.push(Shot { loaded: img.is_some(), image: img.unwrap_or_default() });
        }
        h.shots = model(shots);

        let about: Vec<SharedString> = match info.as_ref().filter(|i| !i.long.is_empty()) {
            Some(i) => i.long.split('\n').map(str::trim).filter(|s| !s.is_empty()).map(SharedString::from).collect(),
            None => g.map(|g| if g.description.is_empty() { vec![g.excerpt.clone()] } else { g.description.clone() }).unwrap_or_default()
                .into_iter().filter(|s| !s.is_empty()).map(SharedString::from).collect(),
        };
        h.about = model(about);
        self.hub_keys = keys;
        if self.overlay == Overlay::Hub && self.zone == Z_HUB && self.idx as usize >= self.hub_actions.len() {
            self.set_focus(Z_HUB, 0);
        }
        self.ui().set_hub(h);
    }

    /// The results behind the Hub's verdict, in smaller text under it: what was seen on Linux
    /// and elsewhere, where that comes from, and this PC.
    fn verdict_detail(&self, t: Target) -> String {
        let platform = self.target_platform(t);
        let low = |s: &str| -> String { let mut c = s.chars(); c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default() };
        let mut lines = Vec::new();
        match self.target_compat(t) {
            Some(e) => {
                let mut seen = Vec::new();
                if e.mine {
                    seen.push(format!("Your result on this PC: {}", low(e.status.meaning())));
                }
                match e.linux {
                    Some(l) => seen.push(format!("{} Linux: {}", if e.mine { "Others on" } else { "On" }, low(l.meaning()))),
                    None if !e.mine => seen.push("On Linux: not tested yet".into()),
                    None => {}
                }
                if let Some(w) = e.windows {
                    seen.push(format!("On Windows: {}", low(w.meaning())));
                } else if !e.on_linux && !e.mine {
                    seen.push(format!("On {}: {}", e.source(), low(e.status.meaning())));
                }
                lines.push(seen.join(" · "));
                let date = util::fmt_date(util::parse_iso_date(&e.tested_date()));
                let reports = match e.reports { 0 => String::new(), 1 => " (1 report)".into(), n => format!(" ({n} reports)") };
                if e.reports > 0 || !date.is_empty() {
                    lines.push(format!("From the {} community list{}{reports}", platform.emulator(),
                        if date.is_empty() { String::new() } else { format!(", last tested {date}") }));
                }
            }
            None => lines.push(format!("Nobody has reported how far it gets on {} yet", platform.emulator())),
        }
        lines.extend(self.pc_line(t));
        lines.join("\n")
    }

    pub fn scroll_hub(&mut self) {
        // Rough layout estimate (see app.slint hub column).
        let facts_rows = self.ui().get_hub().facts.row_count() as f32;
        let details = if self.ui().get_hub_details_open() {
            self.ui().get_hub().details.iter().map(|row| row.items.row_count()).sum::<usize>() as f32 * 38.0
        } else { 0.0 };
        // The verdict (~100) and the line under Download (~36) sit above the screenshots too.
        let versions = if self.ui().get_hub().versions.row_count() > 0 { 170.0 } else { 0.0 };
        let shots_top = 130.0 + 30.0 + 16.0 + 210.0 + 22.0 + 40.0 + 30.0 + 68.0 + 160.0 + 80.0 + versions + facts_rows * 106.0 + details;
        let ui = self.ui();
        // Never scroll past the real end of the page (measured by the UI, not estimated).
        let max = ((ui.get_hub_content_h() - ui.get_hub_view_h()) / self.scale).max(0.0);
        let y = match self.zone {
            Z_SHOTS => (shots_top - 320.0).max(0.0),
            Z_DESC => shots_top + if self.hub_shots.is_empty() { 0.0 } else { 230.0 } - 200.0 + self.idx as f32 * 360.0,
            _ => 0.0,
        };
        if self.zone == Z_DESC && y > max && self.idx > 0 {
            // At the bottom already: don't let further presses build up.
            self.idx = ((max - (shots_top + if self.hub_shots.is_empty() { 0.0 } else { 230.0 } - 200.0)) / 360.0).ceil().max(0.0) as i32;
        }
        ui.set_hub_y(-y.clamp(0.0, max) * self.scale);
    }

    /// The viewer's full-size copies of the selected screenshot and the next one.
    pub fn prefetch_viewer(&mut self) {
        for d in 0..2 {
            if let Some(u) = self.hub_shots.get(self.idx.max(0) as usize + d).cloned() {
                if let Some(r) = req_url(&u, 1920, 0.0) {
                    self.images.want(&r.key, r.src, r.w, r.crop, prio::PREFETCH + 2);
                }
            }
        }
    }

    pub fn scroll_shots(&mut self) {
        self.prefetch_viewer();
        let (w, _) = self.logical_size();
        let view = (w - 540.0 - 96.0).min(1060.0) + 60.0;
        let x = self.idx as f32 * 314.0;
        let total = self.hub_shots.len() as f32 * 314.0 + 72.0;
        self.ui().set_shots_x(-(x - view * 0.4).clamp(0.0, (total - view).max(0.0)) * self.scale);
    }

    pub fn push_viewer(&mut self) {
        let Some(u) = self.hub_shots.get(self.viewer_i).cloned() else { return };
        let r = req_url(&u, 1920, 0.0);
        self.viewer_key = r.as_ref().map(|r| r.key.clone()).unwrap_or_default();
        let img = self.get_img(&r, prio::HERO);
        // Preload the neighbours for instant browsing.
        for d in [1, self.hub_shots.len().saturating_sub(1)] {
            if let Some(n) = self.hub_shots.get((self.viewer_i + d) % self.hub_shots.len().max(1)).cloned() {
                let r = req_url(&n, 1920, 0.0);
                self.get_img(&r, prio::PREFETCH);
            }
        }
        let ui = self.ui();
        // Show the already-loaded thumbnail instantly while the full-size image decodes.
        let thumb = req_url(&u, 480, 0.0).and_then(|r| self.images.get(&r.key));
        if let Some(img) = img.or(thumb) {
            ui.set_viewer_image(img);
        }
        ui.set_viewer_count(format!("{} / {}", self.viewer_i + 1, self.hub_shots.len()).into());
    }

    pub fn push_menu(&mut self, title: &str) {
        let items: Vec<MenuItem> = self.menu_actions.iter().map(|a| MenuItem { label: a.label.clone().into(), icon: a.icon.into() }).collect();
        let ui = self.ui();
        ui.set_menu_title(title.into());
        ui.set_menu_items(model(items));
    }

    pub fn push_launch(&mut self, l: usize) {
        self.launch_local = Some(l);
        let t = Target { game: self.locals[l].cat, local: Some(l) };
        let bg = self.target_bg_url(t);
        let icon = self.tile_req(RowItem::Local(l));
        self.launch_keys = [&bg, &icon].iter().filter_map(|r| r.as_ref().map(|r| r.key.clone())).collect();
        let (bgi, ici) = (self.get_img(&bg, prio::HERO), self.get_img(&icon, prio::HERO));
        let ui = self.ui();
        ui.set_launch_name(self.locals[l].name.clone().into());
        ui.set_launch_bg(bgi.unwrap_or_default());
        ui.set_launch_icon(ici.unwrap_or_default());
    }

    /// Row by row, so the cards already showing keep their place and their animations.
    pub fn push_toasts(&mut self) {
        let m = &self.toast_model;
        let mut row = 0;
        while row < m.row_count() {
            let shown = m.row_data(row);
            match self.toasts.iter().find(|t| shown.as_ref().is_some_and(|s| s.id == t.id)) {
                Some(t) => {
                    if shown.as_ref() != Some(t) { m.set_row_data(row, t.clone()); }
                    row += 1;
                }
                None => { m.remove(row); }
            }
        }
        for t in &self.toasts {
            if !m.iter().any(|shown| shown.id == t.id) { m.push(t.clone()); }
        }
    }
}

fn audio_move() {
    crate::audio::play(crate::audio::Sound::Move);
}

/// A release's download size, the same everywhere it's shown ("183.4 GB", "766.9 MB").
pub fn size_label(g: &crate::catalog::Game) -> Option<String> {
    g.size_gb.map(|gb| if gb >= 1.0 { format!("{gb:.1} GB") } else { format!("{:.1} MB", gb * 1024.0) })
        .or_else(|| (!g.size.is_empty()).then(|| g.size.clone()))
}

pub fn thousands(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}…", cut.rsplit_once(' ').map(|x| x.0).unwrap_or(&cut))
}
