use kernel::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What a daily collector must fetch: declared by the installed systems, so there is no second list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub system: SystemId,
    pub provider: ProviderId,
    pub params: serde_json::Value,
}

pub fn manifest(reg: &Registry) -> Vec<ManifestEntry> {
    reg.systems().iter().flat_map(|s| {
        let d = s.descriptor();
        d.discovery.iter().map(|x| ManifestEntry { system: d.id.clone(), provider: x.provider.clone(), params: x.params.clone() })
    }).collect()
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub releases: Vec<Release>,
    /// Discoveries skipped because nobody installed the provider, or it failed. Reported, never fatal.
    pub skipped: Vec<String>,
}

pub fn collect(reg: &Registry, ctx: &Ctx) -> Snapshot {
    let mut snap = Snapshot::default();
    for e in manifest(reg) {
        let Some(src) = reg.source(&e.provider) else {
            snap.skipped.push(format!("{}: no provider {}", e.system, e.provider));
            continue;
        };
        match src.list(&e.params, ctx) {
            Ok(rs) => snap.releases.extend(rs.into_iter().filter(|r| r.system == e.system)),
            Err(err) => snap.skipped.push(format!("{}: {} failed: {err}", e.system, e.provider)),
        }
    }
    snap
}

pub fn save(snap: &Snapshot, path: &Path) -> std::io::Result<()> {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(snap).expect("snapshot serialises"))?;
    std::fs::rename(tmp, path)
}

/// Offline use: releases of systems that are installed. A removed plugin's releases simply do not show.
pub fn load_for(reg: &Registry, path: &Path) -> Snapshot {
    let Ok(bytes) = std::fs::read(path) else { return Snapshot::default() };
    let mut s: Snapshot = serde_json::from_slice(&bytes).unwrap_or_default();
    s.releases.retain(|r| reg.system(&r.system).is_some());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::{fakes, ToyPlugin};

    #[test]
    fn manifest_comes_from_installed_systems_only() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let m = manifest(&reg);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].provider.as_str(), "toy-feed");
        assert!(manifest(&Registry::build(&[]).unwrap()).is_empty());
    }

    #[test]
    fn collects_snapshots_and_filters_when_the_system_goes_away() {
        let d = tempfile::tempdir().unwrap();
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        let f = fakes();
        let snap = collect(&reg, &f.ctx);
        assert_eq!(snap.releases.len(), 2);
        assert!(snap.skipped.is_empty());
        let p = d.path().join("catalog.json");
        save(&snap, &p).unwrap();
        assert_eq!(load_for(&reg, &p).releases.len(), 2);
        assert!(load_for(&Registry::build(&[]).unwrap(), &p).releases.is_empty());
        assert!(load_for(&reg, &d.path().join("nope.json")).releases.is_empty());
    }

    #[test]
    fn missing_provider_is_skipped_with_a_reason() {
        // A system whose feed provider is not installed: reported, not an error.
        struct Lonely;
        impl Plugin for Lonely {
            fn manifest(&self) -> Manifest { Manifest { id: "lonely".into(), version: "1".into(), api: API_VERSION, permissions: Default::default() } }
            fn register(&self, r: &mut Registrar) { r.system(std::sync::Arc::new(testkit::toy::ToySystem::new())); }
        }
        let reg = Registry::build(&[&Lonely]).unwrap();
        let snap = collect(&reg, &fakes().ctx);
        assert!(snap.releases.is_empty());
        assert_eq!(snap.skipped.len(), 1);
    }
}
