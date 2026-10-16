use crate::update::{Screen, SettingsPage, State};
use application::config::Config;
use application::session::{choose_runner, resolve_exe, RunnerLocator};
use application::{assess, library, settings as app_settings};
use kernel::*;
use serde::Serialize;

/// Everything a view is a pure function of, besides `State`.
pub struct Env<'a> {
    pub reg: &'a Registry,
    pub cfg: &'a Config,
    pub games: &'a [InstalledGame],
    pub bundle: &'a Bundle,
    pub facts: &'a Facts,
    pub locator: &'a dyn RunnerLocator,
    pub games_dir: Option<&'a std::path::Path>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chip { pub id: Option<String>, pub name: String, pub color: String, pub count: usize, pub selected: bool }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card { pub system: String, pub key: String, pub title: String, pub short: String, pub color: String, pub cover_w: u16, pub cover_h: u16 }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Empty { NoSystems, NoGames, NoMatches }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryView { pub chips: Vec<Chip>, pub cards: Vec<Card>, pub empty: Option<Empty>, pub hints: Vec<String> }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Option_ { pub value: String, pub label: String }

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "widget", rename_all = "snake_case")]
pub enum Widget {
    Switch { on: bool },
    Dropdown { options: Vec<Option_>, selected: String },
    Stepper { value: i64, min: i64, max: i64, step: i64 },
    TextField { value: String },
    PathField { value: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingRow { pub runner: String, pub key: String, pub label: String, pub hint: String, pub advanced: bool, pub widget: Widget }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunnerOption { pub id: String, pub name: String, pub selected: bool, pub installed: bool }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check { pub blocks: bool, pub text: String }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HubView {
    pub title: String, pub short: String, pub color: String,
    pub runners: Vec<RunnerOption>,
    /// No installed runner supports this game's system (or its system is gone).
    pub no_runner: bool,
    pub can_play: bool,
    pub checks: Vec<Check>,
    pub settings: Vec<SettingRow>,
    pub controls: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NavItem { pub heading: bool, pub label: String, pub active: bool }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginRow { pub id: String, pub version: String, pub provides: Vec<String>, pub contacts: Vec<String>, pub writes: Vec<String> }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Kept { pub what: String, pub detail: String }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum SettingsBody { Plugins { plugins: Vec<PluginRow>, kept: Vec<Kept> }, Runner { title: String, rows: Vec<SettingRow> } }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsView { pub nav: Vec<NavItem>, pub body: SettingsBody }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum View { Library(LibraryView), Hub(HubView), Settings(SettingsView) }

fn widget(spec: &SettingSpec, value: &Value, b: &Bundle) -> Widget {
    match (&spec.kind, value) {
        (SettingKind::Bool, v) => Widget::Switch { on: v.truthy() },
        (SettingKind::Int { min, max, step }, Value::Int(i)) => Widget::Stepper { value: *i, min: *min, max: *max, step: *step },
        (SettingKind::Int { min, max, step }, _) => Widget::Stepper { value: *min, min: *min, max: *max, step: *step },
        (SettingKind::Choice { options }, v) => Widget::Dropdown {
            options: options.iter().map(|o| Option_ { value: o.value.clone(), label: b.render(&o.label) }).collect(),
            selected: v.render(),
        },
        (SettingKind::Text, v) => Widget::TextField { value: v.render() },
        (SettingKind::Path, v) => Widget::PathField { value: v.render() },
    }
}

fn rows(runner: &RunnerId, specs: &[SettingSpec], eff: &Settings, b: &Bundle) -> Vec<SettingRow> {
    specs.iter().map(|s| SettingRow {
        runner: runner.to_string(), key: s.key.clone(), label: b.render(&s.label), hint: b.render(&s.hint),
        advanced: s.advanced, widget: widget(s, &eff[&s.key], b),
    }).collect()
}

fn library(st: &State, e: &Env) -> LibraryView {
    let installed: Vec<&InstalledGame> = e.games.iter().filter(|g| e.reg.system(&g.game.system).is_some()).collect();
    let chips = {
        let mut v = vec![Chip { id: None, name: e.bundle.render(&Text::new("library.all")), color: "#e9ecf1".into(), count: installed.len(), selected: st.system_filter.is_none() }];
        for s in e.reg.systems() {
            let d = s.descriptor();
            v.push(Chip {
                id: Some(d.id.to_string()), name: e.bundle.render(&d.name), color: d.style.color.clone(),
                count: installed.iter().filter(|g| g.game.system == d.id).count(), selected: st.system_filter.as_ref() == Some(&d.id),
            });
        }
        v
    };
    let q = st.query.to_lowercase();
    let cards: Vec<Card> = installed.iter()
        .filter(|g| st.system_filter.as_ref().map(|f| &g.game.system == f).unwrap_or(true))
        .filter(|g| q.is_empty() || g.name.to_lowercase().contains(&q))
        .map(|g| {
            let d = e.reg.system(&g.game.system).unwrap().descriptor();
            Card { system: d.id.to_string(), key: g.game.key.to_string(), title: g.name.clone(), short: e.bundle.render(&d.short),
                   color: d.style.color.clone(), cover_w: d.style.cover.w, cover_h: d.style.cover.h }
        }).collect();
    let empty = if e.reg.systems().is_empty() { Some(Empty::NoSystems) }
        else if installed.is_empty() { Some(Empty::NoGames) }
        else if cards.is_empty() { Some(Empty::NoMatches) } else { None };
    LibraryView { chips, cards, empty, hints: vec!["select".into(), "details".into(), "search".into(), "settings".into()] }
}

fn hub(g: &GameRef, e: &Env) -> Option<HubView> {
    let game = e.games.iter().find(|x| &x.game == g)?;
    let sys = e.reg.system(&g.system)?;
    let d = sys.descriptor();
    let chosen = choose_runner(e.reg, e.cfg, &g.system);
    let runners: Vec<RunnerOption> = e.reg.runners_for(&g.system).iter().map(|r| {
        let rd = r.descriptor();
        RunnerOption { id: rd.id.to_string(), name: e.bundle.render(&rd.name), selected: chosen.map(|c| c.descriptor().id == rd.id).unwrap_or(false), installed: resolve_exe(e.locator, rd).is_some() }
    }).collect();
    let (checks, settings, controls, can_play) = match chosen {
        None => (vec![], vec![], vec![], false),
        Some(r) => {
            let rd = r.descriptor();
            let v = assess::verdict(e.reg, &rd.id, e.facts).unwrap_or_default();
            let mut checks: Vec<Check> = v.blocks.iter().map(|t| Check { blocks: true, text: e.bundle.render(t) }).collect();
            checks.extend(v.slows.iter().map(|t| Check { blocks: false, text: e.bundle.render(t) }));
            let eff = effective(&rd.settings, &e.cfg.runner_settings(&rd.id));
            let controls = rd.controls.as_ref().map(|c| c.rows.iter().map(|r| (e.bundle.render(&r.action), r.input.clone())).collect()).unwrap_or_default();
            (checks, rows(&rd.id, &rd.settings, &eff, e.bundle), controls, resolve_exe(e.locator, rd).is_some())
        }
    };
    Some(HubView {
        title: game.name.clone(), short: e.bundle.render(&d.short), color: d.style.color.clone(),
        no_runner: chosen.is_none(), runners, can_play, checks, settings, controls,
    })
}

fn settings(page: &SettingsPage, e: &Env) -> SettingsView {
    let pages = app_settings::pages(e.reg, e.cfg);
    let mut nav = vec![NavItem { heading: true, label: e.bundle.render(&Text::new("settings.plugins")), active: false },
                       NavItem { heading: false, label: e.bundle.render(&Text::new("settings.overview")), active: *page == SettingsPage::Plugins }];
    for p in &pages {
        nav.push(NavItem { heading: false, label: e.bundle.render(&p.title), active: *page == SettingsPage::Runner(p.runner.clone()) });
    }
    let body = match page {
        SettingsPage::Runner(id) if pages.iter().any(|p| &p.runner == id) => {
            let p = pages.iter().find(|p| &p.runner == id).unwrap();
            let eff = effective(&p.rows.iter().map(|r| r.spec.clone()).collect::<Vec<_>>(), &e.cfg.runner_settings(id));
            SettingsBody::Runner { title: e.bundle.render(&p.title), rows: rows(id, &p.rows.iter().map(|r| r.spec.clone()).collect::<Vec<_>>(), &eff, e.bundle) }
        }
        _ => {
            let plugins = e.reg.manifests().iter().map(|m| PluginRow {
                id: m.id.clone(), version: m.version.clone(), provides: e.reg.provides(&m.id),
                contacts: m.permissions.network.clone(), writes: m.permissions.writes.clone(),
            }).collect();
            let mut kept: Vec<Kept> = app_settings::kept_for_absent_plugins(e.reg, e.cfg).into_iter()
                .map(|id| Kept { what: id, detail: "settings".into() }).collect();
            if let Some(dir) = e.games_dir {
                kept.extend(library::kept_for_absent_systems(e.reg, dir).into_iter().map(|k| Kept { what: k.folder, detail: format!("{} games", k.games) }));
            }
            SettingsBody::Plugins { plugins, kept }
        }
    };
    SettingsView { nav, body }
}

pub fn view(st: &State, e: &Env) -> View {
    match &st.screen {
        Screen::Library => View::Library(library(st, e)),
        // A game whose system was removed after navigation falls back to the library, never a crash.
        Screen::Hub(g) => hub(g, e).map(View::Hub).unwrap_or_else(|| View::Library(library(&State::default(), e))),
        Screen::Settings(p) => View::Settings(settings(p, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use testkit::ToyPlugin;

    struct Loc(bool);
    impl RunnerLocator for Loc { fn executable(&self, _: &RunnerDescriptor) -> Option<PathBuf> { self.0.then(|| "/r/exe".into()) } }

    fn game(key: &str) -> InstalledGame {
        InstalledGame { game: GameRef { system: SystemId::new("toy-sys"), key: GameKey::new(key) }, name: key.to_uppercase(), root: format!("/g/{key}").into(), extra: serde_json::Value::Null }
    }
    fn env<'a>(reg: &'a Registry, cfg: &'a Config, games: &'a [InstalledGame], b: &'a Bundle, f: &'a Facts, l: &'a dyn RunnerLocator) -> Env<'a> {
        Env { reg, cfg, games, bundle: b, facts: f, locator: l, games_dir: None }
    }

    #[test]
    fn library_chips_and_cards_come_from_descriptors() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let (cfg, b, f, l) = (Config::default(), Bundle::new(), Facts::new(), Loc(true));
        let games = [game("a"), game("b")];
        let View::Library(v) = view(&State::default(), &env(&reg, &cfg, &games, &b, &f, &l)) else { panic!() };
        assert_eq!(v.chips.len(), 2);
        assert_eq!(v.chips[1].name, "Toy System");
        assert_eq!(v.chips[1].count, 2);
        assert_eq!((v.cards[0].cover_w, v.cards[0].cover_h), (3, 4));
        assert_eq!(v.empty, None);
    }

    #[test]
    fn removing_the_system_removes_its_chip_and_cards_and_shows_the_empty_state() {
        let none = Registry::build(&[]).unwrap();
        let (cfg, b, f, l) = (Config::default(), Bundle::new(), Facts::new(), Loc(true));
        let games = [game("a")];
        let View::Library(v) = view(&State::default(), &env(&none, &cfg, &games, &b, &f, &l)) else { panic!() };
        assert_eq!(v.chips.len(), 1, "only the All chip remains");
        assert!(v.cards.is_empty());
        assert_eq!(v.empty, Some(Empty::NoSystems));
    }

    #[test]
    fn filter_and_search_narrow_cards() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let (cfg, b, f, l) = (Config::default(), Bundle::new(), Facts::new(), Loc(true));
        let games = [game("alpha"), game("beta")];
        let st = State { query: "alp".into(), ..State::default() };
        let View::Library(v) = view(&st, &env(&reg, &cfg, &games, &b, &f, &l)) else { panic!() };
        assert_eq!(v.cards.len(), 1);
        let st = State { query: "zzz".into(), ..State::default() };
        let View::Library(v) = view(&st, &env(&reg, &cfg, &games, &b, &f, &l)) else { panic!() };
        assert_eq!(v.empty, Some(Empty::NoMatches));
    }

    #[test]
    fn hub_builds_runner_choices_checks_and_setting_widgets() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let (cfg, b, l) = (Config::default(), Bundle::new(), Loc(true));
        let games = [game("alpha")];
        let mut facts = Facts::new();
        facts.insert("ram.gib".into(), Fact::Num(2.0));
        let st = State { screen: Screen::Hub(games[0].game.clone()), ..State::default() };
        let View::Hub(h) = view(&st, &env(&reg, &cfg, &games, &b, &facts, &l)) else { panic!() };
        assert_eq!(h.runners.len(), 1);
        assert!(h.runners[0].selected && h.runners[0].installed && h.can_play);
        assert_eq!(h.checks.len(), 1);
        assert!(h.checks[0].blocks);
        assert_eq!(h.settings.len(), 3);
        assert!(matches!(h.settings[0].widget, Widget::Switch { on: true }));
        assert!(matches!(h.settings[1].widget, Widget::Stepper { value: 60, .. }));
        assert!(matches!(h.settings[2].widget, Widget::Dropdown { .. }));
        assert_eq!(h.controls.len(), 1);
    }

    #[test]
    fn hub_without_a_runner_says_so_and_a_vanished_game_falls_back() {
        let sys_only = {
            struct SysOnly;
            impl Plugin for SysOnly {
                fn manifest(&self) -> Manifest { Manifest { id: "so".into(), version: "1".into(), api: API_VERSION, permissions: Default::default() } }
                fn register(&self, r: &mut Registrar) { r.system(std::sync::Arc::new(testkit::toy::ToySystem::new())); }
            }
            Registry::build(&[&SysOnly]).unwrap()
        };
        let (cfg, b, f, l) = (Config::default(), Bundle::new(), Facts::new(), Loc(false));
        let games = [game("alpha")];
        let st = State { screen: Screen::Hub(games[0].game.clone()), ..State::default() };
        let View::Hub(h) = view(&st, &env(&sys_only, &cfg, &games, &b, &f, &l)) else { panic!() };
        assert!(h.no_runner && !h.can_play && h.runners.is_empty());
        let none = Registry::build(&[]).unwrap();
        assert!(matches!(view(&st, &env(&none, &cfg, &games, &b, &f, &l)), View::Library(_)));
    }

    #[test]
    fn settings_nav_and_pages_are_generated_and_absent_plugin_data_is_listed_as_kept() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let (mut cfg, b, f, l) = (Config::default(), Bundle::new(), Facts::new(), Loc(true));
        cfg.set_runner_setting(&RunnerId::new("removed"), "x", &Value::Int(1));
        let st = State { screen: Screen::Settings(SettingsPage::Plugins), ..State::default() };
        let View::Settings(v) = view(&st, &env(&reg, &cfg, &[], &b, &f, &l)) else { panic!() };
        assert_eq!(v.nav.len(), 3);
        let SettingsBody::Plugins { plugins, kept } = v.body else { panic!() };
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].provides, vec!["system", "runner", "catalog source"]);
        assert_eq!(kept[0].what, "removed");
        let st = State { screen: Screen::Settings(SettingsPage::Runner(RunnerId::new("toy-run"))), ..State::default() };
        let View::Settings(v) = view(&st, &env(&reg, &cfg, &[], &b, &f, &l)) else { panic!() };
        let SettingsBody::Runner { rows, .. } = v.body else { panic!() };
        assert_eq!(rows.len(), 3);
        // A page for a runner that is gone falls back to the overview.
        let st = State { screen: Screen::Settings(SettingsPage::Runner(RunnerId::new("gone"))), ..State::default() };
        let View::Settings(v) = view(&st, &env(&reg, &cfg, &[], &b, &f, &l)) else { panic!() };
        assert!(matches!(v.body, SettingsBody::Plugins { .. }));
    }
}
