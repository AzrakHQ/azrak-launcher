use crate::error::LaunchError;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One argv entry. `value` may contain `{game}` and `{setting.<key>}`; `when` names a setting that must be
/// truthy (`key`) or falsy (`!key`). No shell, no expressions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArgTemplate {
    pub value: String,
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LaunchTemplate {
    #[serde(default)]
    pub args: Vec<ArgTemplate>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Run with the game folder as the working directory.
    #[serde(default)]
    pub cwd_game: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

pub struct LaunchInputs<'a> {
    pub runner_exe: &'a Path,
    pub game_root: &'a Path,
    pub settings: &'a Settings,
}

fn substitute(s: &str, i: &LaunchInputs<'_>) -> Result<String, LaunchError> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| LaunchError::Placeholder(after.to_string()))?;
        let name = &after[..end];
        let value = if name == "game" {
            i.game_root.display().to_string()
        } else if let Some(key) = name.strip_prefix("setting.") {
            i.settings
                .get(key)
                .ok_or_else(|| LaunchError::Placeholder(name.to_string()))?
                .render()
        } else {
            return Err(LaunchError::Placeholder(name.to_string()));
        };
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn guard(when: &Option<String>, settings: &Settings) -> bool {
    match when.as_deref() {
        None => true,
        Some(w) => {
            let (neg, key) = w.strip_prefix('!').map(|k| (true, k)).unwrap_or((false, w));
            settings.get(key).map(|v| v.truthy()).unwrap_or(false) != neg
        }
    }
}

impl LaunchTemplate {
    pub fn render(&self, i: &LaunchInputs<'_>) -> Result<LaunchPlan, LaunchError> {
        let mut args = Vec::new();
        for a in &self.args {
            if guard(&a.when, i.settings) {
                args.push(substitute(&a.value, i)?);
            }
        }
        let mut env = BTreeMap::new();
        for (k, v) in &self.env {
            env.insert(k.clone(), substitute(v, i)?);
        }
        Ok(LaunchPlan {
            program: i.runner_exe.to_path_buf(),
            args,
            env,
            cwd: self.cwd_game.then(|| i.game_root.to_path_buf()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Value;
    fn t() -> LaunchTemplate {
        LaunchTemplate {
            args: vec![
                ArgTemplate {
                    value: "--fullscreen".into(),
                    when: Some("fs".into()),
                },
                ArgTemplate {
                    value: "--windowed".into(),
                    when: Some("!fs".into()),
                },
                ArgTemplate {
                    value: "--fps={setting.fps}".into(),
                    when: None,
                },
                ArgTemplate {
                    value: "{game}".into(),
                    when: None,
                },
            ],
            env: BTreeMap::new(),
            cwd_game: true,
        }
    }
    #[test]
    fn renders_guards_and_placeholders() {
        let mut s = Settings::new();
        s.insert("fs".into(), Value::Bool(true));
        s.insert("fps".into(), Value::Int(60));
        let plan = t()
            .render(&LaunchInputs {
                runner_exe: Path::new("/r/exe"),
                game_root: Path::new("/g"),
                settings: &s,
            })
            .unwrap();
        assert_eq!(plan.args, vec!["--fullscreen", "--fps=60", "/g"]);
        assert_eq!(plan.cwd, Some(PathBuf::from("/g")));
        s.insert("fs".into(), Value::Bool(false));
        let plan = t()
            .render(&LaunchInputs {
                runner_exe: Path::new("/r/exe"),
                game_root: Path::new("/g"),
                settings: &s,
            })
            .unwrap();
        assert_eq!(plan.args[0], "--windowed");
    }
    #[test]
    fn unknown_placeholder_is_an_error_not_a_shell() {
        let tpl = LaunchTemplate {
            args: vec![ArgTemplate {
                value: "{env.HOME}".into(),
                when: None,
            }],
            ..Default::default()
        };
        let s = Settings::new();
        assert!(tpl
            .render(&LaunchInputs {
                runner_exe: Path::new("x"),
                game_root: Path::new("g"),
                settings: &s
            })
            .is_err());
    }
}
