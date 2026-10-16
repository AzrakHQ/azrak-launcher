use crate::ids::{ProviderId, RunnerId, SystemId};
use crate::launch::LaunchTemplate;
use crate::provision::Provision;
use crate::requirements::Requirement;
use crate::settings::SettingSpec;
use crate::text::Text;
use serde::{Deserialize, Serialize};

/// Cover aspect ratio as width:height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cover { pub w: u16, pub h: u16 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    /// `#rrggbb`
    pub color: String,
    pub cover: Cover,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Discovery {
    pub provider: ProviderId,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemDescriptor {
    pub id: SystemId,
    pub name: Text,
    pub short: Text,
    /// Folder name under the games directory.
    pub folder: String,
    pub style: Style,
    #[serde(default)]
    pub discovery: Vec<Discovery>,
    #[serde(default)]
    pub metadata: Vec<ProviderId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessMatch { pub names: Vec<String> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlRow { pub action: Text, pub input: String }

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Controls { pub rows: Vec<ControlRow> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunnerDescriptor {
    pub id: RunnerId,
    pub name: Text,
    pub systems: Vec<SystemId>,
    #[serde(default)]
    pub process: ProcessMatch,
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
    #[serde(default)]
    pub controls: Option<Controls>,
    #[serde(default)]
    pub requires: Vec<Requirement>,
    #[serde(default)]
    pub launch: Option<LaunchTemplate>,
    #[serde(default)]
    pub provision: Option<Provision>,
}
