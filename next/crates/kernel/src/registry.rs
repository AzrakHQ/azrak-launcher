use crate::error::RegistryError;
use crate::ext::*;
use crate::ids::{ProviderId, RunnerId, SystemId};
use std::collections::BTreeMap;

/// Collects what a plugin registers.
#[derive(Default)]
pub struct Registrar {
    pub(crate) systems: Vec<DynSystem>,
    pub(crate) runners: Vec<DynRunner>,
    pub(crate) sources: Vec<(ProviderId, DynSource)>,
    pub(crate) metadata: Vec<(ProviderId, DynMetadata)>,
    pub(crate) containers: Vec<DynContainer>,
    pub(crate) collectors: Vec<DynCollector>,
    pub(crate) compat: Vec<DynCompat>,
}

impl Registrar {
    pub fn system(&mut self, s: DynSystem) -> &mut Self {
        self.systems.push(s);
        self
    }
    pub fn runner(&mut self, r: DynRunner) -> &mut Self {
        self.runners.push(r);
        self
    }
    pub fn source(&mut self, id: &str, s: DynSource) -> &mut Self {
        self.sources.push((ProviderId::new(id), s));
        self
    }
    pub fn metadata(&mut self, id: &str, m: DynMetadata) -> &mut Self {
        self.metadata.push((ProviderId::new(id), m));
        self
    }
    pub fn container(&mut self, c: DynContainer) -> &mut Self {
        self.containers.push(c);
        self
    }
    pub fn collector(&mut self, c: DynCollector) -> &mut Self {
        self.collectors.push(c);
        self
    }
    pub fn compat(&mut self, c: DynCompat) -> &mut Self {
        self.compat.push(c);
        self
    }
}

/// Immutable after construction. Answers the degradation questions in one place.
#[derive(Default)]
pub struct Registry {
    manifests: Vec<Manifest>,
    systems: Vec<DynSystem>,
    runners: Vec<DynRunner>,
    sources: BTreeMap<ProviderId, DynSource>,
    metadata: BTreeMap<ProviderId, DynMetadata>,
    containers: Vec<DynContainer>,
    collectors: Vec<DynCollector>,
    compat: Vec<DynCompat>,
    provides: BTreeMap<String, Vec<&'static str>>,
}

impl Registry {
    pub fn build(plugins: &[&dyn Plugin]) -> Result<Registry, RegistryError> {
        let mut reg = Registry::default();
        for p in plugins {
            let m = p.manifest();
            if m.api != API_VERSION {
                return Err(RegistryError::ApiMismatch(m.id, m.api, API_VERSION));
            }
            if reg.manifests.iter().any(|x| x.id == m.id) {
                return Err(RegistryError::Duplicate("plugin", m.id));
            }
            let mut r = Registrar::default();
            p.register(&mut r);
            let mut kinds = vec![];
            for (n, c) in [
                ("system", r.systems.len()),
                ("runner", r.runners.len()),
                ("catalog source", r.sources.len()),
                ("metadata", r.metadata.len()),
                ("container", r.containers.len()),
                ("fact collector", r.collectors.len()),
                ("compatibility list", r.compat.len()),
            ] {
                if c > 0 {
                    kinds.push(n);
                }
            }
            reg.provides.insert(m.id.clone(), kinds);
            for s in r.systems {
                let id = s.descriptor().id.to_string();
                if reg.systems.iter().any(|x| x.descriptor().id.as_str() == id) {
                    return Err(RegistryError::Duplicate("system", id));
                }
                reg.systems.push(s);
            }
            for s in r.runners {
                let id = s.descriptor().id.to_string();
                if reg.runners.iter().any(|x| x.descriptor().id.as_str() == id) {
                    return Err(RegistryError::Duplicate("runner", id));
                }
                reg.runners.push(s);
            }
            for (id, s) in r.sources {
                if reg.sources.insert(id.clone(), s).is_some() {
                    return Err(RegistryError::Duplicate("source", id.to_string()));
                }
            }
            for (id, s) in r.metadata {
                if reg.metadata.insert(id.clone(), s).is_some() {
                    return Err(RegistryError::Duplicate(
                        "metadata provider",
                        id.to_string(),
                    ));
                }
            }
            reg.containers.extend(r.containers);
            reg.collectors.extend(r.collectors);
            reg.compat.extend(r.compat);
            reg.manifests.push(m);
        }
        Ok(reg)
    }

    /// What a plugin registered, as display words. Derived, so it cannot drift from reality.
    pub fn provides(&self, plugin: &str) -> Vec<String> {
        self.provides
            .get(plugin)
            .map(|v| v.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }
    pub fn manifests(&self) -> &[Manifest] {
        &self.manifests
    }
    pub fn systems(&self) -> &[DynSystem] {
        &self.systems
    }
    pub fn system(&self, id: &SystemId) -> Option<&DynSystem> {
        self.systems.iter().find(|s| s.descriptor().id == *id)
    }
    pub fn runners(&self) -> &[DynRunner] {
        &self.runners
    }
    pub fn runner(&self, id: &RunnerId) -> Option<&DynRunner> {
        self.runners.iter().find(|r| r.descriptor().id == *id)
    }
    /// Runners that declare they can run a system. Empty when none are installed.
    pub fn runners_for(&self, system: &SystemId) -> Vec<&DynRunner> {
        self.runners
            .iter()
            .filter(|r| r.descriptor().systems.contains(system))
            .collect()
    }
    pub fn source(&self, id: &ProviderId) -> Option<&DynSource> {
        self.sources.get(id)
    }
    pub fn metadata_provider(&self, id: &ProviderId) -> Option<&DynMetadata> {
        self.metadata.get(id)
    }
    pub fn containers(&self) -> &[DynContainer] {
        &self.containers
    }
    pub fn collectors(&self) -> &[DynCollector] {
        &self.collectors
    }
    pub fn compat_for(&self, runner: &RunnerId) -> Option<&DynCompat> {
        self.compat.iter().find(|c| c.runner() == runner)
    }
}
