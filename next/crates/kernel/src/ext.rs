use crate::descriptors::{RunnerDescriptor, SystemDescriptor};
use crate::error::{LaunchError, PortError};
use crate::ids::{GameKey, ProviderId, RunnerId};
use crate::launch::{LaunchInputs, LaunchPlan};
use crate::model::{GameInfo, InstalledGame, Recognised, Release, Role};
use crate::path::RelPath;
use crate::ports::Ctx;
use crate::requirements::Facts;
use crate::settings::Settings;
use crate::tree::Tree;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::sync::Arc;

pub const API_VERSION: u32 = 1;

pub trait System: Send + Sync {
    fn descriptor(&self) -> &SystemDescriptor;
    /// Which games are inside these files, and in which role. Must never panic on any tree.
    fn recognise(&self, tree: &dyn Tree) -> Vec<Recognised>;
}

pub trait Runner: Send + Sync {
    fn descriptor(&self) -> &RunnerDescriptor;

    /// Pure launch planning. The default renders the descriptor's template.
    fn plan(
        &self,
        runner_exe: &Path,
        game: &InstalledGame,
        settings: &Settings,
    ) -> Result<LaunchPlan, LaunchError> {
        self.descriptor()
            .launch
            .as_ref()
            .ok_or(LaunchError::NoTemplate)?
            .render(&LaunchInputs {
                runner_exe,
                game_root: &game.root,
                settings,
            })
    }

    /// Where files of a role land inside the game folder. The consumer of the files knows where it looks.
    fn layout(&self, _role: Role, _key: &GameKey) -> RelPath {
        RelPath::root()
    }
}

pub trait CatalogSource: Send + Sync {
    fn list(&self, params: &serde_json::Value, ctx: &Ctx) -> Result<Vec<Release>, PortError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    No,
    Maybe,
    Yes,
}

pub trait Container: Send + Sync {
    fn sniff(&self, head: &[u8], file_name: &str) -> Confidence;
    fn open(&self, path: &Path) -> io::Result<Box<dyn Tree>>;
}

pub trait MetadataProvider: Send + Sync {
    fn lookup(
        &self,
        system: &crate::ids::SystemId,
        key: &GameKey,
        name: &str,
        ctx: &Ctx,
    ) -> Result<Option<GameInfo>, PortError>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompatEntry {
    pub status: String,
    pub url: Option<String>,
}

pub trait CompatSource: Send + Sync {
    fn runner(&self) -> &RunnerId;
    fn entry(&self, key: &GameKey) -> Option<CompatEntry>;
}

pub trait FactCollector: Send + Sync {
    fn namespace(&self) -> &str;
    /// Fill only the wanted keys that this collector owns.
    fn collect(&self, wanted: &BTreeSet<String>) -> Facts;
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Permissions {
    pub network: Vec<String>,
    pub writes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    pub api: u32,
    #[serde(default)]
    pub permissions: Permissions,
}

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn register(&self, r: &mut crate::registry::Registrar);
}

pub type DynSystem = Arc<dyn System>;
pub type DynRunner = Arc<dyn Runner>;
pub type DynContainer = Arc<dyn Container>;
pub type DynSource = Arc<dyn CatalogSource>;
pub type DynMetadata = Arc<dyn MetadataProvider>;
pub type DynCollector = Arc<dyn FactCollector>;
pub type DynCompat = Arc<dyn CompatSource>;

pub fn provider(id: &str) -> ProviderId {
    ProviderId::new(id)
}
