use flate2::read::GzDecoder;
use kernel::*;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

fn bad(msg: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, msg.into()) }

/// A folder on disk. Symlinks are reported, never followed.
pub struct DirTree { root: PathBuf }
impl DirTree {
    pub fn open(root: &Path) -> io::Result<Self> {
        if !root.is_dir() { return Err(bad("not a directory")); }
        Ok(Self { root: root.to_path_buf() })
    }
    fn walk(&self, dir: &Path, rel: &RelPath, out: &mut Vec<Entry>, depth: usize) -> io::Result<()> {
        if depth > 128 { return Err(bad("directory nesting too deep")); }
        for e in fs::read_dir(dir)? {
            let e = e?;
            let name = e.file_name().into_string().map_err(|_| bad("non UTF-8 file name"))?;
            let p = rel.join(&RelPath::new(&name).map_err(|e| bad(e.to_string()))?);
            let md = fs::symlink_metadata(e.path())?;
            let ft = md.file_type();
            if ft.is_symlink() { out.push(Entry { path: p, size: 0, kind: EntryKind::Symlink }); }
            else if ft.is_dir() {
                out.push(Entry { path: p.clone(), size: 0, kind: EntryKind::Dir });
                self.walk(&e.path(), &p, out, depth + 1)?;
            } else if ft.is_file() { out.push(Entry { path: p, size: md.len(), kind: EntryKind::File }); }
            else { out.push(Entry { path: p, size: 0, kind: EntryKind::Other }); }
        }
        Ok(())
    }
}
impl Tree for DirTree {
    fn name(&self) -> String { self.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() }
    fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut v = vec![];
        self.walk(&self.root, &RelPath::root(), &mut v, 0)?;
        Ok(v)
    }
    fn read_to(&self, path: &RelPath, out: &mut dyn Write) -> io::Result<u64> {
        let p = path.to_path(&self.root);
        if fs::symlink_metadata(&p)?.file_type().is_symlink() { return Err(bad("refusing to read a symlink")); }
        io::copy(&mut File::open(p)?, out)
    }
}

/// `.tar.gz` / `.tgz`
pub struct TarGzContainer;
impl Container for TarGzContainer {
    fn sniff(&self, head: &[u8], file_name: &str) -> Confidence {
        let name = file_name.to_lowercase();
        let gz = head.starts_with(&[0x1f, 0x8b]);
        match (gz, name.ends_with(".tar.gz") || name.ends_with(".tgz")) {
            (true, true) => Confidence::Yes,
            (true, false) => Confidence::Maybe,
            _ => Confidence::No,
        }
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn Tree>> {
        Ok(Box::new(TarGzTree { path: path.to_path_buf() }))
    }
}

struct TarGzTree { path: PathBuf }
impl TarGzTree {
    fn archive(&self) -> io::Result<tar::Archive<GzDecoder<File>>> { Ok(tar::Archive::new(GzDecoder::new(File::open(&self.path)?))) }
}
impl Tree for TarGzTree {
    fn name(&self) -> String {
        let n = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        n.trim_end_matches(".gz").trim_end_matches(".tar").trim_end_matches(".tgz").to_string()
    }
    fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut out = vec![];
        for e in self.archive()?.entries()? {
            let e = e?;
            let t = e.header().entry_type();
            let raw = e.path()?.to_string_lossy().to_string();
            let raw = raw.strip_prefix("./").unwrap_or(&raw).to_string();
            if raw.is_empty() || raw == "." { continue; }
            let path = RelPath::new(&raw).map_err(|er| bad(format!("{raw}: {er}")))?;
            let kind = if t.is_file() { EntryKind::File } else if t.is_dir() { EntryKind::Dir }
                else if t.is_symlink() || t.is_hard_link() { EntryKind::Symlink } else { EntryKind::Other };
            out.push(Entry { path, size: e.header().size()?, kind });
        }
        Ok(out)
    }
    fn read_to(&self, path: &RelPath, out: &mut dyn Write) -> io::Result<u64> {
        for e in self.archive()?.entries()? {
            let mut e = e?;
            let raw = e.path()?.to_string_lossy().to_string();
            let raw = raw.strip_prefix("./").unwrap_or(&raw).to_string();
            if raw.trim_end_matches('/') == path.as_str() && e.header().entry_type().is_file() {
                let mut limited = (&mut e).take(u64::MAX);
                return io::copy(&mut limited, out);
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "entry not found"))
    }
}

/// Containers every build gets. Extensions add the rest.
pub fn builtin() -> Vec<DynContainer> { vec![std::sync::Arc::new(TarGzContainer)] }

/// Open a source path: a folder, or a file some container recognises.
pub fn open_source(containers: &[DynContainer], path: &Path) -> io::Result<Box<dyn Tree>> {
    if path.is_dir() { return Ok(Box::new(DirTree::open(path)?)); }
    let mut head = [0u8; 512];
    let n = File::open(path)?.read(&mut head)?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut best: Option<(&DynContainer, Confidence)> = None;
    for c in containers {
        let conf = c.sniff(&head[..n], &name);
        let rank = |c: Confidence| match c { Confidence::No => 0, Confidence::Maybe => 1, Confidence::Yes => 2 };
        if rank(conf) > 0 && best.map(|(_, b)| rank(conf) > rank(b)).unwrap_or(true) { best = Some((c, conf)); }
    }
    match best {
        Some((c, _)) => c.open(path),
        None => Err(io::Error::new(io::ErrorKind::Unsupported, "no container recognises this file")),
    }
}
