//! Tier-0 plugins: a TOML manifest, no code. A system, a runner, or both, built from the generic engines.
use kernel::*;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeclError {
    #[error("manifest: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(String),
}

/// A system is recognised when every marker path exists in the tree; its key is the source's name.
#[derive(Debug, Clone, Deserialize)]
struct Recognise { markers: Vec<RelPath> }

#[derive(Debug, Clone, Deserialize)]
struct SystemDecl {
    #[serde(flatten)]
    desc: SystemDescriptor,
    recognise: Recognise,
}

#[derive(Debug, Deserialize)]
struct File {
    plugin: Manifest,
    #[serde(default)]
    system: Vec<SystemDecl>,
    #[serde(default)]
    runner: Vec<RunnerDescriptor>,
}

struct DeclSystem { desc: SystemDescriptor, markers: Vec<RelPath> }
impl System for DeclSystem {
    fn descriptor(&self) -> &SystemDescriptor { &self.desc }
    fn recognise(&self, tree: &dyn Tree) -> Vec<Recognised> {
        if self.markers.is_empty() || !self.markers.iter().all(|m| tree.exists(m)) { return vec![]; }
        let key = tree.name();
        vec![Recognised { name: key.clone(), key: GameKey::new(key), role: Role::Game, root: RelPath::root() }]
    }
}

struct DeclRunner(RunnerDescriptor);
impl Runner for DeclRunner { fn descriptor(&self) -> &RunnerDescriptor { &self.0 } }

pub struct DeclPlugin { manifest: Manifest, systems: Vec<Arc<DeclSystem>>, runners: Vec<Arc<DeclRunner>> }

impl Plugin for DeclPlugin {
    fn manifest(&self) -> Manifest { self.manifest.clone() }
    fn register(&self, r: &mut Registrar) {
        for s in &self.systems { r.system(s.clone()); }
        for x in &self.runners { r.runner(x.clone()); }
    }
}

pub fn parse(text: &str) -> Result<DeclPlugin, DeclError> {
    let f: File = toml::from_str(text).map_err(|e| DeclError::Parse(e.to_string()))?;
    Ok(DeclPlugin {
        manifest: f.plugin,
        systems: f.system.into_iter().map(|s| Arc::new(DeclSystem { desc: s.desc, markers: s.recognise.markers })).collect(),
        runners: f.runner.into_iter().map(|r| Arc::new(DeclRunner(r))).collect(),
    })
}

/// Load every `*.toml` in a directory. A broken manifest is reported and skipped, never fatal.
pub fn load_dir(dir: &Path) -> (Vec<DeclPlugin>, Vec<(PathBuf, DeclError)>) {
    let (mut ok, mut bad) = (vec![], vec![]);
    let Ok(rd) = std::fs::read_dir(dir) else { return (ok, bad) };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "toml").unwrap_or(false)).collect();
    paths.sort();
    for p in paths {
        match std::fs::read_to_string(&p).map_err(|e| DeclError::Io(e.to_string())).and_then(|t| parse(&t)) {
            Ok(pl) => ok.push(pl),
            Err(e) => bad.push((p, e)),
        }
    }
    (ok, bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::{contracts, MemTree};

    pub const SAMPLE: &str = r##"
[plugin]
id = "sample"
version = "0.1.0"
api = 1

[[system]]
id = "sample-sys"
name = "Sample System"
short = "SS"
folder = "sample"
style = { color = "#ffaa44", cover = { w = 1, h = 1 } }
recognise = { markers = ["boot.bin"] }

[[runner]]
id = "sample-run"
name = "Sample Runner"
systems = ["sample-sys"]
process = { names = ["samplerun"] }

[[runner.settings]]
key = "fullscreen"
label = "Fullscreen"
type = "bool"
default = true

[[runner.settings]]
key = "scale"
label = "Scale"
type = "int"
min = 1
max = 4
default = 2

[[runner.settings]]
key = "api"
label = "Graphics API"
type = "choice"
options = [{ value = "vk", label = "Vulkan" }, { value = "gl", label = "OpenGL" }]
default = "vk"

[runner.launch]
cwd_game = true
args = [
  { value = "--fullscreen", when = "fullscreen" },
  { value = "--scale={setting.scale}" },
  { value = "{game}" },
]

[[runner.requires]]
expr = { at_least = ["ram.gib", 4.0] }
severity = "blocks"
message = "Needs 4 GiB of memory"

[runner.provision]
policy = "on_demand"
executable = "bin/sample"
assets = [{ os = "linux", arch = "x86_64", pattern = "sample-*.tar.gz" }]
source = { type = "github_releases", repo = "o/r" }
"##;

    #[test]
    fn manifest_becomes_working_system_and_runner() {
        let p = parse(SAMPLE).unwrap();
        let reg = Registry::build(&[&p]).unwrap();
        contracts::check_registry(&reg);
        let sys = reg.system(&SystemId::new("sample-sys")).unwrap();
        let tree = MemTree::new("mygame").with("boot.bin", "");
        let rec = sys.recognise(&tree);
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].key.as_str(), "mygame");
        assert!(sys.recognise(&MemTree::new("x")).is_empty());
        let run = reg.runner(&RunnerId::new("sample-run")).unwrap();
        assert_eq!(run.descriptor().settings.len(), 3);
        assert!(run.descriptor().provision.is_some());
    }

    #[test]
    fn broken_manifest_is_reported_not_fatal() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("good.toml"), SAMPLE).unwrap();
        std::fs::write(d.path().join("bad.toml"), "[plugin]\nid = 1").unwrap();
        let (ok, bad) = load_dir(d.path());
        assert_eq!(ok.len(), 1);
        assert_eq!(bad.len(), 1);
        let (ok, bad) = load_dir(&d.path().join("missing"));
        assert!(ok.is_empty() && bad.is_empty());
    }
}
