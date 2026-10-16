use kernel::{RunnerId, Settings, SystemId, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {0}: {1}")]
    Read(PathBuf, String),
    #[error("{0} is not valid settings JSON: {1}")]
    Corrupt(PathBuf, String),
    #[error("cannot write {0}: {1}")]
    Write(PathBuf, String),
}

/// Versioned, namespaced and lossless: anything this build does not understand is kept as it was.
/// `plugins.<id>` belongs to that plugin alone and survives the plugin being absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub core: Map<String, Json>,
    #[serde(default)]
    pub plugins: BTreeMap<String, Json>,
    /// Which runner the user picked for a system.
    #[serde(default)]
    pub choices: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Json>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            core: Map::new(),
            plugins: BTreeMap::new(),
            choices: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

/// Upgrade an older document. Only moves data; never drops any.
pub fn migrate(mut doc: Json) -> Json {
    let Some(obj) = doc.as_object_mut() else {
        return doc;
    };
    let version = obj.get("version").and_then(Json::as_u64).unwrap_or(1);
    if version < 2 {
        // v1 kept per-runner values in one flat map keyed "<runner>.<key>".
        if let Some(Json::Object(flat)) = obj.remove("emulator_settings") {
            let plugins = obj
                .entry("plugins")
                .or_insert_with(|| Json::Object(Map::new()));
            if let Some(plugins) = plugins.as_object_mut() {
                for (dotted, v) in flat {
                    if let Some((id, key)) = dotted.split_once('.') {
                        let p = plugins
                            .entry(id.to_string())
                            .or_insert_with(|| Json::Object(Map::new()));
                        if let Some(p) = p.as_object_mut() {
                            let s = p
                                .entry("settings")
                                .or_insert_with(|| Json::Object(Map::new()));
                            if let Some(s) = s.as_object_mut() {
                                s.insert(key.to_string(), v);
                            }
                        }
                    }
                }
            }
        }
        obj.insert("version".into(), Json::from(2));
    }
    doc
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(ConfigError::Read(path.into(), e.to_string())),
        };
        let doc: Json = serde_json::from_slice(&bytes)
            .map_err(|e| ConfigError::Corrupt(path.into(), e.to_string()))?;
        serde_json::from_value(migrate(doc))
            .map_err(|e| ConfigError::Corrupt(path.into(), e.to_string()))
    }

    /// Atomic: write a sibling file and rename it over the original.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let werr = |e: std::io::Error| ConfigError::Write(path.into(), e.to_string());
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(werr)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).expect("config serialises"),
        )
        .map_err(werr)?;
        std::fs::rename(&tmp, path).map_err(werr)
    }

    pub fn games_dir(&self) -> Option<PathBuf> {
        self.core
            .get("games_dir")
            .and_then(Json::as_str)
            .map(PathBuf::from)
    }
    pub fn set_games_dir(&mut self, p: &Path) {
        self.core
            .insert("games_dir".into(), Json::from(p.display().to_string()));
    }

    pub fn runner_settings(&self, runner: &RunnerId) -> Settings {
        let mut out = Settings::new();
        if let Some(Json::Object(m)) = self
            .plugins
            .get(runner.as_str())
            .and_then(|p| p.get("settings"))
        {
            for (k, v) in m {
                if let Ok(v) = serde_json::from_value::<Value>(v.clone()) {
                    out.insert(k.clone(), v);
                }
            }
        }
        out
    }

    pub fn set_runner_setting(&mut self, runner: &RunnerId, key: &str, value: &Value) {
        let p = self
            .plugins
            .entry(runner.to_string())
            .or_insert_with(|| Json::Object(Map::new()));
        if !p.is_object() {
            *p = Json::Object(Map::new());
        }
        let s = p
            .as_object_mut()
            .unwrap()
            .entry("settings")
            .or_insert_with(|| Json::Object(Map::new()));
        if !s.is_object() {
            *s = Json::Object(Map::new());
        }
        s.as_object_mut().unwrap().insert(
            key.into(),
            serde_json::to_value(value).expect("value serialises"),
        );
    }

    pub fn choice(&self, system: &SystemId) -> Option<RunnerId> {
        self.choices.get(system.as_str()).map(RunnerId::new)
    }
    pub fn set_choice(&mut self, system: &SystemId, runner: &RunnerId) {
        self.choices.insert(system.to_string(), runner.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_file_is_default_and_corrupt_file_is_an_error_not_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        assert_eq!(Config::load(&p).unwrap(), Config::default());
        std::fs::write(&p, "{ not json").unwrap();
        assert!(matches!(Config::load(&p), Err(ConfigError::Corrupt(..))));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ not json");
    }

    #[test]
    fn v1_migrates_and_loses_nothing() {
        let old = json!({
            "theme": "dark",
            "legacy_field": {"keep": [1, 2, 3]},
            "emulator_settings": {"alpha.fps": 30, "alpha.fullscreen": false, "beta.mode": "x"},
        });
        let new = migrate(old);
        let cfg: Config = serde_json::from_value(new).unwrap();
        assert_eq!(cfg.version, 2);
        assert_eq!(cfg.extra["theme"], json!("dark"));
        assert_eq!(cfg.extra["legacy_field"], json!({"keep": [1, 2, 3]}));
        assert_eq!(
            cfg.runner_settings(&RunnerId::new("alpha"))["fps"],
            Value::Int(30)
        );
        assert_eq!(
            cfg.runner_settings(&RunnerId::new("beta"))["mode"],
            Value::Text("x".into())
        );
        assert!(!cfg.extra.contains_key("emulator_settings"));
    }

    #[test]
    fn unknown_plugin_data_round_trips_through_load_save() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        let doc = json!({
            "version": 2,
            "plugins": {"absent": {"settings": {"x": 1}, "private": {"anything": [true]}}},
            "future_top_level": 42,
        });
        std::fs::write(&p, doc.to_string()).unwrap();
        let mut cfg = Config::load(&p).unwrap();
        cfg.set_runner_setting(&RunnerId::new("present"), "fps", &Value::Int(60));
        cfg.save(&p).unwrap();
        let back: Json = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(back["plugins"]["absent"], doc["plugins"]["absent"]);
        assert_eq!(back["future_top_level"], json!(42));
        assert_eq!(back["plugins"]["present"]["settings"]["fps"], json!(60));
    }

    #[test]
    fn setting_a_value_keeps_the_plugins_other_data() {
        let mut cfg = Config::default();
        cfg.plugins
            .insert("r".into(), json!({"settings": {"a": 1}, "cache": "keep"}));
        cfg.set_runner_setting(&RunnerId::new("r"), "b", &Value::Bool(true));
        assert_eq!(cfg.plugins["r"]["cache"], json!("keep"));
        assert_eq!(cfg.plugins["r"]["settings"]["a"], json!(1));
    }
}
