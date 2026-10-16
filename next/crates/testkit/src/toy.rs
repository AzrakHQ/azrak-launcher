use kernel::*;
use std::sync::Arc;

pub const TOY_SYSTEM: &str = "toy-sys";
pub const TOY_RUNNER: &str = "toy-run";
pub const TOY_FEED: &str = "toy-feed";
pub const MARKER: &str = "toy.game";

pub struct ToySystem(SystemDescriptor);
impl ToySystem {
    pub fn new() -> Self {
        Self(SystemDescriptor {
            id: SystemId::new(TOY_SYSTEM),
            name: "Toy System".into(),
            short: "TS".into(),
            folder: "toy".into(),
            style: Style { color: "#6ea8ff".into(), cover: Cover { w: 3, h: 4 } },
            discovery: vec![Discovery { provider: ProviderId::new(TOY_FEED), params: serde_json::json!({"page": 1}) }],
            metadata: vec![],
        })
    }
}
impl System for ToySystem {
    fn descriptor(&self) -> &SystemDescriptor { &self.0 }
    fn recognise(&self, tree: &dyn Tree) -> Vec<Recognised> {
        let marker = RelPath::new(MARKER).unwrap();
        if !tree.exists(&marker) { return vec![]; }
        let name = tree.read(&marker).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default().trim().to_string();
        let key = tree.name();
        let name = if name.is_empty() { key.clone() } else { name };
        vec![Recognised { key: GameKey::new(key), name, role: Role::Game, root: RelPath::root(), extra: serde_json::Value::Null }]
    }
}

pub struct ToyRunner(RunnerDescriptor);
impl ToyRunner {
    pub fn new() -> Self {
        Self(RunnerDescriptor {
            id: RunnerId::new(TOY_RUNNER),
            name: "Toy Runner".into(),
            systems: vec![SystemId::new(TOY_SYSTEM)],
            process: ProcessMatch { names: vec!["toyrun".into()] },
            settings: vec![
                SettingSpec { key: "fullscreen".into(), label: "Fullscreen".into(), hint: Text::default(), kind: SettingKind::Bool, default: Value::Bool(true), advanced: false },
                SettingSpec { key: "fps".into(), label: "Frame limit".into(), hint: Text::default(), kind: SettingKind::Int { min: 0, max: 240, step: 1 }, default: Value::Int(60), advanced: false },
                SettingSpec { key: "quality".into(), label: "Quality".into(), hint: Text::default(),
                    kind: SettingKind::Choice { options: vec![Choice { value: "low".into(), label: "Low".into() }, Choice { value: "high".into(), label: "High".into() }] },
                    default: Value::Text("high".into()), advanced: false },
            ],
            controls: Some(Controls { rows: vec![ControlRow { action: "Confirm".into(), input: "A".into() }] }),
            requires: vec![Requirement { expr: Expr::AtLeast("ram.gib".into(), 4.0), severity: Severity::Blocks, message: "Needs 4 GiB of memory".into() }],
            launch: Some(LaunchTemplate {
                args: vec![
                    ArgTemplate { value: "--fullscreen".into(), when: Some("fullscreen".into()) },
                    ArgTemplate { value: "--fps={setting.fps}".into(), when: None },
                    ArgTemplate { value: "{game}".into(), when: None },
                ],
                env: Default::default(),
                cwd_game: false,
            }),
            provision: None,
            needs_executable: true,
        })
    }
}
impl Runner for ToyRunner { fn descriptor(&self) -> &RunnerDescriptor { &self.0 } }

pub struct ToyFeed;
impl CatalogSource for ToyFeed {
    fn list(&self, _params: &serde_json::Value, _ctx: &Ctx) -> Result<Vec<Release>, PortError> {
        Ok(["alpha", "beta"].iter().map(|n| Release {
            id: format!("toy-{n}"), source: ProviderId::new(TOY_FEED), system: SystemId::new(TOY_SYSTEM),
            title: format!("Toy {n}"), size: Some(1024), published: None,
            acquire: Acquire::Http { url: format!("https://example.invalid/{n}") },
            games: vec![GameKey::new(*n)], extra: serde_json::Value::Null,
        }).collect())
    }
}

pub struct ToyPlugin;
impl Plugin for ToyPlugin {
    fn manifest(&self) -> Manifest {
        Manifest { id: "toy".into(), version: "0.1.0".into(), api: API_VERSION, permissions: Permissions::default() }
    }
    fn register(&self, r: &mut Registrar) {
        r.system(Arc::new(ToySystem::new()));
        r.runner(Arc::new(ToyRunner::new()));
        r.source(TOY_FEED, Arc::new(ToyFeed));
    }
}
