//! Opt-in native torrent transfers. Downloads only start from an explicit action. Completed
//! downloads seed while "Keep sharing finished downloads" is on, including after a restart; only that
//! setting turns seeding off for good (Stop seeding on one transfer lasts until the next launch).
//! Magnet lookup can use DHT/trackers before metadata (including its private flag) is known.
//! A dedicated Tokio worker owns the engine; the UI reads bounded snapshots once per second.
//! Our manifest restores unfinished transfers paused, not via rqbit's session persistence.

use anyhow::{bail, ensure, Context, Result};
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions, TorrentStatsState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, Id, JoinError, JoinSet};

const MAX_JOBS: usize = 32;
const MAX_METADATA: u64 = 16 * 1024 * 1024;
const MARKER: &str = ".ps5-launcher-owned";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State { Resolving, Ready, Checking, Downloading, Paused, Complete, Failed, Cancelled }

impl State {
    pub fn active(self) -> bool { matches!(self, Self::Resolving | Self::Checking | Self::Downloading) }
    pub fn label(self) -> &'static str {
        match self {
            Self::Resolving => "Resolving magnet metadata", Self::Ready => "Ready · review files before downloading",
            Self::Checking => "Checking existing pieces", Self::Downloading => "Downloading", Self::Paused => "Paused",
            Self::Complete => "Complete · stopped", Self::Failed => "Failed", Self::Cancelled => "Cancelled · partial files kept",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub key: String,
    pub topic: i64,
    pub name: String,
    magnet: String,
    pub folder: PathBuf,
    pub state: State,
    pub done: u64,
    pub total: u64,
    pub file_count: usize,
    pub files: Vec<String>,
    pub error: String,
    #[serde(skip)] pub speed: f64,
    #[serde(skip)] pub eta: String,
    #[serde(skip)] pub peers: usize,
    #[serde(skip)] pub seeding: bool,
    #[serde(skip)] pub upload_speed: f64,
    /// Checking a completed download's files before it seeds again.
    #[serde(skip)] pub verifying: bool,
    /// Stopped because "download only through a VPN" is on and the VPN isn't connected;
    /// resumes by itself when it is.
    #[serde(default)] pub waiting_vpn: bool,
}

impl Job {
    pub fn label(&self) -> &'static str {
        if self.state == State::Complete && self.seeding { "Complete · seeding" }
        else if self.state == State::Complete && self.verifying { "Complete · checking files to seed" }
        else if self.waiting_vpn && !self.state.active() && self.state != State::Complete { "Paused · waiting for your VPN" }
        else { self.state.label() }
    }
    fn clear_runtime(&mut self) {
        self.speed = 0.0; self.peers = 0; self.eta.clear(); self.seeding = false; self.upload_speed = 0.0; self.verifying = false;
    }
    pub fn progress(&self) -> f32 {
        if self.state == State::Complete { return 1.0; }
        if self.total == 0 { 0.0 } else { (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32 }
    }
    pub fn primary(&self) -> &'static str {
        match self.state {
            State::Ready => "Start download", State::Checking | State::Downloading => "Pause",
            State::Paused => "Resume", State::Failed | State::Cancelled => "Retry", _ => "",
        }
    }
    pub fn action_id(&self) -> &'static str {
        match self.state {
            State::Ready => "start", State::Checking | State::Downloading => "pause",
            State::Paused => "resume", State::Failed | State::Cancelled => "retry", _ => "",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Manifest { schema: u32, jobs: Vec<Job> }

#[derive(Clone)]
struct Policy {
    local_only: bool,
    initial_peers: Vec<SocketAddr>,
    download_bps: Option<NonZeroU32>,
    seed_after_download: bool,
    /// Download only through a VPN: `Some(preferred interface)` ("" = any connected VPN).
    vpn: Option<String>,
    /// Finds the connected VPN interface (tests substitute their own).
    vpn_probe: fn(&str) -> Option<String>,
    // Test-only listener is restricted to ephemeral loopback TCP in local-only sessions.
    #[cfg(test)] loopback_listener: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            local_only: false, initial_peers: vec![], download_bps: None, seed_after_download: true,
            vpn: None, vpn_probe: crate::vpn::active,
            #[cfg(test)] loopback_listener: false,
        }
    }
}

enum Command { Prepare(Job), Start(String), Pause(String), Cancel(String), Remove(String), StopSeed(String), SetSeedAfterDownload(bool), SetVpn(Option<String>), Shutdown }

const VPN_WAIT: &str = "Waiting for your VPN: downloads only run through it (Settings → Downloads)";
const FILES_CHANGED: &str = "The downloaded files were moved, changed or deleted, so this download can't seed. Remove it, or download it again.";

pub struct Manager {
    jobs: Arc<Mutex<Vec<Job>>>,
    tx: mpsc::Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Manager {
    /// `vpn`: download only through a VPN (`Some(preferred interface)`, "" for any).
    pub fn load(seed_after_download: bool, vpn: Option<String>) -> Self {
        Self::new(crate::util::config_dir().join("transfers"), Policy { seed_after_download, vpn, ..Default::default() })
    }

    fn new(store: PathBuf, policy: Policy) -> Self {
        let jobs = load_manifest(&store);
        let jobs = Arc::new(Mutex::new(jobs));
        let (tx, rx) = mpsc::channel(64);
        let shared = jobs.clone();
        let thread = std::thread::Builder::new().name("torrent-worker".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => { crate::log!("Could not create download runtime: {error}"); return; }
            };
            runtime.block_on(Worker::new(store, shared, policy).run(rx));
            runtime.shutdown_timeout(Duration::from_secs(2));
        }).expect("could not create download worker");
        Self { jobs, tx, thread: Some(thread) }
    }

    pub fn snapshot(&self) -> Vec<Job> { self.jobs.lock().unwrap().clone() }

    /// Called only after the user confirms metadata lookup and its network behavior.
    pub fn prepare(&self, topic: i64, name: &str, magnet: &str, base: PathBuf) -> Result<String> {
        let (key, magnet) = normalize_magnet(magnet)?;
        ensure!(base.is_absolute(), "Choose an absolute download folder (or a ~/ path)");
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.iter().any(|job| job.key == key) { return Ok(key); }
        ensure!(jobs.len() < MAX_JOBS, "Remove an old transfer first (maximum {MAX_JOBS})");
        let job = Job {
            folder: base.join(format!("torrent-{key}")), key: key.clone(), topic,
            name: name.chars().filter(|c| !c.is_control()).take(200).collect(), magnet,
            state: State::Resolving, done: 0, total: 0, file_count: 0, files: vec![], error: String::new(),
            speed: 0.0, eta: String::new(), peers: 0, seeding: false, upload_speed: 0.0, verifying: false, waiting_vpn: false,
        };
        self.tx.try_send(Command::Prepare(job.clone())).context("Download worker is busy or unavailable")?;
        jobs.push(job);
        Ok(key)
    }

    pub fn action(&self, key: &str, action: &str) -> Result<()> {
        let jobs = self.jobs.lock().unwrap();
        let job = jobs.iter().find(|job| job.key == key).context("Transfer not found")?;
        let command = match action {
            "start" if job.state == State::Ready => Command::Start(key.into()),
            "resume" if job.state == State::Paused => Command::Start(key.into()),
            "retry" if matches!(job.state, State::Failed | State::Cancelled) => Command::Start(key.into()),
            "pause" if matches!(job.state, State::Checking | State::Downloading) => Command::Pause(key.into()),
            "cancel" if !matches!(job.state, State::Complete | State::Cancelled) => Command::Cancel(key.into()),
            "remove" if !job.state.active() => Command::Remove(key.into()),
            "stop_seed" if job.state == State::Complete && (job.seeding || job.verifying) => Command::StopSeed(key.into()),
            _ => return Ok(()),
        };
        self.tx.try_send(command).context("Download worker is busy or unavailable")
    }

    /// Changes future completion behavior; enabling never starts a restored/stopped torrent.
    pub fn set_seed_after_download(&self, enabled: bool) -> Result<()> {
        self.tx.try_send(Command::SetSeedAfterDownload(enabled)).context("Download worker is busy or unavailable")
    }

    /// Download only through a VPN (`Some(preferred interface)`), or through any connection.
    pub fn set_vpn(&self, vpn: Option<String>) -> Result<()> {
        self.tx.try_send(Command::SetVpn(vpn)).context("Download worker is busy or unavailable")
    }

    pub fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            // The worker continues draining the bounded command channel during shutdown.
            let _ = self.tx.blocking_send(Command::Shutdown);
            let _ = thread.join();
        }
    }
}

impl Drop for Manager { fn drop(&mut self) { self.shutdown(); } }

fn load_manifest(store: &Path) -> Vec<Job> {
    use std::io::Read;
    let bytes = std::fs::File::open(store.join("jobs.json")).ok().and_then(|file| {
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    });
    let manifest: Manifest = bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    if manifest.schema != 1 { return vec![]; }
    let mut seen = std::collections::HashSet::new();
    manifest.jobs.into_iter().filter(|job| {
        normalize_magnet(&job.magnet).is_ok_and(|(key, _)| key == job.key)
            && job.folder.is_absolute() && seen.insert(job.key.clone())
    }).take(MAX_JOBS).map(|mut job| {
        if job.state.active() { job.state = State::Paused; }
        job.clear_runtime();
        job
    }).collect()
}

/// Reject ambiguous hashes, huge file-selection ranges and unsupported sources.
/// Normalize base32 BTIH too; never pass unknown URL parameters to the engine.
pub(crate) fn normalize_magnet(input: &str) -> Result<(String, String)> {
    ensure!(input.len() <= 32 * 1024 && input.starts_with("magnet:?"), "Invalid magnet link");
    let source = url::Url::parse(input).context("Invalid magnet URL")?;
    let mut hash = None;
    let mut trackers = Vec::new();
    let mut name = None;
    for (key, value) in source.query_pairs() {
        match key.as_ref() {
            "xt" => {
                ensure!(hash.is_none(), "Ambiguous magnet info hash");
                let raw = value.strip_prefix("urn:btih:").context("Only BitTorrent v1 magnets are supported")?;
                let normalized = if raw.len() == 40 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
                    raw.to_ascii_lowercase()
                } else if raw.len() == 32 {
                    let bytes = data_encoding::BASE32.decode(raw.to_ascii_uppercase().as_bytes()).context("Invalid base32 info hash")?;
                    ensure!(bytes.len() == 20, "Invalid info hash length");
                    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
                } else { bail!("Invalid magnet info hash"); };
                hash = Some(normalized);
            }
            "tr" => {
                ensure!(trackers.len() < 64 && value.len() <= 4096, "Too many or oversized trackers");
                let tracker = url::Url::parse(&value).context("Invalid tracker URL")?;
                ensure!(matches!(tracker.scheme(), "http" | "https" | "udp") && tracker.host_str().is_some(), "Unsupported tracker URL");
                trackers.push(value.into_owned());
            }
            "dn" => name = Some(value.chars().filter(|c| !c.is_control()).take(200).collect::<String>()),
            "so" => bail!("File-selection magnets are not supported; use the complete torrent magnet"),
            _ => {},
        }
    }
    let key = hash.context("Magnet has no info hash")?;
    let mut normalized = url::Url::parse("magnet:?")?;
    {
        let mut query = normalized.query_pairs_mut();
        query.append_pair("xt", &format!("urn:btih:{key}"));
        for tracker in trackers { query.append_pair("tr", &tracker); }
        if let Some(name) = name { query.append_pair("dn", &name); }
    }
    Ok((key, normalized.into()))
}

fn secure_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Missing parent directory")?;
    std::fs::create_dir_all(parent)?;
    ensure!(!std::fs::symlink_metadata(parent)?.file_type().is_symlink(), "State folder must not be a symlink");
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    crate::util::atomic_write(path, bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn owned_folder(job: &Job) -> Result<PathBuf> {
    ensure!(job.folder.file_name().is_some_and(|name| name == format!("torrent-{}", job.key).as_str()), "Invalid owned transfer folder");
    let base = job.folder.parent().context("Invalid destination")?;
    std::fs::create_dir_all(base).context("Cannot create download folder")?;
    let base = base.canonicalize()?;
    let folder = base.join(job.folder.file_name().context("Invalid destination")?);
    match std::fs::symlink_metadata(&folder) {
        Ok(metadata) => {
            ensure!(metadata.is_dir() && !metadata.file_type().is_symlink(), "Transfer folder is not a regular directory");
            ensure!(metadata.uid() == unsafe { libc::geteuid() }, "Transfer folder belongs to another user");
            let marker = std::fs::symlink_metadata(folder.join(MARKER)).context("Missing ownership marker")?;
            ensure!(marker.is_file() && !marker.file_type().is_symlink() && marker.nlink() == 1
                && marker.uid() == unsafe { libc::geteuid() } && marker.len() == job.key.len() as u64,
                "Unsafe ownership marker");
            ensure!(std::fs::read_to_string(folder.join(MARKER)).ok().as_deref() == Some(job.key.as_str()),
                "Refusing to overwrite an existing folder not owned by this transfer");
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&folder)?;
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
            secure_write(&folder.join(MARKER), job.key.as_bytes())?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(folder)
}

fn check_path(folder: &Path, relative: &Path) -> Result<()> {
    ensure!(!relative.as_os_str().is_empty(), "Torrent contains an empty filename");
    let mut path = folder.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(component) = component else { bail!("Unsafe torrent file path"); };
        let name = component.to_string_lossy();
        ensure!(!name.contains(['\0', '\\']) && name != MARKER && !name.chars().any(char::is_control), "Unsafe torrent filename");
        path.push(component);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(!metadata.file_type().is_symlink(), "Refusing a symlink destination");
                if components.peek().is_some() {
                    ensure!(metadata.is_dir(), "Torrent parent path is not a directory");
                } else {
                    ensure!(metadata.is_file() && metadata.nlink() == 1, "Refusing a directory, special file or hard-link destination");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn inspect_metadata(bytes: &[u8], key: &str, folder: &Path) -> Result<(u64, Vec<String>)> {
    ensure!(bytes.len() as u64 <= MAX_METADATA, "Torrent metadata is too large");
    let meta = librqbit::torrent_from_bytes(bytes).context("Invalid torrent metadata")?;
    ensure!(meta.info_hash.as_string() == key, "Metadata info hash does not match the magnet");
    let info = meta.info.data.validate().context("Unsafe torrent metadata")?;
    let mut total = 0u64;
    let mut files = Vec::new();
    let mut paths = std::collections::BTreeSet::new();
    for file in info.iter_file_details() {
        ensure!(files.len() < 10_000, "Torrent contains too many files");
        ensure!(!file.attrs().symlink && file.symlink_path.is_none(), "Symlink torrents are not supported");
        let components = file.filename.to_vec();
        ensure!(!components.is_empty() && components.iter().all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(['/', '\\'])), "Unsafe path component");
        let path = file.filename.to_pathbuf();
        check_path(folder, &path)?;
        ensure!(paths.insert(path.clone()), "Duplicate torrent file path");
        total = total.checked_add(file.len).context("Torrent size overflow")?;
        ensure!(total <= i64::MAX as u64, "Torrent is too large");
        files.push(format!("{} · {}", path.display(), bytes_label(file.len)));
    }
    for path in &paths {
        ensure!(!path.ancestors().skip(1).any(|parent| paths.contains(parent)), "Conflicting torrent file paths");
    }
    ensure!(total > 0, "Torrent is empty");
    Ok((total, files))
}

/// Every file of a completed download is still in place at its full size: seeding must never
/// turn into downloading missing files again.
fn payload_present(bytes: &[u8], folder: &Path) -> Result<()> {
    let meta = librqbit::torrent_from_bytes(bytes).context("Invalid torrent metadata")?;
    let info = meta.info.data.validate().context("Unsafe torrent metadata")?;
    for file in info.iter_file_details() {
        let found = std::fs::symlink_metadata(folder.join(file.filename.to_pathbuf())).ok();
        ensure!(found.is_some_and(|m| m.is_file() && m.len() == file.len), FILES_CHANGED);
    }
    Ok(())
}

/// Disk allocation, not saved verified progress: deleted files and sparse holes need space again.
fn required_space(bytes: &[u8], folder: &Path) -> Result<u64> {
    let meta = librqbit::torrent_from_bytes(bytes)?;
    let info = meta.info.data.validate()?;
    let mut required = 0u64;
    for file in info.iter_file_details() {
        let path = file.filename.to_pathbuf();
        check_path(folder, &path)?;
        let allocated = match std::fs::symlink_metadata(folder.join(path)) {
            Ok(metadata) => metadata.blocks().saturating_mul(512).min(file.len),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        required = required.checked_add(file.len - allocated).context("Torrent size overflow")?;
    }
    Ok(required)
}

fn available_space(folder: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    ensure!(unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0, "Cannot check destination free space");
    let stat = unsafe { stat.assume_init() };
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

pub fn bytes_label(bytes: u64) -> String {
    let value = bytes as f64;
    if bytes >= 1024 * 1024 * 1024 { format!("{:.2} GiB", value / (1024.0 * 1024.0 * 1024.0)) }
    else if bytes >= 1024 * 1024 { format!("{:.1} MiB", value / (1024.0 * 1024.0)) }
    else if bytes >= 1024 { format!("{:.1} KiB", value / 1024.0) } else { format!("{bytes} B") }
}

struct Resolved { key: String, bytes: Vec<u8>, peers: Vec<SocketAddr> }
struct Worker {
    store: PathBuf,
    jobs: Arc<Mutex<Vec<Job>>>,
    policy: Policy,
    session: Option<Arc<Session>>,
    handles: HashMap<String, Arc<ManagedTorrent>>,
    peers: HashMap<String, Vec<SocketAddr>>,
    resolving: HashMap<String, AbortHandle>,
    task_keys: HashMap<Id, String>,
    tasks: JoinSet<Result<Resolved>>,
    /// Completed downloads added paused while their files are checked, before seeding again.
    reseeding: std::collections::HashSet<String>,
    /// The VPN interface the current session is bound to.
    bound: Option<String>,
}

impl Worker {
    fn new(store: PathBuf, jobs: Arc<Mutex<Vec<Job>>>, policy: Policy) -> Self {
        Self { store, jobs, policy, session: None, handles: HashMap::new(), peers: HashMap::new(), resolving: HashMap::new(), task_keys: HashMap::new(), tasks: JoinSet::new(), reseeding: Default::default(), bound: None }
    }
    fn job(&self, key: &str) -> Option<Job> { self.jobs.lock().unwrap().iter().find(|job| job.key == key).cloned() }
    fn update(&self, key: &str, f: impl FnOnce(&mut Job)) {
        if let Some(job) = self.jobs.lock().unwrap().iter_mut().find(|job| job.key == key) { f(job); }
    }
    fn persist(&self) {
        let bytes = serde_json::to_vec_pretty(&Manifest { schema: 1, jobs: self.jobs.lock().unwrap().clone() });
        if let Err(error) = bytes.context("Cannot serialize transfers").and_then(|bytes| secure_write(&self.store.join("jobs.json"), &bytes)) {
            // Local path/IO errors only: never log magnets or tracker credentials.
            crate::log!("Could not save transfer state: {error}");
        }
    }
    fn fail(&self, key: &str, error: anyhow::Error) {
        // Engine errors can contain tracker/magnet URLs; redact them from the UI.
        let message = regex::Regex::new(r"(?:https?://|udp://|magnet:\?)[^\s]+")
            .unwrap().replace_all(&error.to_string(), "[network URL]").chars().take(500).collect::<String>();
        self.update(key, |job| {
            if job.state != State::Complete { job.state = State::Failed; }
            job.error = message; job.clear_runtime();
        });
        self.persist();
    }
    async fn session(&mut self) -> Result<Arc<Session>> {
        if let Some(session) = &self.session { return Ok(session.clone()); }
        let mut opts = SessionOptions {
            persistence: None, fastresume: false, disable_local_service_discovery: true,
            peer_limit: Some(50), concurrent_init_limit: Some(2),
            ratelimits: librqbit::limits::LimitsConfig { upload_bps: NonZeroU32::new(128 * 1024), download_bps: self.policy.download_bps },
            ..Default::default()
        };
        if self.policy.local_only { opts.dht = None; opts.disable_trackers = true; opts.listen = None; }
        // Every connection, listener, tracker and DHT socket is bound to the VPN's interface, so
        // nothing can leave by another route; without a connected VPN nothing starts.
        let bound = match &self.policy.vpn {
            Some(preferred) => Some((self.policy.vpn_probe)(preferred).context(VPN_WAIT)?),
            None => None,
        };
        opts.bind_device_name = bound.clone();
        #[cfg(test)]
        if self.policy.local_only && self.policy.loopback_listener {
            opts.listen = Some(librqbit::ListenerOptions {
                mode: librqbit::ListenerMode::TcpOnly, listen_addr: "127.0.0.1:0".parse().unwrap(),
                enable_upnp_port_forwarding: false, ..Default::default()
            });
        }
        let session = Session::new_with_opts(self.store.join("unused-default-output"), opts).await?;
        self.session = Some(session.clone());
        self.bound = bound;
        Ok(session)
    }

    /// The VPN interface downloads must use, when "only through a VPN" is on: `Err` while it
    /// isn't connected.
    fn vpn_route(&self) -> std::result::Result<Option<String>, ()> {
        match &self.policy.vpn {
            None => Ok(None),
            Some(preferred) => (self.policy.vpn_probe)(preferred).map(Some).ok_or(()),
        }
    }

    /// Keep a transfer for later, until the VPN is connected.
    fn wait_for_vpn(&mut self, key: &str) {
        self.update(key, |job| {
            if job.state != State::Complete { job.state = State::Paused; }
            job.waiting_vpn = true; job.error.clear(); job.clear_runtime();
        });
        self.persist();
    }

    /// The kill switch, every couple of seconds: if the VPN downloads are bound to went away (or
    /// another one replaced it), stop everything; once a VPN is connected again (or the setting
    /// is off), resume what was stopped and seed again.
    async fn vpn_check(&mut self) {
        let route = self.vpn_route();
        if self.session.is_some() && route.as_ref().map_or(true, |iface| *iface != self.bound) {
            self.stop_all_for_vpn().await;
        }
        if route.is_ok() && self.session.is_none() {
            let waiting: Vec<String> = self.jobs.lock().unwrap().iter().filter(|j| j.waiting_vpn).map(|j| j.key.clone()).collect();
            for key in waiting {
                self.update(&key, |job| job.waiting_vpn = false);
                let complete = self.job(&key).is_some_and(|job| job.state == State::Complete);
                if !complete {
                    if let Err(error) = self.start(&key).await { self.fail(&key, error); }
                }
            }
            if self.policy.seed_after_download { self.seed_all().await; }
            self.persist();
        }
    }

    /// Stop every transfer and seed and close the session (it is bound to a VPN that is gone or
    /// changed); unfinished transfers resume when downloads may run again.
    async fn stop_all_for_vpn(&mut self) {
        let resolving: Vec<String> = self.resolving.keys().cloned().collect();
        let keys: Vec<String> = self.handles.keys().cloned().chain(resolving).collect();
        for key in keys {
            let complete = self.job(&key).is_some_and(|job| job.state == State::Complete)
                || self.handles.get(&key).is_some_and(|handle| handle.stats().finished);
            let _ = self.stop_job(&key, if complete { State::Complete } else { State::Paused }).await;
            self.wait_for_vpn(&key);
        }
        if let Some(session) = self.session.take() { let _ = tokio::time::timeout(Duration::from_secs(5), session.stop()).await; }
        self.bound = None;
        crate::log!("downloads stopped: the VPN they use isn't connected");
    }
    async fn resolve(&mut self, key: &str) -> Result<()> {
        if self.resolving.contains_key(key) || self.handles.contains_key(key) { return Ok(()); }
        if self.vpn_route().is_err() { self.wait_for_vpn(key); return Ok(()); }
        let job = self.job(key).context("Missing transfer")?;
        let folder = owned_folder(&job)?;
        self.update(key, |job| { job.folder = folder.clone(); job.state = State::Resolving; job.error.clear(); });
        self.persist();
        let session = self.session().await?;
        let initial_peers = self.policy.initial_peers.clone();
        let key = key.to_string();
        let abort = self.tasks.spawn(async move {
                let response = tokio::time::timeout(Duration::from_secs(180), session.add_torrent(
                    AddTorrent::from_url(job.magnet), Some(AddTorrentOptions {
                        list_only: true, output_folder: Some(folder.to_string_lossy().into_owned()),
                        initial_peers: Some(initial_peers), ..Default::default()
                    }),
                )).await.context("Metadata lookup timed out; no reachable peers. Retry later.")??;
                let AddTorrentResponse::ListOnly(response) = response else { bail!("Unexpected metadata response"); };
                Ok(Resolved { key: key.clone(), bytes: response.torrent_bytes.to_vec(), peers: response.seen_peers })
        });
        self.task_keys.insert(abort.id(), job.key.clone());
        self.resolving.insert(job.key, abort);
        Ok(())
    }
    fn resolution_finished(&mut self, result: std::result::Result<(Id, Result<Resolved>), JoinError>) {
        let id = match &result { Ok((id, _)) => *id, Err(error) => error.id() };
        let Some(key) = self.task_keys.remove(&id) else { return; };
        // A cancelled generation can finish (or panic) after Retry has installed a new task.
        if !self.resolving.get(&key).is_some_and(|active| active.id() == id) { return; }
        self.resolving.remove(&key);
        if !self.job(&key).is_some_and(|job| job.state == State::Resolving) { return; }
        let resolved = match result {
            Ok((_, result)) => result,
            Err(_) => Err(anyhow::anyhow!("Metadata lookup task stopped unexpectedly; retry")),
        };
        if let Err(error) = resolved.and_then(|resolved| self.resolved(resolved)) { self.fail(&key, error); }
    }
    fn resolved(&mut self, resolved: Resolved) -> Result<()> {
        let job = self.job(&resolved.key).context("Missing transfer")?;
        if job.state != State::Resolving { return Ok(()); }
        let (total, files) = inspect_metadata(&resolved.bytes, &job.key, &job.folder)?;
        secure_write(&self.store.join(format!("{}.torrent", job.key)), &resolved.bytes)?;
        self.peers.insert(job.key.clone(), resolved.peers);
        self.update(&job.key, |job| {
            job.total = total; job.file_count = files.len(); job.files = files.into_iter().take(100).collect();
            job.state = State::Ready; job.error.clear();
        });
        self.persist();
        Ok(())
    }
    async fn start(&mut self, key: &str) -> Result<()> {
        let job = self.job(key).context("Missing transfer")?;
        if job.state.active() || job.state == State::Complete { return Ok(()); }
        if self.vpn_route().is_err() { self.wait_for_vpn(key); return Ok(()); }
        self.update(key, |job| job.waiting_vpn = false);
        if let Some(handle) = self.handles.get(key).cloned() {
            self.session().await?.unpause(&handle).await?;
            self.update(key, |job| { job.state = State::Checking; job.error.clear(); });
        } else {
            let path = self.store.join(format!("{key}.torrent"));
            if !path.is_file() { return self.resolve(key).await; }
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(path)?.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
            let folder = owned_folder(&job)?;
            let (total, _) = inspect_metadata(&bytes, key, &folder)?;
            ensure!(available_space(&folder)? >= required_space(&bytes, &folder)?, "Not enough free space in the download folder");
            let session = self.session().await?;
            let magnet = librqbit::Magnet::parse(&job.magnet)?;
            let peers = self.peers.get(key).cloned().unwrap_or_else(|| self.policy.initial_peers.clone());
            let response = session.add_torrent(AddTorrent::from_bytes(bytes), Some(AddTorrentOptions {
                paused: false, overwrite: true, output_folder: Some(folder.to_string_lossy().into_owned()),
                trackers: Some(magnet.trackers), initial_peers: Some(peers), ..Default::default()
            })).await?;
            let handle = response.into_handle().context("No transfer handle")?;
            self.handles.insert(key.into(), handle);
            self.update(key, |job| { job.folder = folder; job.total = total; job.done = 0; job.state = State::Checking; job.error.clear(); });
        }
        self.persist();
        Ok(())
    }
    async fn stop_job(&mut self, key: &str, mut state: State) -> Result<()> {
        self.reseeding.remove(key);
        if let Some(abort) = self.resolving.remove(key) { abort.abort(); }
        let mut pause_error = None;
        if let Some(handle) = self.handles.get(key).cloned() {
            if let Some(session) = &self.session {
                // Snapshot only after pause has stopped piece writes; delete never removes payload.
                // rqbit rejects pause() for already Paused/Error handles. They still need deletion.
                pause_error = if matches!(handle.stats().state, TorrentStatsState::Live | TorrentStatsState::Initializing { .. }) {
                    session.pause(&handle).await.err()
                } else { None };
                let stats = handle.stats();
                // Completion can race a pause/shutdown command before the next tick.
                if state == State::Paused && stats.finished { state = State::Complete; }
                self.update(key, |job| {
                    // Error/None engine states report zero progress even for a previously verified seed.
                    if job.state != State::Complete || stats.finished {
                        job.done = stats.progress_bytes; job.total = stats.total_bytes;
                    }
                });
                session.delete(handle.id().into(), false).await?;
                self.handles.remove(key);
            }
            self.handles.remove(key);
        }
        self.update(key, |job| { job.state = state; job.clear_runtime(); });
        self.persist();
        if let Some(error) = pause_error { return Err(error); }
        Ok(())
    }

    async fn stop_idle_session(&mut self) {
        // Completed seeds count as active network handles even though State::active() is false.
        if self.handles.is_empty() && self.resolving.is_empty() {
            if let Some(session) = self.session.take() { let _ = tokio::time::timeout(Duration::from_secs(5), session.stop()).await; }
        }
    }

    async fn set_seed_after_download(&mut self, enabled: bool) -> Result<()> {
        self.policy.seed_after_download = enabled;
        if enabled {
            self.seed_all().await;
            return Ok(());
        }
        let keys: Vec<_> = self.handles.iter().filter(|(key, handle)| {
            handle.stats().finished || self.job(key).is_some_and(|job| job.state == State::Complete)
        }).map(|(key, _)| key.clone()).collect();
        let mut first_error = None;
        for key in keys {
            if let Err(error) = self.stop_job(&key, State::Complete).await {
                if first_error.is_none() { first_error = Some(error); }
            }
        }
        self.stop_idle_session().await;
        if let Some(error) = first_error { return Err(error); }
        Ok(())
    }

    /// Seed every completed download that isn't seeding yet (at startup, and when seeding is
    /// turned back on). Unfinished transfers are never started here.
    async fn seed_all(&mut self) {
        let keys: Vec<String> = self.jobs.lock().unwrap().iter()
            .filter(|job| job.state == State::Complete && !self.handles.contains_key(&job.key))
            .map(|job| job.key.clone()).collect();
        for key in keys {
            if let Err(error) = self.seed(&key).await { self.fail(&key, error); }
        }
    }

    /// Seed a completed download again. Its files are checked against the torrent with the
    /// engine paused first, so a download whose files changed stops instead of downloading.
    async fn seed(&mut self, key: &str) -> Result<()> {
        let Some(job) = self.job(key) else { return Ok(()) };
        if job.state != State::Complete || self.handles.contains_key(key) || !self.policy.seed_after_download { return Ok(()); }
        if self.vpn_route().is_err() { self.wait_for_vpn(key); return Ok(()); }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(self.store.join(format!("{key}.torrent"))).context("Torrent metadata is missing")?
            .take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
        // Never create a folder just to find it empty.
        ensure!(job.folder.is_dir(), FILES_CHANGED);
        let folder = owned_folder(&job)?;
        inspect_metadata(&bytes, key, &folder)?;
        payload_present(&bytes, &folder)?;
        let session = self.session().await?;
        let magnet = librqbit::Magnet::parse(&job.magnet)?;
        let peers = self.peers.get(key).cloned().unwrap_or_else(|| self.policy.initial_peers.clone());
        let response = session.add_torrent(AddTorrent::from_bytes(bytes), Some(AddTorrentOptions {
            paused: true, overwrite: true, output_folder: Some(folder.to_string_lossy().into_owned()),
            trackers: Some(magnet.trackers), initial_peers: Some(peers), ..Default::default()
        })).await?;
        let handle = response.into_handle().context("No transfer handle")?;
        self.handles.insert(key.into(), handle);
        self.reseeding.insert(key.into());
        self.update(key, |job| { job.verifying = true; job.error.clear(); });
        Ok(())
    }

    /// After the paused check: seed if every piece is intact, otherwise stop without downloading.
    async fn reseed_checked(&mut self, key: &str, handle: &Arc<ManagedTorrent>) {
        let stats = handle.stats();
        match stats.state {
            TorrentStatsState::Initializing { .. } => {}
            TorrentStatsState::Paused if stats.finished && self.policy.seed_after_download => {
                self.reseeding.remove(key);
                self.update(key, |job| job.verifying = false);
                let unpaused = match self.session().await { Ok(session) => session.unpause(handle).await, Err(error) => Err(error) };
                if let Err(error) = unpaused {
                    let _ = self.stop_job(key, State::Complete).await;
                    self.fail(key, error);
                }
            }
            _ => {
                self.reseeding.remove(key);
                let _ = self.stop_job(key, State::Complete).await;
                self.fail(key, anyhow::anyhow!(FILES_CHANGED));
            }
        }
    }

    async fn remove(&mut self, key: &str) -> Result<()> {
        let Some(job) = self.job(key).filter(|job| !job.state.active()) else { return Ok(()); };
        // Stop/delete the engine handle BEFORE removing history. Never delete payload.
        self.stop_job(key, job.state).await?;
        self.jobs.lock().unwrap().retain(|job| job.key != key);
        let _ = std::fs::remove_file(self.store.join(format!("{key}.torrent")));
        self.peers.remove(key); self.persist();
        self.stop_idle_session().await;
        Ok(())
    }

    async fn tick(&mut self) {
        let handles: Vec<_> = self.handles.iter().map(|(key, handle)| (key.clone(), handle.clone())).collect();
        for (key, handle) in handles {
            if self.reseeding.contains(&key) {
                self.reseed_checked(&key, &handle).await;
                continue;
            }
            let stats = handle.stats();
            let was_complete = self.job(&key).is_some_and(|job| job.state == State::Complete);
            let complete = stats.finished || was_complete;
            let seeding = stats.finished && self.policy.seed_after_download && matches!(stats.state, TorrentStatsState::Live);
            let state = if complete { State::Complete } else { match stats.state {
                TorrentStatsState::Initializing { .. } => State::Checking,
                TorrentStatsState::Live => State::Downloading,
                TorrentStatsState::Paused => State::Paused,
                TorrentStatsState::Error => State::Failed,
            } };
            let (speed, upload_speed, peers, eta) = stats.live.as_ref().map(|live| {
                let peers = live.snapshot.peer_stats.live as usize;
                (live.download_speed.mbps, live.upload_speed.mbps, peers, live.time_remaining.as_ref().map(ToString::to_string).unwrap_or_default())
            }).unwrap_or_default();
            let newly_complete = stats.finished && !was_complete;
            self.update(&key, |job| {
                if !was_complete || stats.finished { job.done = stats.progress_bytes; job.total = stats.total_bytes; }
                job.state = state;
                job.speed = if complete { 0.0 } else { speed }; job.peers = peers;
                job.eta = if complete { String::new() } else { eta };
                job.seeding = seeding; job.upload_speed = if seeding { upload_speed } else { 0.0 };
            });
            if complete {
                if seeding {
                    if newly_complete { self.persist(); }
                } else if let Err(error) = self.stop_job(&key, State::Complete).await { self.fail(&key, error); }
            } else if let Some(error) = stats.error {
                let _ = self.stop_job(&key, State::Failed).await;
                self.fail(&key, anyhow::anyhow!(error));
            }
        }
        self.stop_idle_session().await;
    }
    async fn run(mut self, mut rx: mpsc::Receiver<Command>) {
        if self.policy.seed_after_download { self.seed_all().await; }
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut dirty_ticks = 0;
        let mut vpn_tick = tokio::time::interval(Duration::from_secs(2));
        vpn_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                command = rx.recv() => {
                    let result = match command {
                        Some(Command::Prepare(job)) => { let key = job.key; Some((key.clone(), self.resolve(&key).await)) },
                        Some(Command::Start(key)) => Some((key.clone(), self.start(&key).await)),
                        Some(Command::Pause(key)) => {
                            self.update(&key, |job| job.waiting_vpn = false);
                            Some((key.clone(), self.stop_job(&key, State::Paused).await))
                        },
                        Some(Command::Cancel(key)) => {
                            self.update(&key, |job| job.waiting_vpn = false);
                            Some((key.clone(), self.stop_job(&key, State::Cancelled).await))
                        },
                        Some(Command::Remove(key)) => Some((key.clone(), self.remove(&key).await)),
                        Some(Command::StopSeed(key)) => {
                            let result = if self.job(&key).is_some_and(|job| job.state == State::Complete) {
                                self.stop_job(&key, State::Complete).await
                            } else { Ok(()) };
                            self.stop_idle_session().await;
                            Some((key, result))
                        },
                        Some(Command::SetSeedAfterDownload(enabled)) => {
                            if self.set_seed_after_download(enabled).await.is_err() {
                                crate::log!("Could not stop every completed seed; retry disabling seeding");
                            }
                            None
                        },
                        Some(Command::SetVpn(vpn)) => {
                            // Reconnect everything under the new rule.
                            self.policy.vpn = vpn;
                            if self.session.is_some() { self.stop_all_for_vpn().await; }
                            self.vpn_check().await;
                            None
                        },
                        Some(Command::Shutdown) | None => break,
                    };
                    if let Some((key, Err(error))) = result { self.fail(&key, error); }
                },
                result = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                    if let Some(result) = result { self.resolution_finished(result); }
                },
                _ = vpn_tick.tick(), if self.policy.vpn.is_some() || self.jobs.lock().unwrap().iter().any(|j| j.waiting_vpn) => {
                    self.vpn_check().await;
                },
                _ = tick.tick(), if self.session.is_some() => {
                    self.tick().await;
                    dirty_ticks += 1;
                    if dirty_ticks == 5 {
                        if self.jobs.lock().unwrap().iter().any(|job| job.state.active()) { self.persist(); }
                        dirty_ticks = 0;
                    }
                },
            }
        }
        self.shutdown().await;
    }

    async fn shutdown(&mut self) {
        self.tasks.abort_all();
        let keys: Vec<_> = self.handles.keys().cloned().collect();
        for key in keys {
            let complete = self.job(&key).is_some_and(|job| job.state == State::Complete)
                || self.handles.get(&key).is_some_and(|handle| handle.stats().finished);
            let state = if complete { State::Complete } else { State::Paused };
            if self.stop_job(&key, state).await.is_err() {
                self.update(&key, |job| { job.state = state; job.clear_runtime(); });
                crate::log!("Could not remove a transfer handle during shutdown; stopping its session");
            }
        }
        if let Some(session) = self.session.take() { let _ = tokio::time::timeout(Duration::from_secs(5), session.stop()).await; }
        self.handles.clear();
        for job in self.jobs.lock().unwrap().iter_mut() {
            if job.state.active() { job.state = State::Paused; }
            job.clear_runtime();
        }
        if !self.jobs.lock().unwrap().is_empty() { self.persist(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(base: &Path, key: &str) -> Job {
        Job {
            key: key.into(), topic: 0, name: "Generated public-domain fixture".into(),
            magnet: format!("magnet:?xt=urn:btih:{key}"), folder: base.join(format!("torrent-{key}")),
            state: State::Resolving, done: 0, total: 0, file_count: 0, files: vec![], error: String::new(),
            speed: 0.0, eta: String::new(), peers: 0, seeding: false, upload_speed: 0.0, verifying: false, waiting_vpn: false,
        }
    }

    fn bstring(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(format!("{}:", bytes.len()).as_bytes());
        out.extend_from_slice(bytes);
    }

    // Hand-encoded malicious metadata never enters a network session.
    fn metadata(paths: &[&[&str]], attrs: Option<&str>, private: bool) -> (String, Vec<u8>) {
        let mut info = b"d5:filesl".to_vec();
        for path in paths {
            info.push(b'd');
            if let Some(attrs) = attrs {
                info.extend_from_slice(b"4:attr"); bstring(&mut info, attrs.as_bytes());
            }
            info.extend_from_slice(b"6:lengthi65536e4:pathl");
            for part in *path { bstring(&mut info, part.as_bytes()); }
            info.extend_from_slice(b"ee");
        }
        info.extend_from_slice(b"e4:name7:fixture12:piece lengthi65536e6:pieces");
        bstring(&mut info, &vec![0; paths.len() * 20]);
        if private { info.extend_from_slice(b"7:privatei1e"); }
        info.push(b'e');
        let key = sha1_smol::Sha1::from(&info).digest().to_string();
        let mut bytes = b"d4:info".to_vec(); bytes.extend(info); bytes.push(b'e');
        (key, bytes)
    }

    async fn wait_job(manager: &Manager, key: &str, predicate: impl Fn(&Job) -> bool) -> Job {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                interval.tick().await;
                let job = manager.snapshot().into_iter().find(|job| job.key == key).expect("job exists");
                assert_ne!(job.state, State::Failed, "backend failed: {}", job.error);
                if predicate(&job) { return job; }
            }
        }).await.unwrap_or_else(|_| panic!("timed out: {:?}", manager.snapshot()))
    }

    async fn assert_unchanged(manager: &Manager, key: &str, path: &Path, state: State, seconds: u64) {
        let before = std::fs::read(path).unwrap();
        let done = manager.snapshot().into_iter().find(|job| job.key == key).unwrap().done;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        for _ in 0..=seconds {
            interval.tick().await;
            let snapshot = manager.snapshot().into_iter().find(|job| job.key == key).unwrap();
            assert_eq!(snapshot.state, state);
            assert_eq!(snapshot.done, done);
            assert_eq!(snapshot.peers, 0);
            assert_eq!(snapshot.speed, 0.0);
            assert!(!snapshot.seeding);
            assert_eq!(snapshot.upload_speed, 0.0);
            assert_eq!(std::fs::read(path).unwrap(), before, "inactive payload changed");
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
    }

    async fn seeder(root: &Path) -> (Arc<Session>, librqbit::CreateTorrentResult, Arc<ManagedTorrent>, Vec<u8>) {
        // Fresh generated bytes are freely distributable; no catalog, tracker, DHT or multicast access.
        let mut seed = 0x12345678u32;
        let payload: Vec<u8> = (0..512 * 1024).map(|_| {
            seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed as u8
        }).collect();
        let source = root.join("public-domain-fixture.bin");
        std::fs::write(&source, &payload).unwrap();
        let session = Session::new_with_opts(root.to_path_buf(), SessionOptions {
            dht: None, disable_trackers: true, disable_local_service_discovery: true,
            persistence: None, fastresume: false,
            listen: Some(librqbit::ListenerOptions {
                mode: librqbit::ListenerMode::TcpOnly, listen_addr: "127.0.0.1:0".parse().unwrap(),
                enable_upnp_port_forwarding: false, ..Default::default()
            }),
            ..Default::default()
        }).await.unwrap();
        let (torrent, handle) = session.create_and_serve_torrent(&source, librqbit::CreateTorrentOptions {
            name: None, trackers: vec![], piece_length: Some(65536),
        }).await.unwrap();
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                interval.tick().await;
                if handle.stats().finished { break; }
            }
        }).await.expect("seeder verifies fixture");
        (session, torrent, handle, payload)
    }

    fn local_policy(seeder: &Session) -> Policy {
        Policy { local_only: true, initial_peers: vec![seeder.listen_addr().unwrap()], download_bps: NonZeroU32::new(32 * 1024), seed_after_download: false, ..Default::default() }
    }

    fn ready_fixture(worker: &mut Worker, base: &Path, torrent: &librqbit::CreateTorrentResult) -> String {
        let key = normalize_magnet(&torrent.as_magnet().to_string()).unwrap().0;
        let mut job = job(base, &key);
        job.state = State::Ready;
        worker.jobs.lock().unwrap().push(job);
        secure_write(&worker.store.join(format!("{key}.torrent")), &torrent.as_bytes().unwrap()).unwrap();
        key
    }

    async fn complete_worker(worker: &mut Worker, key: &str) -> Job {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                interval.tick().await;
                worker.tick().await;
                let job = worker.job(key).unwrap();
                assert_ne!(job.state, State::Failed, "fixture failed: {}", job.error);
                if job.state == State::Complete { return job; }
            }
        }).await.expect("authored loopback transfer completes")
    }

    async fn completed_worker(root: &Path, enabled: bool) -> (Worker, Arc<Session>, Arc<ManagedTorrent>, Vec<u8>, String) {
        let (source, torrent, source_handle, expected) = seeder(root).await;
        let policy = Policy {
            local_only: true, initial_peers: vec![source.listen_addr().unwrap()], download_bps: None,
            seed_after_download: enabled, loopback_listener: true, ..Default::default()
        };
        let mut worker = Worker::new(root.join("state"), Arc::new(Mutex::new(vec![])), policy);
        assert!(worker.session.is_none(), "fixture has no engine before explicit Start");
        let key = ready_fixture(&mut worker, &root.join("downloads"), &torrent);
        worker.start(&key).await.unwrap();
        let job = complete_worker(&mut worker, &key).await;
        assert_eq!(job.done, expected.len() as u64);
        assert_eq!(job.total, expected.len() as u64);
        assert_eq!(std::fs::read(job.folder.join("public-domain-fixture.bin")).unwrap(), expected);
        (worker, source, source_handle, expected, key)
    }

    #[test]
    fn loopback_completed_seed_uploads_verified_bytes_and_restart_seeds_again() {
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let (mut worker, source, source_handle, expected, key) = completed_worker(temp.path(), true).await;
            assert!(Policy::default().seed_after_download, "production seeding defaults on");
            let job = worker.job(&key).unwrap();
            assert!(job.seeding);
            assert_eq!(job.label(), "Complete · seeding");
            assert_eq!(job.progress(), 1.0);
            assert!(!job.state.active(), "completion remains installable, not a download");
            assert_eq!(job.speed, 0.0);
            assert!(job.eta.is_empty());
            let session = worker.session.as_ref().unwrap().clone();
            let handle = worker.handles[&key].clone();
            assert!(matches!(handle.stats().state, TorrentStatsState::Live));
            // Only the newly completed worker can serve the second leecher: source is offline.
            source.stop().await;
            let uploaded_before = handle.stats().uploaded_bytes;
            let leecher = Session::new_with_opts(temp.path().join("second-leecher"), SessionOptions {
                dht: None, disable_trackers: true, disable_local_service_discovery: true,
                persistence: None, fastresume: false, listen: None, ..Default::default()
            }).await.unwrap();
            let bytes = std::fs::read(worker.store.join(format!("{key}.torrent"))).unwrap();
            let leecher_handle = leecher.add_torrent(AddTorrent::from_bytes(bytes), Some(AddTorrentOptions {
                initial_peers: Some(vec![session.listen_addr().unwrap()]), ..Default::default()
            })).await.unwrap().into_handle().unwrap();
            let mut observed_upload_speed = false;
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            tokio::time::timeout(Duration::from_secs(90), async {
                loop {
                    interval.tick().await;
                    worker.tick().await;
                    let job = worker.job(&key).unwrap();
                    assert_eq!(job.state, State::Complete);
                    assert!(job.seeding, "idle download checks must not stop a completed seed");
                    observed_upload_speed |= job.upload_speed > 0.0;
                    if leecher_handle.stats().finished { break; }
                }
            }).await.expect("second loopback leecher receives completed seed");
            assert!(observed_upload_speed, "live upload speed reaches the job snapshot");
            assert!(handle.stats().uploaded_bytes >= uploaded_before + expected.len() as u64);
            assert_eq!(std::fs::read(temp.path().join("second-leecher/public-domain-fixture.bin")).unwrap(), expected);
            leecher.stop().await;
            worker.shutdown().await;
            assert!(worker.session.is_none());
            assert!(worker.handles.is_empty());
            assert!(session.get(handle.id().into()).is_none());
            let saved = std::fs::read(worker.store.join("jobs.json")).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
            assert_eq!(json["schema"], 1);
            assert_eq!(json["jobs"][0]["state"], "Complete");
            assert!(json["jobs"][0].get("seeding").is_none());
            assert!(json["jobs"][0].get("upload_speed").is_none());
            let restored = load_manifest(&worker.store);
            assert_eq!(restored[0].state, State::Complete);
            assert!(!restored[0].seeding);
            assert_eq!(restored[0].upload_speed, 0.0);
            assert_eq!(restored[0].label(), State::Complete.label());
            let source_uploaded = source_handle.stats().uploaded_bytes;
            // A restart seeds again: the files are checked first, then shared, never downloaded.
            let manager = Manager::new(worker.store.clone(), worker.policy.clone());
            let resumed = wait_job(&manager, &key, |job| job.seeding).await;
            assert_eq!(resumed.state, State::Complete);
            assert_eq!(resumed.label(), "Complete · seeding");
            assert!(resumed.error.is_empty());
            // Only the Settings switch stops it.
            manager.set_seed_after_download(false).unwrap();
            wait_job(&manager, &key, |job| !job.seeding && !job.verifying).await;
            let mut manager = manager;
            // The UI owns Manager outside Tokio. Its synchronous shutdown joins
            // the worker; keep that join off this test's async executor too.
            tokio::task::spawn_blocking(move || manager.shutdown()).await.unwrap();
            assert_eq!(source_handle.stats().uploaded_bytes, source_uploaded, "seeding again never downloads");
            assert_eq!(std::fs::read(job.folder.join("public-domain-fixture.bin")).unwrap(), expected);
        });
    }

    #[test]
    fn loopback_disabled_completion_stops_and_enable_seeds_finished_but_never_paused() {
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let (mut worker, source, _, expected, key) = completed_worker(temp.path(), false).await;
            let complete = worker.job(&key).unwrap();
            assert!(!complete.seeding);
            assert_eq!(complete.upload_speed, 0.0);
            assert_eq!(complete.peers, 0);
            assert!(worker.handles.is_empty());
            assert!(worker.session.is_none());
            let mut paused = job(&temp.path().join("paused"), &"0".repeat(40));
            paused.state = State::Paused;
            worker.jobs.lock().unwrap().push(paused.clone());
            worker.set_seed_after_download(true).await.unwrap();
            assert!(worker.handles.contains_key(&key), "turning seeding on seeds the completed download again");
            assert!(worker.job(&key).unwrap().verifying);
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            tokio::time::timeout(Duration::from_secs(30), async {
                while !worker.job(&key).unwrap().seeding { interval.tick().await; worker.tick().await; }
            }).await.expect("completed download seeds again after its files are checked");
            assert!(worker.resolving.is_empty());
            assert_eq!(worker.job(&key).unwrap().state, State::Complete);
            assert_eq!(worker.job(&paused.key).unwrap().state, State::Paused);
            assert!(!paused.folder.exists(), "enable never starts restored paused transfer");
            assert_eq!(std::fs::read(complete.folder.join("public-domain-fixture.bin")).unwrap(), expected);
            worker.shutdown().await;
            source.stop().await;
        });
    }

    #[test]
    fn loopback_restart_with_missing_or_changed_files_never_downloads() {
        runtime().block_on(async {
            for change in ["delete", "corrupt"] {
                let temp = tempfile::tempdir().unwrap();
                let (mut worker, source, source_handle, expected, key) = completed_worker(temp.path(), true).await;
                let payload = worker.job(&key).unwrap().folder.join("public-domain-fixture.bin");
                worker.shutdown().await;
                if change == "delete" {
                    std::fs::remove_file(&payload).unwrap();
                } else {
                    let mut bytes = expected.clone();
                    bytes[1000] ^= 0xff;
                    std::fs::write(&payload, bytes).unwrap();
                }
                let changed = std::fs::read(&payload).ok();
                let uploaded = source_handle.stats().uploaded_bytes;
                let mut restarted = Worker::new(worker.store.clone(), Arc::new(Mutex::new(load_manifest(&worker.store))), worker.policy.clone());
                restarted.seed_all().await;
                let mut interval = tokio::time::interval(Duration::from_millis(50));
                tokio::time::timeout(Duration::from_secs(30), async {
                    while restarted.job(&key).unwrap().error.is_empty() { interval.tick().await; restarted.tick().await; }
                }).await.expect("changed files are reported");
                let job = restarted.job(&key).unwrap();
                assert_eq!(job.state, State::Complete, "{change}");
                assert!(!job.seeding && !job.verifying);
                assert!(job.error.contains("can't seed"), "{change}: {}", job.error);
                assert!(restarted.handles.is_empty());
                for _ in 0..20 { interval.tick().await; restarted.tick().await; }
                assert_eq!(source_handle.stats().uploaded_bytes, uploaded, "{change}: nothing was downloaded again");
                assert_eq!(std::fs::read(&payload).ok(), changed, "{change}: files left as they were");
                restarted.shutdown().await;
                source.stop().await;
            }
        });
    }

    #[test]
    fn loopback_settings_off_stops_all_completed_handles_immediately_and_keeps_data() {
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let (mut worker, source, _, expected, key) = completed_worker(temp.path(), true).await;
            // A second independently authored torrent proves OFF stops all seeds, not just one.
            let second_path = temp.path().join("second-fixture.bin");
            let second_expected: Vec<_> = expected.iter().map(|byte| byte ^ 0xa5).collect();
            std::fs::write(&second_path, &second_expected).unwrap();
            let (torrent, _) = source.create_and_serve_torrent(&second_path, librqbit::CreateTorrentOptions {
                name: None, trackers: vec![], piece_length: Some(65536),
            }).await.unwrap();
            let second_key = ready_fixture(&mut worker, &temp.path().join("downloads"), &torrent);
            worker.start(&second_key).await.unwrap();
            complete_worker(&mut worker, &second_key).await;
            let session = worker.session.as_ref().unwrap().clone();
            let handles: Vec<_> = worker.handles.values().cloned().collect();
            assert_eq!(handles.len(), 2);
            // Exercise the production bounded command dispatch, without production test hooks.
            let (tx, rx) = mpsc::channel(64);
            let mut manager = Manager { jobs: worker.jobs.clone(), tx, thread: None };
            let running = tokio::spawn(worker.run(rx));
            manager.set_seed_after_download(false).unwrap();
            wait_job(&manager, &key, |job| !job.seeding).await;
            wait_job(&manager, &second_key, |job| !job.seeding).await;
            for handle in &handles { assert!(session.get(handle.id().into()).is_none()); }
            manager.set_seed_after_download(true).unwrap();
            manager.tx.send(Command::Shutdown).await.unwrap();
            running.await.unwrap();
            for (key, filename, bytes) in [(&key, "public-domain-fixture.bin", &expected), (&second_key, "second-fixture.bin", &second_expected)] {
                let job = manager.snapshot().into_iter().find(|job| &job.key == key).unwrap();
                assert_eq!(job.state, State::Complete);
                assert_eq!(job.done, bytes.len() as u64);
                assert!(!job.seeding);
                assert_eq!(job.upload_speed, 0.0);
                assert_eq!(job.peers, 0);
                assert_eq!(std::fs::read(job.folder.join(filename)).unwrap(), *bytes);
            }
            manager.shutdown();
            source.stop().await;
        });
    }

    #[test]
    fn loopback_manual_stop_seed_and_remove_live_seed_preserve_complete_payload() {
        runtime().block_on(async {
            // Separate fixtures exercise Remove both while seeding and after manual Stop seeding.
            for stop_first in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let (worker, source, _, expected, key) = completed_worker(temp.path(), true).await;
                let store = worker.store.clone();
                let session = worker.session.as_ref().unwrap().clone();
                let handle = worker.handles[&key].clone();
                let payload = worker.job(&key).unwrap().folder.join("public-domain-fixture.bin");
                let (tx, rx) = mpsc::channel(64);
                let manager = Manager { jobs: worker.jobs.clone(), tx, thread: None };
                let running = tokio::spawn(worker.run(rx));
                if stop_first {
                    manager.action(&key, "stop_seed").unwrap();
                    let stopped = wait_job(&manager, &key, |job| !job.seeding).await;
                    assert_eq!(stopped.state, State::Complete);
                    assert_eq!(stopped.progress(), 1.0);
                    assert_eq!(stopped.label(), State::Complete.label());
                    assert!(session.get(handle.id().into()).is_none());
                    assert_unchanged(&manager, &key, &payload, State::Complete, 1).await;
                }
                manager.action(&key, "remove").unwrap();
                let mut interval = tokio::time::interval(Duration::from_millis(25));
                tokio::time::timeout(Duration::from_secs(15), async {
                    while !manager.snapshot().is_empty() { interval.tick().await; }
                }).await.unwrap();
                assert!(session.get(handle.id().into()).is_none(), "handle removed before history disappears");
                manager.tx.send(Command::Shutdown).await.unwrap();
                running.await.unwrap();
                assert_eq!(std::fs::read(&payload).unwrap(), expected);
                assert!(payload.parent().unwrap().join(MARKER).is_file());
                assert!(!store.join(format!("{key}.torrent")).exists());
                assert!(load_manifest(&store).is_empty());
                source.stop().await;
            }
        });
    }

    static FAKE_VPN: Mutex<Option<String>> = Mutex::new(None);
    fn fake_vpn(preferred: &str) -> Option<String> {
        FAKE_VPN.lock().unwrap().clone().filter(|vpn| preferred.is_empty() || vpn == preferred)
    }

    #[test]
    fn vpn_only_downloads_wait_run_bound_to_the_vpn_and_stop_when_it_drops() {
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let (source, torrent, _, expected) = seeder(temp.path()).await;
            let mut policy = local_policy(&source);
            policy.vpn = Some(String::new());
            policy.vpn_probe = fake_vpn;
            *FAKE_VPN.lock().unwrap() = None;
            let mut worker = Worker::new(temp.path().join("state"), Arc::new(Mutex::new(vec![])), policy);
            let key = ready_fixture(&mut worker, &temp.path().join("downloads"), &torrent);

            // No VPN: nothing connects, the transfer waits.
            worker.start(&key).await.unwrap();
            let job = worker.job(&key).unwrap();
            assert!(job.waiting_vpn && job.state == State::Paused, "{:?}", job.state);
            assert_eq!(job.label(), "Paused · waiting for your VPN");
            assert!(worker.session.is_none() && worker.handles.is_empty());

            // The VPN connects (here: loopback, where the authored seed listens): the session is
            // bound to that interface and the transfer resumes by itself.
            *FAKE_VPN.lock().unwrap() = Some("lo".into());
            worker.vpn_check().await;
            assert_eq!(worker.bound.as_deref(), Some("lo"));
            assert!(!worker.job(&key).unwrap().waiting_vpn);
            assert!(worker.handles.contains_key(&key));
            worker.tick().await;

            // The VPN drops mid-transfer: everything stops, the session closes.
            *FAKE_VPN.lock().unwrap() = None;
            worker.vpn_check().await;
            let job = worker.job(&key).unwrap();
            assert!(job.waiting_vpn && job.state == State::Paused);
            assert!(worker.session.is_none() && worker.handles.is_empty() && worker.bound.is_none());

            // Back: it resumes and finishes intact.
            *FAKE_VPN.lock().unwrap() = Some("lo".into());
            worker.vpn_check().await;
            let job = complete_worker(&mut worker, &key).await;
            assert!(!job.waiting_vpn);
            let payload = std::fs::read(owned_folder(&job).unwrap().join("public-domain-fixture.bin")).unwrap();
            assert_eq!(payload, expected);

            // Turning the setting off lets downloads run without a VPN again.
            *FAKE_VPN.lock().unwrap() = None;
            worker.policy.vpn = None;
            assert_eq!(worker.vpn_route(), Ok(None));
            worker.shutdown().await;
        });
    }

    /// Proof that the binding, not our bookkeeping, keeps traffic on the VPN. Runs in a private
    /// network namespace whose "VPN" (dummy0) can't reach the local seed:
    ///   unshare -rn sh -c 'ip link set lo up; ip link add dummy0 type dummy;
    ///     ip addr add 10.9.9.1/24 dev dummy0; ip link set dummy0 up;
    ///     PS5_NETNS_TEST=1 target/release/deps/ps5_launcher-* --ignored vpn_binding'
    #[test]
    #[ignore]
    fn vpn_binding_keeps_traffic_off_every_other_route() {
        if std::env::var_os("PS5_NETNS_TEST").is_none() { return; }
        fn dummy(_: &str) -> Option<String> { Some("dummy0".into()) }
        fn loopback(_: &str) -> Option<String> { Some("lo".into()) }
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let (source, torrent, _, expected) = seeder(temp.path()).await;
            let mut policy = local_policy(&source);
            policy.download_bps = None;
            policy.vpn = Some(String::new());
            policy.vpn_probe = dummy;
            let mut worker = Worker::new(temp.path().join("state"), Arc::new(Mutex::new(vec![])), policy);
            let key = ready_fixture(&mut worker, &temp.path().join("downloads"), &torrent);
            worker.start(&key).await.unwrap();
            assert_eq!(worker.bound.as_deref(), Some("dummy0"));
            // The seed is reachable over loopback, but every socket is bound to dummy0.
            for _ in 0..16 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                worker.tick().await;
            }
            let job = worker.job(&key).unwrap();
            assert_eq!((job.done, job.peers), (0, 0), "no byte may arrive outside the VPN interface");
            // Control: the same transfer bound to loopback completes.
            worker.policy.vpn_probe = loopback;
            worker.vpn_check().await;
            let job = complete_worker(&mut worker, &key).await;
            let payload = std::fs::read(owned_folder(&job).unwrap().join("public-domain-fixture.bin")).unwrap();
            assert_eq!(payload, expected);
            worker.shutdown().await;
        });
    }

    #[test]
    fn loopback_shutdown_before_completion_tick_preserves_fully_verified_handle() {
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let (source, torrent, _, expected) = seeder(temp.path()).await;
            let mut worker = Worker::new(temp.path().join("state"), Arc::new(Mutex::new(vec![])), Policy {
                local_only: true, loopback_listener: true, ..Default::default()
            });
            let key = ready_fixture(&mut worker, &temp.path().join("downloads"), &torrent);
            let folder = owned_folder(&worker.job(&key).unwrap()).unwrap();
            let payload = folder.join("public-domain-fixture.bin");
            std::fs::write(&payload, &expected).unwrap();
            source.stop().await;
            worker.start(&key).await.unwrap();
            let handle = worker.handles[&key].clone();
            let session = worker.session.as_ref().unwrap().clone();
            let mut interval = tokio::time::interval(Duration::from_millis(25));
            tokio::time::timeout(Duration::from_secs(30), async {
                while !handle.stats().finished { interval.tick().await; }
            }).await.expect("existing authored bytes verify without a peer");
            assert_eq!(worker.job(&key).unwrap().state, State::Checking, "no completion tick ran");
            worker.shutdown().await;
            assert!(session.get(handle.id().into()).is_none());
            assert!(worker.handles.is_empty());
            assert!(worker.session.is_none());
            let restored = load_manifest(&worker.store);
            assert_eq!(restored[0].state, State::Complete);
            assert_eq!(restored[0].done, expected.len() as u64);
            assert!(!restored[0].seeding);
            assert_eq!(restored[0].speed, 0.0);
            assert_eq!(restored[0].upload_speed, 0.0);
            assert_eq!(restored[0].peers, 0);
            assert!(restored[0].eta.is_empty());
            assert_eq!(std::fs::read(payload).unwrap(), expected);
        });
    }

    #[test]
    fn seeding_flags_are_never_persisted_or_restored_and_settings_commands_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let mut job = job(temp.path(), "0123456789abcdef0123456789abcdef01234567");
        job.state = State::Complete; job.done = 65536; job.total = 65536;
        job.seeding = true; job.upload_speed = 0.125; job.speed = 1.0; job.peers = 5; job.eta = "old ETA".into();
        let mut json = serde_json::to_value(&Manifest { schema: 1, jobs: vec![job] }).unwrap();
        for field in ["seeding", "upload_speed", "speed", "peers", "eta"] {
            assert!(json["jobs"][0].get(field).is_none(), "runtime field {field} leaked to manifest");
        }
        // Even an old/external manifest containing runtime fields cannot advertise a stale seed.
        json["jobs"][0]["seeding"] = true.into();
        json["jobs"][0]["upload_speed"] = 42.0.into();
        secure_write(&temp.path().join("jobs.json"), &serde_json::to_vec(&json).unwrap()).unwrap();
        let restored = load_manifest(temp.path());
        assert_eq!(restored[0].state, State::Complete);
        assert!(!restored[0].seeding);
        assert_eq!(restored[0].upload_speed, 0.0);
        let (tx, mut rx) = mpsc::channel(1);
        let manager = Manager { jobs: Arc::new(Mutex::new(restored)), tx, thread: None };
        manager.set_seed_after_download(false).unwrap();
        assert!(manager.set_seed_after_download(true).is_err(), "full channel must report busy, not block UI");
        assert!(matches!(rx.try_recv(), Ok(Command::SetSeedAfterDownload(false))));
        drop(rx);
        assert!(manager.set_seed_after_download(true).is_err(), "closed worker must report unavailable");
    }

    #[test]
    fn loopback_prepare_start_pause_restore_resume_and_complete() {
        let temp = tempfile::tempdir().unwrap();
        let rt = runtime();
        let (seeder, torrent, seed_handle, expected) = rt.block_on(seeder(temp.path()));
        let store = temp.path().join("state");
        let base = temp.path().join("downloads");
        let mut manager = Manager::new(store.clone(), local_policy(&seeder));
        assert!(manager.snapshot().is_empty());
        assert!(!store.exists(), "load alone creates no session or manifest");
        let key = manager.prepare(1, "Public-domain generated bytes", &torrent.as_magnet().to_string(), base).unwrap();
        let ready = rt.block_on(wait_job(&manager, &key, |job| job.state == State::Ready));
        assert_eq!(ready.total, expected.len() as u64);
        assert_eq!(ready.done, 0);
        assert_eq!(ready.file_count, 1);
        let names: Vec<_> = std::fs::read_dir(&ready.folder).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from(MARKER)], "Prepare must not write ANY payload");
        assert!(store.join(format!("{key}.torrent")).is_file());
        assert_eq!(seed_handle.stats().uploaded_bytes, 0, "metadata lookup did not fetch pieces");
        manager.action(&key, "primary").unwrap();
        assert_eq!(manager.snapshot()[0].state, State::Ready, "legacy ambiguous action is ignored");
        manager.action(&key, "start").unwrap();
        let partial = rt.block_on(wait_job(&manager, &key, |job| job.state == State::Downloading && job.done > 0 && job.done < job.total));
        assert_eq!(partial.peers, 1, "typed live count reports the loopback seeder");
        assert!(partial.progress() > 0.0 && partial.progress() < 1.0);
        let payload = partial.folder.join("public-domain-fixture.bin");
        manager.action(&key, "pause").unwrap();
        let paused = rt.block_on(wait_job(&manager, &key, |job| job.state == State::Paused));
        assert!(paused.done >= partial.done && paused.done < paused.total);
        rt.block_on(assert_unchanged(&manager, &key, &payload, State::Paused, 3));
        manager.shutdown();
        let saved = std::fs::read(&payload).unwrap();
        let uploaded = seed_handle.stats().uploaded_bytes;
        let mut restored = Manager::new(store.clone(), local_policy(&seeder));
        assert_eq!(restored.snapshot()[0].state, State::Paused);
        assert_eq!(restored.snapshot()[0].done, paused.done);
        rt.block_on(assert_unchanged(&restored, &key, &payload, State::Paused, 2));
        assert_eq!(std::fs::read(&payload).unwrap(), saved);
        assert_eq!(seed_handle.stats().uploaded_bytes, uploaded, "restore must not resume traffic");
        // Damage an already verified piece: explicit resume must rehash, not trust manifest progress.
        use std::io::{Seek, Write};
        let mut file = std::fs::OpenOptions::new().write(true).open(&payload).unwrap();
        file.seek(std::io::SeekFrom::Start(0)).unwrap(); file.write_all(&[expected[0] ^ 0xff]).unwrap(); drop(file);
        restored.action(&key, "resume").unwrap();
        let complete = rt.block_on(wait_job(&restored, &key, |job| job.state == State::Complete));
        assert_eq!(complete.done, expected.len() as u64);
        assert_eq!(complete.progress(), 1.0);
        assert_eq!(std::fs::read(&payload).unwrap(), expected, "resume repaired the damaged piece");
        rt.block_on(assert_unchanged(&restored, &key, &payload, State::Complete, 2));
        restored.shutdown();
        rt.block_on(seeder.stop());
        eprintln!("loopback: Ready(no payload) -> {}/{} verified -> Paused -> restored Paused -> Complete", partial.done, partial.total);
    }

    #[test]
    fn loopback_active_shutdown_cancel_and_remove_keep_partial_payload() {
        let temp = tempfile::tempdir().unwrap();
        let rt = runtime();
        let (seeder, torrent, _, _) = rt.block_on(seeder(temp.path()));
        let store = temp.path().join("state");
        let mut manager = Manager::new(store.clone(), local_policy(&seeder));
        let key = manager.prepare(2, "Generated bytes", &torrent.as_magnet().to_string(), temp.path().join("out")).unwrap();
        rt.block_on(wait_job(&manager, &key, |job| job.state == State::Ready));
        manager.action(&key, "start").unwrap();
        let partial = rt.block_on(wait_job(&manager, &key, |job| job.state == State::Downloading && job.done > 0 && job.done < job.total));
        let payload = partial.folder.join("public-domain-fixture.bin");
        manager.shutdown();
        let mut restored = Manager::new(store.clone(), local_policy(&seeder));
        let paused = restored.snapshot()[0].clone();
        assert_eq!(paused.state, State::Paused);
        assert!(paused.done >= partial.done && paused.done < paused.total);
        rt.block_on(assert_unchanged(&restored, &key, &payload, State::Paused, 2));
        restored.action(&key, "resume").unwrap();
        rt.block_on(wait_job(&restored, &key, |job| job.state == State::Downloading && job.done > paused.done && job.done < job.total));
        restored.action(&key, "cancel").unwrap();
        rt.block_on(wait_job(&restored, &key, |job| job.state == State::Cancelled));
        rt.block_on(assert_unchanged(&restored, &key, &payload, State::Cancelled, 2));
        let before = std::fs::read(&payload).unwrap();
        restored.action(&key, "remove").unwrap();
        rt.block_on(async {
            let mut interval = tokio::time::interval(Duration::from_millis(25));
            tokio::time::timeout(Duration::from_secs(15), async {
                while !restored.snapshot().is_empty() { interval.tick().await; }
            }).await.unwrap();
        });
        restored.shutdown();
        assert_eq!(std::fs::read(&payload).unwrap(), before);
        assert!(partial.folder.join(MARKER).is_file());
        assert!(!store.join(format!("{key}.torrent")).exists());
        assert!(load_manifest(&store).is_empty(), "Remove clears persisted history only");
        rt.block_on(seeder.stop());
    }

    #[test]
    fn owned_folder_rejects_unowned_and_linked_markers_and_resets_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let job = job(temp.path(), "0123456789abcdef0123456789abcdef01234567");
        std::fs::create_dir(&job.folder).unwrap();
        assert!(owned_folder(&job).is_err(), "preexisting unmarked directory is unowned");
        let original = temp.path().join("marker-target");
        std::fs::write(&original, &job.key).unwrap();
        let marker = job.folder.join(MARKER);
        std::os::unix::fs::symlink(&original, &marker).unwrap();
        assert!(owned_folder(&job).is_err());
        std::fs::remove_file(&marker).unwrap();
        std::fs::hard_link(&original, &marker).unwrap();
        assert!(owned_folder(&job).is_err());
        std::fs::remove_file(&marker).unwrap();
        std::fs::write(&marker, "wrong-owner").unwrap();
        assert!(owned_folder(&job).is_err());
        std::fs::write(&marker, &job.key).unwrap();
        std::fs::set_permissions(&job.folder, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(owned_folder(&job).unwrap(), job.folder);
        assert_eq!(std::fs::metadata(&job.folder).unwrap().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&job.folder).unwrap();
        std::os::unix::fs::symlink(temp.path(), &job.folder).unwrap();
        assert!(owned_folder(&job).is_err());
    }

    #[test]
    fn metadata_rejects_unsafe_names_links_conflicts_and_hash_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        for part in ["", ".", "..", "/absolute", "dir/file", "dir\\file", "nul\0name", "control\nname", MARKER] {
            let (key, bytes) = metadata(&[&[part]], None, false);
            assert!(inspect_metadata(&bytes, &key, temp.path()).is_err(), "accepted {part:?}");
        }
        for paths in [&[&["same"][..], &["same"][..]][..], &[&["a"][..], &["a", "b"][..]][..]] {
            let (key, bytes) = metadata(paths, None, false);
            assert!(inspect_metadata(&bytes, &key, temp.path()).is_err());
        }
        let (key, bytes) = metadata(&[&["safe"]], Some("l"), false);
        assert!(inspect_metadata(&bytes, &key, temp.path()).is_err());
        let (key, bytes) = metadata(&[&["safe", "nested.bin"]], None, true);
        assert!(librqbit::torrent_from_bytes(&bytes).unwrap().info.data.private);
        assert_eq!(inspect_metadata(&bytes, &key, temp.path()).unwrap().0, 65536);
        assert!(inspect_metadata(&bytes, &"0".repeat(40), temp.path()).is_err());
        assert!(inspect_metadata(b"bad", &key, temp.path()).is_err());
        assert!(inspect_metadata(&vec![0; MAX_METADATA as usize + 1], &key, temp.path()).is_err());
    }

    #[test]
    fn space_check_uses_missing_and_sparse_files_not_saved_verified_progress() {
        let temp = tempfile::tempdir().unwrap();
        let (key, bytes) = metadata(&[&["payload"]], None, false);
        inspect_metadata(&bytes, &key, temp.path()).unwrap();
        assert_eq!(required_space(&bytes, temp.path()).unwrap(), 65536);
        let path = temp.path().join("payload");
        std::fs::File::create(&path).unwrap().set_len(65536).unwrap();
        assert_eq!(required_space(&bytes, temp.path()).unwrap(), 65536, "sparse length is not allocation");
        std::fs::write(&path, vec![42; 65536]).unwrap();
        assert_eq!(required_space(&bytes, temp.path()).unwrap(), 0);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(required_space(&bytes, temp.path()).unwrap(), 65536, "deleted completed file needs space");
        assert!(available_space(temp.path()).unwrap() > 0);
    }

    #[test]
    fn stale_resolution_success_error_abort_and_panic_cannot_replace_retry() {
        runtime().block_on(async {
            for outcome in 0..4 {
                let temp = tempfile::tempdir().unwrap();
                let key = "0123456789abcdef0123456789abcdef01234567";
                let jobs = Arc::new(Mutex::new(vec![job(temp.path(), key)]));
                let mut worker = Worker::new(temp.path().join("store"), jobs, Policy { local_only: true, ..Default::default() });
                let old = worker.tasks.spawn(async move {
                    match outcome {
                        0 => Ok(Resolved { key: key.into(), bytes: vec![], peers: vec![] }),
                        1 => Err(anyhow::anyhow!("stale failure")),
                        2 => { std::future::pending::<()>().await; unreachable!() },
                        _ => panic!("synthetic metadata task panic"),
                    }
                });
                worker.task_keys.insert(old.id(), key.into());
                worker.resolving.insert(key.into(), old.clone());
                if outcome != 2 {
                    let mut interval = tokio::time::interval(Duration::from_millis(5));
                    tokio::time::timeout(Duration::from_secs(5), async {
                        while !old.is_finished() { interval.tick().await; }
                    }).await.unwrap();
                }
                worker.stop_job(key, State::Cancelled).await.unwrap();
                worker.update(key, |job| job.state = State::Resolving);
                let new = worker.tasks.spawn(std::future::pending::<Result<Resolved>>());
                worker.task_keys.insert(new.id(), key.into());
                worker.resolving.insert(key.into(), new.clone());
                let result = tokio::time::timeout(Duration::from_secs(5), worker.tasks.join_next_with_id()).await.unwrap().unwrap();
                worker.resolution_finished(result);
                assert_eq!(worker.job(key).unwrap().state, State::Resolving);
                assert_eq!(worker.resolving.get(key).unwrap().id(), new.id());
                assert!(!worker.task_keys.contains_key(&old.id()));
                assert!(worker.session.is_none());
                worker.tasks.abort_all();
            }
        });
    }

    #[test]
    fn current_resolution_panic_fails_job_and_releases_active_handle() {
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let key = "0123456789abcdef0123456789abcdef01234567";
            let jobs = Arc::new(Mutex::new(vec![job(temp.path(), key)]));
            let mut worker = Worker::new(temp.path().join("store"), jobs, Policy::default());
            let handle = worker.tasks.spawn(async { panic!("synthetic current task panic"); #[allow(unreachable_code)] Ok::<_, anyhow::Error>(Resolved { key: String::new(), bytes: vec![], peers: vec![] }) });
            worker.task_keys.insert(handle.id(), key.into());
            worker.resolving.insert(key.into(), handle);
            let result = worker.tasks.join_next_with_id().await.unwrap();
            worker.resolution_finished(result);
            assert_eq!(worker.job(key).unwrap().state, State::Failed);
            assert!(worker.resolving.is_empty());
            assert!(worker.task_keys.is_empty());
        });
    }

    #[test]
    fn restoring_active_manifest_is_paused_and_constructs_no_engine() {
        let temp = tempfile::tempdir().unwrap();
        let key = "0123456789abcdef0123456789abcdef01234567";
        let mut job = job(temp.path(), key);
        job.state = State::Downloading; job.done = 100; job.total = 200;
        secure_write(&temp.path().join("jobs.json"), &serde_json::to_vec(&Manifest { schema: 1, jobs: vec![job] }).unwrap()).unwrap();
        let jobs = load_manifest(temp.path());
        assert_eq!(jobs[0].state, State::Paused);
        let worker = Worker::new(temp.path().to_path_buf(), Arc::new(Mutex::new(jobs)), Policy::default());
        assert!(worker.session.is_none());
        assert!(worker.handles.is_empty());
        assert!(worker.resolving.is_empty());
        assert!(!temp.path().join(format!("torrent-{key}")).exists());
    }

    #[test]
    fn immediate_checking_cancel_and_already_paused_handle_are_removed() {
        runtime().block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let (key, bytes) = metadata(&[&["payload"]], None, false);
            let mut job = job(temp.path(), &key); job.state = State::Ready;
            let jobs = Arc::new(Mutex::new(vec![job]));
            let store = temp.path().join("store");
            secure_write(&store.join(format!("{key}.torrent")), &bytes).unwrap();
            let mut worker = Worker::new(store, jobs, Policy { local_only: true, ..Default::default() });
            worker.start(&key).await.unwrap();
            assert_eq!(worker.job(&key).unwrap().state, State::Checking);
            worker.stop_job(&key, State::Cancelled).await.unwrap();
            assert!(worker.handles.is_empty());
            assert_eq!(worker.job(&key).unwrap().state, State::Cancelled);
            // A paused engine handle can occur after checking or an engine transition.
            let session = worker.session().await.unwrap();
            let response = session.add_torrent(AddTorrent::from_bytes(bytes), Some(AddTorrentOptions {
                paused: true, overwrite: true,
                output_folder: Some(worker.job(&key).unwrap().folder.to_string_lossy().into_owned()),
                ..Default::default()
            })).await.unwrap();
            let handle = response.into_handle().unwrap();
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            tokio::time::timeout(Duration::from_secs(15), async {
                while !matches!(handle.stats().state, TorrentStatsState::Paused) { interval.tick().await; }
            }).await.unwrap();
            worker.handles.insert(key.clone(), handle);
            worker.stop_job(&key, State::Cancelled).await.unwrap();
            assert!(worker.handles.is_empty());
            worker.tick().await;
            assert!(worker.session.is_none(), "no engine remains when every job is inactive");
        });
    }

    #[test]
    fn rejects_unsafe_magnets_and_normalizes_hashes() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let base32 = data_encoding::BASE32.encode(&data_encoding::HEXLOWER.decode(hex.as_bytes()).unwrap());
        assert_eq!(normalize_magnet(&format!("magnet:?xt=urn:btih:{base32}")).unwrap().0, hex);
        assert!(normalize_magnet("https://example.com/file.torrent").is_err());
        assert!(normalize_magnet("magnet:?xt=urn:btih:bad").is_err());
        assert!(normalize_magnet(&format!("magnet:?xt=urn:btih:{hex}&so=0-999999999")).is_err());
        assert!(normalize_magnet(&format!("magnet:?xt=urn:btih:{hex}&xt=urn:btih:{hex}")).is_err());
        assert!(normalize_magnet(&format!("magnet:?xt=urn:btih:{hex}&tr=file:///etc/passwd")).is_err());
    }

    #[test]
    fn rejects_unowned_symlink_hardlink_and_traversal_destinations() {
        let temp = tempfile::tempdir().unwrap();
        assert!(check_path(temp.path(), Path::new("../escape")).is_err());
        assert!(check_path(temp.path(), Path::new("/etc/passwd")).is_err());
        assert!(check_path(temp.path(), Path::new("folder\\escape")).is_err());
        std::os::unix::fs::symlink("/tmp", temp.path().join("link")).unwrap();
        assert!(check_path(temp.path(), Path::new("link/file")).is_err());
        std::fs::write(temp.path().join("original"), "fixture").unwrap();
        std::fs::hard_link(temp.path().join("original"), temp.path().join("alias")).unwrap();
        assert!(check_path(temp.path(), Path::new("alias")).is_err());
        assert!(check_path(temp.path(), Path::new("safe/nested/file")).is_ok());
    }
}