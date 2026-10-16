use crate::path::RelPath;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: RelPath,
    pub size: u64,
    pub kind: EntryKind,
}

/// A read-only virtual file tree: a folder, an archive, a disk image or a package all look the same.
pub trait Tree: Send + Sync {
    /// Display name of the source (folder or file stem), used when a system derives a key from it.
    fn name(&self) -> String;
    fn entries(&self) -> io::Result<Vec<Entry>>;
    fn read_to(&self, path: &RelPath, out: &mut dyn Write) -> io::Result<u64>;
}

impl dyn Tree + '_ {
    pub fn read(&self, path: &RelPath) -> io::Result<Vec<u8>> {
        let mut v = Vec::new();
        self.read_to(path, &mut v)?;
        Ok(v)
    }
    pub fn exists(&self, path: &RelPath) -> bool {
        self.entries()
            .map(|es| es.iter().any(|e| &e.path == path))
            .unwrap_or(false)
    }
}
