//! Read-only reader for PS5 debug packages: `.pkg` files that start with the bytes `7F 'F' 'I' 'H'`
//! and carry a zero "signed" byte at offset 5. Retail packages are refused, because their image key
//! lives on the console. No key is needed for a debug package.
//!
//! A package holds three things. A `\x7FCNT` block of metadata (`param.json`, icons, trophies) is
//! stored in the clear. An outer file system holds two files, `pfs_image.dat` and `naps_pkg_layout.dat`.
//! The layout file describes how `pfs_image.dat` is cut into blocks of up to 256 KiB. Each block is
//! stored raw or compressed with Oodle Kraken, with the Kraken headers removed. Joined in order, the
//! blocks form the "mount": an inner file system with a flat inode table at its end. This module
//! rebuilds the mount lazily, one block at a time. Only the metadata region at the end of the mount (up to 1 GiB) is held in memory.
//!
//! The format is not documented by its vendor. The offsets and bit fields below were checked against
//! packages built by other open tools; every size and offset is range-checked, so a damaged or hostile
//! package fails with an error and never reads outside the file.

use anyhow::{anyhow, ensure, Context, Result};
use std::{collections::HashSet, fs::File, os::unix::fs::FileExt};

/// First four bytes of a PS5 debug package ("FIH").
const FIH_MAGIC: &[u8; 4] = b"\x7fFIH";
/// First four bytes of the metadata block that holds the sce_sys files ("CNT").
const CNT_MAGIC: &[u8; 4] = b"\x7fCNT";
/// Version number stored in a file system header (a date, 2013-03-15).
const PFS_MAGIC: i64 = 20_130_315;
/// Seed of a file system that is not encrypted: it needs no key.
const NOAUTH_SEED: &[u8; 16] = b"PPRPLAIN-NOAUTH!";
/// Largest unit the layout file describes; a "ublock" decodes to at most this many bytes.
const UBLOCK: u64 = 0x40000;
/// Kraken splits a ublock into chunks of this size.
const HALF: usize = 0x20000;
/// Size limit for the layout file ("naps_pkg_layout.dat"), so a bad package cannot make us read a huge one.
const MAX_NAPS: u64 = 64 * 1024 * 1024;
/// Size limit for the metadata region after the layout, for the same reason.
const MAX_TAIL: u64 = 1 << 30;
/// Most files and folders accepted in the inner file system.
const MAX_INODES: usize = 1_000_000;
/// Largest folder listing accepted, in bytes.
const MAX_DIR: u64 = 8 * 1024 * 1024;
/// Deepest folder nesting accepted; a package that goes deeper is treated as a loop.
const MAX_DEPTH: usize = 128;
/// Most entries accepted in the metadata table.
const MAX_CNT_ENTRIES: u32 = 4096;
const CHUNK: usize = 1024 * 1024;

/// Whether a file starts with the finalized-image magic.
pub fn is_package(file: &File) -> bool {
    let mut b = [0u8; 4];
    file.read_exact_at(&mut b, 0).is_ok() && &b == FIH_MAGIC
}

/// A file or folder inside the package. `path` uses `/` and is relative to the game folder.
#[derive(Clone, Debug)]
pub struct Node {
    pub path: String,
    pub directory: bool,
    pub size: u64,
    origin: Origin,
}

#[derive(Clone, Copy, Debug)]
enum Origin {
    None,
    /// Byte address in the rebuilt mount.
    Inner(u64),
    /// Byte offset in the package file.
    Cnt(u64),
}

/// One step of the mount: `uncomp` bytes at `logical`, stored as `comp` bytes at `on_disk`.
/// A block with `comp == 0` is a hole that reads as zeros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block { logical: u64, on_disk: u64, comp: u32, uncomp: u32, even: u32, flags: u8 }

pub struct Package {
    file: File,
    /// File offset of `pfs_image.dat`.
    image: u64,
    blocks: Vec<Block>,
    mount: u64,
    /// The last blocks of the mount, which hold the inner superblock, inode table and folders.
    tail: Vec<u8>,
    tail_base: u64,
    superblock: usize,
    cnt: Vec<CntFile>,
    decoder: Decoder,
}

struct CntFile { name: String, offset: u64, size: u64 }

impl Package {
    pub fn open(file: File, cancel: &dyn Fn() -> Result<()>) -> Result<Self> {
        let len = file.metadata()?.len();
        let mut head = [0u8; 0x100];
        file.read_exact_at(&mut head, 0).context("Read package header")?;
        ensure!(&head[..4] == FIH_MAGIC, "Not a PS5 package");
        ensure!(head[5] == 0, "This is a retail PS5 package. Its key is held by the console, so the launcher cannot unpack it");
        let u64_at = |o: usize| u64::from_le_bytes(head[o..o + 8].try_into().unwrap());
        let (pfs, pfs_len, superblock, cnt) = (u64_at(0x10), u64_at(0x18), u64_at(0x20), u64_at(0x58));
        ensure!(pfs >= 0x100 && pfs_len >= 0x10000 && pfs.checked_add(pfs_len).is_some_and(|end| end <= len),
            "The package is shorter than its header says; wait until the download is complete");
        ensure!(cnt >= pfs + pfs_len && cnt < len, "Invalid package metadata offset");

        let outer = Outer::open(&file, pfs, pfs_len, superblock)?;
        let image = outer.find(&file, "pfs_image.dat")?.context("The package has no pfs_image.dat")?;
        let naps = outer.find(&file, "naps_pkg_layout.dat")?
            .context("The package has no NAPS layout; only packages that include one are supported")?;
        ensure!(naps.size <= MAX_NAPS, "Package layout file is too large");
        let mut blob = vec![0u8; naps.size as usize];
        file.read_exact_at(&mut blob, outer.extent(&naps)?).context("Read package layout")?;
        let layout = Layout::parse(&blob)?;
        let mount = layout.mount()?;
        let blocks = layout.blocks(mount)?;
        let image_start = outer.extent(&image)?;
        for b in blocks.iter().filter(|b| b.comp > 0) {
            ensure!(u64::from(b.comp) <= UBLOCK && b.on_disk.checked_add(u64::from(b.comp)).is_some_and(|end| end <= image.size),
                "A package block lies outside pfs_image.dat; wait until the download is complete");
        }

        let mut decoder = Decoder::new();
        let meta = layout.offsets.iter().map(|o| o.1).filter(|&o| o > 0 && o < mount).max().context("The package layout has no metadata region")?;
        let first = blocks.iter().position(|b| b.logical + u64::from(b.uncomp) > meta).context("The package metadata region is empty")?;
        let tail_base = blocks[first].logical;
        let total: u64 = blocks[first..].iter().map(|b| u64::from(b.uncomp)).sum();
        ensure!(total <= MAX_TAIL, "Package metadata region is too large");
        let mut tail = Vec::with_capacity(total as usize);
        for (i, b) in blocks.iter().enumerate().skip(first) {
            cancel()?;
            tail.extend_from_slice(decoder.block(&file, image_start, i, *b)?);
        }
        let superblock = find_superblock(&tail, mount).context("The package has no inner file system that matches its layout")?;
        let cnt = read_cnt(&file, cnt, len)?;
        Ok(Self { file, image: image_start, blocks, mount, tail, tail_base, superblock, cnt, decoder })
    }

    /// Every file and folder of the game, plus the metadata files that belong in `sce_sys`.
    pub fn list(&mut self, cancel: &dyn Fn() -> Result<()>) -> Result<Vec<Node>> {
        let sb = &self.tail[self.superblock..];
        let block = u32::from_le_bytes(sb[0x20..0x24].try_into().unwrap()) as usize;
        let count = i64::from_le_bytes(sb[0x30..0x38].try_into().unwrap());
        let mode = u16::from_le_bytes(sb[0x1c..0x1e].try_into().unwrap());
        ensure!(block >= 0x1000 && block.is_multiple_of(0x1000) && count > 0 && count as usize <= MAX_INODES, "Implausible inner file system header");
        // Flat inodes keep a byte address at 0x60. Signed inodes keep block hashes there instead.
        ensure!(mode & 0x13 == 0x10, "Unsupported inner file system format");
        let count = count as usize;
        let per_block = block / INODE;
        let table = self.superblock + block;
        let table_len = count.div_ceil(per_block) * block;
        ensure!(table.checked_add(table_len).is_some_and(|end| end <= self.tail.len()), "The inner inode table is cut short");
        let inodes: Vec<Inode> = (0..count).map(|i| {
            let at = table + (i / per_block) * block + (i % per_block) * INODE;
            let e = &self.tail[at..at + INODE];
            Inode { mode: u16::from_le_bytes([e[0], e[1]]), size: u64::from_le_bytes(e[8..16].try_into().unwrap()), logical: u64::from_le_bytes(e[0x60..0x68].try_into().unwrap()) }
        }).collect();

        let mut nodes = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(0usize, String::new(), false, 0usize)];
        while let Some((ino, path, under, depth)) = stack.pop() {
            cancel()?;
            ensure!(depth <= MAX_DEPTH && seen.insert(ino), "The inner folder tree is cyclic or too deep");
            let dir = inodes[ino];
            ensure!(dir.mode & 0xf000 == 0x4000 && dir.size > 0 && dir.size <= MAX_DIR, "Invalid inner folder");
            let bytes = self.logical_bytes(dir.logical, dir.size as usize)?;
            for ent in dirents(&bytes, block)? {
                if ent.name == "." || ent.name == ".." { continue; }
                ensure!(!ent.name.contains(['/', '\\', ':']) && (ent.ino as usize) < inodes.len(), "Invalid inner folder entry");
                let child_under = under || ent.name == "uroot";
                let child = if ent.name == "uroot" && path.is_empty() { String::new() } else if path.is_empty() { ent.name.clone() } else { format!("{path}/{}", ent.name) };
                let node = inodes[ent.ino as usize];
                match ent.kind {
                    3 => {
                        if under && !child.is_empty() { nodes.push(Node { path: child.clone(), directory: true, size: 0, origin: Origin::None }); }
                        stack.push((ent.ino as usize, child, child_under, depth + 1));
                    }
                    2 if under && !child.is_empty() => nodes.push(Node { path: child, directory: false, size: node.size, origin: Origin::Inner(node.logical) }),
                    _ => (),
                }
                ensure!(nodes.len() <= MAX_INODES, "The package holds too many files");
            }
        }
        ensure!(!nodes.is_empty(), "The package holds no files");
        // Metadata files live beside the inner tree; the tree wins when both name a path.
        let known: HashSet<String> = nodes.iter().map(|n| n.path.clone()).collect();
        for f in &self.cnt {
            let path = format!("sce_sys/{}", f.name);
            if !known.contains(&path) {
                nodes.push(Node { path, directory: false, size: f.size, origin: Origin::Cnt(f.offset) });
            }
        }
        Ok(nodes)
    }

    /// Stream a file's bytes to `sink`. Memory use is bounded by one block.
    pub fn copy(&mut self, node: &Node, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        match node.origin {
            Origin::None => Ok(()),
            Origin::Cnt(offset) => {
                let mut buf = vec![0u8; CHUNK.min(node.size as usize)];
                let mut done = 0u64;
                while done < node.size {
                    let n = buf.len().min((node.size - done) as usize);
                    self.file.read_exact_at(&mut buf[..n], offset + done).context("Read package metadata file")?;
                    sink(&buf[..n])?;
                    done += n as u64;
                }
                Ok(())
            }
            Origin::Inner(logical) => {
                let (mut pos, mut left) = (logical, node.size);
                ensure!(pos.checked_add(left).is_some_and(|end| end <= self.mount), "A package file lies outside the mount");
                while left > 0 {
                    let (index, block) = self.find(pos)?;
                    let data = self.decoder.block(&self.file, self.image, index, block)?;
                    let from = (pos - block.logical) as usize;
                    let n = ((data.len() - from) as u64).min(left) as usize;
                    sink(&data[from..from + n])?;
                    pos += n as u64;
                    left -= n as u64;
                }
                Ok(())
            }
        }
    }

    fn find(&self, pos: u64) -> Result<(usize, Block)> {
        let i = self.blocks.partition_point(|b| b.logical + u64::from(b.uncomp) <= pos);
        let block = *self.blocks.get(i).filter(|b| b.logical <= pos).context("A package file points outside its blocks")?;
        Ok((i, block))
    }

    fn logical_bytes(&mut self, logical: u64, len: usize) -> Result<Vec<u8>> {
        ensure!(logical.checked_add(len as u64).is_some_and(|end| end <= self.mount), "A package folder lies outside the mount");
        if logical >= self.tail_base && ((logical - self.tail_base) as usize).checked_add(len).is_some_and(|end| end <= self.tail.len()) {
            let from = (logical - self.tail_base) as usize;
            return Ok(self.tail[from..from + len].to_vec());
        }
        let mut out = Vec::with_capacity(len);
        let mut pos = logical;
        while out.len() < len {
            let (index, block) = self.find(pos)?;
            let data = self.decoder.block(&self.file, self.image, index, block)?;
            let from = (pos - block.logical) as usize;
            let n = (data.len() - from).min(len - out.len());
            out.extend_from_slice(&data[from..from + n]);
            pos += n as u64;
        }
        Ok(out)
    }
}

const INODE: usize = 0xA8;
#[derive(Clone, Copy)]
struct Inode { mode: u16, size: u64, logical: u64 }

struct Dirent { ino: u32, kind: u32, name: String }

/// Folder entries: `ino`, `kind` (2 file, 3 folder, 4 and 5 dot entries), name length, record size, name.
/// A zero record pads to the end of a block.
fn dirents(buf: &[u8], block: usize) -> Result<Vec<Dirent>> {
    let (mut out, mut at) = (Vec::new(), 0usize);
    while at < buf.len() {
        ensure!(buf.len() - at >= 16, "A package folder is cut short");
        let u32_at = |o: usize| u32::from_le_bytes(buf[at + o..at + o + 4].try_into().unwrap());
        let (ino, kind, name_len, size) = (u32_at(0), u32_at(4), u32_at(8) as usize, u32_at(12) as usize);
        if ino == 0 && kind == 0 && name_len == 0 && size == 0 {
            at += (block - at % block).min(buf.len() - at);
            continue;
        }
        ensure!((16..=0x1000).contains(&size) && name_len <= size - 16 && size <= buf.len() - at, "Invalid package folder entry");
        let raw = &buf[at + 16..at + 16 + name_len];
        let raw = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        ensure!(!raw.is_empty(), "A package folder entry has no name");
        out.push(Dirent { ino, kind, name: String::from_utf8(raw.to_vec()).map_err(|_| anyhow!("Non-UTF-8 filenames are unsupported"))? });
        at += size;
    }
    Ok(out)
}

/// The superblock of the inner file system: version 2, the file system magic, and a size that
/// matches the mount. Matching the size avoids mistaking another superblock for it.
fn find_superblock(tail: &[u8], mount: u64) -> Option<usize> {
    (0..).step_by(0x10000).take_while(|off| off + 0x40 <= tail.len()).find(|&off| {
        let i64_at = |o: usize| i64::from_le_bytes(tail[off + o..off + o + 8].try_into().unwrap());
        let block = u32::from_le_bytes(tail[off + 0x20..off + 0x24].try_into().unwrap()) as u64;
        i64_at(0) == 2 && i64_at(8) == PFS_MAGIC && i64_at(0x30) > 0 && i64_at(0x38) > 0 && (i64_at(0x38) as u64).checked_mul(block) == Some(mount)
    })
}

// ---- Outer file system -------------------------------------------------------------------------

struct Outer { offset: u64, len: u64, block: u64, inodes: Vec<OuterInode> }
#[derive(Clone)]
struct OuterInode { mode: u16, size: u64, blocks: u64, first: i64, second: i64 }

const OUTER_INODE: usize = 0x2C8;

impl Outer {
    fn open(file: &File, offset: u64, len: u64, superblock: u64) -> Result<Self> {
        let mut sb = [0u8; 0x400];
        let at = if superblock >= offset && superblock.checked_add(0x400).is_some_and(|end| end <= offset + len) { superblock } else { offset };
        file.read_exact_at(&mut sb, at).context("Read package file system header")?;
        let i64_at = |o: usize| i64::from_le_bytes(sb[o..o + 8].try_into().unwrap());
        ensure!(i64_at(8) == PFS_MAGIC && matches!(i64_at(0), 1 | 2), "The package has no readable file system");
        let mode = u16::from_le_bytes([sb[0x1c], sb[0x1d]]);
        let block = u64::from(u32::from_le_bytes(sb[0x20..0x24].try_into().unwrap()));
        ensure!(block >= 0x1000 && block.is_multiple_of(0x1000) && block <= 0x100000, "Unsupported package block size");
        ensure!(mode & 2 == 0, "Packages with 64-bit inodes are not supported");
        ensure!(mode & 4 == 0 || &sb[0x370..0x380] == NOAUTH_SEED, "This package's file system is encrypted, so the launcher cannot unpack it");
        let (count, table_blocks) = (i64_at(0x30), i64_at(0x40).max(1));
        ensure!((1..=4096).contains(&count) && table_blocks <= 64, "Implausible package inode table");
        let here = (at - offset) / block;
        let table = match i64_at(0xd8) { t if t > 0 && (t as u64) < len / block => t as u64, _ => here + 1 };
        let mut inodes = Vec::new();
        let mut buf = vec![0u8; block as usize];
        'table: for b in 0..table_blocks as u64 {
            ensure!((table + b + 1) * block <= len, "The package inode table is cut short");
            file.read_exact_at(&mut buf, offset + (table + b) * block).context("Read package inode table")?;
            for i in 0..block as usize / OUTER_INODE {
                if inodes.len() == count as usize { break 'table; }
                let e = &buf[i * OUTER_INODE..(i + 1) * OUTER_INODE];
                let direct = |k: usize| i32::from_le_bytes(e[0x64 + k * 36 + 32..0x64 + k * 36 + 36].try_into().unwrap()) as i64;
                inodes.push(OuterInode {
                    mode: u16::from_le_bytes([e[0], e[1]]),
                    size: u64::from_le_bytes(e[8..16].try_into().unwrap()),
                    blocks: u64::from(u32::from_le_bytes(e[0x60..0x64].try_into().unwrap())),
                    first: direct(0), second: direct(1),
                });
            }
        }
        Ok(Self { offset, len, block, inodes })
    }

    /// The regular file called `name`, found by reading every folder inode.
    fn find(&self, file: &File, name: &str) -> Result<Option<OuterInode>> {
        for dir in self.inodes.iter().filter(|i| i.mode & 0x4000 != 0 && i.first > 0) {
            let n = dir.size.min(self.block) as usize;
            let mut buf = vec![0u8; n];
            if file.read_exact_at(&mut buf, self.offset + dir.first as u64 * self.block).is_err() { continue; }
            let Ok(list) = dirents(&buf, self.block as usize) else { continue };
            for ent in list.iter().filter(|e| e.name == name && e.kind == 2) {
                let Some(i) = self.inodes.get(ent.ino as usize) else { continue };
                if i.mode & 0x4000 == 0 && i.size > 0 {
                    return Ok(Some(i.clone()));
                }
            }
        }
        Ok(None)
    }

    /// File offset of a file's first byte. The file must be stored in one run of blocks.
    fn extent(&self, inode: &OuterInode) -> Result<u64> {
        ensure!(inode.first >= 0 && (inode.blocks <= 1 || inode.second == inode.first + 1), "A package file is not stored in one piece");
        let start = inode.first as u64 * self.block;
        ensure!(start.checked_add(inode.size).is_some_and(|end| end <= self.len), "A package file lies outside the package file system");
        Ok(self.offset + start)
    }
}

// ---- Package metadata (CNT) --------------------------------------------------------------------

/// The metadata files of a package, under the names the package gives them. Encrypted entries and
/// entries without a name (digests, keys, licenses) are skipped.
fn read_cnt(file: &File, offset: u64, len: u64) -> Result<Vec<CntFile>> {
    let mut head = [0u8; 0x40];
    file.read_exact_at(&mut head, offset).context("Read package metadata header")?;
    ensure!(&head[..4] == CNT_MAGIC, "The package has no metadata block");
    let be = |o: usize| u32::from_be_bytes(head[o..o + 4].try_into().unwrap());
    let (count, table) = (be(16), u64::from(be(24)));
    ensure!(count <= MAX_CNT_ENTRIES && (offset + table + u64::from(count) * 0x20) <= len, "Invalid package metadata table");
    let mut raw = vec![0u8; count as usize * 0x20];
    file.read_exact_at(&mut raw, offset + table).context("Read package metadata table")?;
    let entries: Vec<[u32; 6]> = raw.as_chunks::<0x20>().0.iter().map(|r| std::array::from_fn(|k| u32::from_be_bytes(r[k * 4..k * 4 + 4].try_into().unwrap()))).collect();
    // [id, name offset, flags, flags, data offset, data size]
    let in_file = |e: &[u32; 6]| offset + u64::from(e[4]) + u64::from(e[5]) <= len;
    const CUT: &str = "The package metadata is cut short; wait until the download is complete";
    let names = match entries.iter().find(|e| e[0] == 0x200 && e[5] <= 1 << 20) {
        Some(e) => { ensure!(in_file(e), CUT); let mut n = vec![0u8; e[5] as usize]; file.read_exact_at(&mut n, offset + u64::from(e[4]))?; n }
        None => Vec::new(),
    };
    let mut out = Vec::new();
    for e in &entries {
        if e[2] & 0x8000_0000 != 0 || e[5] == 0 || e[1] == 0 || e[1] as usize >= names.len() { continue; }
        let raw = &names[e[1] as usize..];
        let Ok(name) = std::str::from_utf8(&raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())]) else { continue };
        let safe = !name.is_empty() && !name.starts_with('/') && !name.contains("..") && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c));
        if safe {
            ensure!(in_file(e), CUT);
            out.push(CntFile { name: name.into(), offset: offset + u64::from(e[4]), size: u64::from(e[5]) });
        }
    }
    Ok(out)
}

// ---- Layout file (NAPS) ------------------------------------------------------------------------

/// One 9-byte record. Plain records describe a block; a "run base" record re-anchors the
/// on-disk position after a gap.
#[derive(Clone, Copy, Debug)]
struct Record { run: bool, offset: u32, delta: u8, shuffle: u8, even_minus_1: u32, base_256k: u32 }

#[derive(Debug)]
struct Layout {
    /// (uncompressed offset, kind). Kind 0x40 marks a hole and, last, the mount size.
    offsets: Vec<(u8, u64)>,
    records: Vec<Record>,
}

impl Layout {
    fn parse(blob: &[u8]) -> Result<Self> {
        ensure!(blob.len() >= 16, "The package layout is cut short");
        let (w0, w1) = (u64::from_le_bytes(blob[0..8].try_into().unwrap()), u64::from_le_bytes(blob[8..16].try_into().unwrap()));
        let files = ((w0 & 0xFF_FFFF) + 1) as usize;
        let shuffles = ((w0 >> 28) & 0xF) as usize;
        let ublocks = ((w0 >> 32) & 0xFF_FFFF) as usize;
        let outer = (w1 & 0xFF_FFFF) as usize;
        let records = (((w1 >> 24) & 0xFF_FFFF) + 2) as usize;
        let mut at = 16 + outer * 8 + shuffles * 8;
        let offsets_at = at;
        at = (at + files * 6).next_multiple_of(16);
        at = (at + ((ublocks + 8) >> 3) * 10).next_multiple_of(8);
        let records_at = at;
        ensure!(records_at + records * 9 <= blob.len(), "The package layout is cut short");
        let offsets = (0..files).map(|i| {
            let e = &blob[offsets_at + i * 6..offsets_at + i * 6 + 6];
            (e[5], e[..5].iter().rev().fold(0u64, |a, &b| a << 8 | u64::from(b)))
        }).collect();
        let records = (0..records).map(|i| {
            let e = &blob[records_at + i * 9..records_at + i * 9 + 9];
            let lo = u64::from_le_bytes(e[..8].try_into().unwrap());
            let run = lo >> 18 & 1 == 1;
            Record {
                run,
                offset: (lo & 0x3_FFFF) as u32,
                delta: if run { 0 } else { (lo >> 56 & 7) as u8 },
                shuffle: if run { 0 } else { (lo >> 59 & 0xF) as u8 },
                even_minus_1: if run { 0 } else { (lo >> 38 & 0x1_FFFF) as u32 },
                base_256k: if run { ((lo >> 49 & 0x7FFF) | (u64::from(e[8]) & 0x1FF) << 15) as u32 } else { 0 },
            }
        }).collect();
        Ok(Self { offsets, records })
    }

    /// The last offset of kind 0x40 is the size of the whole mount.
    fn mount(&self) -> Result<u64> {
        let size = self.offsets.iter().rev().find(|o| o.0 == 0x40).map_or(0, |o| o.1);
        ensure!(size > 0 && size < 1 << 48, "The package layout has no mount size");
        Ok(size)
    }

    /// Turn the records into blocks in mount order.
    fn blocks(&self, mount: u64) -> Result<Vec<Block>> {
        let mut bounds: Vec<u64> = self.offsets.iter().map(|o| o.1).filter(|&o| o > 0 && o <= mount).collect();
        bounds.sort_unstable();
        bounds.dedup();
        let mut walk = Walk { layout: self, bounds: &bounds, cursor: 0, mount, logical: 0, out: Vec::with_capacity(self.records.len()) };
        let mut on_disk = 0u64;
        let mut i = 0;
        while i < self.records.len() {
            let rec = self.records[i];
            if rec.run {
                let next = self.records.get(i + 1).filter(|r| !r.run).context("The package layout has a run marker without a block")?;
                on_disk = u64::from(rec.base_256k / 2) * UBLOCK + u64::from(next.offset);
                i += 1;
                continue;
            }
            walk.holes()?;
            let Some(next) = self.records.get(i + 1) else { break };
            if walk.logical >= mount { break; }
            let uncomp = UBLOCK.min(walk.boundary() - walk.logical) as u32;
            if uncomp == 0 { break; }
            let diff = i64::from(next.offset) - i64::from(rec.offset);
            let comp = if diff <= 0 { diff + UBLOCK as i64 } else { diff } as u32;
            walk.out.push(Block { logical: walk.logical, on_disk, comp, uncomp, even: rec.even_minus_1 + 1, flags: rec.delta | rec.shuffle << 4 });
            on_disk += u64::from(comp);
            walk.logical += u64::from(uncomp);
            i += 1;
        }
        walk.holes()?;
        ensure!(walk.logical == mount, "The package layout does not cover the whole mount; wait until the download is complete");
        Ok(walk.out)
    }
}

struct Walk<'a> { layout: &'a Layout, bounds: &'a [u64], cursor: usize, mount: u64, logical: u64, out: Vec<Block> }

impl Walk<'_> {
    /// The next file boundary after the current position; blocks never cross one.
    fn boundary(&self) -> u64 {
        self.bounds.get(self.bounds.partition_point(|&b| b <= self.logical)).copied().unwrap_or(self.mount)
    }

    /// A kind-0x40 offset below the mount size starts a region the records do not describe. It reads as zeros.
    fn holes(&mut self) -> Result<()> {
        let offsets = &self.layout.offsets;
        while self.logical < self.mount {
            while self.cursor < offsets.len() && offsets[self.cursor].1 < self.logical { self.cursor += 1; }
            match offsets.get(self.cursor) {
                Some(&(0x40, at)) if at == self.logical => (),
                _ => return Ok(()),
            }
            let uncomp = UBLOCK.min(self.boundary() - self.logical) as u32;
            ensure!(uncomp > 0, "The package layout is inconsistent");
            self.out.push(Block { logical: self.logical, on_disk: 0, comp: 0, uncomp, even: 0, flags: 0 });
            self.logical += u64::from(uncomp);
        }
        Ok(())
    }
}

// ---- Block decoding ----------------------------------------------------------------------------

struct Decoder {
    kraken: oozextract::Extractor,
    raw: Vec<u8>,
    plain: Vec<u8>,
    stream: Vec<u8>,
    cached: Option<usize>,
}

impl Decoder {
    fn new() -> Self {
        Self { kraken: oozextract::Extractor::new(), raw: Vec::new(), plain: Vec::new(), stream: Vec::new(), cached: None }
    }

    /// The decoded bytes of block `index`. The last block stays cached, so neighbouring small files cost one decode.
    fn block(&mut self, file: &File, image: u64, index: usize, b: Block) -> Result<&[u8]> {
        if self.cached != Some(index) {
            self.cached = None;
            self.plain.clear();
            self.plain.resize(b.uncomp as usize, 0);
            if b.comp > 0 {
                self.raw.clear();
                self.raw.resize(b.comp as usize, 0);
                file.read_exact_at(&mut self.raw, image + b.on_disk).context("Read package block")?;
                if b.comp == b.uncomp {
                    self.plain.copy_from_slice(&self.raw);
                } else {
                    if let Err(e) = kraken(&mut self.kraken, &mut self.stream, &self.raw, b.flags, b.even as usize, &mut self.plain) {
                        let others = alternatives(&mut self.kraken, &mut self.stream, &self.raw, b.flags, b.even as usize, self.plain.len());
                        let head: String = self.raw.iter().take(12).map(|x| format!("{x:02x}")).collect();
                        return Err(e.context(format!("Decode package block at 0x{:x} (flags 0x{:02x}, split {}, {} bytes stored as {}, starts {head}; also decodes with flags: {})",
                            b.logical, b.flags, b.even, b.uncomp, b.comp, if others.is_empty() { "none".into() } else { others })));
                    }
                }
            }
            self.cached = Some(index);
        }
        Ok(&self.plain)
    }
}

/// One Kraken chunk: up to 128 KiB of output. `lz` chunks hold an LZ table; others are one entropy array.
/// `delta` picks the literal mode: delta-coded literals when set, raw literals when not.
struct Chunk<'a> { data: &'a [u8], lz: bool, delta: bool }

/// Decode a block whose Kraken headers were stripped. The layout keeps what the headers held: where
/// the first chunk ends (`even`), and per chunk whether it is an LZ chunk (bits 1 and 5 of `flags`),
/// its literal mode (bits 0 and 4), and whether the second chunk starts a fresh window (bit 6).
/// This puts the headers back so the standard Oodle decoder can read the block.
fn kraken(ex: &mut oozextract::Extractor, stream: &mut Vec<u8>, payload: &[u8], flags: u8, even: usize, dst: &mut [u8]) -> Result<()> {
    let first = Chunk { lz: flags & 0x02 != 0, delta: flags & 0x01 != 0, data: payload };
    let second = |data| Chunk { lz: flags & 0x20 != 0, delta: flags & 0x10 != 0, data };
    if even == 0 || even >= payload.len() || dst.len() <= HALF {
        ensure!(dst.len() <= HALF, "Unsupported package block layout ({} bytes in one chunk)", dst.len());
        return run(ex, stream, &[first], dst);
    }
    let (a, b) = payload.split_at(even);
    let first = Chunk { data: a, ..first };
    if flags & 0x40 == 0 {
        run(ex, stream, &[first, second(b)], dst)
    } else {
        // A restarted chunk does not look back into the first one, so it decodes on its own.
        let (d0, d1) = dst.split_at_mut(HALF);
        run(ex, stream, &[first], d0)?;
        run(ex, stream, &[second(b)], d1)
    }
}

/// For an error report: the other flag values that would have decoded this block. The layout's
/// flags are used as they are; this only tells a developer whether a bit is misread.
fn alternatives(ex: &mut oozextract::Extractor, stream: &mut Vec<u8>, payload: &[u8], flags: u8, even: usize, len: usize) -> String {
    let mut out = vec![0u8; len];
    let ok: Vec<String> = (0..0x80u8).filter(|&f| f & !0x73 == 0 && f != flags && kraken(ex, stream, payload, f, even, &mut out).is_ok())
        .map(|f| format!("0x{f:02x}")).collect();
    ok.join(" ")
}

fn run(ex: &mut oozextract::Extractor, stream: &mut Vec<u8>, chunks: &[Chunk], dst: &mut [u8]) -> Result<()> {
    // Block header (restart, Kraken), then a 3-byte quantum header holding the size minus one.
    stream.clear();
    stream.extend_from_slice(&[0x8c, 0x06, 0, 0, 0]);
    let mut left = dst.len();
    for c in chunks {
        let want = left.min(HALF);
        left -= want;
        if c.data.len() == want {
            stream.extend_from_slice(&(0x80_0000 | want as u32).to_be_bytes()[1..]);
        } else if c.lz {
            ensure!(c.data.len() < want, "A package block is larger than its output");
            let mode = if c.delta { 0 } else { 1 };
            stream.extend_from_slice(&(0x80_0000 | mode << 19 | c.data.len() as u32).to_be_bytes()[1..]);
        } else {
            // The decoder tells an entropy array from an LZ chunk by the top bit of its first byte.
            ensure!(c.data.first().is_some_and(|b| b & 0x80 == 0), "Invalid package block");
        }
        stream.extend_from_slice(c.data);
    }
    ensure!(left == 0, "A package block is shorter than its layout says");
    let size = stream.len() - 5;
    ensure!((1..0x3_FFFF).contains(&size), "A package block is too large");
    stream[2..5].copy_from_slice(&((size - 1) as u32).to_be_bytes()[1..]);
    let n = ex.read_from_slice(stream, dst).map_err(|e| anyhow!("Kraken: {e}"))?;
    ensure!(n == dst.len(), "A package block decoded to the wrong size");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(pos: usize) -> Record { Record { run: false, offset: pos as u32 & 0x3_FFFF, delta: 0, shuffle: 0, even_minus_1: 0, base_256k: 0 } }
    fn layout(offsets: &[(u8, u64)], records: Vec<Record>) -> Layout { Layout { offsets: offsets.to_vec(), records } }

    #[test]
    fn stored_blocks_follow_file_boundaries() {
        // Two files (96 and 0x50000 bytes) and a closing record; the long file splits into 256 KiB blocks.
        let l = layout(&[(0, 0), (0, 96), (0x40, 96 + 0x50000)], vec![plain(0), plain(96), plain(96 + 0x40000), plain(96 + 0x50000)]);
        let blocks = l.blocks(96 + 0x50000).unwrap();
        let shape: Vec<_> = blocks.iter().map(|b| (b.logical, b.on_disk, b.comp, b.uncomp)).collect();
        assert_eq!(shape, [(0, 0, 96, 96), (96, 96, 0x40000, 0x40000), (96 + 0x40000, 96 + 0x40000, 0x10000, 0x10000)]);
    }

    #[test]
    fn holes_read_as_zeros_and_run_markers_move_the_disk_position() {
        let run = Record { run: true, offset: 0x1000, delta: 0, shuffle: 0, even_minus_1: 0, base_256k: 6 };
        // A hole of 0x40000 at 0x1000, then a block after a run marker that points at 3 * 0x40000 + 0x20.
        let l = layout(&[(0, 0), (0x40, 0x1000), (0, 0x41000), (0x40, 0x41100)],
            vec![plain(0), run, plain(0x20), plain(0x20 + 0x100)]);
        let blocks = l.blocks(0x41100).unwrap();
        assert_eq!(blocks[1], Block { logical: 0x1000, on_disk: 0, comp: 0, uncomp: 0x40000, even: 0, flags: 0 });
        assert_eq!(blocks[2].on_disk, 3 * UBLOCK + 0x20);
        assert_eq!((blocks[2].logical, blocks[2].uncomp), (0x41000, 0x100));
    }

    #[test]
    fn a_layout_that_stops_short_is_refused() {
        let l = layout(&[(0, 0), (0, 0x1000), (0x40, 0x2000)], vec![plain(0), plain(0x1000)]);
        assert!(l.blocks(0x2000).unwrap_err().to_string().contains("does not cover"));
    }

    #[test]
    fn layout_rejects_truncated_input_and_bad_counts() {
        assert!(Layout::parse(&[0u8; 8]).is_err());
        // Header claims more records than the blob holds.
        let mut blob = vec![0u8; 16 + 40];
        blob[8..16].copy_from_slice(&(100u64 << 24).to_le_bytes());
        assert!(Layout::parse(&blob).unwrap_err().to_string().contains("cut short"));
    }

    #[test]
    fn dirents_skip_block_padding_and_reject_bad_records() {
        let mut buf = vec![0u8; 128];
        let put = |buf: &mut Vec<u8>, at: usize, ino: u32, name: &str| {
            buf[at..at + 4].copy_from_slice(&ino.to_le_bytes());
            buf[at + 4..at + 8].copy_from_slice(&2u32.to_le_bytes());
            buf[at + 8..at + 12].copy_from_slice(&(name.len() as u32).to_le_bytes());
            buf[at + 12..at + 16].copy_from_slice(&24u32.to_le_bytes());
            buf[at + 16..at + 16 + name.len()].copy_from_slice(name.as_bytes());
        };
        put(&mut buf, 0, 5, "a.bin");
        put(&mut buf, 64, 6, "b.bin"); // after zero padding up to the 64-byte block boundary
        let names: Vec<_> = dirents(&buf, 64).unwrap().into_iter().map(|d| (d.ino, d.name)).collect();
        assert_eq!(names, [(5, "a.bin".to_string()), (6, "b.bin".to_string())]);
        buf[12..16].copy_from_slice(&8u32.to_le_bytes());
        assert!(dirents(&buf, 64).is_err());
    }

    #[test]
    fn entropy_blocks_decode_through_synthesized_headers() {
        // A bare entropy array in "stored" form: a 3-byte size, then the bytes themselves.
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let mut payload = vec![0, (data.len() >> 8) as u8, data.len() as u8];
        payload.extend_from_slice(&data);
        let mut ex = oozextract::Extractor::new();
        let (mut stream, mut out) = (Vec::new(), vec![0u8; data.len()]);
        kraken(&mut ex, &mut stream, &payload, 0, 0, &mut out).unwrap();
        assert_eq!(out, data);
        // A damaged payload is an error, never a panic.
        payload[1] = 0xff;
        assert!(kraken(&mut ex, &mut stream, &payload, 0, 0, &mut out).is_err());
        assert!(kraken(&mut ex, &mut stream, &[0x80, 1, 2, 3], 0, 0, &mut out).is_err());
    }

    #[test]
    fn a_failed_block_reports_the_flags_that_would_decode_it() {
        // An entropy-only payload read with the LZ bit set fails; flag value 0 decodes it.
        let data: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let mut payload = vec![0, (data.len() >> 8) as u8, data.len() as u8];
        payload.extend_from_slice(&data);
        let mut ex = oozextract::Extractor::new();
        let mut stream = Vec::new();
        assert!(kraken(&mut ex, &mut stream, &payload, 0x02, 0, &mut vec![0u8; 300]).is_err());
        let others = alternatives(&mut ex, &mut stream, &payload, 0x02, 0, 300);
        assert!(others.split(' ').any(|f| f == "0x00"), "{others}");
    }

    #[test]
    fn a_split_block_decodes_both_chunks() {
        let data: Vec<u8> = (0..HALF + 500).map(|i| (i % 251) as u8).collect();
        let entropy = |d: &[u8]| { let mut p = vec![(d.len() >> 16) as u8, (d.len() >> 8) as u8, d.len() as u8]; p.extend_from_slice(d); p };
        let (a, b) = (entropy(&data[..HALF]), entropy(&data[HALF..]));
        let payload = [a.clone(), b].concat();
        let mut ex = oozextract::Extractor::new();
        let (mut stream, mut out) = (Vec::new(), vec![0u8; data.len()]);
        kraken(&mut ex, &mut stream, &payload, 0, a.len(), &mut out).unwrap();
        assert_eq!(out, data);
        // The restart bit decodes the second chunk alone; stored data does not care.
        out.fill(0);
        kraken(&mut ex, &mut stream, &payload, 0x40, a.len(), &mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn synthetic_package_lists_and_copies_every_file() {
        for entropy in [false, true] {
            let bytes = build::package(&build::Spec { files: &[("eboot.bin", b"fixture executable"), ("sce_module/a.prx", b"module"), ("empty", b"")], entropy, ..Default::default() });
            let dir = crate::hostos::real_tempdir();
            let path = dir.path().join("game.pkg");
            std::fs::write(&path, &bytes).unwrap();
            assert!(is_package(&File::open(&path).unwrap()));
            let mut pkg = Package::open(File::open(&path).unwrap(), &|| Ok(())).unwrap();
            let nodes = pkg.list(&|| Ok(())).unwrap();
            let mut seen = std::collections::BTreeMap::new();
            for n in nodes.iter().filter(|n| !n.directory) {
                let mut data = Vec::new();
                pkg.copy(n, &mut |c| { data.extend_from_slice(c); Ok(()) }).unwrap();
                assert_eq!(data.len() as u64, n.size);
                seen.insert(n.path.clone(), data);
            }
            assert_eq!(seen["eboot.bin"], b"fixture executable");
            assert_eq!(seen["sce_module/a.prx"], b"module");
            assert_eq!(seen["empty"], b"");
            assert_eq!(seen["sce_sys/param.json"], build::PARAM);
            assert!(nodes.iter().any(|n| n.directory && n.path == "sce_module"));
        }
    }

    #[test]
    fn damaged_packages_fail_with_an_error() {
        let good = build::package(&build::Spec { files: &[("eboot.bin", b"x")], ..Default::default() });
        let open = |bytes: &[u8]| {
            let dir = crate::hostos::real_tempdir();
            std::fs::write(dir.path().join("a.pkg"), bytes).unwrap();
            Package::open(File::open(dir.path().join("a.pkg")).unwrap(), &|| Ok(())).map(|_| ()).unwrap_err().to_string()
        };
        assert!(open(&build::package(&build::Spec { files: &[("eboot.bin", b"x")], retail: true, ..Default::default() })).contains("retail"));
        assert!(open(&good[..good.len() - 100]).contains("download"));
        assert!(open(&good[..0x20000]).contains("shorter"));
        assert!(open(&[0u8; 0x400]).contains("Not a PS5 package"));
        // Every truncation must be an error, not a panic.
        for cut in (0x10000..good.len()).step_by(0x7001) {
            let dir = crate::hostos::real_tempdir();
            std::fs::write(dir.path().join("a.pkg"), &good[..cut]).unwrap();
            assert!(Package::open(File::open(dir.path().join("a.pkg")).unwrap(), &|| Ok(())).is_err());
        }
    }

    /// Opt-in check against a real package: `PS5_TEST_PKG=/path/to/game.pkg cargo test real_package -- --ignored`.
    /// It lists the package and decodes every block of the largest file to prove the layout and Kraken path.
    #[test]
    #[ignore = "needs a real package in PS5_TEST_PKG"]
    fn real_package_lists_and_decodes() {
        let path = std::env::var("PS5_TEST_PKG").expect("set PS5_TEST_PKG");
        let mut pkg = Package::open(File::open(path).unwrap(), &|| Ok(())).unwrap();
        let nodes = pkg.list(&|| Ok(())).unwrap();
        let find = |p: &str| nodes.iter().find(|n| n.path == p).unwrap_or_else(|| panic!("missing {p}")).clone();
        let mut param = Vec::new();
        pkg.copy(&find("sce_sys/param.json"), &mut |c| { param.extend_from_slice(c); Ok(()) }).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&param).unwrap()["titleId"].is_string());
        let mut head = Vec::new();
        pkg.copy(&find("eboot.bin"), &mut |c| { if head.len() < 4 { head.extend_from_slice(c); } Ok(()) }).unwrap();
        assert_eq!(&head[..4], &[0x54, 0x14, 0xf5, 0xee], "eboot.bin is a PS5 SELF file");
        eprintln!("{} entries", nodes.len());
        let mut biggest: Vec<_> = nodes.iter().filter(|n| !n.directory).map(|n| (n.size, n.path.as_str())).collect();
        biggest.sort_unstable_by(|a, b| b.cmp(a));
        for (size, path) in biggest.iter().take(5) { eprintln!("{size:>14} {path}"); }
        // PS5_TEST_FULL=1 also decodes every file, which takes as long as an install without the disk writes.
        if std::env::var_os("PS5_TEST_FULL").is_some() {
            let total: u64 = nodes.iter().map(|n| n.size).sum();
            let mut done = 0u64;
            for (i, n) in nodes.iter().filter(|n| !n.directory).enumerate() {
                let mut got = 0u64;
                pkg.copy(n, &mut |c| { got += c.len() as u64; Ok(()) }).unwrap_or_else(|e| panic!("{}: {e:#}", n.path));
                assert_eq!(got, n.size, "{}", n.path);
                done += got;
                if i % 5000 == 0 { eprintln!("{done} of {total} bytes"); }
            }
        }
    }
}

/// A builder for small, generated packages. No real package or archiver is needed to test the reader.
#[cfg(test)]
pub(crate) mod build {
    use super::*;

    pub const PARAM: &[u8] = br#"{"titleId":"PPSA12345","localizedParameters":{"en-US":{"titleName":"Generated fixture"}}}"#;
    const CONTENT_ID: &[u8] = b"UP9000-PPSA12345_00-0000000000000000";
    const BS: usize = 0x10000;

    #[derive(Default)]
    pub struct Spec<'a> {
        pub files: &'a [(&'a str, &'a [u8])],
        /// Store the data block as an entropy array, so the Kraken path runs.
        pub entropy: bool,
        pub retail: bool,
        /// Replace the CNT param.json (the default is `PARAM`).
        pub param: Option<&'a [u8]>,
    }

    fn pad(v: &mut Vec<u8>, to: usize) { v.resize(v.len().div_ceil(to) * to, 0); }

    fn dirent(out: &mut Vec<u8>, ino: u32, kind: u32, name: &str) {
        let size = (name.len() + 17).next_multiple_of(8);
        for v in [ino, kind, name.len() as u32, size as u32] { out.extend_from_slice(&v.to_le_bytes()); }
        out.extend_from_slice(name.as_bytes());
        out.resize(out.len() + size - 16 - name.len(), 0);
    }

    fn put(buf: &mut [u8], at: usize, v: &[u8]) { buf[at..at + v.len()].copy_from_slice(v); }

    fn inode(mode: u16, size: u64, logical: u64) -> Vec<u8> {
        let mut e = vec![0u8; INODE];
        put(&mut e, 0, &mode.to_le_bytes());
        put(&mut e, 8, &size.to_le_bytes());
        put(&mut e, 0x60, &logical.to_le_bytes());
        e
    }

    /// The inner file system: files first, then a superblock block, an inode block and a folder block.
    fn inner(files: &[(&str, &[u8])]) -> (Vec<u8>, usize) {
        let mut data = Vec::new();
        let mut at = Vec::new();
        for (_, bytes) in files { at.push(data.len()); data.extend_from_slice(bytes); pad(&mut data, 16); }
        assert!(data.len() <= BS);
        data.resize(BS, 0);
        let mut dirs: Vec<String> = vec![String::new()];
        for (p, _) in files { let mut acc = String::new(); for part in p.split('/').take(p.matches('/').count()) { if !acc.is_empty() { acc.push('/'); } acc.push_str(part); if !dirs.contains(&acc) { dirs.push(acc.clone()); } } }
        // Inode 0: super root, 1: uroot (= dirs[0]), then the other folders, then files.
        let dir_ino = |d: &str| dirs.iter().position(|x| x == d).unwrap() as u32 + 1;
        let file_ino = |i: usize| (dirs.len() + 1 + i) as u32;
        let meta = data.len();
        let mut folders = Vec::new();
        let mut dir_at = Vec::new();
        let mut root = Vec::new();
        dirent(&mut root, 0, 4, "."); dirent(&mut root, 0, 5, ".."); dirent(&mut root, 1, 3, "uroot");
        let root_at = folders.len(); folders.extend_from_slice(&root); pad(&mut folders, 8);
        for d in &dirs {
            let mut b = Vec::new();
            dirent(&mut b, dir_ino(d), 4, "."); dirent(&mut b, 0, 5, "..");
            for other in dirs.iter().filter(|o| !o.is_empty() && o.rsplit_once('/').map_or("", |p| p.0) == d) { dirent(&mut b, dir_ino(other), 3, other.rsplit('/').next().unwrap()); }
            for (i, (p, _)) in files.iter().enumerate().filter(|(_, (p, _))| p.rsplit_once('/').map_or("", |s| s.0) == d) { dirent(&mut b, file_ino(i), 2, p.rsplit('/').next().unwrap()); }
            dir_at.push((folders.len(), b.len()));
            folders.extend_from_slice(&b); pad(&mut folders, 8);
        }
        let folder_base = (meta + 2 * BS) as u64;
        let mut inodes = inode(0x416d, root.len() as u64, folder_base + root_at as u64);
        for (off, len) in &dir_at { inodes.extend(inode(0x416d, *len as u64, folder_base + *off as u64)); }
        for (i, (_, bytes)) in files.iter().enumerate() { inodes.extend(inode(0x816d, bytes.len() as u64, at[i] as u64)); }
        let count = 1 + dirs.len() + files.len();
        let mut sb = vec![0u8; BS];
        put(&mut sb, 0, &2i64.to_le_bytes());
        put(&mut sb, 8, &PFS_MAGIC.to_le_bytes());
        put(&mut sb, 0x1c, &0x18u16.to_le_bytes());
        put(&mut sb, 0x20, &(BS as u32).to_le_bytes());
        put(&mut sb, 0x30, &(count as i64).to_le_bytes());
        put(&mut sb, 0x38, &(((meta + 3 * BS) / BS) as i64).to_le_bytes());
        // 0xA8-byte inodes: a block holds BS / 0xA8 of them.
        assert!(count <= BS / INODE);
        let mut out = data;
        out.extend(sb);
        pad(&mut inodes, BS); out.extend(inodes);
        assert!(folders.len() <= BS);
        pad(&mut folders, BS); out.extend(folders);
        (out, meta)
    }

    fn naps(first_len: usize, tail_len: usize, outer_blocks: usize) -> Vec<u8> {
        let mount = (BS + tail_len) as u64;
        let entry = |at: u64, kind: u8| { let mut e = at.to_le_bytes()[..5].to_vec(); e.push(kind); e };
        let mut blob = Vec::new();
        let w0 = 2u64 | 2 << 32; // three offsets, two ublocks
        let w1 = outer_blocks as u64 | 1 << 24; // three records
        blob.extend(w0.to_le_bytes()); blob.extend(w1.to_le_bytes());
        blob.resize(blob.len() + outer_blocks * 8, 0);
        for (at, kind) in [(0, 0u8), (BS as u64, 0), (mount, 0x40)] { blob.extend(entry(at, kind)); }
        pad(&mut blob, 16);
        blob.resize(blob.len() + 10, 0); // one u2c entry
        pad(&mut blob, 8);
        let first_end = first_len as u64;
        for off in [0, first_end & 0x3_FFFF, (first_end + tail_len as u64) & 0x3_FFFF] {
            blob.extend_from_slice(&off.to_le_bytes()); blob.push(0);
        }
        blob
    }

    pub fn package(spec: &Spec) -> Vec<u8> {
        let (mount_bytes, meta) = inner(spec.files);
        let (data, tail) = mount_bytes.split_at(meta);
        let first = if spec.entropy {
            let mut p = vec![(data.len() >> 16) as u8, (data.len() >> 8) as u8, data.len() as u8];
            p.extend_from_slice(data);
            p
        } else { data.to_vec() };
        let mut image = first.clone();
        image.extend_from_slice(tail);
        let outer_blocks = image.len().div_ceil(BS);
        let naps = naps(first.len(), tail.len(), outer_blocks);
        let mut pfs = image.clone();
        pad(&mut pfs, BS);
        let naps_block = pfs.len() / BS;
        pfs.extend_from_slice(&naps);
        pad(&mut pfs, BS);
        let (sb_block, table_block, root_block, uroot_block) = (pfs.len() / BS, pfs.len() / BS + 1, pfs.len() / BS + 2, pfs.len() / BS + 3);
        let total = uroot_block + 1;
        let mut sb = vec![0u8; BS];
        put(&mut sb, 0, &2i64.to_le_bytes());
        put(&mut sb, 8, &PFS_MAGIC.to_le_bytes());
        put(&mut sb, 0x1c, &0x0du16.to_le_bytes());
        put(&mut sb, 0x20, &(BS as u32).to_le_bytes());
        put(&mut sb, 0x30, &4i64.to_le_bytes());
        put(&mut sb, 0x38, &(total as i64).to_le_bytes());
        put(&mut sb, 0x40, &1i64.to_le_bytes());
        put(&mut sb, 0xd8, &(table_block as i64).to_le_bytes());
        put(&mut sb, 0x370, NOAUTH_SEED);
        let outer_inode = |mode: u16, size: usize, first: usize, blocks: usize| {
            let mut e = vec![0u8; OUTER_INODE];
            put(&mut e, 0, &mode.to_le_bytes());
            put(&mut e, 8, &(size as u64).to_le_bytes());
            put(&mut e, 0x60, &(blocks as u32).to_le_bytes());
            for k in 0..blocks.min(12) { put(&mut e, 0x64 + k * 36 + 32, &((first + k) as i32).to_le_bytes()); }
            e
        };
        let mut table = outer_inode(0x416d, BS, root_block, 1);
        table.extend(outer_inode(0x416d, BS, uroot_block, 1));
        table.extend(outer_inode(0x816d, image.len(), 0, outer_blocks));
        table.extend(outer_inode(0x816d, naps.len(), naps_block, 1));
        pad(&mut table, BS);
        let (mut root, mut uroot) = (Vec::new(), Vec::new());
        dirent(&mut root, 0, 4, "."); dirent(&mut root, 0, 5, ".."); dirent(&mut root, 1, 3, "uroot");
        dirent(&mut uroot, 1, 4, "."); dirent(&mut uroot, 0, 5, ".."); dirent(&mut uroot, 2, 2, "pfs_image.dat"); dirent(&mut uroot, 3, 2, "naps_pkg_layout.dat");
        pad(&mut root, BS); pad(&mut uroot, BS);
        for part in [sb, table, root, uroot] { pfs.extend(part); }
        assert_eq!(pfs.len(), total * BS);

        let param = spec.param.unwrap_or(PARAM);
        let names = b"\0param.json\0".to_vec();
        let (table_at, names_at, param_at) = (0x80usize, 0xe0usize, 0x100usize);
        let mut cnt = vec![0u8; param_at];
        put(&mut cnt, 0, CNT_MAGIC);
        put(&mut cnt, 16, &2u32.to_be_bytes());
        put(&mut cnt, 24, &(table_at as u32).to_be_bytes());
        put(&mut cnt, 0x40, CONTENT_ID);
        for (i, (id, name, at, size)) in [(0x200u32, 0u32, names_at, names.len()), (0x2000, 1, param_at, param.len())].into_iter().enumerate() {
            let o = table_at + i * 0x20;
            put(&mut cnt, o, &id.to_be_bytes()); put(&mut cnt, o + 4, &name.to_be_bytes());
            put(&mut cnt, o + 16, &(at as u32).to_be_bytes()); put(&mut cnt, o + 20, &(size as u32).to_be_bytes());
        }
        put(&mut cnt, names_at, &names);
        cnt.extend_from_slice(param);

        let mut head = vec![0u8; BS];
        put(&mut head, 0, FIH_MAGIC);
        head[4] = 1;
        head[5] = if spec.retail { 0x80 } else { 0 };
        put(&mut head, 0x10, &(BS as u64).to_le_bytes());
        put(&mut head, 0x18, &(pfs.len() as u64).to_le_bytes());
        put(&mut head, 0x20, &((BS + sb_block * BS) as u64).to_le_bytes());
        put(&mut head, 0x58, &((BS + pfs.len()) as u64).to_le_bytes());
        [head, pfs, cnt].concat()
    }
}
