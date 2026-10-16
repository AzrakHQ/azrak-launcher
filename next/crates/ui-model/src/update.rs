use kernel::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Plugins,
    Runner(RunnerId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    Library,
    Hub(GameRef),
    Settings(SettingsPage),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub screen: Screen,
    pub system_filter: Option<SystemId>,
    pub query: String,
    /// Index of the focused item on the current screen (the shell maps it to a widget).
    pub focus: usize,
}
impl Default for State {
    fn default() -> Self {
        Self {
            screen: Screen::Library,
            system_filter: None,
            query: String::new(),
            focus: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    FilterSystem(Option<SystemId>),
    Search(String),
    OpenGame(GameRef),
    Back,
    OpenSettings,
    OpenSettingsPage(SettingsPage),
    SetSetting {
        runner: RunnerId,
        key: String,
        value: Value,
    },
    ChooseRunner {
        system: SystemId,
        runner: RunnerId,
    },
    Play(GameRef),
    FocusMove(i32),
}

/// Work for the shell to do; `update` itself never touches the outside world.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    StoreSetting {
        runner: RunnerId,
        key: String,
        value: Value,
    },
    StoreChoice {
        system: SystemId,
        runner: RunnerId,
    },
    Launch(GameRef),
}

pub fn update(mut s: State, a: Action) -> (State, Vec<Effect>) {
    let mut fx = vec![];
    match a {
        Action::FilterSystem(f) => {
            s.system_filter = f;
            s.focus = 0;
        }
        Action::Search(q) => {
            s.query = q;
            s.focus = 0;
        }
        Action::OpenGame(g) => {
            s.screen = Screen::Hub(g);
            s.focus = 0;
        }
        Action::Back => {
            s.screen = match s.screen {
                Screen::Settings(SettingsPage::Runner(_)) => {
                    Screen::Settings(SettingsPage::Plugins)
                }
                _ => Screen::Library,
            };
            s.focus = 0;
        }
        Action::OpenSettings => {
            s.screen = Screen::Settings(SettingsPage::Plugins);
            s.focus = 0;
        }
        Action::OpenSettingsPage(p) => {
            s.screen = Screen::Settings(p);
            s.focus = 0;
        }
        Action::SetSetting { runner, key, value } => {
            fx.push(Effect::StoreSetting { runner, key, value })
        }
        Action::ChooseRunner { system, runner } => fx.push(Effect::StoreChoice { system, runner }),
        Action::Play(g) => fx.push(Effect::Launch(g)),
        Action::FocusMove(d) => s.focus = (s.focus as i64 + d as i64).max(0) as usize,
    }
    (s, fx)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn g() -> GameRef {
        GameRef {
            system: SystemId::new("s"),
            key: GameKey::new("k"),
        }
    }

    #[test]
    fn navigation_is_a_typed_stack() {
        let (s, _) = update(State::default(), Action::OpenGame(g()));
        assert_eq!(s.screen, Screen::Hub(g()));
        let (s, _) = update(s, Action::Back);
        assert_eq!(s.screen, Screen::Library);
        let (s, _) = update(s, Action::OpenSettings);
        let (s, _) = update(
            s,
            Action::OpenSettingsPage(SettingsPage::Runner(RunnerId::new("r"))),
        );
        let (s, _) = update(s, Action::Back);
        assert_eq!(s.screen, Screen::Settings(SettingsPage::Plugins));
        let (s, _) = update(s, Action::Back);
        assert_eq!(s.screen, Screen::Library);
    }

    #[test]
    fn side_effects_are_returned_not_performed() {
        let (s, fx) = update(State::default(), Action::Play(g()));
        assert_eq!(s, State::default());
        assert_eq!(fx, vec![Effect::Launch(g())]);
    }

    #[test]
    fn focus_never_goes_negative() {
        let (s, _) = update(State::default(), Action::FocusMove(-5));
        assert_eq!(s.focus, 0);
    }
}
