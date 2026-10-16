//! Glue between the MVU core and the application services. Toolkit-free, so every shell shares it
//! and it is tested without a window.
use crate::{App, CliError};
use application::config::Config;
use application::session::{self, StoreLocator};
use application::{assess, library, settings as app_settings};
use kernel::*;
use ui_model::{update, view, Action, Effect, Env, State, View};

pub struct Controller {
    pub app: App,
    pub cfg: Config,
    pub state: State,
    pub games: Vec<InstalledGame>,
    pub facts: Facts,
    pub ctx: Ctx,
    /// A short, user-facing result of the last effect (success or failure). Never a panic.
    pub message: Option<String>,
}

impl Controller {
    pub fn new(app: App, ctx: Ctx) -> Result<Self, CliError> {
        let mut c = Controller {
            cfg: Config::default(),
            state: State::default(),
            games: vec![],
            facts: Facts::new(),
            ctx,
            message: None,
            app,
        };
        c.reload()?;
        Ok(c)
    }

    pub fn reload(&mut self) -> Result<(), CliError> {
        self.cfg = Config::load(&self.app.config_path())?;
        self.games = library::scan(&self.app.registry, &self.app.games_dir(&self.cfg));
        self.facts = assess::collect(&self.app.registry);
        Ok(())
    }

    pub fn dispatch(&mut self, a: Action) {
        let (s, fx) = update(self.state.clone(), a);
        self.state = s;
        for e in fx {
            self.apply(e);
        }
    }

    fn apply(&mut self, e: Effect) {
        self.message = match e {
            Effect::StoreSetting { runner, key, value } => {
                match app_settings::set(&self.app.registry, &mut self.cfg, &runner, &key, value)
                    .map_err(CliError::from)
                    .and_then(|_| {
                        self.cfg
                            .save(&self.app.config_path())
                            .map_err(CliError::from)
                    }) {
                    Ok(()) => None,
                    Err(err) => Some(err.to_string()),
                }
            }
            Effect::StoreChoice { system, runner } => {
                self.cfg.set_choice(&system, &runner);
                self.cfg
                    .save(&self.app.config_path())
                    .err()
                    .map(|e| e.to_string())
            }
            Effect::Launch(g) => {
                let store = self.app.store();
                match self.games.iter().find(|x| x.game == g) {
                    None => Some("that game is no longer installed".into()),
                    Some(game) => match session::launch(
                        &self.app.registry,
                        &self.cfg,
                        &StoreLocator(&store),
                        game,
                        &self.ctx,
                    ) {
                        Ok(l) => Some(format!("started {} with {}", game.name, l.runner)),
                        Err(e) => Some(e.to_string()),
                    },
                }
            }
        };
    }

    pub fn view(&self) -> View {
        let store = self.app.store();
        let loc = StoreLocator(&store);
        let dir = self.app.games_dir(&self.cfg);
        view(
            &self.state,
            &Env {
                reg: &self.app.registry,
                cfg: &self.cfg,
                games: &self.games,
                bundle: &self.app.bundle,
                facts: &self.facts,
                locator: &loc,
                games_dir: Some(&dir),
            },
        )
    }
}

#[cfg(all(test, feature = "toy"))]
mod tests {
    use super::*;
    use testkit::fakes;
    use ui_model::{Screen, SettingsPage, Widget};

    fn setup() -> (tempfile::TempDir, Controller, testkit::Fakes) {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("alpha");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(testkit::toy::MARKER), "Alpha").unwrap();
        let app = App::open(d.path().to_path_buf()).unwrap();
        engines::install::install(
            &app.registry,
            &src,
            &d.path().join("games"),
            &Default::default(),
            &CancelToken::new(),
        )
        .unwrap();
        let f = fakes();
        let c = Controller::new(app, f.ctx.clone()).unwrap();
        (d, c, f)
    }

    #[test]
    fn changing_a_setting_through_the_view_persists_and_is_reflected() {
        let (d, mut c, _f) = setup();
        let r = RunnerId::new("toy-run");
        c.dispatch(Action::OpenSettingsPage(SettingsPage::Runner(r.clone())));
        c.dispatch(Action::SetSetting {
            runner: r.clone(),
            key: "fps".into(),
            value: Value::Int(30),
        });
        assert_eq!(c.message, None);
        let View::Settings(v) = c.view() else {
            panic!()
        };
        let ui_model::SettingsBody::Runner { rows, .. } = v.body else {
            panic!()
        };
        assert!(matches!(
            rows.iter().find(|r| r.key == "fps").unwrap().widget,
            Widget::Stepper { value: 30, .. }
        ));
        // survives a restart
        let again =
            Controller::new(App::open(d.path().to_path_buf()).unwrap(), c.ctx.clone()).unwrap();
        assert_eq!(again.cfg.runner_settings(&r)["fps"], Value::Int(30));
        // invalid values are rejected with a message and change nothing
        c.dispatch(Action::SetSetting {
            runner: r.clone(),
            key: "fps".into(),
            value: Value::Int(9999),
        });
        assert!(c.message.is_some());
        assert_eq!(c.cfg.runner_settings(&r)["fps"], Value::Int(30));
    }

    #[test]
    fn play_without_an_installed_runner_reports_instead_of_failing() {
        let (_d, mut c, f) = setup();
        let g = c.games[0].game.clone();
        c.dispatch(Action::OpenGame(g.clone()));
        assert!(matches!(c.state.screen, Screen::Hub(_)));
        c.dispatch(Action::Play(g));
        assert!(c.message.as_deref().unwrap().contains("not installed"));
        assert!(f.processes.spawned.lock().unwrap().is_empty());
    }
}
