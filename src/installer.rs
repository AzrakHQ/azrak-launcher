//! Explicit, offline Linux installation. No payload execution, networking, or UI calls.
//! Output paths are chosen here, never by libarchive. All filesystem traversal uses
//! pinned directory descriptors and O_NOFOLLOW, including source reads and cleanup.

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::{CStr, CString},
    fs::{self, File, Metadata},
    io::{Read, Write},
    os::{fd::{AsRawFd, FromRawFd}, unix::{ffi::OsStrExt, fs::{MetadataExt, PermissionsExt}}},
    path::{Component, Path, PathBuf},
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, mpsc::{self, SyncSender}, Arc, Mutex},
    thread::{self, JoinHandle},
};

const OWNED: &str = ".ps5-launcher-owned";
const STAGE_MARKER: &str = ".ps5-launcher-install-stage";
const MAX_ENTRIES: usize = 100_000;
const MAX_PACKAGE_ENTRIES: usize = 1_000_000;
const MAX_PATH: usize = 4096;
const MAX_DEPTH: usize = 128;
pub(crate) const MAX_PARAM: u64 = 1024 * 1024;
const MAX_TOTAL: u64 = 2 * 1024 * 1024 * 1024 * 1024;
const MAX_FILE: u64 = 1024 * 1024 * 1024 * 1024;
static UNIQUE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum State { Inspecting, Extracting, Validating, Installed, Failed, Cancelled }

impl State {
    pub fn active(self) -> bool { matches!(self, Self::Inspecting | Self::Extracting | Self::Validating) }
    pub fn label(self) -> &'static str {
        match self {
            Self::Inspecting => "Inspecting", Self::Extracting => "Extracting",
            Self::Validating => "Validating", Self::Installed => "Installed",
            Self::Failed => "Failed", Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub key: String, pub topic: i64, pub name: String, pub state: State,
    pub done: u64, pub total: u64, pub error: String, pub path: Option<PathBuf>,
    #[serde(default)] pub title_id: String,
}

impl Record {
    pub fn progress(&self) -> f32 {
        if self.state == State::Installed { 1.0 }
        else if self.total == 0 { 0.0 }
        else { (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32 }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub key: String, pub topic: i64, pub name: String, pub source: PathBuf,
    pub destination: PathBuf, pub expected_ids: Vec<String>,
    /// PC games install differently: everything is unpacked and the way to start it is found.
    pub platform: crate::platform::Platform,
}

struct Core { jobs: Mutex<Vec<Record>>, store: PathBuf }
struct Worker { tx: SyncSender<Request>, join: JoinHandle<()>, cancel: Arc<AtomicBool> }
pub struct Manager { core: Arc<Core>, worker: Mutex<Option<Worker>>, stopped: AtomicBool }

impl Manager {
    /// Restore records only. Even the worker is created lazily, on explicit start.
    pub fn load() -> Self { Self::at(crate::util::config_dir().join("installs")) }

    fn at(store: PathBuf) -> Self {
        let mut jobs = load_records(&store).unwrap_or_default();
        let mut changed = false;
        for job in &mut jobs {
            if job.state.active() {
                job.state = State::Failed;
                job.error = "Interrupted; retry".into();
                changed = true;
            }
        }
        let core = Arc::new(Core { jobs: Mutex::new(jobs), store });
        if changed { let _ = core.save(&core.jobs.lock().unwrap()); }
        Self { core, worker: Mutex::new(None), stopped: AtomicBool::new(false) }
    }

    pub fn snapshot(&self) -> Vec<Record> { self.core.jobs.lock().unwrap().clone() }

    pub fn start(&self, request: Request) -> Result<()> {
        validate_request(&request)?;
        let mut worker = self.worker.lock().unwrap();
        ensure!(!self.stopped.load(Ordering::Acquire), "Installer has shut down");
        let mut jobs = self.core.jobs.lock().unwrap();
        ensure!(!jobs.iter().any(|j| j.state.active()), "Another installation is active");
        ensure!(!jobs.iter().any(|j| j.key == request.key && j.state == State::Installed),
            "This download is already installed; existing games are never overwritten");
        if worker.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Request>(1);
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = cancel.clone();
            let core = self.core.clone();
            let join = thread::Builder::new().name("ps5-installer".into()).spawn(move || {
                while let Ok(req) = rx.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        install(&req, &flag, &|state, done, total| {
                            core.update(&req.key, state, done, total);
                        }, &|stage, root, leaf, title| {
                            // Linearize cancellation and publication with cancel()'s job lock.
                            let mut jobs = core.jobs.lock().unwrap();
                            check_cancel(&flag)?;
                            stage.publish(root, leaf)?;
                            let job = jobs.iter_mut().find(|j| j.key == req.key).unwrap();
                            job.state = State::Installed;
                            job.done = job.total;
                            job.path = Some(req.destination.join(leaf));
                            job.title_id = title.into();
                            if let Err(e) = core.save(&jobs) {
                                jobs.iter_mut().find(|j| j.key == req.key).unwrap().error =
                                    format!("Installed, but could not persist record: {e:#}");
                            }
                            Ok(())
                        })
                    }));
                    let error = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(e)) => Some(format!("{e:#}")),
                        Err(_) => Some("Installer worker panicked; retry".into()),
                    };
                    if let Some(error) = error {
                        let mut jobs = core.jobs.lock().unwrap();
                        if let Some(job) = jobs.iter_mut().find(|j| j.key == req.key) {
                            // Publication is irreversible; never claim a published game failed.
                            if job.state != State::Installed {
                                job.state = if flag.load(Ordering::Acquire) { State::Cancelled } else { State::Failed };
                                job.error = error;
                                job.path = None;
                            }
                        }
                        let _ = core.save(&jobs);
                    }
                }
            }).context("Create installer worker")?;
            *worker = Some(Worker { tx, join, cancel });
        }
        let old = jobs.clone();
        jobs.retain(|j| j.key != request.key);
        jobs.push(Record { key: request.key.clone(), topic: request.topic, name: request.name.clone(),
            state: State::Inspecting, done: 0, total: 0, error: String::new(), path: None, title_id: String::new() });
        if let Err(e) = self.core.save(&jobs) { *jobs = old; return Err(e); }
        let w = worker.as_ref().unwrap();
        w.cancel.store(false, Ordering::Release);
        if let Err(e) = w.tx.try_send(request) {
            *jobs = old;
            let _ = self.core.save(&jobs);
            bail!("Installer queue unavailable: {e}");
        }
        Ok(())
    }

    pub fn cancel(&self, key: &str) -> Result<()> {
        let worker = self.worker.lock().unwrap();
        let jobs = self.core.jobs.lock().unwrap();
        let job = jobs.iter().find(|j| j.key == key).context("Unknown installation")?;
        ensure!(job.state.active(), "Installation is not active (publication may have completed)");
        worker.as_ref().context("Installer is not running")?.cancel.store(true, Ordering::Release);
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let worker = self.worker.lock().unwrap().take();
        if let Some(Worker { tx, join, cancel }) = worker {
            cancel.store(true, Ordering::Release);
            drop(tx);
            let _ = join.join();
        }
    }
}

impl Drop for Manager { fn drop(&mut self) { self.shutdown(); } }

impl Core {
    fn save(&self, jobs: &[Record]) -> Result<()> { save_records(&self.store, jobs) }
    fn update(&self, key: &str, state: State, done: u64, total: u64) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.iter_mut().find(|j| j.key == key) {
            let changed = job.state != state;
            job.state = state; job.done = done; job.total = total;
            if changed { let _ = self.save(&jobs); }
        }
    }
}

fn validate_request(req: &Request) -> Result<()> {
    ensure!((12..=128).contains(&req.key.len()) && req.key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "Invalid installation key");
    ensure!(req.source.is_absolute() && req.destination.is_absolute(), "Source and explicit destination must be absolute paths");
    ensure!(req.expected_ids.len() <= 100 && req.expected_ids.iter().all(|id| crate::psn::valid_title_id(id)), "Invalid expected title ID");
    ensure!(req.name.len() <= 4096, "Installation name is too long");
    Ok(())
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "Cancelled; source files retained");
    Ok(())
}

// Descriptor-relative filesystem primitives; no symlink traversal, even in ancestors.
pub(crate) struct Dir(pub(crate) File);
impl Dir {
    pub(crate) fn absolute(path: &Path, create: bool) -> Result<Self> {
        ensure!(path.is_absolute(), "Absolute path required");
        let root = CString::new("/").unwrap();
        let fd = unsafe { libc::open(root.as_ptr(), libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_RDONLY) };
        ensure!(fd >= 0, "Open root: {}", std::io::Error::last_os_error());
        let mut dir = Self(unsafe { File::from_raw_fd(fd) });
        for part in path.components() {
            match part {
                Component::RootDir => (),
                Component::Normal(name) => { dir = dir.child(name.as_bytes(), create)?; },
                _ => bail!("Unsafe filesystem path: {}", path.display()),
            }
        }
        Ok(dir)
    }
    pub(crate) fn child(&self, name: &[u8], create: bool) -> Result<Self> {
        let name = component(name)?;
        if create {
            let r = unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), 0o700) };
            if r != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let fd = unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        ensure!(fd >= 0, "Open directory without following links: {}", std::io::Error::last_os_error());
        Ok(Self(unsafe { File::from_raw_fd(fd) }))
    }
    pub(crate) fn parent(&self, path: &str, create: bool) -> Result<(Self, CString)> {
        let mut dir = Self(self.0.try_clone()?);
        let parts: Vec<_> = path.split('/').collect();
        for name in &parts[..parts.len() - 1] { dir = dir.child(name.as_bytes(), create)?; }
        Ok((dir, component(parts.last().unwrap().as_bytes())?))
    }
    pub(crate) fn read(&self, path: &str) -> Result<File> {
        let (parent, name) = self.parent(path, false)?;
        // O_NONBLOCK prevents an attacker-swapped FIFO from blocking before fstat.
        let fd = unsafe { libc::openat(parent.0.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK) };
        ensure!(fd >= 0, "Open source {path}: {}", std::io::Error::last_os_error());
        let file = unsafe { File::from_raw_fd(fd) };
        regular(&file.metadata()?)?;
        Ok(file)
    }
    pub(crate) fn new_file(&self, path: &str) -> Result<File> {
        let (parent, name) = self.parent(path, true)?;
        let fd = unsafe { libc::openat(parent.0.as_raw_fd(), name.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
        ensure!(fd >= 0, "Create private output {path}: {}", std::io::Error::last_os_error());
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn symlink(&self, path: &str, target: &str) -> Result<()> {
        let (parent, name) = self.parent(path, true)?;
        let target = CString::new(target)?;
        let r = unsafe { libc::symlinkat(target.as_ptr(), parent.0.as_raw_fd(), name.as_ptr()) };
        ensure!(r == 0, "Create symlink {path}: {}", std::io::Error::last_os_error());
        Ok(())
    }
    pub(crate) fn names(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in crate::hostos::dir_names(self.0.as_raw_fd())? {
            ensure!(names.len() < MAX_ENTRIES, "Too many source entries");
            let name = entry.into_string().map_err(|_| anyhow::anyhow!("Non-UTF-8 filenames are unsupported"))?;
            component(name.as_bytes())?;
            names.push(name);
        }
        names.sort();
        Ok(names)
    }
    fn metadata(&self, name: &str) -> Result<libc::stat> {
        let name = component(name.as_bytes())?;
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        let r = unsafe { libc::fstatat(self.0.as_raw_fd(), name.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
        ensure!(r == 0, "Inspect source: {}", std::io::Error::last_os_error());
        Ok(unsafe { st.assume_init() })
    }
}

pub(crate) fn component(name: &[u8]) -> Result<CString> {
    ensure!(!name.is_empty() && name.len() <= 255 && name != b"." && name != b".." && !name.contains(&b'/'), "Unsafe path component");
    Ok(CString::new(name)?)
}
fn regular(meta: &Metadata) -> Result<()> {
    ensure!(meta.is_file() && meta.nlink() == 1, "Symlinks, hardlinks and special source files are forbidden");
    Ok(())
}
pub(crate) fn bounded_read(mut file: File, max: u64) -> Result<Vec<u8>> {
    ensure!(file.metadata()?.len() <= max, "File exceeds size limit ({max} bytes)");
    let mut bytes = Vec::new();
    (&mut file).take(max + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= max, "File grew beyond size limit");
    Ok(bytes)
}
/// A symlink at `path` pointing to `target` must stay inside the game: relative, and not climbing
/// above the folder it's installed in.
fn inside_link(path: &str, target: &str) -> Result<()> {
    ensure!(!target.is_empty() && target.len() < MAX_PATH && !target.starts_with('/') && !target.contains('\\'),
        "Symlink {path} points outside the game");
    let mut depth = path.matches('/').count() as i64;
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => { depth -= 1; ensure!(depth >= 0, "Symlink {path} points outside the game"); }
            _ => depth += 1,
        }
    }
    Ok(())
}

pub(crate) fn normal_path(raw: &str) -> Result<String> {
    ensure!(!raw.is_empty() && raw.len() < MAX_PATH && !raw.chars().any(|c| c.is_control()), "Invalid or overlong archive path");
    let raw = raw.replace('\\', "/");
    ensure!(!raw.starts_with('/') && !raw.contains(':'), "Absolute or drive-qualified archive path is forbidden");
    let mut parts = Vec::new();
    for p in raw.split('/') {
        ensure!(p != "..", "Archive path traversal is forbidden");
        if p.is_empty() || p == "." { continue; }
        component(p.as_bytes())?;
        ensure!(p != OWNED && p != STAGE_MARKER, "Archive uses a reserved ownership marker");
        parts.push(p);
    }
    ensure!(!parts.is_empty() && parts.len() <= MAX_DEPTH, "Empty or excessively nested archive path");
    Ok(parts.join("/"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) path: String, pub(crate) directory: bool, pub(crate) size: u64,
    /// A symlink's target (PC games only; always relative and inside the game).
    link: Option<String>,
    /// The archive marks it executable.
    exec: bool,
}
impl Entry {
    pub(crate) fn file(path: String, directory: bool, size: u64) -> Self { Self { path, directory, size, link: None, exec: false } }
}
pub(crate) struct Plan { pub(crate) entries: Vec<Entry>, paths: BTreeMap<String, bool>, pub(crate) explicit: HashSet<String>, pub(crate) total: u64, limit: usize,
    /// A PC archive holds a program (native, Windows or AppImage).
    starts: bool }
impl Default for Plan {
    fn default() -> Self { Self::limited(MAX_ENTRIES) }
}
impl Plan {
    /// An empty plan that only says how many bytes will be written (for the free-space check).
    pub(crate) fn of_total(total: u64) -> Self {
        Self { total, ..Self::default() }
    }
    /// A plan that accepts up to `limit` entries. A game package lists more files than an archive.
    pub(crate) fn limited(limit: usize) -> Self {
        Self { entries: Vec::new(), paths: BTreeMap::new(), explicit: HashSet::new(), total: 0, limit, starts: false }
    }
    pub(crate) fn add(&mut self, entry: Entry) -> Result<()> {
        let limit = self.limit;
        ensure!(self.entries.len() < limit, "Archive/source exceeds {limit} entries");
        ensure!(entry.size <= MAX_FILE, "File exceeds 1 TiB safety limit");
        if entry.path.ends_with("sce_sys/param.json") || entry.path.ends_with("sce_sys/param.sfo") { ensure!(entry.size <= MAX_PARAM, "Game metadata exceeds 1 MiB limit"); }
        ensure!(self.explicit.insert(entry.path.clone()), "Duplicate archive entry: {}", entry.path);
        if let Some(kind) = self.paths.get(&entry.path) {
            ensure!(*kind && entry.directory, "File/directory conflict: {}", entry.path);
        }
        let mut prefix = String::new();
        let parts: Vec<_> = entry.path.split('/').collect();
        for p in &parts[..parts.len() - 1] {
            if !prefix.is_empty() { prefix.push('/'); }
            prefix.push_str(p);
            ensure!(self.paths.get(&prefix) != Some(&false), "File/directory conflict: {prefix}");
            self.paths.insert(prefix.clone(), true);
        }
        self.paths.insert(entry.path.clone(), entry.directory);
        ensure!(self.paths.len() <= limit, "Too many implicit directories");
        self.total = self.total.checked_add(entry.size).context("Output size overflow")?;
        ensure!(self.total <= MAX_TOTAL, "Output exceeds 2 TiB safety limit");
        self.entries.push(entry);
        Ok(())
    }
}

pub(crate) struct Source { pub(crate) dir: Dir, pub(crate) plan: Plan, pub(crate) stamps: BTreeMap<String, Stamp> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stamp { dev: u64, ino: u64, len: u64, mtime: i64, ns: i64, ctime: i64, cns: i64 }
impl Stamp {
    fn of(m: &Metadata) -> Self {
        Self { dev: m.dev(), ino: m.ino(), len: m.len(), mtime: m.mtime(), ns: m.mtime_nsec(), ctime: m.ctime(), cns: m.ctime_nsec() }
    }
}
impl Source {
    fn inspect(req: &Request, cancel: &AtomicBool) -> Result<Self> {
        let dir = Dir::absolute(&req.source, false).context("Open owned torrent folder")?;
        ensure!(dir.0.metadata()?.uid() == unsafe { libc::geteuid() }, "Torrent folder belongs to another user");
        let marker = bounded_read(dir.read(OWNED).context("Source must be a launcher-owned torrent folder")?, 128)?;
        ensure!(marker == req.key.as_bytes(), "Torrent ownership marker does not match installation key");
        let mut source = Self { dir, plan: Plan::default(), stamps: BTreeMap::new() };
        let root = Dir(source.dir.0.try_clone()?);
        source.walk(&root, "", cancel)?;
        Ok(source)
    }
    pub(crate) fn walk(&mut self, dir: &Dir, prefix: &str, cancel: &AtomicBool) -> Result<()> {
        for name in dir.names()? {
            check_cancel(cancel)?;
            if prefix.is_empty() && name == OWNED { continue; }
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let normalized = normal_path(&path)?;
            ensure!(normalized == path, "Ambiguous source filename (backslash separators are archive-only)");
            let st = dir.metadata(&name)?;
            let kind = st.st_mode & libc::S_IFMT;
            if kind == libc::S_IFDIR {
                self.plan.add(Entry::file(path.clone(), true, 0))?;
                let child = dir.child(name.as_bytes(), false)?;
                ensure!(child.0.metadata()?.ino() == st.st_ino && child.0.metadata()?.dev() == st.st_dev as u64, "Source directory changed during inspection");
                self.walk(&child, &path, cancel)?;
            } else {
                ensure!(kind == libc::S_IFREG && st.st_nlink == 1, "Symlinks, hardlinks and special source entries are forbidden: {path}");
                let file = dir.read(&name)?;
                let meta = file.metadata()?;
                ensure!(meta.ino() == st.st_ino && meta.dev() == st.st_dev as u64, "Source changed during inspection");
                self.plan.add(Entry::file(path.clone(), false, meta.len()))?;
                self.stamps.insert(path, Stamp::of(&meta));
            }
        }
        Ok(())
    }
    pub(crate) fn file(&self, path: &str) -> Result<File> {
        let file = self.dir.read(path)?;
        ensure!(self.stamps.get(path) == Some(&Stamp::of(&file.metadata()?)), "Source changed after inspection: {path}; wait until download is complete");
        Ok(file)
    }
}

// Identify exactly one archive family. The ordered paths are passed as pinned fds
// to libarchive's multi-volume reader (also concatenates split .7z.001 streams).
/// Which archive `path` is a volume of, and its volume number: `game.part2.rar` and `game.r00`
/// are volumes 2 of "game"'s RAR set, `game.7z.001` volume 1 of a split 7z. None if not an archive.
fn volume_of(path: &str) -> Result<Option<(String, u32)>> {
    static PART: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)^(.*)\.part(\d+)\.rar$").unwrap());
    static SPLIT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)^(.*\.7z)\.(\d{3})$").unwrap());
    static OLD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)^(.*)\.r(\d{2})$").unwrap());
    let lower = path.to_ascii_lowercase();
    Ok(Some(if let Some(c) = PART.captures(path) {
        (format!("part:{}", c[1].to_ascii_lowercase()), c[2].parse::<u32>().context("Invalid RAR volume number")?)
    } else if let Some(c) = SPLIT.captures(path) {
        (format!("split:{}", c[1].to_ascii_lowercase()), c[2].parse::<u32>()?)
    } else if let Some(c) = OLD.captures(path) {
        (format!("rar:{}", c[1].to_ascii_lowercase()), c[2].parse::<u32>()? + 2)
    } else if lower.ends_with(".rar") {
        (format!("rar:{}", lower.trim_end_matches(".rar")), 1)
    } else if [".zip", ".7z", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar.zst"].iter().any(|ext| lower.ends_with(ext)) {
        (format!("single:{lower}"), 1)
    } else { return Ok(None); }))
}

fn volumes(plan: &Plan) -> Result<Option<Vec<String>>> {
    let mut groups: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();
    for entry in plan.entries.iter().filter(|e| !e.directory) {
        let Some((family, n)) = volume_of(&entry.path)? else { continue };
        ensure!(n > 0 && n <= MAX_ENTRIES as u32, "Invalid multipart volume number");
        ensure!(groups.entry(family).or_default().insert(n, entry.path.clone()).is_none(), "Duplicate multipart volume number");
    }
    ensure!(groups.len() <= 1, "Multiple independent archives found; choose a single release folder");
    let Some((_, group)) = groups.into_iter().next() else { return Ok(None); };
    let mut out = Vec::new();
    for (i, (n, path)) in group.into_iter().enumerate() {
        ensure!(n == i as u32 + 1, "Missing multipart volume {}; all volumes must be complete", i + 1);
        out.push(path);
    }
    Ok(Some(out))
}

type Handle = *mut libc::c_void;
type Unary = unsafe extern "C" fn(Handle) -> libc::c_int;
type Text = unsafe extern "C" fn(Handle) -> *const libc::c_char;
struct Api {
    _lib: libloading::Library,
    new: unsafe extern "C" fn() -> Handle, formats: Unary, filters: Unary,
    open: unsafe extern "C" fn(Handle, *const *const libc::c_char, usize) -> libc::c_int,
    next: unsafe extern "C" fn(Handle, *mut Handle) -> libc::c_int,
    data: unsafe extern "C" fn(Handle, *mut libc::c_void, usize) -> isize,
    skip: Unary, free: Unary, error: Text, pathname: Text, hardlink: Text, symlink: Text,
    size: unsafe extern "C" fn(Handle) -> i64, size_set: Unary,
    kind: unsafe extern "C" fn(Handle) -> libc::mode_t, encrypted: Unary,
    perm: unsafe extern "C" fn(Handle) -> libc::mode_t,
    version: unsafe extern "C" fn() -> *const libc::c_char,
}
impl Api {
    fn load() -> Result<Self> {
        let lib = crate::hostos::libarchive_candidates()
            .iter()
            .find_map(|name| unsafe { libloading::Library::new(name) }.ok())
            .with_context(|| format!("System libarchive missing; install libarchive ({}). No external extraction fallback is used", crate::hostos::libarchive_install_hint()))?;
        // Each signature follows libarchive's public C API. Library stays alive
        // longer than every function pointer and archive handle using it.
        unsafe {
            macro_rules! sym { ($name:literal) => { *lib.get(concat!($name, "\0").as_bytes()).context(concat!("Missing libarchive symbol ", $name))? }; }
            Ok(Self { new: sym!("archive_read_new"), formats: sym!("archive_read_support_format_all"),
                filters: sym!("archive_read_support_filter_all"), open: sym!("archive_read_open_filenames"),
                next: sym!("archive_read_next_header"), data: sym!("archive_read_data"),
                skip: sym!("archive_read_data_skip"), free: sym!("archive_read_free"),
                error: sym!("archive_error_string"), pathname: sym!("archive_entry_pathname"),
                hardlink: sym!("archive_entry_hardlink"), symlink: sym!("archive_entry_symlink"),
                size: sym!("archive_entry_size"), size_set: sym!("archive_entry_size_is_set"),
                kind: sym!("archive_entry_filetype"), encrypted: sym!("archive_entry_is_encrypted"),
                perm: sym!("archive_entry_perm"),
                version: sym!("archive_version_string"), _lib: lib })
        }
    }
    fn version(&self) -> String { unsafe { CStr::from_ptr((self.version)()).to_string_lossy().into_owned() } }
}

struct Archive<'a> { api: &'a Api, handle: Handle, files: Vec<File>, _names: Vec<CString>, links: bool }
impl<'a> Archive<'a> {
    fn open(api: &'a Api, source: &Source, volumes: &[String]) -> Result<Self> {
        let files: Vec<File> = volumes.iter().map(|p| source.file(p)).collect::<Result<_>>()?;
        let names: Vec<CString> = files.iter().map(|f| CString::new(crate::hostos::fd_open_path(f.as_raw_fd())).unwrap()).collect();
        let mut pointers: Vec<_> = names.iter().map(|n| n.as_ptr()).collect();
        pointers.push(std::ptr::null());
        let handle = unsafe { (api.new)() };
        ensure!(!handle.is_null(), "libarchive allocation failed");
        let archive = Self { api, handle, files, _names: names, links: false };
        archive.status(unsafe { (api.formats)(handle) }, "Enable archive formats")?;
        archive.status(unsafe { (api.filters)(handle) }, "Enable archive filters")?;
        archive.status(unsafe { (api.open)(handle, pointers.as_ptr(), 64 * 1024) }, "Open archive (unsupported format, missing volume, or password protection)")?;
        Ok(archive)
    }
    fn status(&self, code: i32, action: &str) -> Result<()> {
        if code != 0 {
            let p = unsafe { (self.api.error)(self.handle) };
            let error = if p.is_null() { "unsupported format or missing volume".into() } else { unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() } };
            bail!("{action}: {error} [{}]; verify all volumes, format support and that the archive is not password-protected", self.api.version());
        }
        Ok(())
    }
    fn header(&self) -> Result<Option<Entry>> {
        let mut entry = std::ptr::null_mut();
        let code = unsafe { (self.api.next)(self.handle, &mut entry) };
        if code == 1 { return Ok(None); } // ARCHIVE_EOF
        self.status(code, "Read archive header")?;
        ensure!(!entry.is_null(), "Missing archive header");
        unsafe {
            ensure!((self.api.encrypted)(entry) <= 0, "Password-protected archives are unsupported");
            ensure!((self.api.hardlink)(entry).is_null(), "Archive hardlinks are unsupported");
            let kind = (self.api.kind)(entry);
            let p = (self.api.pathname)(entry);
            ensure!(!p.is_null(), "Missing archive pathname");
            let path = normal_path(CStr::from_ptr(p).to_str().context("Non-UTF-8 archive path is unsupported")?)?;
            // PC games ship library symlinks (libfoo.so -> libfoo.so.1); only ones that stay
            // inside the game are kept. Console games have none.
            let target = (self.api.symlink)(entry);
            if !target.is_null() || kind == libc::S_IFLNK {
                ensure!(self.links, "Archive symlinks are forbidden");
                ensure!(!target.is_null(), "Symlink without a target: {path}");
                let target = CStr::from_ptr(target).to_str().context("Non-UTF-8 symlink target")?;
                inside_link(&path, target)?;
                return Ok(Some(Entry { path, directory: false, size: 0, link: Some(target.into()), exec: false }));
            }
            ensure!(kind == libc::S_IFREG || kind == libc::S_IFDIR, "Archive special files are forbidden");
            let directory = kind == libc::S_IFDIR;
            ensure!(directory || (self.api.size_set)(entry) != 0, "Archive has unknown uncompressed size; cannot safely install");
            let size = (self.api.size)(entry);
            ensure!(size >= 0 && (!directory || size == 0), "Invalid declared archive size");
            let exec = !directory && (self.api.perm)(entry) & 0o111 != 0;
            Ok(Some(Entry { path, directory, size: size as u64, link: None, exec }))
        }
    }
    fn verify_sources(&self, source: &Source, volumes: &[String]) -> Result<()> {
        for (file, path) in self.files.iter().zip(volumes) {
            ensure!(source.stamps.get(path) == Some(&Stamp::of(&file.metadata()?)), "Archive source changed while reading: {path}");
            let _ = source.file(path)?;
        }
        Ok(())
    }
}
impl Drop for Archive<'_> { fn drop(&mut self) { unsafe { (self.api.free)(self.handle); } } }

fn inspect_archive(api: &Api, source: &Source, paths: &[String], cancel: &AtomicBool) -> Result<Plan> {
    inspect_archive_with(api, source, paths, false, cancel)
}

fn inspect_archive_with(api: &Api, source: &Source, paths: &[String], links: bool, cancel: &AtomicBool) -> Result<Plan> {
    let mut archive = Archive::open(api, source, paths)?;
    archive.links = links;
    let mut plan = Plan::default();
    loop {
        check_cancel(cancel)?;
        let Some(entry) = archive.header()? else { break; };
        // PC archives: is there a program in it? Read by its first bytes, as pcgame does once
        // it's installed (torrents and tar files often drop the executable flag).
        if links && !plan.starts && !entry.directory && entry.link.is_none() && entry.size > 0 {
            let mut head = vec![0u8; entry.size.min(4096) as usize];
            let mut got = 0;
            while got < head.len() {
                let n = unsafe { (api.data)(archive.handle, head[got..].as_mut_ptr().cast(), head.len() - got) };
                if n < 0 { archive.status(n as i32, "Inspect archive data (corruption, password, or missing volume)")?; }
                if n <= 0 { break; }
                got += (n as usize).min(head.len() - got);
            }
            use crate::pcgame::Probe;
            plan.starts = matches!(crate::pcgame::probe(&head[..got]), Probe::Elf { .. } | Probe::Exe | Probe::AppImage);
        }
        plan.add(entry)?;
        archive.status(unsafe { (api.skip)(archive.handle) }, "Inspect archive data (missing volume or corrupt archive)")?;
    }
    archive.verify_sources(source, paths)?;
    ensure!(!plan.entries.is_empty(), "Archive is empty");
    Ok(plan)
}

pub(crate) struct Stage { pub(crate) destination: Dir, dir: Dir, name: String, token: String }
impl Stage {
    pub(crate) fn new(destination: Dir, key: &str) -> Result<Self> {
        let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let name = format!(".ps5-install-{}-{}-{n}", &key[..12], std::process::id());
        let cname = component(name.as_bytes())?;
        let r = unsafe { libc::mkdirat(destination.0.as_raw_fd(), cname.as_ptr(), 0o700) };
        ensure!(r == 0, "Create exclusive staging directory: {}", std::io::Error::last_os_error());
        let dir = destination.child(name.as_bytes(), false)?;
        let token = format!("{key}:{}:{n}", std::process::id());
        let stage = Self { destination, dir, name, token };
        stage.dir.new_file(STAGE_MARKER)?.write_all(stage.token.as_bytes())?;
        stage.dir.child(b"payload", true)?;
        Ok(stage)
    }
    pub(crate) fn payload(&self) -> Result<Dir> { self.dir.child(b"payload", false) }
    pub(crate) fn publish(&self, root: &str, leaf: &str) -> Result<()> {
        let payload = self.payload()?;
        let (parent, name) = if root.is_empty() {
            (Dir(self.dir.0.try_clone()?), component(b"payload")?)
        } else { payload.parent(root, false)? };
        let leaf = component(leaf.as_bytes())?;
        crate::hostos::rename_noreplace(parent.0.as_raw_fd(), &name, self.destination.0.as_raw_fd(), &leaf)
            .map_err(|e| anyhow::anyhow!("Atomic publication refused (existing destination is never overwritten): {e}"))?;
        // rename already committed; failure to sync must not mark it unpublished.
        let _ = self.destination.0.sync_all();
        Ok(())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        // Never delete an arbitrary path: verify descriptor identity and our marker.
        let cleanup = || -> Result<()> {
            let st = self.destination.metadata(&self.name)?;
            let meta = self.dir.0.metadata()?;
            ensure!(st.st_ino == meta.ino() && st.st_dev as u64 == meta.dev() && st.st_mode & libc::S_IFMT == libc::S_IFDIR, "Stage replaced; refusing cleanup");
            ensure!(bounded_read(self.dir.read(STAGE_MARKER)?, 512)? == self.token.as_bytes(), "Stage ownership mismatch");
            remove_contents(&self.dir, 0)?;
            let name = component(self.name.as_bytes())?;
            let r = unsafe { libc::unlinkat(self.destination.0.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
            ensure!(r == 0, "Remove stage: {}", std::io::Error::last_os_error());
            Ok(())
        };
        if let Err(e) = cleanup() { eprintln!("Installer staging cleanup refused/failed: {e:#}"); }
    }
}
fn remove_contents(dir: &Dir, depth: usize) -> Result<()> {
    ensure!(depth <= MAX_DEPTH + 2, "Cleanup depth limit");
    for name in dir.names()? {
        let st = dir.metadata(&name)?;
        let cname = component(name.as_bytes())?;
        let flags = if st.st_mode & libc::S_IFMT == libc::S_IFDIR {
            let child = dir.child(name.as_bytes(), false)?;
            remove_contents(&child, depth + 1)?;
            libc::AT_REMOVEDIR
        } else { 0 }; // unlink links, never follow them
        let r = unsafe { libc::unlinkat(dir.0.as_raw_fd(), cname.as_ptr(), flags) };
        ensure!(r == 0, "Cleanup failed: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn disk_check(destination: &Dir, plan: &Plan) -> Result<()> {
    let mut v = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let r = unsafe { libc::fstatvfs(destination.0.as_raw_fd(), v.as_mut_ptr()) };
    ensure!(r == 0, "Cannot check destination free space: {}", std::io::Error::last_os_error());
    let v = unsafe { v.assume_init() };
    let free = (v.f_bavail as u64).checked_mul(v.f_frsize as u64).context("Disk capacity overflow")?;
    let overhead = (plan.paths.len() as u64).checked_mul((v.f_frsize as u64).max(4096)).context("Disk overhead overflow")?;
    let required = plan.total.checked_add(overhead).and_then(|n| n.checked_add(16 * 1024 * 1024)).context("Required disk space overflow")?;
    ensure!(free >= required, "Not enough destination disk space: need {required} bytes including staging overhead; available {free}");
    Ok(())
}

pub(crate) type Progress<'a> = &'a dyn Fn(State, u64, u64);
fn copy_folder(source: &Source, output: &Dir, cancel: &AtomicBool, progress: Progress<'_>) -> Result<()> {
    copy_entries(source, source.plan.entries.iter(), output, 0, source.plan.total, cancel, progress)
}

/// Copy these source entries into `output`; `done` bytes of `all` are already installed.
fn copy_entries<'e>(source: &Source, entries: impl Iterator<Item = &'e Entry>, output: &Dir, mut done: u64, all: u64, cancel: &AtomicBool, progress: Progress<'_>) -> Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    for entry in entries {
        check_cancel(cancel)?;
        if entry.directory { output.parent(&format!("{}/_", entry.path), true)?; continue; }
        let mut input = source.file(&entry.path)?;
        let mut target = output.new_file(&entry.path)?;
        let mut count = 0u64;
        loop {
            check_cancel(cancel)?;
            let n = input.read(&mut buf)?;
            if n == 0 { break; }
            count = count.checked_add(n as u64).context("Copy size overflow")?;
            ensure!(count <= entry.size, "Source grew beyond inspected size");
            target.write_all(&buf[..n])?;
            done += n as u64;
            progress(State::Extracting, done, all);
        }
        ensure!(count == entry.size && source.stamps.get(&entry.path) == Some(&Stamp::of(&input.metadata()?)), "Source changed during copy");
        let _ = source.file(&entry.path)?;
        target.sync_all()?;
    }
    Ok(())
}
fn extract(api: &Api, source: &Source, paths: &[String], plan: &Plan, output: &Dir, cancel: &AtomicBool, progress: Progress<'_>) -> Result<()> {
    extract_with(api, source, paths, plan, output, 0, plan.total, cancel, progress)
}

/// Extract one archive into `output`; `before` bytes of a larger install are already done.
#[allow(clippy::too_many_arguments)]
fn extract_with(api: &Api, source: &Source, paths: &[String], plan: &Plan, output: &Dir, before: u64, all: u64, cancel: &AtomicBool, progress: Progress<'_>) -> Result<()> {
    let mut archive = Archive::open(api, source, paths)?;
    archive.links = plan.entries.iter().any(|e| e.link.is_some());
    let mut done = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    for expected in &plan.entries {
        check_cancel(cancel)?;
        let entry = archive.header()?.context("Archive truncated since inspection")?;
        ensure!(&entry == expected, "Archive headers changed since inspection");
        if entry.directory {
            output.parent(&format!("{}/_", entry.path), true)?;
        } else if let Some(link) = &entry.link {
            output.symlink(&entry.path, link)?;
        } else {
            let mut target = output.new_file(&entry.path)?;
            if entry.exec { target.set_permissions(fs::Permissions::from_mode(0o700))?; }
            let mut count = 0u64;
            loop {
                check_cancel(cancel)?;
                let n = unsafe { (api.data)(archive.handle, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 { archive.status(n as i32, "Extract archive data (corruption, password, or missing volume)")?; }
                if n == 0 { break; }
                ensure!(n as usize <= buf.len(), "Invalid libarchive read length");
                count = count.checked_add(n as u64).context("Output size overflow")?;
                done = done.checked_add(n as u64).context("Total output overflow")?;
                ensure!(count <= entry.size && done <= plan.total, "Archive output exceeds inspected header size");
                target.write_all(&buf[..n as usize])?;
                progress(State::Extracting, before + done, all);
            }
            ensure!(count == entry.size, "Archive entry truncated: {}", entry.path);
            target.sync_all()?;
        }
    }
    check_cancel(cancel)?;
    ensure!(archive.header()?.is_none(), "Archive added entries after inspection");
    archive.verify_sources(source, paths)?;
    ensure!(done == plan.total, "Archive total differs from inspected size");
    Ok(())
}

/// A release shipped as a single exFAT disk image (`.exfat`), which libarchive can't read.
fn exfat_image(source: &Source) -> Result<Option<String>> {
    let images: Vec<&Entry> = source.plan.entries.iter()
        .filter(|e| !e.directory && e.path.to_ascii_lowercase().ends_with(".exfat")).collect();
    ensure!(images.len() <= 1, "This download has {} disk images; install supports one game image at a time", images.len());
    let Some(image) = images.first() else { return Ok(None) };
    ensure!(crate::exfat::is_image(&source.file(&image.path)?), "{} is not an exFAT disk image", image.path);
    Ok(Some(image.path.clone()))
}

fn inspect_image(volume: &crate::exfat::Volume, cancel: &AtomicBool) -> Result<(Vec<crate::exfat::Node>, Plan)> {
    let nodes = volume.list(&|| check_cancel(cancel))?;
    let mut plan = Plan::default();
    for node in &nodes {
        ensure!(normal_path(&node.path)? == node.path, "Unsafe path in disk image: {}", node.path);
        plan.add(Entry::file(node.path.clone(), node.directory, node.size))?;
    }
    ensure!(!plan.entries.is_empty(), "Disk image is empty");
    Ok((nodes, plan))
}

fn extract_image(volume: &crate::exfat::Volume, nodes: &[crate::exfat::Node], plan: &Plan, output: &Dir, cancel: &AtomicBool, progress: Progress<'_>) -> Result<()> {
    let mut done = 0u64;
    for node in nodes {
        check_cancel(cancel)?;
        if node.directory { output.parent(&format!("{}/_", node.path), true)?; continue; }
        let mut target = output.new_file(&node.path)?;
        let mut count = 0u64;
        volume.copy(node, &mut |chunk| {
            check_cancel(cancel)?;
            count += chunk.len() as u64;
            done += chunk.len() as u64;
            ensure!(count <= node.size && done <= plan.total, "Disk image output exceeds inspected size");
            target.write_all(chunk)?;
            progress(State::Extracting, done, plan.total);
            Ok(())
        })?;
        ensure!(count == node.size, "Disk image file truncated: {}", node.path);
        target.sync_all()?;
    }
    ensure!(done == plan.total, "Disk image total differs from inspected size");
    Ok(())
}

/// Whether a PS4 param.sfo describes a full game ("gd"), not an update or add-on.
fn ps4_is_game(output: &Dir, path: &str) -> bool {
    output.read(path).ok().and_then(|f| bounded_read(f, MAX_PARAM).ok())
        .and_then(|b| crate::sfo::parse(&b))
        .is_some_and(|p| p.get("CATEGORY").is_some_and(|c| c.text().starts_with("gd")))
}

const PKG_UNSUPPORTED: &str = "Unsupported PKG package: the launcher unpacks only PS5 debug packages and PS4 packages that hold a game. Install other .pkg files on the console, or provide an already extracted game with sce_sys/param.json and eboot.bin";

/// Whether a plan holds a game: it has a sce_sys/param.json, at its root or in a folder.
fn holds_game(plan: &Plan) -> bool {
    plan.entries.iter().any(|e| !e.directory && (e.path == "sce_sys/param.json" || e.path.ends_with("/sce_sys/param.json")))
}

/// Whether the release folder already holds an extracted game: a sce_sys/param.json (PS5) or a
/// game's sce_sys/param.sfo (PS4, not an update's), with a nonempty eboot.bin beside sce_sys. The
/// folder then installs as it is, and packages next to the game (DLC unlockers, for example) are
/// copied with it, not unpacked.
fn holds_extracted_game(source: &Source) -> bool {
    source.plan.entries.iter().filter(|e| !e.directory).any(|e| {
        let root = match (e.path.strip_suffix("sce_sys/param.json"), e.path.strip_suffix("sce_sys/param.sfo")) {
            (Some(root), _) => root,
            (_, Some(root)) if ps4_is_game(&source.dir, &e.path) => root,
            _ => return false,
        };
        (root.is_empty() || root.ends_with('/'))
            && source.plan.entries.iter().any(|x| !x.directory && x.size > 0 && x.path == format!("{root}eboot.bin"))
    })
}

/// What an emulator gets when the installer finds `.pkg` files it doesn't unpack itself.
pub struct PackageJob<'a> {
    pub req: &'a Request,
    pub source: &'a Source,
    pub pkgs: &'a [String],
    pub cancel: &'a AtomicBool,
    pub progress: Progress<'a>,
    pub commit: &'a dyn Fn(&Stage, &str, &str, &str) -> Result<()>,
}

/// The PS4 update (param.sfo category `gp`) for `tid` shipped with a game, if there is exactly one.
pub(crate) fn update_root(output: &Dir, plan: &Plan, tid: &str) -> Option<String> {
    let roots: Vec<String> = plan.entries.iter().filter(|e| !e.directory).filter_map(|e| {
        let root = e.path.strip_suffix("/sce_sys/param.sfo")?;
        let sfo = output.read(&e.path).ok().and_then(|f| bounded_read(f, MAX_PARAM).ok()).and_then(|b| crate::sfo::parse(&b))?;
        let is_update = sfo.get("CATEGORY").is_some_and(|c| c.text().starts_with("gp"));
        let same_game = sfo.get("TITLE_ID").is_some_and(|t| t.text() == tid);
        (is_update && same_game).then(|| root.to_string())
    }).collect();
    if roots.len() > 1 { crate::log!("{} updates for {tid} in one release; none installed", roots.len()); }
    (roots.len() == 1).then(|| roots[0].clone())
}

/// Put a game's update next to it as `<leaf>-patch`, where shadPS4 looks for updates. Runs after
/// the game is published: a failure leaves the game playable without its update.
pub(crate) fn place_update(stage: &Stage, destination: &Path, game_root: &str, update: &str, leaf: &str) {
    let patch = format!("{leaf}-patch");
    let inside = if game_root.is_empty() { Some(update) } else { update.strip_prefix(&format!("{game_root}/")) };
    let result = match inside {
        // Shipped inside the game's folder, so it moved with it: move it out beside the game.
        Some(rel) => (|| -> Result<()> {
            let from = std::ffi::CString::new(destination.join(leaf).join(rel).into_os_string().into_encoded_bytes())?;
            let to = std::ffi::CString::new(destination.join(&patch).into_os_string().into_encoded_bytes())?;
            crate::hostos::rename_noreplace(libc::AT_FDCWD, &from, libc::AT_FDCWD, &to)?;
            Ok(())
        })(),
        None => stage.publish(update, &patch),
    };
    match result {
        Ok(()) => crate::log!("Installed the update as {patch}"),
        Err(e) => crate::log!("Installed {leaf}, but its update could not be placed: {e:#}"),
    }
}

/// A game package opened and listed, ready to install.
struct Chosen { package: crate::emulator::kytyps5::pkg::Package, nodes: Vec<crate::emulator::kytyps5::pkg::Node>, plan: Plan }

// Debug `.pkg` files in the release. Each one that opens is listed; the one that holds a game wins.
// A game has a sce_sys/param.json and an eboot.bin. DLC packs and update-only packages hold neither,
// so they are skipped. A backport overlay does hold a game, so when several do, the package nearest
// the top of the release is the base game. Equal depth is never guessed. A package that fails to open
// at or above the winner's depth might be the base game, so that failure stops the install.
fn select_package(source: &Source, cancel: &AtomicBool) -> Result<Option<Chosen>> {
    let depth = |path: &str| path.matches('/').count();
    let mut best: Option<(usize, Vec<String>, Chosen)> = None;
    let mut failure: Option<(usize, anyhow::Error)> = None;
    for entry in source.plan.entries.iter().filter(|e| !e.directory && e.path.to_ascii_lowercase().ends_with(".pkg")) {
        check_cancel(cancel)?;
        let file = source.file(&entry.path)?;
        if !crate::emulator::kytyps5::pkg::is_package(&file) { continue; }
        let level = depth(&entry.path);
        match open_package(file, cancel) {
            Ok(chosen) if holds_game(&chosen.plan) && chosen.plan.explicit.contains("eboot.bin") => match &mut best {
                Some((top, names, _)) if level == *top => names.push(entry.path.clone()),
                Some((top, ..)) if level > *top => (),
                _ => best = Some((level, vec![entry.path.clone()], chosen)),
            },
            Ok(_) => (),
            Err(e) if cancel.load(Ordering::Acquire) => return Err(e),
            Err(e) => if failure.as_ref().is_none_or(|f| level < f.0) { failure = Some((level, e.context(format!("Open {}", entry.path)))); },
        }
    }
    if let Some((level, e)) = failure {
        if best.as_ref().is_none_or(|b| level <= b.0) { return Err(e); }
    }
    let Some((_, names, chosen)) = best else { return Ok(None) };
    ensure!(names.len() == 1, "Several packages hold a game ({}); choose a single release folder", names.join(", "));
    Ok(Some(chosen))
}

fn open_package(file: File, cancel: &AtomicBool) -> Result<Chosen> {
    let mut package = crate::emulator::kytyps5::pkg::Package::open(file, &|| check_cancel(cancel))?;
    let nodes = package.list(&|| check_cancel(cancel))?;
    let mut plan = Plan::limited(MAX_PACKAGE_ENTRIES);
    for node in &nodes {
        ensure!(normal_path(&node.path)? == node.path, "Unsafe path in package: {}", node.path);
        plan.add(Entry::file(node.path.clone(), node.directory, node.size))?;
    }
    Ok(Chosen { package, nodes, plan })
}

fn extract_package(package: &mut crate::emulator::kytyps5::pkg::Package, nodes: &[crate::emulator::kytyps5::pkg::Node], plan: &Plan, output: &Dir, cancel: &AtomicBool, progress: Progress<'_>) -> Result<()> {
    let total = plan.total;
    let mut done = 0u64;
    for node in nodes {
        check_cancel(cancel)?;
        if node.directory { output.parent(&format!("{}/_", node.path), true)?; continue; }
        write_file(output, &node.path, node.size, (&mut done, total), cancel, progress, &mut |sink| package.copy(node, sink))?;
    }
    ensure!(done == total, "Package total differs from inspected size");
    Ok(())
}

/// Write one file of `size` bytes from `copy` into a new file in `output`, counting toward
/// `(done, total)`. The file must be exactly `size` bytes.
pub(crate) fn write_file(output: &Dir, path: &str, size: u64, (done, total): (&mut u64, u64), cancel: &AtomicBool, progress: Progress<'_>,
    copy: &mut dyn FnMut(&mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()>) -> Result<()> {
    let mut target = output.new_file(path)?;
    let mut count = 0u64;
    copy(&mut |chunk| {
        check_cancel(cancel)?;
        count += chunk.len() as u64;
        *done += chunk.len() as u64;
        ensure!(count <= size && *done <= total, "Package output exceeds inspected size");
        target.write_all(chunk)?;
        progress(State::Extracting, *done, total);
        Ok(())
    })?;
    ensure!(count == size, "Package file truncated: {path}");
    target.sync_all()?;
    Ok(())
}

pub(crate) fn game_root(output: &Dir, plan: &Plan, expected_ids: &[String], cancel: &AtomicBool) -> Result<(String, String)> {
    let mut games = Vec::new();
    let mut failures = Vec::new();
    for entry in &plan.entries {
        check_cancel(cancel)?;
        if entry.directory { continue; }
        // PS5 games describe themselves in param.json, PS4 games in param.sfo.
        let (root, ps4) = if entry.path == "sce_sys/param.json" { ("", false) }
            else if let Some(p) = entry.path.strip_suffix("/sce_sys/param.json") { (p, false) }
            else if entry.path == "sce_sys/param.sfo" { ("", true) }
            else if let Some(p) = entry.path.strip_suffix("/sce_sys/param.sfo") { (p, true) }
            else { continue; };
        if ps4 && !ps4_is_game(output, &entry.path) {
            continue; // an update or add-on next to the game, not a second game
        }
        let validate = || -> Result<String> {
            let bytes = bounded_read(output.read(&entry.path)?, MAX_PARAM)?;
            let tid = if ps4 {
                let sfo = crate::sfo::parse(&bytes).context("Invalid sce_sys/param.sfo")?;
                sfo.get("TITLE_ID").map(|v| v.text().to_string()).context("param.sfo is missing TITLE_ID")?
            } else {
                let value: serde_json::Value = serde_json::from_slice(&bytes).context("Invalid sce_sys/param.json JSON")?;
                value["titleId"].as_str().context("param.json is missing titleId")?.to_string()
            };
            let tid = tid.as_str();
            ensure!(crate::psn::valid_title_id(tid), "Invalid title ID in the game's metadata");
            let executable = if root.is_empty() { "eboot.bin".into() } else { format!("{root}/eboot.bin") };
            let file = output.read(&executable).context("Missing eboot.bin; an update/PKG is not a complete extracted game")?;
            ensure!(file.metadata()?.len() > 0, "eboot.bin is empty");
            Ok(tid.into())
        };
        match validate() {
            Ok(tid) => games.push((root.to_string(), tid)),
            Err(e) => failures.push(format!("{root}: {e:#}")),
        }
    }
    ensure!(games.len() <= 1, "Multiple valid game roots found; install one complete game at a time");
    let Some((root, tid)) = games.pop() else {
        if plan.entries.iter().any(|e| e.path.to_ascii_lowercase().ends_with(".pkg")) {
            bail!(PKG_UNSUPPORTED);
        }
        bail!("No valid extracted game found; require bounded sce_sys/param.json (PS5) or param.sfo (PS4) with a title ID and nonempty eboot.bin. {}", failures.join("; "));
    };
    ensure!(expected_ids.is_empty() || expected_ids.contains(&tid), "Title ID mismatch: extracted {tid}, expected {}", expected_ids.join(", "));
    Ok((root, tid))
}

fn install(req: &Request, cancel: &AtomicBool, progress: Progress<'_>, commit: &dyn Fn(&Stage, &str, &str, &str) -> Result<()>) -> Result<()> {
    validate_request(req)?;
    check_cancel(cancel)?;
    if req.platform == crate::platform::Platform::Pc {
        return install_pc(req, cancel, progress, commit);
    }
    progress(State::Inspecting, 0, 0);
    let source = Source::inspect(req, cancel)?;
    let paths = volumes(&source.plan)?;
    let api = if paths.is_some() { Some(Api::load()?) } else { None };
    let archive_plan = if let Some(paths) = &paths { Some(inspect_archive(api.as_ref().unwrap(), &source, paths, cancel)?) } else { None };
    let image = exfat_image(&source)?;
    ensure!(image.is_none() || paths.is_none(), "This download has both an archive and a disk image; choose a single release folder");
    let volume = match &image { Some(path) => Some(crate::exfat::Volume::open(source.file(path)?).context("Open exFAT disk image")?), None => None };
    let image_plan = match &volume { Some(v) => Some(inspect_image(v, cancel)?), None => None };
    // A release folder that is not an archive, an image or an extracted game: look for packages.
    let wants_package = paths.is_none() && image.is_none() && !holds_extracted_game(&source);
    let (mut package, package_plan) = match if wants_package { select_package(&source, cancel)? } else { None } {
        Some(Chosen { package, nodes, plan }) => (Some((package, nodes)), Some(plan)),
        None => (None, None),
    };
    let plan = archive_plan.as_ref().or(image_plan.as_ref().map(|(_, plan)| plan)).or(package_plan.as_ref()).unwrap_or(&source.plan);
    // PS5 packages are read natively above; any other .pkg in the release folder is a PS4 package
    // for the extractor. Only the folder's own files: a .pkg inside an archive, image or package isn't one.
    let mut pkgs = Vec::new();
    if wants_package && package.is_none() {
        for entry in source.plan.entries.iter().filter(|e| !e.directory && e.path.to_ascii_lowercase().ends_with(".pkg")) {
            if !crate::emulator::kytyps5::pkg::is_package(&source.file(&entry.path)?) { pkgs.push(entry.path.clone()); }
        }
    }
    if !pkgs.is_empty() {
        let job = PackageJob { req, source: &source, pkgs: &pkgs, cancel, progress, commit };
        if let Some(result) = crate::emulator::all().iter().find_map(|e| e.install_packages(&job)) {
            return result;
        }
    }
    check_cancel(cancel)?;
    let destination = Dir::absolute(&req.destination, true).context("Open explicit installation destination")?;
    // Prevent installing into the torrent tree or a parent of it; preserve originals.
    let actual_dest = crate::hostos::fd_real_path(destination.0.as_raw_fd())?;
    let actual_source = crate::hostos::fd_real_path(source.dir.0.as_raw_fd())?;
    ensure!(!actual_dest.starts_with(&actual_source) && !actual_source.starts_with(&actual_dest), "Source and destination trees must be separate");
    disk_check(&destination, plan)?;
    let stage = Stage::new(destination, &req.key)?;
    let output = stage.payload()?;
    progress(State::Extracting, 0, plan.total);
    if let Some(paths) = &paths { extract(api.as_ref().unwrap(), &source, paths, plan, &output, cancel, progress)?; }
    else if let (Some(v), Some((nodes, _)), Some(path)) = (&volume, &image_plan, &image) {
        extract_image(v, nodes, plan, &output, cancel, progress)?;
        let _ = source.file(path).context("Disk image changed while installing")?;
    }
    else if let Some((package, nodes)) = &mut package { extract_package(package, nodes, plan, &output, cancel, progress)?; }
    else { copy_folder(&source, &output, cancel, progress)?; }
    check_cancel(cancel)?;
    progress(State::Validating, plan.total, plan.total);
    let (root, tid) = game_root(&output, plan, &req.expected_ids, cancel)?;
    let update = update_root(&output, plan, &tid);
    let leaf = format!("{tid}-{}", &req.key[..12]);
    check_cancel(cancel)?;
    commit(&stage, &root, &leaf, &tid)?;
    if let Some(update) = update { place_update(&stage, &req.destination, &root, &update, &leaf); }
    Ok(())
}

/// An archive at the top of a PC release.
struct PcArchive {
    volumes: Vec<String>,
    /// A GOG Linux installer (`.sh` with a zip appended, read as that zip; the script never runs).
    gog: bool,
    /// What sits beside it that could start the game.
    beside: Beside,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Beside { Nothing, Script, Program }

/// The archives at the top of a PC release (or in a folder at its top): a game's own data
/// archives deeper inside stay as they are. Each volume set (game.rar, game.r00, …) is one.
fn pc_archives(source: &Source, plan: &Plan) -> Result<Vec<PcArchive>> {
    let top: Vec<&Entry> = plan.entries.iter().filter(|e| !e.directory && e.path.matches('/').count() <= 1).collect();
    let folder = |path: &str| path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
    let mut gog = HashSet::new();
    for e in top.iter().filter(|e| e.path.to_ascii_lowercase().ends_with(".sh")) {
        if gog_installer(source, &e.path)? { gog.insert(e.path.clone()); }
    }
    let mut starters: std::collections::HashMap<String, Beside> = std::collections::HashMap::new();
    for e in &top {
        if gog.contains(&e.path) || volume_of(&e.path)?.is_some() { continue; }
        let mut head = Vec::with_capacity(4096);
        source.file(&e.path)?.take(4096).read_to_end(&mut head)?;
        use crate::pcgame::Probe;
        let kind = match crate::pcgame::probe(&head) {
            Probe::Elf { .. } | Probe::Exe | Probe::AppImage => Beside::Program,
            Probe::Script => Beside::Script,
            Probe::Other => continue,
        };
        let best = starters.entry(folder(&e.path)).or_insert(kind);
        *best = (*best).max(kind);
    }
    let mut archives = Vec::new();
    let mut families: Vec<(String, Plan)> = Vec::new();
    for e in &top {
        if gog.contains(&e.path) {
            archives.push(PcArchive { volumes: vec![e.path.clone()], gog: true, beside: Beside::Nothing });
            continue;
        }
        let Some((family, _)) = volume_of(&e.path)? else { continue };
        let i = match families.iter().position(|(f, _)| *f == family) {
            Some(i) => i,
            None => { families.push((family, Plan::default())); families.len() - 1 }
        };
        families[i].1.add(Entry::file(e.path.clone(), false, e.size))?;
    }
    for (_, one) in &families {
        if let Some(volumes) = volumes(one)? {
            let beside = starters.get(&folder(&volumes[0])).copied().unwrap_or(Beside::Nothing);
            archives.push(PcArchive { volumes, gog: false, beside });
        }
    }
    Ok(archives)
}

/// A GOG Linux installer: a shell script followed by a zip (its end-of-directory record is in the
/// last 64 KiB).
fn gog_installer(source: &Source, path: &str) -> Result<bool> {
    use std::io::{Seek, SeekFrom};
    let mut file = source.file(path)?;
    let len = file.metadata()?.len();
    if len < 1024 { return Ok(false); }
    let mut head = [0u8; 2];
    file.read_exact(&mut head)?;
    if &head != b"#!" { return Ok(false); }
    let tail = len.min(64 * 1024);
    file.seek(SeekFrom::Start(len - tail))?;
    let mut buf = Vec::with_capacity(tail as usize);
    file.take(tail).read_to_end(&mut buf)?;
    Ok(buf.windows(4).any(|w| w == b"PK\x05\x06"))
}

/// Install a PC game: unpack its archives (or copy the folder), make its programs executable
/// (torrents drop the flag), find how it starts, and leave a manifest for the Library. Nothing in
/// it runs.
fn install_pc(req: &Request, cancel: &AtomicBool, progress: Progress<'_>, commit: &dyn Fn(&Stage, &str, &str, &str) -> Result<()>) -> Result<()> {
    progress(State::Inspecting, 0, 0);
    let source = Source::inspect(req, cancel)?;
    let archives = pc_archives(&source, &source.plan)?;
    let api = if archives.is_empty() { None } else { Some(Api::load()?) };
    // Which archives are the release's packaging, to unpack: a GOG installer; one with nothing
    // beside it that could start the game; one beside only a start script that holds a program
    // (the script starts it). The rest are the game's data, copied: `data.tar` beside
    // `game.x86_64` (helper scripts in it or not), or beside the `start.sh` that reads it.
    let mut groups = Vec::new();
    let mut plans = Vec::new();
    for archive in archives {
        let plan = inspect_archive_with(api.as_ref().unwrap(), &source, &archive.volumes, true, cancel)?;
        let packaging = match archive.beside { Beside::Nothing => true, Beside::Script => plan.starts, Beside::Program => false };
        if archive.gog || packaging {
            groups.push(archive.volumes);
            plans.push(plan);
        }
    }
    // Everything that isn't one of those archives is copied as it is.
    let unpacked: HashSet<&String> = groups.iter().flatten().collect();
    let loose = |e: &&Entry| !unpacked.contains(&e.path);
    let mut total = Plan::default();
    for plan in &plans {
        for e in &plan.entries { total.total = total.total.checked_add(e.size).context("Output size overflow")?; }
        total.paths.extend(plan.paths.clone());
    }
    for e in source.plan.entries.iter().filter(loose) {
        total.total = total.total.checked_add(e.size).context("Output size overflow")?;
        total.paths.insert(e.path.clone(), e.directory);
    }
    check_cancel(cancel)?;
    let destination = Dir::absolute(&req.destination, true).context("Open explicit installation destination")?;
    let actual_dest = crate::hostos::fd_real_path(destination.0.as_raw_fd())?;
    let actual_source = crate::hostos::fd_real_path(source.dir.0.as_raw_fd())?;
    ensure!(!actual_dest.starts_with(&actual_source) && !actual_source.starts_with(&actual_dest), "Source and destination trees must be separate");
    disk_check(&destination, &total)?;
    let stage = Stage::new(destination, &req.key)?;
    let output = stage.payload()?;
    progress(State::Extracting, 0, total.total);
    let mut before = 0;
    for (group, plan) in groups.iter().zip(&plans) {
        extract_with(api.as_ref().unwrap(), &source, group, plan, &output, before, total.total, cancel, progress)?;
        before += plan.total;
    }
    copy_entries(&source, source.plan.entries.iter().filter(loose), &output, before, total.total, cancel, progress)?;
    check_cancel(cancel)?;
    progress(State::Validating, total.total, total.total);
    // The game's folder: the payload, or the single folder in it (a release wrapped in a
    // folder, or GOG's data/noarch).
    let mut root = String::new();
    loop {
        let dir = if root.is_empty() { stage.payload()? } else { output.parent(&format!("{root}/_"), false)?.0 };
        let names: Vec<String> = dir.names()?.into_iter().filter(|n| !n.starts_with('.')).collect();
        let only_dir = names.len() == 1 && dir.metadata(&names[0])?.st_mode & libc::S_IFMT == libc::S_IFDIR;
        // GOG's installer zip: the game is data/noarch (next to installer files).
        let gog = names.iter().any(|n| n == "data") && dir.child(b"data", false).and_then(|d| d.child(b"noarch", false)).is_ok();
        if only_dir {
            root = if root.is_empty() { names[0].clone() } else { format!("{root}/{}", names[0]) };
        } else if gog {
            root = if root.is_empty() { "data/noarch".into() } else { format!("{root}/data/noarch") };
        } else { break; }
    }
    let game = if root.is_empty() { stage.payload()? } else { output.parent(&format!("{root}/_"), false)?.0 };
    let mut files = Vec::new();
    pc_files(&game, "", &mut files, 0, cancel)?;
    let name = crate::pcgame::clean_name(&req.name);
    let (launch, kind, alternatives) = crate::pcgame::choose(&files, &name).context(
        "Nothing in this release can start the game: no start script, Linux program, AppImage or Windows .exe was found")?;
    let manifest = crate::pcgame::Manifest { version: 1, name: name.clone(), topic: req.topic, launch, kind, alternatives };
    let mut file = game.new_file(crate::pcgame::MANIFEST)?;
    file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    file.sync_all()?;
    let leaf = format!("{}-{}", crate::pcgame::folder_name(&name), &req.key[..12]);
    check_cancel(cancel)?;
    commit(&stage, &root, &leaf, "")
}

/// Every regular file of an installed PC game with what it is; programs and scripts become
/// executable on the way.
fn pc_files(dir: &Dir, prefix: &str, out: &mut Vec<(String, crate::pcgame::Probe)>, depth: usize, cancel: &AtomicBool) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "Game folder is nested too deeply");
    for name in dir.names()? {
        check_cancel(cancel)?;
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let st = dir.metadata(&name)?;
        match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR => pc_files(&dir.child(name.as_bytes(), false)?, &path, out, depth + 1, cancel)?,
            libc::S_IFREG => {
                let file = dir.read(&name)?;
                // Enough for any file's signature and a program's headers.
                let mut head = Vec::with_capacity(4096);
                (&file).take(4096).read_to_end(&mut head)?;
                let probe = crate::pcgame::probe(&head);
                if probe.runnable() || st.st_mode & 0o100 != 0 {
                    file.set_permissions(fs::Permissions::from_mode(0o700))?;
                }
                ensure!(out.len() < MAX_ENTRIES, "Too many files");
                out.push((path, probe));
            }
            _ => {} // symlinks: checked when unpacked; never followed here
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Saved { version: u32, jobs: Vec<Record> }
fn load_records(store: &Path) -> Result<Vec<Record>> {
    let dir = Dir::absolute(store, false)?;
    let saved: Saved = serde_json::from_slice(&bounded_read(dir.read("jobs.json")?, 16 * 1024 * 1024)?)?;
    ensure!(saved.version == 1 && saved.jobs.len() <= 10_000, "Unsupported or oversized installer manifest");
    Ok(saved.jobs)
}
fn save_records(store: &Path, jobs: &[Record]) -> Result<()> {
    ensure!(jobs.len() <= 10_000, "Too many persistent installation records");
    let dir = Dir::absolute(store, true)?;
    ensure!(dir.0.metadata()?.uid() == unsafe { libc::geteuid() }, "Installation record directory has another owner");
    ensure!(unsafe { libc::fchmod(dir.0.as_raw_fd(), 0o700) } == 0, "Cannot make installation records private");
    let bytes = serde_json::to_vec(&Saved { version: 1, jobs: jobs.to_vec() })?;
    ensure!(bytes.len() as u64 <= 16 * 1024 * 1024, "Installation manifest exceeds 16 MiB");
    let name = format!(".jobs-{}-{}.tmp", std::process::id(), UNIQUE.fetch_add(1, Ordering::Relaxed));
    let result = || -> Result<()> {
        let mut file = dir.new_file(&name)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let old = component(name.as_bytes())?;
        let new = component(b"jobs.json")?;
        ensure!(unsafe { libc::renameat(dir.0.as_raw_fd(), old.as_ptr(), dir.0.as_raw_fd(), new.as_ptr()) } == 0, "Persist installation records: {}", std::io::Error::last_os_error());
        dir.0.sync_all()?;
        Ok(())
    };
    let result = result();
    if result.is_err() { if let Ok(name) = component(name.as_bytes()) { unsafe { libc::unlinkat(dir.0.as_raw_fd(), name.as_ptr(), 0); } } }
    result
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};
    use super::*;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::symlink;
    const KEY: &str = "0123456789abcdef0123456789abcdef01234567";
    const PARAM: &[u8] = br#"{"titleId":"PPSA12345","localizedParameters":{"en-US":{"titleName":"Generated fixture"}}}"#;

    fn fixture() -> (tempfile::TempDir, Request) {
        let t = crate::hostos::real_tempdir();
        let source = t.path().join("torrent");
        fs::create_dir(&source).unwrap();
        fs::write(source.join(OWNED), KEY).unwrap();
        let req = Request { key: KEY.into(), topic: 1, name: "Generated fixture".into(), source,
            destination: t.path().join("games"), expected_ids: vec!["PPSA12345".into()], platform: Default::default() };
        (t, req)
    }
    fn game(path: &Path) {
        fs::create_dir_all(path.join("sce_sys")).unwrap();
        fs::write(path.join("sce_sys/param.json"), PARAM).unwrap();
        fs::write(path.join("eboot.bin"), b"not an executable; generated bytes\0\xff").unwrap();
        fs::write(path.join("data"), b"fixture contents").unwrap();
    }
    fn run(req: &Request, cancel: &AtomicBool, progress: Progress<'_>) -> Result<PathBuf> {
        let result = Mutex::new(None);
        install(req, cancel, progress, &|s, root, leaf, _| {
            check_cancel(cancel)?;
            s.publish(root, leaf)?;
            *result.lock().unwrap() = Some(req.destination.join(leaf));
            Ok(())
        })?;
        Ok(result.into_inner().unwrap().unwrap())
    }
    fn clean(req: &Request) {
        if req.destination.exists() {
            assert!(!fs::read_dir(&req.destination).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".ps5-install")));
        }
        assert_eq!(fs::read(req.source.join(OWNED)).unwrap(), KEY.as_bytes());
    }
    // Handcrafted ustar permits adversarial paths/links/duplicate records without
    // asking a shell extractor to materialize anything on disk.
    fn tar(entries: &[(&str, &[u8], u8, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, data, kind, link) in entries {
            let mut h = [0u8; 512];
            h[..path.len()].copy_from_slice(path.as_bytes());
            h[100..108].copy_from_slice(b"0000644\0");
            h[108..116].copy_from_slice(b"0000000\0"); h[116..124].copy_from_slice(b"0000000\0");
            h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
            h[136..148].copy_from_slice(b"00000000000\0");
            h[148..156].fill(b' '); h[156] = *kind;
            h[157..157 + link.len()].copy_from_slice(link.as_bytes());
            h[257..263].copy_from_slice(b"ustar\0"); h[263..265].copy_from_slice(b"00");
            let sum: u32 = h.iter().map(|b| *b as u32).sum();
            h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
            out.extend(h); out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.resize(out.len() + 1024, 0); out
    }
    fn valid_tar() -> Vec<u8> { tar(&[("wrapper/game/sce_sys/param.json", PARAM, b'0', ""), ("wrapper/game/eboot.bin", b"fixture binary bytes", b'0', ""), ("wrapper/game/data", b"data", b'0', "")]) }

    // Small, generated RAR4 stored records: no archiver, downloaded fixture or
    // dependency is needed. CRC-32/IEEE is bounded and checked against a known
    // vector below; RAR headers store its low 16 bits, file data the full CRC.
    fn rar_crc32(bytes: &[u8]) -> u32 {
        assert!(bytes.len() <= 1024 * 1024, "Generated CRC input is oversized");
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        !crc
    }
    fn rar_header(out: &mut Vec<u8>, mut header: Vec<u8>) {
        assert!((7..=u16::MAX as usize).contains(&header.len()));
        assert_eq!(usize::from(u16::from_le_bytes([header[5], header[6]])), header.len());
        let crc = (rar_crc32(&header[2..]) as u16).to_le_bytes();
        header[..2].copy_from_slice(&crc);
        out.extend(header);
    }
    fn rar4(entries: &[(&str, &[u8])], file_flags: u16) -> Vec<u8> {
        assert!(entries.len() <= 16);
        let mut out = b"Rar!\x1a\x07\0".to_vec();
        rar_header(&mut out, vec![0, 0, 0x73, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0]);
        for &(name, data) in entries {
            assert!(!name.is_empty() && name.len() <= 255 && data.len() <= 64 * 1024);
            let mut h = vec![0, 0, 0x74];
            h.extend_from_slice(&(0x8000 | file_flags).to_le_bytes()); // LONG_BLOCK
            h.extend_from_slice(&((32 + name.len()) as u16).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes()); // packed size
            h.extend_from_slice(&(data.len() as u32).to_le_bytes()); // unpacked size
            h.push(3); // Unix host OS
            h.extend_from_slice(&rar_crc32(data).to_le_bytes());
            h.extend_from_slice(&0x00210000u32.to_le_bytes()); // DOS 1980-01-01
            h.extend_from_slice(&[20, 0x30]); // unpack version 2.0, stored method
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0o100644u32.to_le_bytes());
            h.extend_from_slice(name.as_bytes());
            rar_header(&mut out, h);
            out.extend_from_slice(data);
        }
        rar_header(&mut out, vec![0, 0, 0x7b, 0, 0, 7, 0]);
        assert!(out.len() <= 1024 * 1024);
        out
    }
    fn rar_binary() -> Vec<u8> {
        (0..8192).map(|i| (i as u8).wrapping_mul(73).wrapping_add(19)).collect()
    }
    fn valid_rar4(binary: &[u8], file_flags: u16) -> Vec<u8> {
        rar4(&[("game/sce_sys/param.json", PARAM), ("game/eboot.bin", binary)], file_flags)
    }
    fn unpublished(req: &Request) {
        clean(req);
        assert!(crate::library::scan(&[req.destination.clone()]).is_empty());
        if req.destination.exists() { assert_eq!(fs::read_dir(&req.destination).unwrap().count(), 0); }
    }

    #[test]
    fn rar4_stored_install_preserves_digest_exact_binary_and_no_overwrite() {
        assert_eq!(rar_crc32(b""), 0);
        assert_eq!(rar_crc32(b"123456789"), 0xcbf43926);
        let (_t, mut req) = fixture();
        // Membership in all expected IDs, not just the first catalog alias.
        req.expected_ids = vec!["PPSA99999".into(), "PPSA12345".into()];
        let binary = rar_binary(); let bytes = valid_rar4(&binary, 0);
        let archive = req.source.join("fixture.rar");
        fs::write(&archive, &bytes).unwrap(); let digest = Sha256::digest(&bytes);
        eprintln!("Generated stored RAR4 installer runtime: {}", Api::load().unwrap().version());
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {
            assert!(crate::library::scan(&[req.destination.clone()]).is_empty());
        }).unwrap();
        assert_eq!(p, req.destination.join(format!("PPSA12345-{}", &KEY[..12])));
        assert_eq!(fs::read(p.join("sce_sys/param.json")).unwrap(), PARAM);
        assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), binary);
        assert_eq!(fs::metadata(p.join("eboot.bin")).unwrap().mode() & 0o7777, 0o600);
        assert_eq!(fs::read_dir(&p).unwrap().count(), 2); // wrapper is stripped
        assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest);
        assert_eq!(crate::library::scan(&[req.destination.clone()]).len(), 1);
        let error = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err();
        assert!(error.to_string().contains("never overwritten"), "{error:#}");
        assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), binary);
        assert_eq!(fs::read(p.join("sce_sys/param.json")).unwrap(), PARAM);
        assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest);
        assert_eq!(fs::read_dir(&req.destination).unwrap().count(), 1); clean(&req);
    }

    #[test]
    fn rar4_corrupt_header_crc_and_truncated_headers_or_data_never_publish() {
        let binary = rar_binary(); let valid = valid_rar4(&binary, 0);
        let mut header_crc = valid.clone(); header_crc[7] ^= 1;
        // This runtime rejects header CRC errors, but does not enforce stored
        // RAR4 payload CRCs. Do not imply data-integrity coverage from this test.
        let cases = [
            ("header CRC", header_crc),
            ("short file header", valid[..25].to_vec()),
            ("short file data", valid[..valid.len() - 7 - 13].to_vec()),
        ];
        for (case, bytes) in cases {
            let (_t, req) = fixture(); let archive = req.source.join("bad.rar");
            fs::write(&archive, &bytes).unwrap(); let digest = Sha256::digest(&bytes);
            let error = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err();
            eprintln!("Generated RAR4 {case} rejected: {error:#}");
            assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest);
            unpublished(&req);
        }
    }

    #[test]
    fn rar4_unsafe_paths_and_duplicate_headers_never_extract() {
        let cases = [
            rar4(&[("../outside", b"x")], 0),
            rar4(&[("/absolute", b"x")], 0),
            rar4(&[("C:\\outside", b"x")], 0),
            rar4(&[("game/eboot.bin", b"a"), ("game/eboot.bin", b"b")], 0),
        ];
        for bytes in cases {
            let (t, req) = fixture(); let archive = req.source.join("unsafe.rar");
            fs::write(&archive, &bytes).unwrap(); let digest = Sha256::digest(&bytes);
            assert!(run(&req, &AtomicBool::new(false), &|_,_,_| {}).is_err());
            assert!(!req.destination.exists()); assert!(!t.path().join("outside").exists());
            assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest); unpublished(&req);
        }
    }

    #[test]
    fn rar4_password_flag_is_rejected_before_extraction() {
        let (_t, req) = fixture(); let binary = rar_binary();
        // Structurally valid, CRC-correct headers declare LHD_PASSWORD. Payload
        // is deliberately not ciphertext: this proves fail-closed flag handling,
        // not decryption of a real password-protected/headers-encrypted archive.
        let bytes = valid_rar4(&binary, 0x0004); let archive = req.source.join("encrypted.rar");
        fs::write(&archive, &bytes).unwrap(); let digest = Sha256::digest(&bytes);
        let extracted = AtomicBool::new(false);
        let error = run(&req, &AtomicBool::new(false), &|state,_,_| {
            if state == State::Extracting { extracted.store(true, Ordering::Relaxed); }
        }).unwrap_err();
        assert!(format!("{error:#}").to_ascii_lowercase().contains("password"), "{error:#}");
        assert!(!extracted.load(Ordering::Relaxed));
        assert!(!req.destination.exists());
        assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest); unpublished(&req);
    }

    #[test]
    fn rar4_cancel_during_extraction_or_validation_retains_source() {
        for at in [State::Extracting, State::Validating] {
            let (_t, req) = fixture(); let binary = rar_binary(); let bytes = valid_rar4(&binary, 0);
            let archive = req.source.join("fixture.rar");
            fs::write(&archive, &bytes).unwrap(); let digest = Sha256::digest(&bytes);
            let cancel = AtomicBool::new(false);
            let error = run(&req, &cancel, &|state,done,_| {
                assert!(crate::library::scan(&[req.destination.clone()]).is_empty());
                if state == at && done > 0 { cancel.store(true, Ordering::Release); }
            }).unwrap_err();
            assert!(cancel.load(Ordering::Acquire));
            assert!(error.to_string().contains("Cancelled"), "{error:#}");
            assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest); unpublished(&req);
        }
    }

    #[test]
    fn rar4_expected_id_mismatch_never_publishes() {
        let (_t, mut req) = fixture(); req.expected_ids = vec!["PPSA99999".into()];
        let binary = rar_binary(); let bytes = valid_rar4(&binary, 0);
        let archive = req.source.join("fixture.rar");
        fs::write(&archive, &bytes).unwrap(); let digest = Sha256::digest(&bytes);
        let error = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err();
        assert!(error.to_string().contains("Title ID mismatch"), "{error:#}");
        assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest); unpublished(&req);
    }

    #[test]
    fn folder_copy_progress_hidden_stage_atomic_and_no_overwrite() {
        let (_t, req) = fixture(); game(&req.source.join("wrapper/game"));
        fs::set_permissions(req.source.join("wrapper/game/eboot.bin"), fs::Permissions::from_mode(0o4755)).unwrap();
        let updates = Mutex::new(Vec::new());
        let p = run(&req, &AtomicBool::new(false), &|s, d, t| {
            updates.lock().unwrap().push((s, d, t));
            assert!(crate::library::scan(&[req.destination.clone()]).is_empty());
        }).unwrap();
        assert_eq!(fs::read(p.join("data")).unwrap(), b"fixture contents");
        assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), fs::read(req.source.join("wrapper/game/eboot.bin")).unwrap());
        assert_eq!(fs::metadata(p.join("eboot.bin")).unwrap().mode() & 0o7777, 0o600);
        assert_eq!(crate::library::scan(&[req.destination.clone()]).len(), 1);
        assert!(updates.lock().unwrap().iter().any(|(s,d,t)| *s == State::Extracting && *d > 0 && *t >= *d));
        assert!(run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err().to_string().contains("never overwritten"));
        assert_eq!(fs::read(p.join("data")).unwrap(), b"fixture contents"); clean(&req);
    }

    #[test]
    fn tar_extraction_preserves_archive_digest_and_identical_bytes() {
        let (_t, req) = fixture(); let bytes = valid_tar(); let archive = req.source.join("fixture.tar");
        fs::write(&archive, &bytes).unwrap(); let digest = Sha256::digest(&bytes);
        eprintln!("Installer runtime: {}", Api::load().unwrap().version());
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("sce_sys/param.json")).unwrap(), PARAM);
        assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), b"fixture binary bytes");
        assert_eq!(Sha256::digest(fs::read(&archive).unwrap()), digest);
        assert_eq!(crate::library::scan(&[req.destination.clone()]).len(), 1); clean(&req);
    }

    // ---------------------------------------------------------------- PC games

    /// A 64-bit Linux program header (PIE, with an interpreter): what a game's binary starts with.
    fn elf_program() -> Vec<u8> {
        let mut h = vec![0u8; 0x200];
        h[..4].copy_from_slice(b"\x7fELF"); h[4] = 2; h[5] = 1;
        h[16..18].copy_from_slice(&3u16.to_le_bytes()); h[18..20].copy_from_slice(&0x3eu16.to_le_bytes());
        h[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        h[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); h[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
        h[0x40..0x44].copy_from_slice(&3u32.to_le_bytes()); // PT_INTERP
        h
    }
    /// A stored (uncompressed) zip with Unix modes, like the zip inside a GOG installer.
    fn zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let (mut out, mut central) = (Vec::new(), Vec::new());
        for (path, data, mode) in entries {
            let crc = rar_crc32(data);
            let offset = out.len() as u32;
            let mut local = vec![];
            local.extend(0x04034b50u32.to_le_bytes()); local.extend(20u16.to_le_bytes()); local.extend(0u16.to_le_bytes());
            local.extend(0u16.to_le_bytes()); local.extend(0u32.to_le_bytes()); local.extend(crc.to_le_bytes());
            local.extend((data.len() as u32).to_le_bytes()); local.extend((data.len() as u32).to_le_bytes());
            local.extend((path.len() as u16).to_le_bytes()); local.extend(0u16.to_le_bytes()); local.extend(path.as_bytes());
            out.extend(&local); out.extend_from_slice(data);
            central.extend(0x02014b50u32.to_le_bytes()); central.extend((3u16 << 8 | 20).to_le_bytes()); central.extend(20u16.to_le_bytes());
            central.extend(0u16.to_le_bytes()); central.extend(0u16.to_le_bytes()); central.extend(0u32.to_le_bytes());
            central.extend(crc.to_le_bytes()); central.extend((data.len() as u32).to_le_bytes()); central.extend((data.len() as u32).to_le_bytes());
            central.extend((path.len() as u16).to_le_bytes()); central.extend([0u8; 8]);
            central.extend(((0o100000 | mode) << 16).to_le_bytes()); central.extend(offset.to_le_bytes()); central.extend(path.as_bytes());
        }
        let start = out.len() as u32;
        out.extend(&central);
        out.extend(0x06054b50u32.to_le_bytes()); out.extend([0u8; 4]);
        out.extend((entries.len() as u16).to_le_bytes()); out.extend((entries.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes()); out.extend(start.to_le_bytes()); out.extend(0u16.to_le_bytes());
        out
    }
    fn pc(req: &mut Request, name: &str) { req.platform = crate::platform::Platform::Pc; req.name = name.into(); req.expected_ids.clear(); }
    fn manifest(dir: &Path) -> crate::pcgame::Manifest {
        serde_json::from_slice(&fs::read(dir.join(crate::pcgame::MANIFEST)).unwrap()).unwrap()
    }
    fn executable(path: &Path) -> bool { fs::metadata(path).unwrap().permissions().mode() & 0o100 != 0 }

    #[test]
    fn pc_gog_installer_is_unpacked_without_running_it() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Celeste");
        let data = zip(&[("data/noarch/start.sh", b"#!/bin/bash\ncd game && ./Celeste\n", 0o755),
            ("data/noarch/game/Celeste", &elf_program(), 0o755), ("data/noarch/gameinfo", b"Celeste\n1.4\n", 0o644),
            ("data/noarch/support/postinst.sh", b"#!/bin/sh\n", 0o755)]);
        // A script that would leave a trace if it ever ran.
        let mut installer = b"#!/bin/sh\ntouch /tmp/ps5-launcher-gog-script-ran\nexit 0\n".to_vec();
        installer.resize(4096, b'#'); installer.extend(&data);
        fs::write(req.source.join("gog_celeste_1.4.sh"), &installer).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert!(p.file_name().unwrap().to_str().unwrap().starts_with("Celeste-"), "{}", p.display());
        let m = manifest(&p);
        assert_eq!((m.launch.as_str(), m.kind, m.topic), ("start.sh", crate::pcgame::Kind::Script, 1));
        assert!(executable(&p.join("start.sh")) && executable(&p.join("game/Celeste")));
        assert!(!Path::new("/tmp/ps5-launcher-gog-script-ran").exists());
        assert_eq!(fs::read(req.source.join("gog_celeste_1.4.sh")).unwrap(), installer, "the download is kept as it was");
        clean(&req);
    }

    #[test]
    fn pc_folder_release_gets_its_executable_flags_back() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Munin");
        fs::create_dir_all(req.source.join("Munin/Munin_Data")).unwrap();
        fs::write(req.source.join("Munin/Munin.x86_64"), elf_program()).unwrap();
        fs::write(req.source.join("Munin/UnityCrashHandler64"), elf_program()).unwrap();
        fs::write(req.source.join("Munin/Munin_Data/level0"), b"level").unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        let m = manifest(&p);
        assert_eq!((m.launch.as_str(), m.kind), ("Munin.x86_64", crate::pcgame::Kind::Native), "the wrapping folder is the game's folder");
        assert!(executable(&p.join("Munin.x86_64")) && !executable(&p.join("Munin_Data/level0")));
        clean(&req);
    }

    #[test]
    fn pc_archive_keeps_symlinks_inside_the_game_and_refuses_others() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Some Game");
        let good = tar(&[("Game/start.sh", b"#!/bin/sh\n./bin/game\n", b'0', ""), ("Game/bin/game", &elf_program(), b'0', ""),
            ("Game/lib/libfoo.so.1", b"library", b'0', ""), ("Game/lib/libfoo.so", b"", b'2', "libfoo.so.1")]);
        fs::write(req.source.join("Game.tar"), &good).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read_link(p.join("lib/libfoo.so")).unwrap(), Path::new("libfoo.so.1"));
        assert_eq!(manifest(&p).launch, "start.sh");
        assert!(executable(&p.join("start.sh")) && executable(&p.join("bin/game")), "torrent and tar modes are 0644: restored");
        clean(&req);
        for bad in ["../../../etc", "/etc/passwd", "a/../../.."] {
            let (_t, mut req) = fixture();
            pc(&mut req, "Some Game");
            fs::write(req.source.join("Game.tar"), tar(&[("Game/start.sh", b"#!/bin/sh\n", b'0', ""), ("Game/evil", b"", b'2', bad)])).unwrap();
            let error = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err();
            assert!(format!("{error:#}").contains("outside the game"), "{bad}: {error:#}");
            assert!(!req.destination.exists() || fs::read_dir(&req.destination).unwrap().count() == 0, "nothing published");
        }
    }

    #[test]
    fn pc_release_unpacks_its_own_archives_and_copies_the_rest() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Some Game");
        fs::create_dir_all(req.source.join("Game/data")).unwrap();
        fs::write(req.source.join("Game/game.x86_64"), elf_program()).unwrap();
        // The game's own data archive: copied, never unpacked.
        let data = zip(&[("level.dat", b"level", 0o644)]);
        fs::write(req.source.join("Game/data/levels.zip"), &data).unwrap();
        // A patch shipped next to the game: unpacked into it.
        fs::write(req.source.join("Patch.zip"), zip(&[("Game/patch.txt", b"patched", 0o644)])).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("patch.txt")).unwrap(), b"patched");
        assert_eq!(fs::read(p.join("data/levels.zip")).unwrap(), data);
        assert!(!p.join("data/level.dat").exists() && !p.join("Patch.zip").exists());
        assert_eq!(manifest(&p).launch, "game.x86_64");
        clean(&req);
    }

    #[test]
    fn pc_data_archive_beside_the_game_is_copied_not_unpacked() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Some Game");
        fs::create_dir_all(req.source.join("Game")).unwrap();
        fs::write(req.source.join("Game/game.x86_64"), elf_program()).unwrap();
        fs::write(req.source.join("Game/readme.txt"), b"read me").unwrap();
        // Read by the game at run time: it must arrive as it is.
        let data = tar(&[("level.dat", b"level", b'0', "")]);
        fs::write(req.source.join("Game/data.tar"), &data).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("data.tar")).unwrap(), data);
        assert!(!p.join("level.dat").exists());
        assert_eq!(manifest(&p).launch, "game.x86_64");
        clean(&req);
    }

    #[test]
    fn pc_archive_beside_its_start_script_is_unpacked() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Some Game");
        // The script starts what's in the archive: the archive is the release's packaging.
        fs::write(req.source.join("start.sh"), b"#!/bin/sh\ncd Game && ./game\n").unwrap();
        fs::write(req.source.join("Game.tar"), tar(&[("Game/game", &elf_program(), b'0', "")])).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert!(executable(&p.join("Game/game")), "the game was unpacked");
        assert!(!p.join("Game.tar").exists());
        assert_eq!(manifest(&p).launch, "start.sh");
        clean(&req);
    }

    #[test]
    fn pc_data_archive_beside_a_start_script_is_copied() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Some Game");
        // A script-run game (Python, LÖVE…) that reads data.tar as it is.
        fs::write(req.source.join("start.sh"), b"#!/bin/sh\npython3 game.py data.tar\n").unwrap();
        fs::write(req.source.join("game.py"), b"print('game')\n").unwrap();
        let data = tar(&[("level.dat", b"level", b'0', "")]);
        fs::write(req.source.join("data.tar"), &data).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("data.tar")).unwrap(), data);
        assert!(!p.join("level.dat").exists());
        assert_eq!(manifest(&p).launch, "start.sh");
        clean(&req);
    }

    #[test]
    fn pc_data_archive_with_helper_scripts_beside_the_game_is_copied() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Some Game");
        fs::write(req.source.join("game.x86_64"), elf_program()).unwrap();
        // The game's data, with a helper script among it: still the game's data.
        let data = tar(&[("tools/convert.sh", b"#!/bin/sh\n", b'0', ""), ("level.dat", b"level", b'0', "")]);
        fs::write(req.source.join("data.tar"), &data).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("data.tar")).unwrap(), data);
        assert!(!p.join("level.dat").exists());
        assert_eq!(manifest(&p).launch, "game.x86_64");
        clean(&req);
    }

    #[test]
    fn pc_classic_multipart_rar_is_one_archive() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Some Game");
        for name in ["game.rar", "game.r00", "game.r01", "readme.txt", "other.zip"] {
            fs::write(req.source.join(name), b"x").unwrap();
        }
        let source = Source::inspect(&req, &AtomicBool::new(false)).unwrap();
        let mut groups: Vec<Vec<String>> = pc_archives(&source, &source.plan).unwrap().into_iter().map(|a| a.volumes).collect();
        groups.sort();
        assert_eq!(groups, vec![vec!["game.rar".to_string(), "game.r00".into(), "game.r01".into()], vec!["other.zip".to_string()]]);
    }

    #[test]
    fn pc_release_without_anything_to_start_is_refused() {
        let (_t, mut req) = fixture();
        pc(&mut req, "Soundtrack");
        fs::write(req.source.join("track01.flac"), b"audio").unwrap();
        let error = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err();
        assert!(format!("{error:#}").contains("Nothing in this release can start the game"), "{error:#}");
    }

    /// Whichever 7-Zip CLI is installed: `7zz` (Homebrew sevenzip), `7z` or `7za` (p7zip).
    fn seven_zip() -> &'static str {
        ["7zz", "7z", "7za"]
            .into_iter()
            .find(|bin| std::process::Command::new(bin).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok())
            .expect("a 7-Zip CLI (7zz, 7z or 7za) is needed for the archive fixtures")
    }

    #[test]
    fn ps4_folder_release_installs_game_and_skips_its_update() {
        use crate::sfo::{build, Value};
        let (_t, mut req) = fixture();
        req.expected_ids = vec!["CUSA04118".into()];
        let sfo = |cat: &str| build(&[("CATEGORY", Value::Text(cat.into())), ("TITLE_ID", Value::Text("CUSA04118".into())), ("TITLE", Value::Text("Firewatch".into()))]);
        for (dir, cat) in [("CUSA04118", "gd"), ("CUSA04118-patch", "gp")] {
            let d = req.source.join(dir);
            fs::create_dir_all(d.join("sce_sys")).unwrap();
            fs::write(d.join("sce_sys/param.sfo"), sfo(cat)).unwrap();
            fs::write(d.join("eboot.bin"), b"generated bytes").unwrap();
        }
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("CUSA04118-"));
        let patch = PathBuf::from(format!("{}-patch", p.display()));
        assert!(patch.join("sce_sys/param.sfo").is_file(), "the update is kept beside the game, where shadPS4 looks");
        let found = crate::library::scan(&[req.destination.clone()]);
        assert_eq!(found.len(), 1, "the update is not listed as a second game");
        assert_eq!(found[0].platform, crate::platform::Platform::Ps4);
        clean(&req);
    }

    #[test]
    fn ps4_update_inside_the_game_folder_moves_beside_it() {
        use crate::sfo::{build, Value};
        let (_t, mut req) = fixture();
        req.expected_ids = vec!["CUSA04118".into()];
        let sfo = |cat: &str| build(&[("CATEGORY", Value::Text(cat.into())), ("TITLE_ID", Value::Text("CUSA04118".into()))]);
        for (dir, cat) in [("", "gd"), ("Update", "gp")] {
            let d = req.source.join(dir);
            fs::create_dir_all(d.join("sce_sys")).unwrap();
            fs::write(d.join("sce_sys/param.sfo"), sfo(cat)).unwrap();
            fs::write(d.join("eboot.bin"), b"generated bytes").unwrap();
        }
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert!(!p.join("Update").exists());
        assert!(PathBuf::from(format!("{}-patch", p.display())).join("sce_sys/param.sfo").is_file());
        clean(&req);
    }

    /// A generated PS4 package in the release folder, and shadPS4's add-on folder under `t`.
    fn ps4(req: &Request, t: &Path, name: &str, kind: crate::emulator::shadps4::pkg::Kind, files: &[(&str, &[u8])], label: &str) -> Vec<u8> {
        crate::emulator::shadps4::install::TEST_ADDONS.with(|c| *c.borrow_mut() = Some(t.join("addcont")));
        let bytes = crate::emulator::shadps4::pkg::build::package(&crate::emulator::shadps4::pkg::build::Spec { files, kind, label, compressed: true, ..Default::default() });
        fs::write(req.source.join(name), &bytes).unwrap();
        bytes
    }
    const PS4_GAME: [(&str, &[u8]); 2] = [("eboot.bin", b"game"), ("data/level", b"level")];

    #[test]
    fn pkg_game_update_and_dlc_install_without_replacing_existing_addons() {
        use crate::emulator::shadps4::pkg::Kind;
        let (t, mut req) = fixture();
        req.expected_ids = vec!["CUSA12345".into()];
        let game = ps4(&req, t.path(), "game.pkg", Kind::Game, &PS4_GAME, "");
        ps4(&req, t.path(), "update.pkg", Kind::Update, &[("eboot.bin", b"patch")], "");
        ps4(&req, t.path(), "kept.pkg", Kind::Dlc, &[("data", b"new")], "SEASONPASS");
        ps4(&req, t.path(), "extra.pkg", Kind::Dlc, &[("data", b"extra")], "EXTRA");
        let addons = t.path().join("addcont/CUSA12345");
        fs::create_dir_all(addons.join("SEASONPASS")).unwrap();
        fs::write(addons.join("SEASONPASS/data"), b"mine").unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), b"game");
        assert_eq!(fs::read(p.join("data/level")).unwrap(), b"level");
        assert!(crate::sfo::parse(&fs::read(p.join("sce_sys/param.sfo")).unwrap()).is_some());
        assert_eq!(fs::read(format!("{}-patch/eboot.bin", p.display())).unwrap(), b"patch");
        assert_eq!(fs::read(addons.join("SEASONPASS/data")).unwrap(), b"mine", "an existing add-on is never replaced");
        assert_eq!(fs::read(addons.join("EXTRA/data")).unwrap(), b"extra", "a new add-on is installed");
        let leftovers: Vec<_> = fs::read_dir(t.path().join("addcont")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("CUSA12345")], "no staging folder is left behind");
        assert_eq!(fs::read(req.source.join("game.pkg")).unwrap(), game, "the download is kept unchanged");
        clean(&req);
    }

    #[test]
    fn of_two_updates_the_later_one_wins_on_shared_files() {
        // As the extractor did: it wrote both into one -patch folder, in release order.
        use crate::emulator::shadps4::pkg::Kind;
        let (t, mut req) = fixture();
        req.expected_ids = vec!["CUSA12345".into()];
        ps4(&req, t.path(), "game.pkg", Kind::Game, &PS4_GAME, "");
        ps4(&req, t.path(), "a fix.pkg", Kind::Update, &[("eboot.bin", b"first"), ("only-a", b"a")], "");
        ps4(&req, t.path(), "b language.pkg", Kind::Update, &[("eboot.bin", b"second"), ("only-b", b"b")], "");
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        let patch = PathBuf::from(format!("{}-patch", p.display()));
        assert_eq!(fs::read(patch.join("eboot.bin")).unwrap(), b"second");
        assert_eq!((fs::read(patch.join("only-a")).unwrap(), fs::read(patch.join("only-b")).unwrap()), (b"a".to_vec(), b"b".to_vec()));
        clean(&req);
    }

    #[test]
    fn pkg_failure_leaves_game_folder_and_addons_untouched() {
        use crate::emulator::shadps4::pkg::Kind;
        for case in ["damaged dlc", "damaged game data", "no game", "retail"] {
            let (t, mut req) = fixture();
            req.expected_ids = vec!["CUSA12345".into()];
            let addons = t.path().join("addcont/CUSA12345");
            fs::create_dir_all(addons.join("SEASONPASS")).unwrap();
            fs::write(addons.join("SEASONPASS/data"), b"mine").unwrap();
            // Compresses to about half, so the end of the package is this file's compressed blocks.
            let mut seed = 1u64;
            let big: Vec<u8> = (0..300_000).map(|_| { seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1); (seed >> 60) as u8 }).collect();
            match case {
                "damaged dlc" => {
                    ps4(&req, t.path(), "game.pkg", Kind::Game, &PS4_GAME, "");
                    let mut dlc = ps4(&req, t.path(), "dlc.pkg", Kind::Dlc, &[("data", &big)], "OTHER");
                    let n = dlc.len();
                    dlc[n - 0x10000..n - 0xF000].fill(0); // inside the file's compressed blocks
                    fs::write(req.source.join("dlc.pkg"), &dlc).unwrap();
                }
                "damaged game data" => {
                    let mut game = ps4(&req, t.path(), "game.pkg", Kind::Game, &[("eboot.bin", b"game"), ("big", &big)], "");
                    let n = game.len();
                    game[n - 0x10000..n - 0xF000].fill(0);
                    fs::write(req.source.join("game.pkg"), &game).unwrap();
                }
                "no game" => { ps4(&req, t.path(), "update.pkg", Kind::Update, &[("eboot.bin", b"patch")], ""); }
                _ => {
                    crate::emulator::shadps4::install::TEST_ADDONS.with(|c| *c.borrow_mut() = Some(t.path().join("addcont")));
                    let spec = crate::emulator::shadps4::pkg::build::Spec { files: &PS4_GAME, retail: true, ..Default::default() };
                    fs::write(req.source.join("game.pkg"), crate::emulator::shadps4::pkg::build::package(&spec)).unwrap();
                }
            }
            let err = format!("{:#}", run(&req, &AtomicBool::new(false), &|_,_,_| {}).err().unwrap_or_else(|| panic!("{case}: installed")));
            let expected: &[&str] = match case {
                "damaged dlc" => &["dlc.pkg", "Damaged compressed block"],
                "damaged game data" => &["game.pkg", "Damaged compressed block"],
                "no game" => &["no game PKG"],
                _ => &["game.pkg", "isn't a fake package"],
            };
            assert!(expected.iter().all(|e| err.contains(e)), "{case}: {err}");
            // Refused before the destination opens (no game, retail), or published nothing.
            assert!(!req.destination.exists() || fs::read_dir(&req.destination).unwrap().count() == 0, "{case}: nothing published, stage removed");
            assert_eq!(fs::read(addons.join("SEASONPASS/data")).unwrap(), b"mine", "{case}");
            assert_eq!(fs::read_dir(t.path().join("addcont")).unwrap().count(), 1, "{case}: add-on folder unchanged");
            assert_eq!(fs::read_dir(&addons).unwrap().count(), 1, "{case}");
        }
    }

    /// End to end with a real PS4 PKG (not in the repo):
    /// PKG_FIXTURE=…/game.pkg PKG_TITLE_ID=CUSA… cargo test -- --ignored pkg_release
    #[test]
    #[ignore]
    fn pkg_release_installs_extracted_game() {
        let (Ok(pkg), Ok(tid)) = (std::env::var("PKG_FIXTURE"), std::env::var("PKG_TITLE_ID")) else { return };
        let (t, mut req) = fixture();
        req.expected_ids = if crate::psn::valid_title_id(&tid) { vec![tid.clone()] } else { vec![] };
        fs::copy(&pkg, req.source.join("game.pkg")).unwrap();
        let result = install(&req, &AtomicBool::new(false), &|_, _, _| {}, &|stage, root, leaf, title| {
            assert_eq!(title, tid);
            stage.publish(root, leaf)
        });
        if !crate::psn::valid_title_id(&tid) {
            // Homebrew (e.g. APOL00004): extracted and inspected, then refused as not a game.
            let err = result.unwrap_err().to_string();
            assert!(err.contains("Invalid title ID in the game's metadata"), "{err}");
            assert_eq!(fs::read_dir(&req.destination).unwrap().count(), 0, "nothing published, stage cleaned");
            return;
        }
        result.unwrap();
        let game = req.destination.join(format!("{tid}-{}", &KEY[..12]));
        assert!(game.join("eboot.bin").is_file() && game.join("sce_sys/param.sfo").is_file());
        assert!(req.source.join("game.pkg").is_file(), "the download is kept for seeding");
        drop(t);
    }

    #[test]
    fn exfat_image_release_installs_game_and_keeps_image() {
        for scatter in [false, true] {
            let (_t, req) = fixture();
            let image = crate::exfat::build::image(&[("PPSA12345-app0/sce_sys/param.json", PARAM), ("PPSA12345-app0/eboot.bin", b"fixture binary bytes"), ("PPSA12345-app0/data", b"data")], scatter);
            let path = req.source.join("PPSA12345.exfat");
            fs::write(&path, &image).unwrap();
            let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
            assert_eq!(fs::read(p.join("sce_sys/param.json")).unwrap(), PARAM);
            assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), b"fixture binary bytes");
            assert_eq!(fs::read(&path).unwrap(), image, "the downloaded image is kept unchanged");
            assert_eq!(crate::library::scan(&[req.destination.clone()]).len(), 1); clean(&req);
        }
        // A .exfat file that isn't an image, and two images, are refused before anything is written.
        let (_t, req) = fixture();
        fs::write(req.source.join("game.exfat"), b"not a disk image").unwrap();
        assert!(run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err().to_string().contains("not an exFAT"));
        let image = crate::exfat::build::image(&[("eboot.bin", b"x")], false);
        fs::write(req.source.join("game.exfat"), &image).unwrap();
        fs::write(req.source.join("update.exfat"), &image).unwrap();
        assert!(run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err().to_string().contains("2 disk images"));
        unpublished(&req);
    }

    fn pkg_files() -> [(&'static str, &'static [u8]); 3] { [("eboot.bin", b"fixture binary bytes"), ("data", b"data"), ("sce_module/a.prx", b"module")] }

    /// Installs a real downloaded release folder into a scratch folder; the release is only read.
    /// PS5_TEST_RELEASE=<download folder> PS5_TEST_DEST=<new folder> cargo test --release real_release -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real release in PS5_TEST_RELEASE"]
    fn real_release_installs() {
        let source = PathBuf::from(std::env::var("PS5_TEST_RELEASE").expect("set PS5_TEST_RELEASE"));
        let destination = PathBuf::from(std::env::var("PS5_TEST_DEST").expect("set PS5_TEST_DEST"));
        let key = fs::read_to_string(source.join(OWNED)).unwrap().trim().to_string();
        // A console release (the platform only matters for PC games).
        let req = Request { key, topic: 0, name: "real release".into(), source, destination, expected_ids: vec![], platform: Default::default() };
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_or_else(|e| panic!("{e:#}"));
        let game = crate::library::read_param(&p).expect("the installed folder is a game");
        eprintln!("installed {} ({}) at {}", game.name, game.title_id, p.display());
    }

    #[test]
    fn extracted_game_wins_over_packages_next_to_it() {
        // A release can ship an extracted game with small DLC unlocker packages beside it.
        let dlc = crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files: &[("data", b"dlc")], ..Default::default() });
        for (case, bytes) in [("PS5 DLC package", dlc), ("damaged package", b"\x7fFIH\0\0damaged".to_vec()), ("PS4 package", b"\x7fCNTfixture".to_vec())] {
            let (_t, req) = fixture();
            game(&req.source);
            fs::create_dir(req.source.join("_DUPLEX_")).unwrap();
            fs::write(req.source.join("_DUPLEX_/unlocker.pkg"), &bytes).unwrap();
            let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_or_else(|e| panic!("{case}: {e:#}"));
            assert_eq!(fs::read(p.join("sce_sys/param.json")).unwrap(), PARAM, "{case}");
            assert_eq!(fs::read(p.join("_DUPLEX_/unlocker.pkg")).unwrap(), bytes, "{case}: copied as it is");
            clean(&req);
        }
    }

    #[test]
    fn game_package_wins_over_a_folder_that_isnt_a_game() {
        // Loose metadata (no eboot.bin) next to a debug game package is not an extracted game.
        let (_t, req) = fixture();
        let bytes = crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files: &pkg_files(), ..Default::default() });
        fs::write(req.source.join("PPSA12345.pkg"), &bytes).unwrap();
        fs::create_dir_all(req.source.join("art/sce_sys")).unwrap();
        fs::write(req.source.join("art/sce_sys/param.json"), PARAM).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), b"fixture binary bytes");
        clean(&req);
    }

    #[test]
    fn a_pkg_file_inside_a_game_package_is_part_of_the_game() {
        let (_t, req) = fixture();
        let files: [(&str, &[u8]); 2] = [("eboot.bin", b"fixture binary bytes"), ("data/inner.pkg", b"not a release file")];
        let bytes = crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files: &files, ..Default::default() });
        fs::write(req.source.join("PPSA12345.pkg"), &bytes).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("data/inner.pkg")).unwrap(), b"not a release file");
        clean(&req);
    }

    #[test]
    fn debug_pkg_release_installs_game_and_keeps_package() {
        for entropy in [false, true] {
            let (_t, req) = fixture();
            let bytes = crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files: &pkg_files(), entropy, ..Default::default() });
            let path = req.source.join("PPSA12345.pkg");
            fs::write(&path, &bytes).unwrap();
            let seen = Mutex::new(Vec::new());
            let p = run(&req, &AtomicBool::new(false), &|s, d, t| seen.lock().unwrap().push((s, d, t))).unwrap();
            assert_eq!(fs::read(p.join("sce_sys/param.json")).unwrap(), PARAM, "param.json comes from the package metadata");
            assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), b"fixture binary bytes");
            assert_eq!(fs::read(p.join("sce_module/a.prx")).unwrap(), b"module");
            assert_eq!(fs::read(&path).unwrap(), bytes, "the downloaded package is kept unchanged");
            let seen = seen.into_inner().unwrap();
            let last = seen.iter().rfind(|s| s.0 == State::Extracting).unwrap();
            assert_eq!(last.1, last.2, "progress reaches the total");
            assert_eq!(crate::library::scan(&[req.destination.clone()]).len(), 1); clean(&req);
        }
    }

    #[test]
    fn pkg_release_picks_the_base_game_and_refuses_ambiguity() {
        let build = |files: &[(&str, &[u8])]| crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files, ..Default::default() });
        let (_t, req) = fixture();
        // The base game sits at the top; a backport overlay beside a DLC pack sits in sub-folders.
        fs::write(req.source.join("game.pkg"), build(&[("eboot.bin", b"base game"), ("data", b"d")])).unwrap();
        fs::create_dir(req.source.join("Backport")).unwrap();
        fs::write(req.source.join("Backport/game_bp.pkg"), build(&[("eboot.bin", b"backport overlay")])).unwrap();
        fs::create_dir(req.source.join("DLC")).unwrap();
        fs::write(req.source.join("DLC/dlc.pkg"), build(&[("dlc/pak", b"content")])).unwrap();
        let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap();
        assert_eq!(fs::read(p.join("eboot.bin")).unwrap(), b"base game");
        clean(&req);

        // Two games at the same depth are never guessed between.
        let (_t, req) = fixture();
        fs::write(req.source.join("one.pkg"), build(&[("eboot.bin", b"1")])).unwrap();
        fs::write(req.source.join("two.pkg"), build(&[("eboot.bin", b"2")])).unwrap();
        let err = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err().to_string();
        assert!(err.contains("one.pkg") && err.contains("two.pkg"), "{err}");
        unpublished(&req);

        // Packages that hold no game are not installable.
        let (_t, req) = fixture();
        fs::write(req.source.join("dlc.pkg"), build(&[("dlc/pak", b"content")])).unwrap();
        assert!(run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err().to_string().contains("Unsupported PKG"));
        unpublished(&req);
    }

    #[test]
    fn retail_and_damaged_pkg_are_refused_before_anything_is_written() {
        let (_t, req) = fixture();
        let retail = crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files: &pkg_files(), retail: true, ..Default::default() });
        fs::write(req.source.join("game.pkg"), &retail).unwrap();
        assert!(format!("{:#}", run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err()).contains("retail"));
        unpublished(&req);
        let good = crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files: &pkg_files(), ..Default::default() });
        fs::write(req.source.join("game.pkg"), &good[..good.len() - 40]).unwrap();
        assert!(format!("{:#}", run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err()).contains("download"));
        unpublished(&req);
    }

    #[test]
    fn a_broken_base_package_never_lets_the_overlay_install() {
        let (_t, req) = fixture();
        let build = |files: &[(&str, &[u8])]| crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files, ..Default::default() });
        let base = build(&[("eboot.bin", b"base game")]);
        fs::write(req.source.join("game.pkg"), &base[..base.len() - 40]).unwrap();
        fs::create_dir(req.source.join("Backport")).unwrap();
        fs::write(req.source.join("Backport/bp.pkg"), build(&[("eboot.bin", b"overlay")])).unwrap();
        assert!(format!("{:#}", run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err()).contains("game.pkg"));
        unpublished(&req);
    }

    #[test]
    fn cancelling_a_pkg_install_cleans_up_and_keeps_the_package() {
        let (_t, req) = fixture();
        let bytes = crate::emulator::kytyps5::pkg::build::package(&crate::emulator::kytyps5::pkg::build::Spec { files: &pkg_files(), ..Default::default() });
        fs::write(req.source.join("game.pkg"), &bytes).unwrap();
        let cancel = AtomicBool::new(false);
        let err = run(&req, &cancel, &|s, d, _| { if s == State::Extracting && d > 0 { cancel.store(true, Ordering::Release); } }).unwrap_err();
        assert!(err.to_string().contains("Cancelled"), "{err:#}");
        assert_eq!(fs::read(req.source.join("game.pkg")).unwrap(), bytes);
        unpublished(&req);
    }

    #[test]
    fn zip_and_7z_generated_fixtures_and_split_7z() {
        for format in ["zip", "7z", "split"] {
            let (t, req) = fixture(); let input = t.path().join("generated"); game(&input);
            let archive = req.source.join(if format == "zip" { "fixture.zip" } else { "fixture.7z" });
            let mut command = std::process::Command::new(seven_zip());
            command.current_dir(&input).args(["a", "-bd", "-y"]);
            command.arg(if format == "zip" { "-tzip" } else { "-t7z" });
            if format == "split" { command.arg("-v128b"); }
            let result = command.arg(&archive).args(["sce_sys", "eboot.bin", "data"]).output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            let before: Vec<_> = fs::read_dir(&req.source).unwrap().map(|e| { let p=e.unwrap().path(); let d=Sha256::digest(fs::read(&p).unwrap()); (p,d) }).collect();
            let p = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_or_else(|e| {
                // Say which fixture and which 7-Zip made it: archive tools differ between systems.
                let mut files: Vec<_> = fs::read_dir(&req.source).unwrap().map(|e| {
                    let p = e.unwrap().path();
                    (p.file_name().unwrap().to_string_lossy().into_owned(), fs::metadata(&p).map(|m| m.len()).unwrap_or(0))
                }).collect();
                files.sort();
                let banner = std::process::Command::new(seven_zip()).output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).lines().find(|l| l.contains("7-Zip") || l.contains("p7zip")).unwrap_or("").trim().to_string())
                    .unwrap_or_default();
                panic!("{format} fixture failed to install: {e:#}\n7-Zip: {banner}\nsource files: {files:?}");
            });
            assert_eq!(fs::read(p.join("sce_sys/param.json")).unwrap(), PARAM);
            assert_eq!(fs::read(p.join("data")).unwrap(), b"fixture contents");
            for (p,d) in before { assert_eq!(Sha256::digest(fs::read(p).unwrap()), d); } clean(&req);
        }
    }

    #[test]
    fn malicious_archive_headers_rejected_before_extraction() {
        let bad = vec![
            tar(&[("../outside", b"x", b'0', "")]), tar(&[("/absolute", b"x", b'0', "")]),
            tar(&[("C:\\outside", b"x", b'0', "")]), tar(&[("a\\..\\outside", b"x", b'0', "")]),
            tar(&[("link", b"", b'2', "outside")]), tar(&[("hard", b"", b'1', "outside")]),
            tar(&[("fifo", b"", b'6', "")]), tar(&[("duplicate", b"a", b'0', ""), ("duplicate", b"b", b'0', "")]),
            tar(&[("a", b"a", b'0', ""), ("a/b", b"b", b'0', "")]),
            tar(&[("a/b", b"b", b'0', ""), ("a", b"a", b'0', "")]),
            tar(&[("same\\path", b"a", b'0', ""), ("same/path", b"b", b'0', "")]),
        ];
        for bytes in bad {
            let (_t, req) = fixture(); fs::write(req.source.join("bad.tar"), bytes).unwrap();
            assert!(run(&req, &AtomicBool::new(false), &|_,_,_| {}).is_err());
            assert!(!req.destination.exists()); clean(&req);
        }
    }

    #[test]
    fn metadata_missing_eboot_expected_id_multiple_games_and_pkg() {
        for case in ["json", "title", "missing", "empty", "mismatch", "multiple", "pkg"] {
            let (_t, mut req) = fixture(); game(&req.source.join("game"));
            match case {
                "json" => fs::write(req.source.join("game/sce_sys/param.json"), b"not json").unwrap(),
                "title" => fs::write(req.source.join("game/sce_sys/param.json"), br#"{"titleId":"BOGUS"}"#).unwrap(),
                "missing" => fs::remove_file(req.source.join("game/eboot.bin")).unwrap(),
                "empty" => fs::write(req.source.join("game/eboot.bin"), b"").unwrap(),
                "mismatch" => req.expected_ids = vec!["PPSA99999".into()],
                "multiple" => game(&req.source.join("second")),
                "pkg" => { fs::remove_dir_all(req.source.join("game")).unwrap(); fs::write(req.source.join("fixture.pkg"), b"fixture").unwrap(); },
                _ => unreachable!(),
            }
            let err = run(&req, &AtomicBool::new(false), &|_,_,_| {}).unwrap_err();
            if case == "pkg" { assert!(err.to_string().contains("PKG"), "{err:#}"); }
            clean(&req); assert!(crate::library::scan(&[req.destination]).is_empty());
        }
    }

    #[test]
    fn cancel_retains_source_and_only_cleans_owned_stage() {
        let (_t, req) = fixture(); game(&req.source);
        fs::create_dir_all(req.destination.join(".ps5-install-unowned")).unwrap();
        fs::write(req.destination.join(".ps5-install-unowned/keep"), b"keep").unwrap();
        let cancel = AtomicBool::new(false);
        let result = run(&req, &cancel, &|s,d,_| { if s == State::Extracting && d > 0 { cancel.store(true, Ordering::Release); } });
        assert!(result.is_err()); assert_eq!(fs::read(req.destination.join(".ps5-install-unowned/keep")).unwrap(), b"keep");
        assert_eq!(fs::read(req.source.join("sce_sys/param.json")).unwrap(), PARAM);
        assert_eq!(fs::read_dir(&req.destination).unwrap().count(), 1);
    }

    #[test]
    fn source_links_markers_and_destination_symlink_rejected() {
        for case in ["symlink", "hardlink", "marker", "destination"] {
            let (t, req) = fixture(); game(&req.source);
            match case {
                "symlink" => symlink(req.source.join("data"), req.source.join("link")).unwrap(),
                "hardlink" => fs::hard_link(req.source.join("data"), req.source.join("link")).unwrap(),
                "marker" => fs::write(req.source.join(OWNED), b"wrong key").unwrap(),
                "destination" => { let outside=t.path().join("outside"); fs::create_dir(&outside).unwrap(); symlink(&outside, &req.destination).unwrap(); },
                _ => unreachable!(),
            }
            assert!(run(&req, &AtomicBool::new(false), &|_,_,_| {}).is_err());
        }
    }

    #[test]
    fn multipart_gap_duplicate_and_multiple_archive_detection() {
        let plan = |names: &[&str]| {
            let mut p = Plan::default();
            for n in names { p.add(Entry::file((*n).into(), false, 1)).unwrap(); } p
        };
        for names in [vec!["x.part02.rar"], vec!["x.part1.rar", "x.part3.rar"], vec!["x.7z.001", "x.7z.003"], vec!["x.r00"], vec!["x.rar", "y.zip"], vec!["x.part1.rar", "x.part01.rar"]] {
            assert!(volumes(&plan(&names)).is_err(), "{names:?}");
        }
        assert_eq!(volumes(&plan(&["x.part02.rar", "x.part01.rar"])).unwrap().unwrap(), ["x.part01.rar", "x.part02.rar"]);
        assert_eq!(volumes(&plan(&["x.r01", "x.rar", "x.r00"])).unwrap().unwrap(), ["x.rar", "x.r00", "x.r01"]);
    }

    #[test]
    fn interrupted_restore_no_worker_no_extraction_and_independent_records() {
        let (t, req) = fixture(); game(&req.source);
        let store=t.path().join("records");
        let job=Record { key: KEY.into(), topic: 1, name: "fixture".into(), state: State::Extracting, done: 1, total: 10, error: String::new(), path: None, title_id: String::new() };
        save_records(&store, &[job]).unwrap();
        let manager=Manager::at(store.clone());
        assert_eq!(manager.snapshot()[0].state, State::Failed);
        assert_eq!(manager.snapshot()[0].error, "Interrupted; retry");
        assert!(manager.worker.lock().unwrap().is_none()); assert!(!req.destination.exists());
        assert_eq!(load_records(&store).unwrap()[0].state, State::Failed);
        assert_eq!(fs::metadata(store.join("jobs.json")).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn manager_explicit_start_worker_shutdown_and_installed_persistence() {
        let (t, req) = fixture(); game(&req.source);
        let store=t.path().join("records"); let mut manager=Manager::at(store.clone());
        manager.start(req.clone()).unwrap();
        let deadline=Instant::now()+Duration::from_secs(10);
        while manager.snapshot()[0].state.active() {
            assert!(Instant::now()<deadline); thread::yield_now();
        }
        let snapshot=manager.snapshot(); assert_eq!(snapshot[0].state, State::Installed, "{:?}", snapshot[0]);
        assert_eq!(snapshot[0].progress(), 1.0); assert!(manager.start(req.clone()).is_err());
        manager.shutdown();
        let restored=Manager::at(store); assert_eq!(restored.snapshot()[0].state, State::Installed);
        assert!(restored.worker.lock().unwrap().is_none()); clean(&req);
    }

    #[test]
    fn safety_bounds_and_path_normalization() {
        assert_eq!(normal_path("a\\b/./c/").unwrap(), "a/b/c");
        for p in ["", "/a", "\\a", "C:/a", "a/../b", "a\\..\\b", "a\0b", OWNED] { assert!(normal_path(p).is_err(), "{p}"); }
        let mut plan=Plan::default();
        assert!(plan.add(Entry::file("sce_sys/param.json".into(), false, MAX_PARAM+1)).is_err());
        assert!(Plan::default().add(Entry::file("large".into(), false, MAX_FILE+1)).is_err());
        assert!(normal_path(&"a/".repeat(MAX_DEPTH+1)).is_err());
    }
}