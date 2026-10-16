//! Native PC games: a folder with a `game.toml` (`name`, `exe`). The runner launches the game itself.
use kernel::*;
use std::path::Path;
use std::sync::Arc;

pub const SYSTEM: &str = "native-pc";
pub const RUNNER: &str = "native";
pub const FILE: &str = "game.toml";

struct Pc(SystemDescriptor);
impl Pc {
    fn new() -> Self {
        Pc(SystemDescriptor {
            id: SystemId::new(SYSTEM),
            name: "Native PC".into(),
            short: "PC".into(),
            folder: "pc".into(),
            style: Style {
                color: "#3ddc97".into(),
                cover: Cover { w: 4, h: 3 },
            },
            discovery: vec![],
            metadata: vec![],
        })
    }
}
impl System for Pc {
    fn descriptor(&self) -> &SystemDescriptor {
        &self.0
    }
    fn recognise(&self, tree: &dyn Tree) -> Vec<Recognised> {
        let f = RelPath::new(FILE).unwrap();
        let Ok(bytes) = tree.read(&f) else {
            return vec![];
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return vec![];
        };
        let Ok(v) = text.parse::<toml::Table>() else {
            return vec![];
        };
        let Some(exe) = v
            .get("exe")
            .and_then(|e| e.as_str())
            .and_then(|e| RelPath::new(e).ok())
            .filter(|e| !e.is_root())
        else {
            return vec![];
        };
        if !tree.exists(&exe) {
            return vec![];
        }
        let key = tree.name();
        let name = v
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or(&key)
            .to_string();
        vec![Recognised {
            key: GameKey::new(key),
            name,
            role: Role::Game,
            root: RelPath::root(),
            extra: serde_json::json!({ "exe": exe.as_str() }),
        }]
    }
}

struct Native(RunnerDescriptor);
impl Native {
    fn new() -> Self {
        Native(RunnerDescriptor {
            id: RunnerId::new(RUNNER),
            name: "Native".into(),
            systems: vec![SystemId::new(SYSTEM)],
            process: ProcessMatch::default(),
            settings: vec![SettingSpec {
                key: "args".into(),
                label: "Extra arguments".into(),
                hint: "Passed to the game as written, split on spaces".into(),
                kind: SettingKind::Text,
                default: Value::Text(String::new()),
                advanced: true,
            }],
            controls: None,
            requires: vec![],
            launch: None,
            provision: None,
            needs_executable: false,
        })
    }
}
impl Runner for Native {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.0
    }
    fn plan(
        &self,
        _runner_exe: &Path,
        game: &InstalledGame,
        settings: &Settings,
    ) -> Result<LaunchPlan, LaunchError> {
        let exe = game
            .extra
            .get("exe")
            .and_then(|e| e.as_str())
            .and_then(|e| RelPath::new(e).ok())
            .ok_or_else(|| LaunchError::Other("this game has no recorded executable".into()))?;
        let args = settings.get("args").map(|v| v.render()).unwrap_or_default();
        Ok(LaunchPlan {
            program: exe.to_path(&game.root),
            args: args.split_whitespace().map(String::from).collect(),
            env: Default::default(),
            cwd: Some(game.root.clone()),
        })
    }
}

pub struct NativePc;
impl Plugin for NativePc {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "native-pc".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api: API_VERSION,
            permissions: Permissions {
                network: vec![],
                writes: vec!["games".into()],
            },
        }
    }
    fn register(&self, r: &mut Registrar) {
        r.system(Arc::new(Pc::new()));
        r.runner(Arc::new(Native::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::{contracts, MemTree};

    #[test]
    fn passes_the_contract_suite() {
        contracts::check_registry(&Registry::build(&[&NativePc]).unwrap());
    }

    #[test]
    fn recognises_only_games_whose_executable_exists_and_is_safe() {
        let pc = Pc::new();
        let ok = MemTree::new("mygame")
            .with(FILE, "name = \"My Game\"\nexe = \"bin/run\"")
            .with("bin/run", "#!/bin/sh");
        let r = pc.recognise(&ok);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "My Game");
        assert_eq!(r[0].extra["exe"], "bin/run");
        assert!(
            pc.recognise(&MemTree::new("g").with(FILE, "exe = \"bin/run\""))
                .is_empty(),
            "exe must exist"
        );
        assert!(
            pc.recognise(
                &MemTree::new("g")
                    .with(FILE, "exe = \"../evil\"")
                    .with("x", "")
            )
            .is_empty(),
            "traversal refused"
        );
        assert!(pc
            .recognise(&MemTree::new("g").with(FILE, "not toml [[["))
            .is_empty());
        assert!(pc
            .recognise(&MemTree::new("g").with(FILE, "exe = \"\""))
            .is_empty());
    }

    #[test]
    fn plans_from_recorded_exe_and_user_args() {
        let n = Native::new();
        let game = InstalledGame {
            game: GameRef {
                system: SystemId::new(SYSTEM),
                key: GameKey::new("g"),
            },
            name: "g".into(),
            root: "/games/pc/g".into(),
            extra: serde_json::json!({"exe": "bin/run"}),
        };
        let mut s = Settings::new();
        s.insert("args".into(), Value::Text("-a  -b".into()));
        let p = n.plan(Path::new(""), &game, &s).unwrap();
        assert_eq!(p.program, Path::new("/games/pc/g/bin/run"));
        assert_eq!(p.args, vec!["-a", "-b"]);
        let mut bad = game.clone();
        bad.extra = serde_json::json!({"exe": "../../etc/passwd"});
        assert!(n.plan(Path::new(""), &bad, &s).is_err());
    }
}
