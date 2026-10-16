//! The composition root and the headless shell. The only crate that names extensions (by Cargo feature).
use application::config::{Config, ConfigError};
use application::session::{self, StoreLocator};
use application::{assess, catalog, library, settings as app_settings};
use engines::declarative;
use engines::install::{install, InstallError, Limits};
use engines::provision::{self, ManagedStore, Platform};
use kernel::*;
use std::io::Write;
use std::path::PathBuf;
use thiserror::Error;
use ui_model::{update, view, Action, Env, Screen, SettingsPage, State};

/// Extensions compiled into this build. Remove a feature and its line disappears; nothing else changes.
pub fn builtin_plugins() -> Vec<Box<dyn Plugin>> {
    #[allow(unused_mut)]
    let mut v: Vec<Box<dyn Plugin>> = vec![];
    #[cfg(feature = "native-pc")]
    v.push(Box::new(native_pc::NativePc));
    #[cfg(feature = "toy")]
    v.push(Box::new(testkit::ToyPlugin));
    v
}

pub fn default_home() -> PathBuf {
    std::env::var_os("LAUNCHER_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/launcher")))
        .unwrap_or_else(|| PathBuf::from(".launcher"))
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)] Registry(#[from] RegistryError),
    #[error(transparent)] Config(#[from] ConfigError),
    #[error(transparent)] Install(#[from] InstallError),
    #[error(transparent)] Session(#[from] session::SessionError),
    #[error(transparent)] Settings(#[from] app_settings::SettingsError),
    #[error(transparent)] Provision(#[from] provision::ProvisionError),
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("{0}")] Usage(String),
}

pub struct App {
    pub home: PathBuf,
    pub registry: Registry,
    /// Tier-0 manifests that failed to load: shown by `diagnose`, never fatal.
    pub skipped_manifests: Vec<(PathBuf, String)>,
    pub bundle: Bundle,
}

impl App {
    pub fn open(home: PathBuf) -> Result<App, CliError> {
        let builtin = builtin_plugins();
        let (decl, bad) = declarative::load_dir(&home.join("plugins"));
        let mut refs: Vec<&dyn Plugin> = builtin.iter().map(|b| b.as_ref()).collect();
        refs.extend(decl.iter().map(|d| d as &dyn Plugin));
        let registry = Registry::build(&refs)?;
        Ok(App { home, registry, skipped_manifests: bad.into_iter().map(|(p, e)| (p, e.to_string())).collect(), bundle: default_bundle() })
    }
    pub fn config_path(&self) -> PathBuf { self.home.join("config.json") }
    pub fn games_dir(&self, cfg: &Config) -> PathBuf { cfg.games_dir().unwrap_or_else(|| self.home.join("games")) }
    pub fn store(&self) -> ManagedStore { ManagedStore::new(self.home.join("runners")) }
}

pub fn default_bundle() -> Bundle {
    let mut b = Bundle::new();
    b.insert("library.all", "All");
    b.insert("settings.plugins", "Plugins");
    b.insert("settings.overview", "Overview");
    b
}

fn find_game(app: &App, cfg: &Config, spec: &str) -> Result<InstalledGame, CliError> {
    let (sys, key) = spec.split_once('/').ok_or_else(|| CliError::Usage("game must be <system>/<key>".into()))?;
    library::scan(&app.registry, &app.games_dir(cfg)).into_iter()
        .find(|g| g.game.system.as_str() == sys && g.game.key.as_str() == key)
        .ok_or_else(|| CliError::Usage(format!("no installed game {spec}")))
}

fn parse_value(spec: &SettingSpec, raw: &str) -> Result<Value, CliError> {
    Ok(match spec.kind {
        SettingKind::Bool => Value::Bool(raw.parse().map_err(|_| CliError::Usage(format!("{} expects true or false", spec.key)))?),
        SettingKind::Int { .. } => Value::Int(raw.parse().map_err(|_| CliError::Usage(format!("{} expects a number", spec.key)))?),
        _ => Value::Text(raw.into()),
    })
}

fn pretty<T: serde::Serialize>(v: &T) -> String { serde_json::to_string_pretty(v).expect("serialises") }

pub fn run(args: &[String], app: &App, out: &mut dyn Write) -> Result<(), CliError> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut cfg = Config::load(&app.config_path())?;
    let locator_store = app.store();
    let locator = StoreLocator(&locator_store);
    match a.as_slice() {
        ["plugins"] => {
            for m in app.registry.manifests() { writeln!(out, "{} {} [{}]", m.id, m.version, app.registry.provides(&m.id).join(", "))?; }
        }
        ["systems"] => {
            for s in app.registry.systems() {
                let d = s.descriptor();
                let runners: Vec<String> = app.registry.runners_for(&d.id).iter().map(|r| r.descriptor().id.to_string()).collect();
                writeln!(out, "{} ({}) runners: [{}]", d.id, app.bundle.render(&d.name), runners.join(", "))?;
            }
        }
        ["list"] => {
            for g in library::scan(&app.registry, &app.games_dir(&cfg)) { writeln!(out, "{}/{}\t{}", g.game.system, g.game.key, g.name)?; }
        }
        ["install", path] => {
            let games = install(&app.registry, std::path::Path::new(path), &app.games_dir(&cfg), &Limits::default(), &CancelToken::new())?;
            for g in games { writeln!(out, "installed {}/{} as {}", g.game.system, g.game.key, g.name)?; }
        }
        ["settings"] => {
            for p in app_settings::pages(&app.registry, &cfg) {
                writeln!(out, "[{}]", app.bundle.render(&p.title))?;
                for r in p.rows { writeln!(out, "{}.{} = {}", p.runner, r.spec.key, r.value.render())?; }
            }
        }
        ["set", runner, key, raw] => {
            let rid = RunnerId::new(*runner);
            let r = app.registry.runner(&rid).ok_or_else(|| CliError::Usage(format!("no runner {runner}")))?;
            let spec = r.descriptor().settings.iter().find(|s| s.key == *key).ok_or_else(|| CliError::Usage(format!("no setting {key}")))?;
            app_settings::set(&app.registry, &mut cfg, &rid, key, parse_value(spec, raw)?)?;
            cfg.save(&app.config_path())?;
            writeln!(out, "{runner}.{key} saved")?;
        }
        ["choose", system, runner] => {
            cfg.set_choice(&SystemId::new(*system), &RunnerId::new(*runner));
            cfg.save(&app.config_path())?;
            writeln!(out, "{system} will use {runner}")?;
        }
        ["plan", game] => {
            let g = find_game(app, &cfg, game)?;
            let (r, p) = session::plan(&app.registry, &cfg, &locator, &g)?;
            writeln!(out, "{r}: {} {}", p.program.display(), p.args.join(" "))?;
        }
        ["run", game] => {
            let g = find_game(app, &cfg, game)?;
            let ctx = host::ctx();
            let l = session::launch(&app.registry, &cfg, &locator, &g, &ctx)?;
            writeln!(out, "started {} with {}", g.name, l.runner)?;
        }
        ["update-runner", runner] => {
            let rid = RunnerId::new(*runner);
            let r = app.registry.runner(&rid).ok_or_else(|| CliError::Usage(format!("no runner {runner}")))?;
            let prov = r.descriptor().provision.clone().ok_or_else(|| CliError::Usage("this runner is not managed".into()))?;
            let o = provision::update(&app.store(), &rid, &prov, &host::ctx(), &Platform::current(), app.registry.containers(), &Limits::default())?;
            writeln!(out, "{o:?}")?;
        }
        ["rollback", runner] => {
            let v = app.store().rollback(&RunnerId::new(*runner))?;
            writeln!(out, "{runner} is back on {v}")?;
        }
        ["catalog-manifest"] => writeln!(out, "{}", pretty(&catalog::manifest(&app.registry)))?,
        ["catalog-refresh"] => {
            let snap = catalog::collect(&app.registry, &host::ctx());
            catalog::save(&snap, &app.home.join("catalog.json"))?;
            writeln!(out, "{} releases, {} skipped", snap.releases.len(), snap.skipped.len())?;
        }
        ["ui", which] => {
            let games = library::scan(&app.registry, &app.games_dir(&cfg));
            let facts = assess::collect(&app.registry);
            let screen = match *which {
                "library" => Screen::Library,
                "settings" => Screen::Settings(SettingsPage::Plugins),
                g => games.iter().find(|x| format!("{}/{}", x.game.system, x.game.key) == g).map(|x| Screen::Hub(x.game.clone()))
                    .ok_or_else(|| CliError::Usage("ui <library|settings|system/key>".into()))?,
            };
            let games_dir = app.games_dir(&cfg);
            let env = Env { reg: &app.registry, cfg: &cfg, games: &games, bundle: &app.bundle, facts: &facts, locator: &locator, games_dir: Some(&games_dir) };
            let (st, _) = update(State::default(), Action::FocusMove(0));
            writeln!(out, "{}", pretty(&view(&State { screen, ..st }, &env)))?;
        }
        ["diagnose"] => {
            writeln!(out, "home: {}", app.home.display())?;
            writeln!(out, "plugins: {}  systems: {}  runners: {}", app.registry.manifests().len(), app.registry.systems().len(), app.registry.runners().len())?;
            for (p, e) in &app.skipped_manifests { writeln!(out, "skipped manifest {}: {e}", p.display())?; }
            for id in app_settings::kept_for_absent_plugins(&app.registry, &cfg) { writeln!(out, "kept settings of absent plugin {id}")?; }
            for k in library::kept_for_absent_systems(&app.registry, &app.games_dir(&cfg)) { writeln!(out, "kept {} games in {}/ (system not installed)", k.games, k.folder)?; }
        }
        _ => return Err(CliError::Usage("commands: plugins | systems | list | install <path> | settings | set <runner> <key> <value> | choose <system> <runner> | plan <sys/key> | run <sys/key> | update-runner <id> | rollback <id> | catalog-manifest | catalog-refresh | ui <library|settings|sys/key> | diagnose".into())),
    }
    Ok(())
}
