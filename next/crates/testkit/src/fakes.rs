use kernel::*;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct FakeHttp {
    pub routes: Mutex<BTreeMap<String, Vec<u8>>>,
    pub calls: Mutex<Vec<String>>,
}
impl FakeHttp {
    pub fn route(&self, url: &str, body: impl Into<Vec<u8>>) { self.routes.lock().unwrap().insert(url.into(), body.into()); }
}
impl Http for FakeHttp {
    fn get(&self, url: &str) -> Result<Vec<u8>, PortError> {
        self.calls.lock().unwrap().push(url.into());
        self.routes.lock().unwrap().get(url).cloned().ok_or_else(|| PortError::Network(format!("no route {url}")))
    }
}

#[derive(Default)]
pub struct FakeProcesses {
    pub spawned: Mutex<Vec<LaunchPlan>>,
    pub external: Mutex<Vec<(String, ProcessId)>>,
}
impl Processes for FakeProcesses {
    fn spawn(&self, plan: &LaunchPlan) -> Result<ProcessId, PortError> {
        let mut s = self.spawned.lock().unwrap();
        s.push(plan.clone());
        Ok(ProcessId(s.len() as u64))
    }
    fn is_running(&self, id: ProcessId) -> bool { id.0 as usize <= self.spawned.lock().unwrap().len() }
    fn find(&self, m: &ProcessMatch) -> Vec<ProcessId> {
        self.external.lock().unwrap().iter().filter(|(n, _)| m.names.contains(n)).map(|(_, id)| *id).collect()
    }
}

pub struct FixedClock(pub u64);
impl Clock for FixedClock { fn now_unix(&self) -> u64 { self.0 } }

#[derive(Default)]
pub struct RecordingProgress(pub Mutex<Vec<(String, u64, u64)>>);
impl ProgressSink for RecordingProgress {
    fn report(&self, label: &Text, done: u64, total: u64) { self.0.lock().unwrap().push((label.key.clone(), done, total)); }
}

pub struct Fakes {
    pub http: Arc<FakeHttp>,
    pub processes: Arc<FakeProcesses>,
    pub progress: Arc<RecordingProgress>,
    pub ctx: Ctx,
}

pub fn fakes() -> Fakes {
    let http = Arc::new(FakeHttp::default());
    let processes = Arc::new(FakeProcesses::default());
    let progress = Arc::new(RecordingProgress::default());
    let ctx = Ctx {
        http: http.clone(),
        processes: processes.clone(),
        clock: Arc::new(FixedClock(1_700_000_000)),
        progress: progress.clone(),
        cancel: CancelToken::new(),
    };
    Fakes { http, processes, progress, ctx }
}

/// An in-memory tree for recognisers and the install engine.
pub struct MemTree {
    pub name: String,
    pub files: BTreeMap<RelPath, Vec<u8>>,
    pub extra: Vec<Entry>,
}
impl MemTree {
    pub fn new(name: &str) -> Self { Self { name: name.into(), files: BTreeMap::new(), extra: vec![] } }
    pub fn with(mut self, path: &str, body: &str) -> Self {
        self.files.insert(RelPath::new(path).unwrap(), body.as_bytes().to_vec());
        self
    }
    pub fn with_entry(mut self, e: Entry) -> Self { self.extra.push(e); self }
}
impl Tree for MemTree {
    fn name(&self) -> String { self.name.clone() }
    fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut v: Vec<Entry> = self.files.iter()
            .map(|(p, b)| Entry { path: p.clone(), size: b.len() as u64, kind: EntryKind::File }).collect();
        v.extend(self.extra.iter().cloned());
        Ok(v)
    }
    fn read_to(&self, path: &RelPath, out: &mut dyn Write) -> io::Result<u64> {
        let b = self.files.get(path).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing"))?;
        out.write_all(b)?;
        Ok(b.len() as u64)
    }
}

/// Build a `.tar.gz` in memory: `(path, body, mode)`.
pub fn tar_gz(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    let mut b = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (name, body, mode) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(*mode);
        h.set_cksum();
        b.append_data(&mut h, name, *body).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}
