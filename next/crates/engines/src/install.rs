use crate::containers::open_source;
use kernel::*;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_file: u64,
    pub max_total: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self { max_entries: 500_000, max_depth: 64, max_file: 256 << 30, max_total: 1 << 40 }
    }
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("cannot read the source: {0}")]
    Source(String),
    #[error("nothing in this source is recognised by an installed system")]
    Unrecognised,
    #[error("recognised by more than one system: {0:?}")]
    Ambiguous(Vec<String>),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("limit exceeded: {0}")]
    Limit(String),
    #[error("already installed: {0}")]
    Exists(String),
    #[error("io: {0}")]
    Io(String),
    #[error("cancelled")]
    Cancelled,
}
impl From<io::Error> for InstallError { fn from(e: io::Error) -> Self { InstallError::Io(e.to_string()) } }

/// Which games, in which system, are inside a tree.
pub fn recognise_all(reg: &Registry, tree: &dyn Tree) -> Result<Vec<(SystemId, Recognised)>, InstallError> {
    let mut found: Vec<(SystemId, Recognised)> = vec![];
    for s in reg.systems() {
        for r in s.recognise(tree) { found.push((s.descriptor().id.clone(), r)); }
    }
    // The same subtree claimed by two systems is ambiguous: never guess.
    for (i, (sa, ra)) in found.iter().enumerate() {
        for (sb, rb) in found.iter().skip(i + 1) {
            if sa != sb && ra.root == rb.root && ra.role == rb.role {
                return Err(InstallError::Ambiguous(vec![sa.to_string(), sb.to_string()]));
            }
        }
    }
    Ok(found)
}

struct Counting<'a, W: Write> { inner: W, written: u64, max: u64, total: &'a mut u64, max_total: u64 }
impl<W: Write> Write for Counting<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written += buf.len() as u64;
        *self.total += buf.len() as u64;
        if self.written > self.max || *self.total > self.max_total {
            return Err(io::Error::other("size limit exceeded"));
        }
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> { self.inner.flush() }
}

/// Extract the subtree `root` of `tree` into `dest`, staged and then committed. Nothing partial is left.
/// Limits are measured on bytes actually written, never on declared sizes.
pub fn extract(tree: &dyn Tree, root: &RelPath, dest: &Path, limits: &Limits, cancel: &CancelToken) -> Result<(), InstallError> {
    let entries = tree.entries()?;
    if entries.len() > limits.max_entries { return Err(InstallError::Limit("too many entries".into())); }
    let parent = dest.parent().ok_or_else(|| InstallError::Rejected("destination has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let stage = tempdir_in(parent, "stage")?;
    let result = (|| {
        let mut total = 0u64;
        for e in &entries {
            if cancel.is_cancelled() { return Err(InstallError::Cancelled); }
            let Some(rel) = e.path.strip_prefix(root) else { continue };
            if rel.is_root() { continue; }
            if rel.components().count() > limits.max_depth { return Err(InstallError::Limit("path too deep".into())); }
            match e.kind {
                EntryKind::Symlink | EntryKind::Other => {
                    return Err(InstallError::Rejected(format!("{} is not a regular file or folder", e.path.as_str())));
                }
                EntryKind::Dir => fs::create_dir_all(rel.to_path(&stage))?,
                EntryKind::File => {
                    let target = rel.to_path(&stage);
                    if let Some(p) = target.parent() { fs::create_dir_all(p)?; }
                    let f = File::create(&target)?;
                    let mut w = Counting { inner: f, written: 0, max: limits.max_file, total: &mut total, max_total: limits.max_total };
                    tree.read_to(&e.path, &mut w).map_err(|er| {
                        if er.to_string().contains("size limit") { InstallError::Limit(format!("{} is too large", e.path.as_str())) } else { er.into() }
                    })?;
                }
            }
        }
        commit(&stage, dest)
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

fn tempdir_in(parent: &Path, tag: &str) -> io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    loop {
        let p = parent.join(format!(".{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        match fs::create_dir(&p) { Ok(()) => return Ok(p), Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue, Err(e) => return Err(e) }
    }
}

fn commit(stage: &Path, dest: &Path) -> Result<(), InstallError> {
    if !dest.exists() {
        fs::rename(stage, dest)?;
        return Ok(());
    }
    // Merge into an existing folder (updates and DLC): move file by file, replacing.
    fn merge(from: &Path, to: &Path) -> io::Result<()> {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            let t = to.join(e.file_name());
            if e.file_type()?.is_dir() { merge(&e.path(), &t)?; } else { fs::rename(e.path(), &t)?; }
        }
        Ok(())
    }
    merge(stage, dest)?;
    Ok(())
}

pub const GAME_FILE: &str = ".launcher/game.json";

/// Install every game recognised in `source` under `games_dir/<system folder>/<key>/`.
pub fn install(reg: &Registry, source: &Path, games_dir: &Path, limits: &Limits, cancel: &CancelToken) -> Result<Vec<InstalledGame>, InstallError> {
    let containers: Vec<DynContainer> = {
        let mut v = crate::containers::builtin();
        v.extend(reg.containers().iter().cloned());
        v
    };
    let tree = open_source(&containers, source).map_err(|e| InstallError::Source(e.to_string()))?;
    // A tree that cannot even be listed safely is rejected up front, never reported as "unrecognised".
    tree.entries().map_err(|e| InstallError::Rejected(e.to_string()))?;
    let found = recognise_all(reg, tree.as_ref())?;
    if found.is_empty() { return Err(InstallError::Unrecognised); }
    let mut done = vec![];
    for (sys, rec) in found {
        let desc = reg.system(&sys).expect("recognised by a registered system").descriptor().clone();
        let game_root = games_dir.join(&desc.folder).join(rec.key.as_str());
        // The runner knows where each role belongs; with no runner installed, the game folder itself.
        let layout = reg.runners_for(&sys).first().map(|r| r.layout(rec.role, &rec.key)).unwrap_or_else(RelPath::root);
        if rec.role == Role::Game && game_root.exists() { return Err(InstallError::Exists(rec.key.to_string())); }
        extract(tree.as_ref(), &rec.root, &layout.to_path(&game_root), limits, cancel)?;
        let game = InstalledGame {
            game: GameRef { system: sys.clone(), key: rec.key.clone() },
            name: rec.name.clone(), root: game_root.clone(), extra: serde_json::Value::Null,
        };
        if rec.role == Role::Game {
            let meta = game_root.join(GAME_FILE);
            fs::create_dir_all(meta.parent().unwrap())?;
            fs::write(&meta, serde_json::to_vec_pretty(&game).map_err(|e| InstallError::Io(e.to_string()))?)?;
        }
        done.push(game);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use testkit::{toy, MemTree, ToyPlugin};

    fn reg() -> Registry { Registry::build(&[&ToyPlugin]).unwrap() }

    #[test]
    fn extract_commits_whole_tree_and_blocks_symlinks_and_traversal_by_construction() {
        let d = tempfile::tempdir().unwrap();
        let tree = MemTree::new("g").with("a/b.txt", "hi").with("c.txt", "yo");
        extract(&tree, &RelPath::root(), &d.path().join("out"), &Limits::default(), &CancelToken::new()).unwrap();
        assert_eq!(fs::read_to_string(d.path().join("out/a/b.txt")).unwrap(), "hi");
        // symlink rejected, nothing left behind
        let bad = MemTree::new("g").with("x", "1").with_entry(Entry { path: RelPath::new("l").unwrap(), size: 0, kind: EntryKind::Symlink });
        let r = extract(&bad, &RelPath::root(), &d.path().join("out2"), &Limits::default(), &CancelToken::new());
        assert!(matches!(r, Err(InstallError::Rejected(_))));
        assert!(!d.path().join("out2").exists());
        let leftovers: Vec<_> = fs::read_dir(d.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).collect();
        assert!(leftovers.is_empty(), "staging dir cleaned up");
    }

    #[test]
    fn limits_are_measured_on_written_bytes() {
        let d = tempfile::tempdir().unwrap();
        let tree = MemTree::new("g").with("big", &"x".repeat(100));
        let l = Limits { max_file: 10, ..Limits::default() };
        let r = extract(&tree, &RelPath::root(), &d.path().join("o"), &l, &CancelToken::new());
        assert!(matches!(r, Err(InstallError::Limit(_))), "{r:?}");
        assert!(!d.path().join("o").exists());
        let l = Limits { max_entries: 1, ..Limits::default() };
        let t2 = MemTree::new("g").with("a", "1").with("b", "2");
        assert!(matches!(extract(&t2, &RelPath::root(), &d.path().join("o2"), &l, &CancelToken::new()), Err(InstallError::Limit(_))));
    }

    #[test]
    fn installs_a_recognised_folder_and_records_it() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("alpha");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(toy::MARKER), "Alpha Game").unwrap();
        fs::write(src.join("data.bin"), "1234").unwrap();
        let games = d.path().join("games");
        let done = install(&reg(), &src, &games, &Limits::default(), &CancelToken::new()).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].name, "Alpha Game");
        assert!(games.join("toy/alpha/data.bin").exists());
        assert!(games.join("toy/alpha").join(GAME_FILE).exists());
        // again: refuses to overwrite
        assert!(matches!(install(&reg(), &src, &games, &Limits::default(), &CancelToken::new()), Err(InstallError::Exists(_))));
    }

    #[test]
    fn unrecognised_and_no_systems_degrade_cleanly() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("junk");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x"), "1").unwrap();
        assert!(matches!(install(&reg(), &src, d.path(), &Limits::default(), &CancelToken::new()), Err(InstallError::Unrecognised)));
        let empty = Registry::build(&[]).unwrap();
        assert!(matches!(install(&empty, &src, d.path(), &Limits::default(), &CancelToken::new()), Err(InstallError::Unrecognised)));
    }

    #[test]
    fn two_systems_claiming_the_same_tree_is_ambiguous() {
        struct Twin;
        impl Plugin for Twin {
            fn manifest(&self) -> Manifest { Manifest { id: "twin".into(), version: "1".into(), api: API_VERSION, permissions: Default::default() } }
            fn register(&self, r: &mut Registrar) {
                struct S(SystemDescriptor);
                impl System for S {
                    fn descriptor(&self) -> &SystemDescriptor { &self.0 }
                    fn recognise(&self, t: &dyn Tree) -> Vec<Recognised> {
                        if t.exists(&RelPath::new(toy::MARKER).unwrap()) { vec![Recognised { key: GameKey::new(t.name()), name: "x".into(), role: Role::Game, root: RelPath::root() }] } else { vec![] }
                    }
                }
                let mut d = testkit::toy::ToySystem::new().descriptor().clone();
                d.id = SystemId::new("twin-sys"); d.folder = "twin".into();
                r.system(Arc::new(S(d)));
            }
        }
        let reg = Registry::build(&[&ToyPlugin, &Twin]).unwrap();
        let tree = MemTree::new("g").with(toy::MARKER, "x");
        assert!(matches!(recognise_all(&reg, &tree), Err(InstallError::Ambiguous(_))));
    }

    #[test]
    fn tar_gz_round_trip_and_traversal_entry_is_refused() {
        use flate2::{write::GzEncoder, Compression};
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("alpha.tar.gz");
        {
            let enc = GzEncoder::new(File::create(&p).unwrap(), Compression::fast());
            let mut b = tar::Builder::new(enc);
            for (name, body) in [(toy::MARKER, "Alpha"), ("sub/f.bin", "data")] {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64); h.set_mode(0o644); h.set_cksum();
                b.append_data(&mut h, name, body.as_bytes()).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        let games = d.path().join("games");
        let done = install(&reg(), &p, &games, &Limits::default(), &CancelToken::new()).unwrap();
        assert_eq!(done[0].game.key.as_str(), "alpha");
        assert_eq!(fs::read_to_string(games.join("toy/alpha/sub/f.bin")).unwrap(), "data");

        // An archive entry that escapes the root is refused when listed.
        let evil = d.path().join("evil.tar.gz");
        {
            let enc = GzEncoder::new(File::create(&evil).unwrap(), Compression::fast());
            let mut raw = tar::Header::new_gnu();
            raw.set_size(1); raw.set_mode(0o644);
            let name = b"../escape.txt";
            raw.as_old_mut().name[..name.len()].copy_from_slice(name);
            raw.set_cksum();
            let mut b = tar::Builder::new(enc);
            b.append(&raw, &b"x"[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let r = install(&reg(), &evil, &games, &Limits::default(), &CancelToken::new());
        let msg = format!("{r:?}");
        assert!(r.is_err() && msg.contains("component"), "must fail on the path itself: {msg}");
        assert!(!d.path().join("escape.txt").exists());
    }
}
