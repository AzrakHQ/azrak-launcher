#![allow(clippy::redundant_closure_call)]
use crate::{AppWindow, Card, CheckRow, Chip, KeptRow, NavRow, PluginRow, RunnerOpt, Setting};
use kernel::{GameKey, GameRef, RunnerId, SystemId, Value};
use launcher::Controller;
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use ui_model::{Action, Empty, Screen, SettingRow, SettingsBody, SettingsPage, View, Widget};

fn color(hex: &str) -> Color {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0xffffff);
    Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}
fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn setting(r: &SettingRow) -> Setting {
    let mut s = Setting {
        label: r.label.clone().into(),
        hint: r.hint.clone().into(),
        kind: 0,
        on: false,
        number: 0,
        min: 0,
        max: 0,
        step: 1,
        text: SharedString::new(),
        options: model(vec![]),
        selected: 0,
    };
    match &r.widget {
        Widget::Switch { on } => {
            s.kind = 0;
            s.on = *on;
        }
        Widget::Dropdown { options, selected } => {
            s.kind = 1;
            s.options = model(
                options
                    .iter()
                    .map(|o| SharedString::from(o.label.as_str()))
                    .collect(),
            );
            s.selected = options
                .iter()
                .position(|o| &o.value == selected)
                .unwrap_or(0) as i32;
        }
        Widget::Stepper {
            value,
            min,
            max,
            step,
        } => {
            s.kind = 2;
            s.number = *value as i32;
            s.min = *min as i32;
            s.max = *max as i32;
            s.step = (*step).max(1) as i32;
        }
        Widget::TextField { value } => {
            s.kind = 3;
            s.text = value.as_str().into();
        }
        Widget::PathField { value } => {
            s.kind = 4;
            s.text = value.as_str().into();
        }
    }
    s
}

/// Push a view into the window. Pure translation: nothing here knows what a system or runner is.
pub fn render(win: &AppWindow, c: &Controller) {
    win.set_message(c.message.clone().unwrap_or_default().into());
    match c.view() {
        View::Library(l) => {
            win.set_screen(0);
            win.set_chips(model(
                l.chips
                    .iter()
                    .map(|x| Chip {
                        name: x.name.as_str().into(),
                        color: color(&x.color),
                        count: x.count as i32,
                        selected: x.selected,
                    })
                    .collect(),
            ));
            win.set_cards(model(
                l.cards
                    .iter()
                    .map(|x| Card {
                        title: x.title.as_str().into(),
                        short: x.short.as_str().into(),
                        color: color(&x.color),
                        cover_w: x.cover_w as i32,
                        cover_h: x.cover_h as i32,
                    })
                    .collect(),
            ));
            let (t, d) = match l.empty {
                Some(Empty::NoSystems) => ("Nothing to play yet", "This build has no game systems installed. Add a system plugin and its games appear here. Your settings and saved games are not touched."),
                Some(Empty::NoGames) => ("No games yet", "Install a game to see it here."),
                Some(Empty::NoMatches) => ("No matches", "Try another search or filter."),
                None => ("", ""),
            };
            win.set_empty_title(t.into());
            win.set_empty_text(d.into());
        }
        View::Hub(h) => {
            win.set_screen(1);
            win.set_hub_title(h.title.into());
            win.set_hub_short(h.short.into());
            win.set_hub_color(color(&h.color));
            win.set_runners(model(
                h.runners
                    .iter()
                    .map(|r| RunnerOpt {
                        name: r.name.as_str().into(),
                        selected: r.selected,
                        installed: r.installed,
                    })
                    .collect(),
            ));
            win.set_checks(model(
                h.checks
                    .iter()
                    .map(|x| CheckRow {
                        blocks: x.blocks,
                        text: x.text.as_str().into(),
                    })
                    .collect(),
            ));
            win.set_settings(model(h.settings.iter().map(setting).collect()));
            win.set_can_play(h.can_play);
            win.set_no_runner_text(if h.no_runner {
                "No installed runner can run this game.".into()
            } else {
                SharedString::new()
            });
        }
        View::Settings(s) => {
            win.set_screen(2);
            win.set_nav(model(
                s.nav
                    .iter()
                    .map(|n| NavRow {
                        heading: n.heading,
                        label: n.label.as_str().into(),
                        active: n.active,
                    })
                    .collect(),
            ));
            match s.body {
                SettingsBody::Plugins { plugins, kept } => {
                    win.set_overview(true);
                    win.set_plugins(model(
                        plugins
                            .iter()
                            .map(|p| PluginRow {
                                id: p.id.as_str().into(),
                                version: p.version.as_str().into(),
                                provides: p.provides.join(", ").into(),
                                notes: {
                                    let mut n = vec![];
                                    if !p.contacts.is_empty() {
                                        n.push(format!("Contacts: {}", p.contacts.join(", ")));
                                    }
                                    if !p.writes.is_empty() {
                                        n.push(format!("Writes: {}", p.writes.join(", ")));
                                    }
                                    n.join("  ").into()
                                },
                            })
                            .collect(),
                    ));
                    win.set_kept(model(
                        kept.iter()
                            .map(|k| KeptRow {
                                what: k.what.as_str().into(),
                                detail: k.detail.as_str().into(),
                            })
                            .collect(),
                    ));
                }
                SettingsBody::Runner { title, rows } => {
                    win.set_overview(false);
                    win.set_page_title(title.into());
                    win.set_settings(model(rows.iter().map(setting).collect()));
                }
            }
        }
    }
}

fn rows_of(c: &Controller) -> Vec<SettingRow> {
    match c.view() {
        View::Hub(h) => h.settings,
        View::Settings(s) => match s.body {
            SettingsBody::Runner { rows, .. } => rows,
            _ => vec![],
        },
        _ => vec![],
    }
}

pub fn bind(win: &AppWindow, ctl: Rc<RefCell<Controller>>) {
    let weak = win.as_weak();
    // After every action: dispatch, then re-render from the new view.
    let act = {
        let ctl = ctl.clone();
        move |a: Action| {
            ctl.borrow_mut().dispatch(a);
            if let Some(w) = weak.upgrade() {
                render(&w, &ctl.borrow());
            }
        }
    };
    macro_rules! on {
        ($setter:ident, $ctl:ident, || $body:expr) => {{ let act = act.clone(); let $ctl = $ctl.clone(); let _ = &$ctl; win.$setter(move || { let a: Option<Action> = (|| -> Option<Action> { $body })(); if let Some(a) = a { act(a) } }); }};
        ($setter:ident, $ctl:ident, |$($p:ident),*| $body:expr) => {{ let act = act.clone(); let $ctl = $ctl.clone(); let _ = &$ctl; win.$setter(move |$($p),*| { let a: Option<Action> = (|| -> Option<Action> { $body })(); if let Some(a) = a { act(a) } }); }};
    }

    on!(on_filter, ctl, |i| {
        let c = ctl.borrow();
        let View::Library(l) = c.view() else {
            return None;
        };
        Some(Action::FilterSystem(
            l.chips
                .get(i as usize)
                .and_then(|x| x.id.clone())
                .map(SystemId::new),
        ))
    });
    on!(on_open_card, ctl, |i| {
        let c = ctl.borrow();
        let View::Library(l) = c.view() else {
            return None;
        };
        l.cards.get(i as usize).map(|x| {
            Action::OpenGame(GameRef {
                system: SystemId::new(&x.system),
                key: GameKey::new(&x.key),
            })
        })
    });
    on!(on_back, ctl, || Some(Action::Back));
    on!(on_open_settings, ctl, || Some(Action::OpenSettings));
    on!(on_search, ctl, |t| Some(Action::Search(t.to_string())));
    on!(on_nav_click, ctl, |i| {
        let c = ctl.borrow();
        let View::Settings(s) = c.view() else {
            return None;
        };
        let item = s.nav.get(i as usize)?;
        Some(Action::OpenSettingsPage(match &item.runner {
            Some(r) => SettingsPage::Runner(RunnerId::new(r)),
            None => SettingsPage::Plugins,
        }))
    });
    on!(on_choose_runner, ctl, |i| {
        let c = ctl.borrow();
        let (View::Hub(h), Screen::Hub(g)) = (c.view(), c.state.screen.clone()) else {
            return None;
        };
        h.runners.get(i as usize).map(|r| Action::ChooseRunner {
            system: g.system,
            runner: RunnerId::new(&r.id),
        })
    });
    on!(on_play, ctl, || {
        let c = ctl.borrow();
        if let Screen::Hub(g) = &c.state.screen {
            Some(Action::Play(g.clone()))
        } else {
            None
        }
    });
    on!(on_toggle, ctl, |i, on| {
        let c = ctl.borrow();
        rows_of(&c).get(i as usize).map(|r| Action::SetSetting {
            runner: RunnerId::new(&r.runner),
            key: r.key.clone(),
            value: Value::Bool(on),
        })
    });
    on!(on_step, ctl, |i, v| {
        let c = ctl.borrow();
        rows_of(&c).get(i as usize).map(|r| Action::SetSetting {
            runner: RunnerId::new(&r.runner),
            key: r.key.clone(),
            value: Value::Int(v as i64),
        })
    });
    on!(on_edit_text, ctl, |i, t| {
        let c = ctl.borrow();
        rows_of(&c).get(i as usize).map(|r| Action::SetSetting {
            runner: RunnerId::new(&r.runner),
            key: r.key.clone(),
            value: Value::Text(t.to_string()),
        })
    });
    on!(on_pick, ctl, |i, idx| {
        let c = ctl.borrow();
        let r = rows_of(&c).get(i as usize).cloned()?;
        let Widget::Dropdown { options, .. } = &r.widget else {
            return None;
        };
        options.get(idx as usize).map(|o| Action::SetSetting {
            runner: RunnerId::new(&r.runner),
            key: r.key.clone(),
            value: Value::Text(o.value.clone()),
        })
    });
    render(win, &ctl.borrow());
}
