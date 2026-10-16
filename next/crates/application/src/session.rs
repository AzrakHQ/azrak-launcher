use crate::config::Config;
use crate::settings::{effective_for, SettingsError};
use engines::provision::ManagedStore;
use kernel::*;
use std::path::PathBuf;
use thiserror::Error;

/// Where a runner's executable is. Managed runners live in the store; tests substitute a fake.
pub trait RunnerLocator {
    fn executable(&self, runner: &RunnerDescriptor) -> Option<PathBuf>;
}

pub struct StoreLocator<'a>(pub &'a ManagedStore);
impl RunnerLocator for StoreLocator<'_> {
    fn executable(&self, r: &RunnerDescriptor) -> Option<PathBuf> {
        self.0.executable(&r.id, r.provision.as_ref()?)
    }
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("no installed runner can run {0}")]
    NoRunner(String),
    #[error("{0} is not installed yet")]
    NotInstalled(String),
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error(transparent)]
    Plan(#[from] LaunchError),
    #[error(transparent)]
    Spawn(#[from] PortError),
}

/// A runner that needs no program of its own resolves to an empty path.
pub fn resolve_exe(loc: &dyn RunnerLocator, d: &RunnerDescriptor) -> Option<PathBuf> {
    if d.needs_executable { loc.executable(d) } else { Some(PathBuf::new()) }
}

/// The user's choice for a system if it is still installed, else the first runner that supports it.
pub fn choose_runner<'a>(reg: &'a Registry, cfg: &Config, system: &SystemId) -> Option<&'a DynRunner> {
    let candidates = reg.runners_for(system);
    cfg.choice(system).and_then(|c| candidates.iter().copied().find(|r| r.descriptor().id == c)).or_else(|| candidates.first().copied())
}

pub fn plan(reg: &Registry, cfg: &Config, loc: &dyn RunnerLocator, game: &InstalledGame) -> Result<(RunnerId, LaunchPlan), SessionError> {
    let runner = choose_runner(reg, cfg, &game.game.system).ok_or_else(|| SessionError::NoRunner(game.game.system.to_string()))?;
    let d = runner.descriptor();
    let exe = resolve_exe(loc, d).ok_or_else(|| SessionError::NotInstalled(d.id.to_string()))?;
    let settings = effective_for(reg, cfg, &d.id)?;
    Ok((d.id.clone(), runner.plan(&exe, game, &settings)?))
}

pub struct Launched { pub runner: RunnerId, pub plan: LaunchPlan, pub process: ProcessId }

pub fn launch(reg: &Registry, cfg: &Config, loc: &dyn RunnerLocator, game: &InstalledGame, ctx: &Ctx) -> Result<Launched, SessionError> {
    let (runner, plan) = plan(reg, cfg, loc, game)?;
    let process = ctx.processes.spawn(&plan)?;
    Ok(Launched { runner, plan, process })
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::{fakes, ToyPlugin};

    struct Fixed(Option<PathBuf>);
    impl RunnerLocator for Fixed { fn executable(&self, _: &RunnerDescriptor) -> Option<PathBuf> { self.0.clone() } }

    fn game() -> InstalledGame {
        InstalledGame { game: GameRef { system: SystemId::new("toy-sys"), key: GameKey::new("alpha") }, name: "Alpha".into(), root: "/g/alpha".into(), extra: serde_json::Value::Null }
    }

    #[test]
    fn launches_with_effective_settings_through_the_process_port() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let mut cfg = Config::default();
        crate::settings::set(&reg, &mut cfg, &RunnerId::new("toy-run"), "fps", Value::Int(30)).unwrap();
        let f = fakes();
        let l = launch(&reg, &cfg, &Fixed(Some("/r/exe".into())), &game(), &f.ctx).unwrap();
        assert_eq!(l.plan.args, vec!["--fullscreen", "--fps=30", "/g/alpha"]);
        assert_eq!(f.processes.spawned.lock().unwrap().len(), 1);
    }

    #[test]
    fn degrades_to_typed_errors_when_pieces_are_missing() {
        let f = fakes();
        let none = Registry::build(&[]).unwrap();
        let cfg = Config::default();
        assert!(matches!(launch(&none, &cfg, &Fixed(None), &game(), &f.ctx), Err(SessionError::NoRunner(_))));
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        assert!(matches!(launch(&reg, &cfg, &Fixed(None), &game(), &f.ctx), Err(SessionError::NotInstalled(_))));
        assert!(f.processes.spawned.lock().unwrap().is_empty());
    }

    #[test]
    fn a_stale_choice_falls_back_to_an_installed_runner() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let mut cfg = Config::default();
        cfg.set_choice(&SystemId::new("toy-sys"), &RunnerId::new("uninstalled-runner"));
        assert_eq!(choose_runner(&reg, &cfg, &SystemId::new("toy-sys")).unwrap().descriptor().id.as_str(), "toy-run");
        assert!(choose_runner(&reg, &cfg, &SystemId::new("other")).is_none());
    }
}
