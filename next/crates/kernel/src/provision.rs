use crate::path::RelPath;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReleaseSource {
    GithubReleases { repo: String },
    Url { url: String },
}

/// Pick a release asset for an OS and architecture. `pattern` supports `*` wildcards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetRule {
    pub os: String,
    pub arch: String,
    pub pattern: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    #[default]
    OnDemand,
    FetchOnFirstRun,
    Manual,
}

/// How a managed runner is obtained, activated, updated and rolled back. Pure data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provision {
    pub source: ReleaseSource,
    pub assets: Vec<AssetRule>,
    pub executable: RelPath,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default = "two")]
    pub keep: u8,
}
fn two() -> u8 { 2 }
