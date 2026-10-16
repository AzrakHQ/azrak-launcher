use crate::config::Config;
use kernel::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("no installed runner has the id {0}")]
    UnknownRunner(String),
    #[error(transparent)]
    Invalid(#[from] kernel::SettingError),
}

/// The values a runner will actually use: defaults overlaid by valid user values.
pub fn effective_for(
    reg: &Registry,
    cfg: &Config,
    runner: &RunnerId,
) -> Result<Settings, SettingsError> {
    let r = reg
        .runner(runner)
        .ok_or_else(|| SettingsError::UnknownRunner(runner.to_string()))?;
    Ok(effective(
        &r.descriptor().settings,
        &cfg.runner_settings(runner),
    ))
}

/// Validate against the runner's spec, then store.
pub fn set(
    reg: &Registry,
    cfg: &mut Config,
    runner: &RunnerId,
    key: &str,
    value: Value,
) -> Result<(), SettingsError> {
    let r = reg
        .runner(runner)
        .ok_or_else(|| SettingsError::UnknownRunner(runner.to_string()))?;
    let spec = r
        .descriptor()
        .settings
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| kernel::SettingError::Unknown(key.into()))?;
    spec.validate(&value)?;
    cfg.set_runner_setting(runner, key, &value);
    Ok(())
}

pub struct Row {
    pub spec: SettingSpec,
    pub value: Value,
}
pub struct Page {
    pub runner: RunnerId,
    pub title: Text,
    pub rows: Vec<Row>,
}

/// One page per installed runner, generated from its specs. This is all the Settings UI needs.
pub fn pages(reg: &Registry, cfg: &Config) -> Vec<Page> {
    reg.runners()
        .iter()
        .filter(|r| !r.descriptor().settings.is_empty())
        .map(|r| {
            let d = r.descriptor();
            let eff = effective(&d.settings, &cfg.runner_settings(&d.id));
            Page {
                runner: d.id.clone(),
                title: d.name.clone(),
                rows: d
                    .settings
                    .iter()
                    .map(|s| Row {
                        spec: s.clone(),
                        value: eff[&s.key].clone(),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Plugin ids that have saved data but are not installed. Their data is kept untouched.
pub fn kept_for_absent_plugins(reg: &Registry, cfg: &Config) -> Vec<String> {
    let known: Vec<&str> = reg
        .runners()
        .iter()
        .map(|r| r.descriptor().id.as_str())
        .collect();
    cfg.plugins
        .keys()
        .filter(|k| !known.contains(&k.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::ToyPlugin;

    fn reg() -> Registry {
        Registry::build(&[&ToyPlugin]).unwrap()
    }

    #[test]
    fn validates_then_stores_and_effective_reflects_it() {
        let reg = reg();
        let mut cfg = Config::default();
        let r = RunnerId::new("toy-run");
        assert_eq!(
            effective_for(&reg, &cfg, &r).unwrap()["fps"],
            Value::Int(60)
        );
        set(&reg, &mut cfg, &r, "fps", Value::Int(30)).unwrap();
        assert_eq!(
            effective_for(&reg, &cfg, &r).unwrap()["fps"],
            Value::Int(30)
        );
        assert!(set(&reg, &mut cfg, &r, "fps", Value::Int(9999)).is_err());
        assert!(set(&reg, &mut cfg, &r, "quality", Value::Text("ultra".into())).is_err());
        assert!(set(&reg, &mut cfg, &r, "nope", Value::Bool(true)).is_err());
        assert!(matches!(
            set(
                &reg,
                &mut cfg,
                &RunnerId::new("gone"),
                "x",
                Value::Bool(true)
            ),
            Err(SettingsError::UnknownRunner(_))
        ));
        assert_eq!(
            effective_for(&reg, &cfg, &r).unwrap()["fps"],
            Value::Int(30),
            "failed sets change nothing"
        );
    }

    #[test]
    fn pages_are_generated_and_absent_plugin_data_is_reported_as_kept() {
        let reg = reg();
        let mut cfg = Config::default();
        cfg.set_runner_setting(&RunnerId::new("removed-runner"), "x", &Value::Int(1));
        let pages = pages(&reg, &cfg);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].rows.len(), 3);
        assert_eq!(kept_for_absent_plugins(&reg, &cfg), vec!["removed-runner"]);
        let none = Registry::build(&[]).unwrap();
        assert!(super::pages(&none, &cfg).is_empty());
    }
}
