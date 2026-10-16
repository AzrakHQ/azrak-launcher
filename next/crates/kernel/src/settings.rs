use crate::error::SettingError;
use crate::text::Text;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Text(s) => !s.is_empty(),
        }
    }
    pub fn render(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Text(s) => s.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    Bool,
    Int {
        min: i64,
        max: i64,
        #[serde(default = "one")]
        step: i64,
    },
    Text,
    Choice {
        options: Vec<Choice>,
    },
    Path,
}
fn one() -> i64 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingSpec {
    pub key: String,
    pub label: Text,
    #[serde(default)]
    pub hint: Text,
    #[serde(flatten)]
    pub kind: SettingKind,
    pub default: Value,
    #[serde(default)]
    pub advanced: bool,
}

pub type Settings = BTreeMap<String, Value>;

impl SettingSpec {
    pub fn validate(&self, v: &Value) -> Result<(), SettingError> {
        let k = &self.key;
        match (&self.kind, v) {
            (SettingKind::Bool, Value::Bool(_)) => Ok(()),
            (SettingKind::Int { min, max, .. }, Value::Int(i)) => {
                if i >= min && i <= max {
                    Ok(())
                } else {
                    Err(SettingError::Range(k.clone()))
                }
            }
            (SettingKind::Text | SettingKind::Path, Value::Text(_)) => Ok(()),
            (SettingKind::Choice { options }, Value::Text(s)) => {
                if options.iter().any(|o| &o.value == s) {
                    Ok(())
                } else {
                    Err(SettingError::Choice(k.clone()))
                }
            }
            _ => Err(SettingError::Type(k.clone())),
        }
    }
}

/// Defaults overlaid by the user's values; values that no longer validate fall back to the default.
pub fn effective(specs: &[SettingSpec], user: &Settings) -> Settings {
    let mut out = Settings::new();
    for s in specs {
        let v = user
            .get(&s.key)
            .filter(|v| s.validate(v).is_ok())
            .unwrap_or(&s.default);
        out.insert(s.key.clone(), v.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec(kind: SettingKind, default: Value) -> SettingSpec {
        SettingSpec {
            key: "k".into(),
            label: "K".into(),
            hint: Text::default(),
            kind,
            default,
            advanced: false,
        }
    }
    #[test]
    fn validates_by_kind() {
        let n = spec(
            SettingKind::Int {
                min: 0,
                max: 10,
                step: 1,
            },
            Value::Int(1),
        );
        assert!(n.validate(&Value::Int(5)).is_ok());
        assert!(n.validate(&Value::Int(11)).is_err());
        assert!(n.validate(&Value::Bool(true)).is_err());
    }
    #[test]
    fn invalid_user_value_falls_back_to_default() {
        let n = spec(
            SettingKind::Int {
                min: 0,
                max: 10,
                step: 1,
            },
            Value::Int(1),
        );
        let mut user = Settings::new();
        user.insert("k".into(), Value::Int(99));
        assert_eq!(effective(&[n], &user)["k"], Value::Int(1));
    }
}
