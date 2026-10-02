// SPDX-FileCopyrightText: Copyright 2024 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later
//! Read-only reader for PS4 fake packages (fPKG): `.pkg` files that start with the bytes
//! `7F 'C' 'N' 'T'` and whose keys are the public "fake" keys. Ported from shadPS4 v0.7.0
//! (src/core/file_format/pkg.cpp and src/core/crypto/crypto.cpp, GPL-2.0-or-later), with the
//! file system walk of LibOrbisPkg (MIT) in place of shadPS4's guesses.
//!
//! A package holds two things. A table of entries: `param.sfo`, icons and the other `sce_sys`
//! files, plus the keys. And a file system image ("outer PFS") encrypted with AES-XTS, whose only
//! file, `pfs_image.dat`, is a PFSC container: 64 KiB blocks, each stored raw or as zlib. Joined,
//! the blocks form a second file system ("inner PFS") that holds the game files. Every size and
//! offset is range-checked, so a damaged or hostile package fails with an error.

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockDecryptMut, BlockEncrypt, KeyInit, KeyIvInit};
use aes::Aes128;
use anyhow::{bail, ensure, Context, Result};
use hmac::{Hmac, Mac};
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPrivateKey};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, io::Read, os::unix::fs::FileExt};

use crate::pkg4_data as data;

/// First four bytes of a PS4 package ("CNT").
const CNT_MAGIC: &[u8; 4] = b"\x7fCNT";
/// Size of the package header.
const HEADER: usize = 0x1000;
/// AES-XTS sector of the outer image.
const SECTOR: u64 = 0x1000;
/// Block of a file system and of the PFSC container.
const BLOCK: u64 = 0x10000;
/// Version number stored in a file system header (a date, 2013-03-15).
const PFS_MAGIC: u64 = 20_130_315;
const PFSC_MAGIC: &[u8; 4] = b"PFSC";
/// Size of an unsigned 32-bit inode.
const DINODE: u64 = 0xA8;
/// File system mode bits.
const MODE_SIGNED: u16 = 0x1;
const MODE_64BIT: u16 = 0x2;
/// Folder entry types.
const DIRENT_FILE: u32 = 2;
const DIRENT_DIR: u32 = 3;
/// Limits, so a bad package cannot make us read or hold huge amounts.
const MAX_ENTRIES: u32 = 10_000;
const MAX_ENTRY_SIZE: u32 = 256 * 1024 * 1024;
const MAX_INODES: u64 = 1_000_000;
const MAX_FILES: usize = 1_000_000;
const MAX_DEPTH: usize = 128;
/// Longest path accepted, the installer's limit.
const MAX_PATH: usize = 4096;
/// Most PFSC blocks accepted: 2^24 blocks of 64 KiB is a 1 TiB image.
const MAX_BLOCKS: u64 = 1 << 24;
/// Content flags whose names contain "PATCH" (FIRST_PATCH, PATCHGO, SUBSEQUENT_PATCH,
/// DELTA_PATCH, CUMULATIVE_PATCH): the package is an update. Tested bit by bit, as shadPS4 does.
const PATCH_FLAGS: u32 = 0x0010_0000 | 0x0020_0000 | 0x4000_0000 | 0x4100_0000 | 0x6000_0000;
/// Package entries that hold keys.
const ENTRY_KEYS: u32 = 0x10;
const IMAGE_KEY: u32 = 0x20;
/// Package entries stored encrypted (license.dat, license.info, nptitle.dat, npbind.dat).
const NP_ENTRIES: [u32; 4] = [0x400, 0x401, 0x402, 0x403];

/// Whether the file starts like a PS4 package ("\x7fCNT").
pub fn is_package(file: &File) -> bool {
    let mut b = [0u8; 4];
    file.read_exact_at(&mut b, 0).is_ok() && &b == CNT_MAGIC
}

/// What a package holds: a base game, an update for it, or DLC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Game,
    Update,
    Dlc,
}

/// A file or folder of the package. `path` uses `/` and is relative to the folder the package
/// unpacks into (see [`Package::root`]).
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
    /// Index into the entry table.
    Entry(usize),
    /// First block in the inner file system.
    Inner(u64),
}

#[derive(Clone, Copy)]
struct Entry {
    id: u32,
    offset: u32,
    size: u32,
    /// The table record as shadPS4 hashes it: 24 bytes as stored, then 8 zero bytes.
    record: [u8; 32],
}

pub struct Package {
    file: File,
    title_id: String,
    kind: Kind,
    /// DLC only: the entitlement label (third part of param.sfo's CONTENT_ID).
    label: Option<String>,
    entries: Vec<Entry>,
    /// Key that decrypts the per-entry keys (from ENTRY_KEYS).
    dk3: [u8; 32],
    pfs_offset: u64,
    pfs_size: u64,
    data_key: Aes128,
    tweak_key: Aes128,
    /// First encrypted sector of the outer image (its header block is stored in the clear).
    xts_start: u64,
    /// Byte offset of the PFSC container in the outer image, and its block offsets.
    pfsc: u64,
    blocks: Vec<u64>,
    /// Last decoded PFSC block, by index.
    cache: Option<(u64, Vec<u8>)>,
}

fn be32(b: &[u8], at: usize) -> u32 { u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) }
fn be64(b: &[u8], at: usize) -> u64 { u64::from_be_bytes(b[at..at + 8].try_into().unwrap()) }
fn le32(b: &[u8], at: usize) -> u32 { u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) }
fn le64(b: &[u8], at: usize) -> u64 { u64::from_le_bytes(b[at..at + 8].try_into().unwrap()) }

fn read_at(file: &File, len: u64, at: u64, size: u64) -> Result<Vec<u8>> {
    ensure!(at.checked_add(size).is_some_and(|end| end <= len), "Package read outside the file");
    let mut buf = vec![0u8; size as usize];
    file.read_exact_at(&mut buf, at)?;
    Ok(buf)
}

fn rsa_key(n: &[u8], e: &[u8], d: &[u8], p: &[u8], q: &[u8]) -> RsaPrivateKey {
    let b = BigUint::from_bytes_be;
    RsaPrivateKey::from_components(b(n), b(e), b(d), vec![b(p), b(q)]).expect("built-in RSA key is valid")
}

/// RSA-2048 PKCS#1 v1.5 decryption; the first 32 bytes of the message are the key.
fn rsa_key32(key: &RsaPrivateKey, cipher: &[u8]) -> Option<[u8; 32]> {
    let plain = key.decrypt(Pkcs1v15Encrypt, cipher).ok()?;
    plain.get(..32)?.try_into().ok()
}

/// SHA-256 of an entry's table record and DK3: IV (first half) and AES key (second half).
fn entry_iv_key(record: &[u8; 32], dk3: &[u8; 32]) -> ([u8; 16], [u8; 16]) {
    let hash = Sha256::new().chain_update(record).chain_update(dk3).finalize();
    (hash[..16].try_into().unwrap(), hash[16..].try_into().unwrap())
}

/// AES-128-CBC decryption of whole 16-byte blocks; a shorter tail is left as stored.
fn cbc_decrypt(iv_key: ([u8; 16], [u8; 16]), data: &mut [u8]) {
    let (iv, key) = iv_key;
    let mut cbc = cbc::Decryptor::<Aes128>::new(&key.into(), &iv.into());
    let whole = data.len() / 16 * 16;
    for block in data[..whole].chunks_exact_mut(16) {
        cbc.decrypt_block_mut(GenericArray::from_mut_slice(block));
    }
}

/// The file system's data and tweak keys: HMAC-SHA256 of index 1 and the seed, keyed with EKPFS.
/// LibOrbisPkg derives once more for a "new crypt" header flag; real fake packages that set that
/// flag decrypt without it, as in shadPS4.
fn pfs_keys(ekpfs: &[u8; 32], seed: &[u8; 16]) -> (Aes128, Aes128) {
    let mut msg = [0u8; 20];
    msg[..4].copy_from_slice(&1u32.to_le_bytes());
    msg[4..].copy_from_slice(seed);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(ekpfs).expect("HMAC takes any key size");
    mac.update(&msg);
    let d = mac.finalize().into_bytes();
    (Aes128::new_from_slice(&d[16..]).unwrap(), Aes128::new_from_slice(&d[..16]).unwrap())
}

/// AES-128-XTS decryption of one sector. The tweak is the sector number (little-endian).
fn xts_decrypt(data_key: &Aes128, tweak_key: &Aes128, sector: u64, buf: &mut [u8]) {
    let mut tweak = [0u8; 16];
    tweak[..8].copy_from_slice(&sector.to_le_bytes());
    tweak_key.encrypt_block(GenericArray::from_mut_slice(&mut tweak));
    for block in buf.chunks_exact_mut(16) {
        block.iter_mut().zip(&tweak).for_each(|(b, t)| *b ^= t);
        data_key.decrypt_block(GenericArray::from_mut_slice(block));
        block.iter_mut().zip(&tweak).for_each(|(b, t)| *b ^= t);
        // Multiply the tweak by x in GF(2^128).
        let carry = tweak[15] >> 7;
        for i in (1..16).rev() {
            tweak[i] = (tweak[i] << 1) | (tweak[i - 1] >> 7);
        }
        tweak[0] = (tweak[0] << 1) ^ (carry * 0x87);
    }
}

/// The sce_sys file name of an entry; entries without one are named by their ID.
fn entry_name(id: u32) -> String {
    match data::ENTRY_NAMES.binary_search_by_key(&id, |&(i, _)| i) {
        Ok(i) => data::ENTRY_NAMES[i].1.to_string(),
        Err(_) => id.to_string(),
    }
}

impl Package {
    /// Open a package and derive its keys. Fails for packages that aren't fake packages.
    pub fn open(file: File, cancel: &dyn Fn() -> Result<()>) -> Result<Self> {
        let len = file.metadata()?.len();
        let head = read_at(&file, len, 0, HEADER as u64).context("Read package header")?;
        ensure!(&head[..4] == CNT_MAGIC, "Not a PS4 package");
        let count = be32(&head, 0x10);
        let table = be32(&head, 0x18) as u64;
        ensure!(count <= MAX_ENTRIES, "Too many package entries");
        let title_id = String::from_utf8_lossy(&head[0x47..0x50]).into_owned();
        ensure!(crate::psn::valid_title_id(&title_id), "Invalid title ID in the package header");
        let flags = be32(&head, 0x78);
        let (pfs_offset, pfs_size) = (be64(&head, 0x410), be64(&head, 0x418));
        ensure!(pfs_offset.checked_add(pfs_size).is_some_and(|end| end <= len) && pfs_size >= 2 * BLOCK,
            "Package file system lies outside the file (is the download complete?)");

        let raw = read_at(&file, len, table, count as u64 * 32).context("Read package entry table")?;
        let mut entries = Vec::with_capacity(count as usize);
        for r in raw.chunks_exact(32) {
            let (id, offset, size) = (be32(r, 0), be32(r, 16), be32(r, 20));
            ensure!(size <= MAX_ENTRY_SIZE && (offset as u64 + size as u64) <= len, "Package entry {id:#x} lies outside the file");
            let mut record = [0u8; 32];
            record[..24].copy_from_slice(&r[..24]);
            entries.push(Entry { id, offset, size, record });
        }
        let find = |id: u32| entries.iter().find(|e| e.id == id).copied();
        let entry = |id: u32| -> Result<Vec<u8>> {
            let e = find(id).with_context(|| format!("Package has no entry {id:#x}"))?;
            read_at(&file, len, e.offset as u64, e.size as u64)
        };
        cancel()?;

        // DK3: the fourth of seven RSA-encrypted keys after a seed and seven digests.
        let keys = entry(ENTRY_KEYS)?;
        ensure!(keys.len() >= 32 + 7 * 32 + 7 * 256, "Package key entry is too short");
        let dk3_key = rsa_key(&data::DK3_N, &data::DK3_E, &data::DK3_D, &data::DK3_P, &data::DK3_Q);
        let at = 32 + 7 * 32 + 3 * 256;
        let not_fake = "This PS4 package isn't a fake package (fPKG), so the launcher can't unpack it";
        let dk3 = rsa_key32(&dk3_key, &keys[at..at + 256]).context(not_fake)?;
        // EKPFS: the image key, AES-CBC encrypted with its entry key, then RSA with the fake key.
        let image = find(IMAGE_KEY).context("Package has no image key")?;
        let mut image_key = entry(IMAGE_KEY)?;
        ensure!(image_key.len() >= 256, "Package image key is too short");
        image_key.truncate(256);
        cbc_decrypt(entry_iv_key(&image.record, &dk3), &mut image_key);
        let fake = rsa_key(&data::FAKE_N, &data::FAKE_E, &data::FAKE_D, &data::FAKE_P, &data::FAKE_Q);
        let ekpfs = rsa_key32(&fake, &image_key).context(not_fake)?;
        let sfo = crate::sfo::parse(&entry(0x1000).context("Package has no param.sfo")?).context("Invalid param.sfo in the package")?;
        let text = |key: &str| sfo.get(key).map(|v| v.text().to_string());
        // As the PKG extractor decided: a "PATCH" content flag makes an update, then CATEGORY "ac" DLC.
        let kind = if flags & PATCH_FLAGS != 0 { Kind::Update } else if text("CATEGORY").as_deref() == Some("ac") { Kind::Dlc } else { Kind::Game };
        // The label becomes one folder name: letters, digits and "_" only.
        let label = text("CONTENT_ID").and_then(|c| c.split('-').nth(2).map(str::to_string))
            .filter(|l| !l.is_empty() && l.len() <= 64 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        cancel()?;

        // The outer file system: its header block is in the clear, the rest is AES-XTS.
        let outer = read_at(&file, len, pfs_offset, 0x400)?;
        ensure!(le64(&outer, 0) == 1 && le64(&outer, 8) == PFS_MAGIC, "Invalid package file system header");
        let block_size = le32(&outer, 0x20) as u64;
        ensure!(block_size >= SECTOR && block_size % SECTOR == 0 && block_size <= 0x10_0000, "Unsupported package block size");
        let seed: [u8; 16] = outer[0x370..0x380].try_into().unwrap();
        let (data_key, tweak_key) = pfs_keys(&ekpfs, &seed);
        let mut pkg = Package {
            file, title_id, kind, label, entries, dk3, pfs_offset, pfs_size, data_key, tweak_key,
            xts_start: block_size / SECTOR, pfsc: 0, blocks: Vec::new(), cache: None,
        };

        // The PFSC container (pfs_image.dat) starts on a 64 KiB boundary after the first two
        // blocks. Found by its magic, as shadPS4 does; a wrong key finds none.
        let mut at = 2 * BLOCK;
        let pfsc = loop {
            ensure!(at + 0x30 <= pfs_size, "{not_fake} (no PFSC container found)");
            if &pkg.read_outer(at, 4)?[..] == PFSC_MAGIC { break at; }
            at += BLOCK;
            if at % (256 * BLOCK) == 0 { cancel()?; }
        };
        let head = pkg.read_outer(pfsc, 0x30)?;
        let (block_sz, block_sz2, offsets, length) = (le32(&head, 0xC) as u64, le64(&head, 0x10), le64(&head, 0x18), le64(&head, 0x28));
        ensure!(block_sz == BLOCK && block_sz2 == BLOCK, "Unsupported PFSC block size");
        let n = length / BLOCK;
        ensure!(n <= MAX_BLOCKS, "PFSC container is too large");
        let table_at = offsets.checked_add(pfsc).filter(|at| at.checked_add((n + 1) * 8).is_some_and(|end| end <= pfs_size));
        let table = pkg.read_outer(table_at.context("PFSC block table lies outside the package")?, (n + 1) * 8)?;
        let blocks: Vec<u64> = table.chunks_exact(8).map(|c| le64(c, 0)).collect();
        ensure!(blocks.windows(2).all(|w| w[0] <= w[1]) && blocks[n as usize].checked_add(pfsc).is_some_and(|end| end <= pfs_size),
            "Damaged PFSC block table");
        pkg.pfsc = pfsc;
        pkg.blocks = blocks;
        Ok(pkg)
    }

    pub fn kind(&self) -> Kind { self.kind }

    /// The folder this package unpacks into, as shadPS4 expects it: `<title>` for a game,
    /// `<title>-patch` for an update, `<title>/<entitlement label>` for DLC.
    pub fn root(&self) -> Result<String> {
        Ok(match self.kind {
            Kind::Game => self.title_id.clone(),
            Kind::Update => format!("{}-patch", self.title_id),
            Kind::Dlc => format!("{}/{}", self.title_id, self.label.as_deref().context("The DLC's param.sfo has no valid CONTENT_ID label")?),
        })
    }

    /// Read `size` bytes at `at` in the outer image, decrypting the sectors they lie in.
    fn read_outer(&self, at: u64, size: u64) -> Result<Vec<u8>> {
        let end = at.checked_add(size).filter(|&e| e <= self.pfs_size).context("Read outside the package file system")?;
        let first = at / SECTOR;
        let last = end.div_ceil(SECTOR);
        let mut buf = vec![0u8; ((last - first) * SECTOR) as usize];
        let len = self.pfs_offset + self.pfs_size;
        let avail = (len - (self.pfs_offset + first * SECTOR)).min(buf.len() as u64) as usize;
        self.file.read_exact_at(&mut buf[..avail], self.pfs_offset + first * SECTOR)?;
        for (i, sector) in buf.chunks_exact_mut(SECTOR as usize).enumerate() {
            let n = first + i as u64;
            if n >= self.xts_start {
                xts_decrypt(&self.data_key, &self.tweak_key, n, sector);
            }
        }
        let skip = (at - first * SECTOR) as usize;
        Ok(buf[skip..skip + size as usize].to_vec())
    }

    /// One 64 KiB block of the inner file system.
    fn block(&mut self, index: u64) -> Result<&[u8]> {
        if self.cache.as_ref().is_none_or(|(i, _)| *i != index) {
            ensure!(index + 1 < self.blocks.len() as u64, "Inner file system block {index} out of range");
            let (start, end) = (self.blocks[index as usize], self.blocks[index as usize + 1]);
            let stored = end - start;
            let out = if stored == BLOCK {
                self.read_outer(self.pfsc + start, BLOCK)?
            } else if stored > BLOCK || stored == 0 {
                vec![0u8; BLOCK as usize] // a block of zeros is stored as a gap
            } else {
                // Read to the end of the stream, so zlib checks its checksum; a block is always 64 KiB.
                let packed = self.read_outer(self.pfsc + start, stored)?;
                let mut out = Vec::with_capacity(BLOCK as usize);
                flate2::read::ZlibDecoder::new(&packed[..]).take(BLOCK + 1).read_to_end(&mut out).context("Damaged compressed block")?;
                ensure!(out.len() as u64 == BLOCK, "Damaged compressed block");
                out
            };
            self.cache = Some((index, out));
        }
        Ok(&self.cache.as_ref().unwrap().1)
    }

    /// Inode `ino` of the inner file system: (is a folder, size, first block, block count).
    fn inode(&mut self, ino: u64, count: u64) -> Result<(bool, u64, u64, u64)> {
        ensure!(ino < count, "Inode {ino} out of range");
        let per_block = BLOCK / DINODE;
        let block = self.block(1 + ino / per_block)?;
        let at = ((ino % per_block) * DINODE) as usize;
        let n = &block[at..at + DINODE as usize];
        let mode = u16::from_le_bytes([n[0], n[1]]);
        let (size, blocks, start) = (le64(n, 8), le32(n, 0x60) as u64, le32(n, 0x64) as u64);
        ensure!(size <= blocks.saturating_mul(BLOCK) && start.checked_add(blocks).is_some_and(|e| e < self.blocks.len() as u64),
            "Inode {ino} is damaged");
        Ok((mode & 0x4000 != 0, size, start, blocks))
    }

    /// Every file and folder: the sce_sys entries first, then the inner file system's `uroot`.
    /// A file the file system also holds comes from the file system.
    pub fn list(&mut self, cancel: &dyn Fn() -> Result<()>) -> Result<Vec<Node>> {
        let head = self.block(0)?.to_vec();
        ensure!(le64(&head, 0) == 1 && le64(&head, 8) == PFS_MAGIC, "Invalid inner file system header");
        let mode = u16::from_le_bytes([head[0x1C], head[0x1D]]);
        ensure!(mode & (MODE_SIGNED | MODE_64BIT) == 0, "Unsupported inner file system (signed or 64-bit inodes)");
        ensure!(le32(&head, 0x20) as u64 == BLOCK, "Unsupported inner file system block size");
        let count = le64(&head, 0x30);
        ensure!(count > 0 && count <= MAX_INODES, "Invalid inode count");

        let mut files: BTreeMap<String, Node> = BTreeMap::new();
        // Each folder is listed once, so a folder that lists itself (a loop) is refused.
        let mut listed = std::collections::HashSet::new();
        let mut uroot = false;
        // Folders: (inode, path, depth); the super-root (inode 0) holds `uroot`, the game's root.
        let mut stack = vec![(0u64, None::<String>, 0usize)];
        while let Some((ino, path, depth)) = stack.pop() {
            cancel()?;
            ensure!(depth <= MAX_DEPTH, "Package folders nest too deep");
            ensure!(listed.insert(ino), "Package folders form a loop");
            let (dir, _, start, blocks) = self.inode(ino, count)?;
            ensure!(dir || ino == 0, "Inode {ino} is not a folder");
            for b in start..start + blocks {
                let block = self.block(b)?.to_vec();
                let mut at = 0usize;
                while at + 16 <= block.len() {
                    let (child, kind, name_len, size) = (le32(&block, at) as u64, le32(&block, at + 4), le32(&block, at + 8) as usize, le32(&block, at + 12) as usize);
                    if size == 0 { break; }
                    ensure!(size >= 16 && at + size <= block.len() && 16 + name_len <= size, "Damaged folder entry");
                    let name = String::from_utf8_lossy(&block[at + 16..at + 16 + name_len]).into_owned();
                    at += size;
                    if kind != DIRENT_FILE && kind != DIRENT_DIR { continue; } // "." and ".."
                    ensure!(!name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']),
                        "Unsafe file name in the package: {name:?}");
                    let full = match &path {
                        None if name == "uroot" && kind == DIRENT_DIR && !uroot => { uroot = true; stack.push((child, Some(String::new()), depth + 1)); continue; }
                        None => continue, // flat_path_table and other super-root files
                        Some(p) if p.is_empty() => name.clone(),
                        Some(p) => format!("{p}/{name}"),
                    };
                    ensure!(full.len() <= MAX_PATH, "Package path too long");
                    ensure!(files.len() < MAX_FILES, "Too many files in the package");
                    if kind == DIRENT_DIR {
                        files.insert(full.clone(), Node { path: full.clone(), directory: true, size: 0, origin: Origin::None });
                        stack.push((child, Some(full), depth + 1));
                    } else {
                        let (_, size, start, _) = self.inode(child, count)?;
                        files.insert(full.clone(), Node { path: full, directory: false, size, origin: Origin::Inner(start) });
                    }
                }
            }
        }
        ensure!(uroot, "Package file system has no uroot folder");

        let mut nodes = Vec::new();
        if !files.contains_key("sce_sys") {
            nodes.push(Node { path: "sce_sys".into(), directory: true, size: 0, origin: Origin::None });
        }
        for (i, e) in self.entries.iter().enumerate() {
            let path = format!("sce_sys/{}", entry_name(e.id));
            if files.contains_key(&path) { continue; }
            if let Some((parent, _)) = path.rsplit_once('/').filter(|(p, _)| *p != "sce_sys") {
                if !files.contains_key(parent) && !nodes.iter().any(|n: &Node| n.path == parent) {
                    nodes.push(Node { path: parent.to_string(), directory: true, size: 0, origin: Origin::None });
                }
            }
            nodes.push(Node { path, directory: false, size: e.size as u64, origin: Origin::Entry(i) });
        }
        nodes.extend(files.into_values());
        Ok(nodes)
    }

    /// Stream a file's bytes to `sink`, at most one 64 KiB block at a time.
    pub fn copy(&mut self, node: &Node, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        match node.origin {
            Origin::None => bail!("{} is not a file", node.path),
            Origin::Entry(i) => {
                let e = self.entries[i];
                let mut buf = vec![0u8; e.size as usize];
                self.file.read_exact_at(&mut buf, e.offset as u64)?;
                if NP_ENTRIES.contains(&e.id) {
                    cbc_decrypt(entry_iv_key(&e.record, &self.dk3), &mut buf);
                }
                sink(&buf)
            }
            Origin::Inner(start) => {
                let mut left = node.size;
                let mut b = start;
                while left > 0 {
                    let n = left.min(BLOCK);
                    let block = self.block(b)?;
                    sink(&block[..n as usize])?;
                    left -= n;
                    b += 1;
                }
                Ok(())
            }
        }
    }
}

/// Builds small fake packages for tests, the inverse of the reader: RSA with the public halves of
/// the same keys, AES-CBC for the image key, AES-XTS for the outer image.
#[cfg(test)]
pub(crate) mod build {
    use super::*;
    use rsa::rand_core::{CryptoRng, RngCore};
    use rsa::traits::PublicKeyParts;

    pub struct Spec<'a> {
        pub files: &'a [(&'a str, &'a [u8])],
        pub kind: Kind,
        /// Store file blocks as zlib (otherwise raw); a block of zeros is always stored as a gap.
        pub compressed: bool,
        /// Encrypt the image key for a key that isn't the fake one.
        pub retail: bool,
        /// Empty files added to uroot/many, enough for its listing to span several blocks.
        pub many: usize,
        /// DLC's entitlement label.
        pub label: &'a str,
    }

    impl Default for Spec<'_> {
        fn default() -> Self { Spec { files: &[], kind: Kind::Game, compressed: false, retail: false, many: 0, label: "ADDON00000000001" } }
    }

    /// Deterministic bytes for RSA padding; tests need no real randomness.
    struct Fixed(u64);
    impl RngCore for Fixed {
        fn next_u32(&mut self) -> u32 { self.next_u64() as u32 }
        fn next_u64(&mut self) -> u64 { self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); self.0 >> 11 }
        fn fill_bytes(&mut self, dest: &mut [u8]) { dest.iter_mut().for_each(|b| *b = (self.next_u64() as u8) | 1); }
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rsa::rand_core::Error> { self.fill_bytes(dest); Ok(()) }
    }
    impl CryptoRng for Fixed {}

    fn rsa_encrypt(key: &RsaPrivateKey, msg: &[u8]) -> Vec<u8> {
        let out = key.to_public_key().encrypt(&mut Fixed(7), Pkcs1v15Encrypt, msg).unwrap();
        assert_eq!(out.len(), key.size());
        out
    }

    fn cbc_encrypt(iv_key: ([u8; 16], [u8; 16]), data: &mut [u8]) {
        use aes::cipher::BlockEncryptMut;
        let (iv, key) = iv_key;
        let mut cbc = cbc::Encryptor::<Aes128>::new(&key.into(), &iv.into());
        for block in data.chunks_exact_mut(16) { cbc.encrypt_block_mut(GenericArray::from_mut_slice(block)); }
    }

    fn xts_encrypt(data_key: &Aes128, tweak_key: &Aes128, sector: u64, buf: &mut [u8]) {
        let mut tweak = [0u8; 16];
        tweak[..8].copy_from_slice(&sector.to_le_bytes());
        tweak_key.encrypt_block(GenericArray::from_mut_slice(&mut tweak));
        for block in buf.chunks_exact_mut(16) {
            block.iter_mut().zip(&tweak).for_each(|(b, t)| *b ^= t);
            data_key.encrypt_block(GenericArray::from_mut_slice(block));
            block.iter_mut().zip(&tweak).for_each(|(b, t)| *b ^= t);
            let carry = tweak[15] >> 7;
            for i in (1..16).rev() { tweak[i] = (tweak[i] << 1) | (tweak[i - 1] >> 7); }
            tweak[0] = (tweak[0] << 1) ^ (carry * 0x87);
        }
    }

    fn put32(v: &mut [u8], at: usize, x: u32) { v[at..at + 4].copy_from_slice(&x.to_le_bytes()); }
    fn put64(v: &mut [u8], at: usize, x: u64) { v[at..at + 8].copy_from_slice(&x.to_le_bytes()); }

    /// The inner file system: block 0 header, inode blocks, then folder listings and file data.
    fn inner(spec: &Spec) -> Vec<u8> {
        // Folders and files by path; inode 0 is the super-root, 1 is uroot.
        let mut paths: Vec<(String, Option<Vec<u8>>)> = Vec::new();
        for (p, bytes) in spec.files {
            let parts: Vec<&str> = p.split('/').collect();
            for i in 1..parts.len() {
                let dir = parts[..i].join("/");
                if !paths.iter().any(|(q, _)| *q == dir) { paths.push((dir, None)); }
            }
            paths.push((p.to_string(), Some(bytes.to_vec())));
        }
        if spec.many > 0 {
            paths.push(("many".into(), None));
            for i in 0..spec.many { paths.push((format!("many/file-with-a-long-name-{i:05}"), Some(Vec::new()))); }
        }
        let count = 2 + paths.len() as u64 + 1; // + flat_path_table
        let inode_blocks = (count).div_ceil(BLOCK / DINODE);
        let mut blocks: Vec<Vec<u8>> = vec![vec![0u8; BLOCK as usize]; 1 + inode_blocks as usize];
        put64(&mut blocks[0], 0, 1);
        put64(&mut blocks[0], 8, PFS_MAGIC);
        put32(&mut blocks[0], 0x20, BLOCK as u32);
        put64(&mut blocks[0], 0x30, count);
        let mut inodes: Vec<(u16, u64, u32, u32)> = vec![(0x4000, 0, 0, 0); count as usize];
        let add_data = |blocks: &mut Vec<Vec<u8>>, bytes: &[u8]| -> (u32, u32) {
            let start = blocks.len() as u32;
            for chunk in bytes.chunks(BLOCK as usize) {
                let mut b = chunk.to_vec();
                b.resize(BLOCK as usize, 0);
                blocks.push(b);
            }
            (start, bytes.len().div_ceil(BLOCK as usize) as u32)
        };
        let dirent = |out: &mut Vec<u8>, ino: u64, kind: u32, name: &str| {
            let size = (name.len() + 17).next_multiple_of(8);
            if out.len() % BLOCK as usize + size > BLOCK as usize { out.resize(out.len().next_multiple_of(BLOCK as usize), 0); }
            let at = out.len();
            out.resize(at + size, 0);
            put32(out, at, ino as u32); put32(out, at + 4, kind); put32(out, at + 8, name.len() as u32); put32(out, at + 12, size as u32);
            out[at + 16..at + 16 + name.len()].copy_from_slice(name.as_bytes());
        };
        let ino_of = |path: &str| 2 + paths.iter().position(|(q, _)| q == path).unwrap() as u64;
        let flat = count - 1;
        // Super-root listing.
        let mut list = Vec::new();
        dirent(&mut list, 0, 4, "."); dirent(&mut list, 0, 5, "..");
        dirent(&mut list, flat, DIRENT_FILE, "flat_path_table"); dirent(&mut list, 1, DIRENT_DIR, "uroot");
        let (s, n) = add_data(&mut blocks, &list);
        inodes[0] = (0x4000, list.len() as u64, s, n);
        inodes[flat as usize] = (0x8000, 0, 0, 0);
        // Folders (uroot is "").
        let mut dirs = vec![String::new()];
        dirs.extend(paths.iter().filter(|(_, b)| b.is_none()).map(|(p, _)| p.clone()));
        for dir in &dirs {
            let mut list = Vec::new();
            let me = if dir.is_empty() { 1 } else { ino_of(dir) };
            dirent(&mut list, me, 4, "."); dirent(&mut list, me, 5, "..");
            for (p, bytes) in &paths {
                let (parent, name) = p.rsplit_once('/').unwrap_or(("", p));
                if parent == dir { dirent(&mut list, ino_of(p), if bytes.is_some() { DIRENT_FILE } else { DIRENT_DIR }, name); }
            }
            let (s, n) = add_data(&mut blocks, &list);
            inodes[me as usize] = (0x4000, list.len() as u64, s, n);
        }
        for (p, bytes) in &paths {
            if let Some(bytes) = bytes {
                let (s, n) = add_data(&mut blocks, bytes);
                inodes[ino_of(p) as usize] = (0x8000, bytes.len() as u64, s, n);
            }
        }
        for (i, (mode, size, start, n)) in inodes.iter().enumerate() {
            let (b, at) = (1 + i / (BLOCK / DINODE) as usize, (i % (BLOCK / DINODE) as usize) * DINODE as usize);
            blocks[b][at..at + 2].copy_from_slice(&mode.to_le_bytes());
            put64(&mut blocks[b], at + 8, *size);
            put32(&mut blocks[b], at + 0x60, *n);
            put32(&mut blocks[b], at + 0x64, *start);
        }
        blocks.concat()
    }

    /// The PFSC container of `image`, its blocks raw or zlib; zero blocks are stored as gaps.
    fn pfsc(image: &[u8], compressed: bool) -> Vec<u8> {
        use std::io::Write;
        let n = image.len() as u64 / BLOCK;
        let table = 0x30u64;
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        let base = (table + (n + 1) * 8).next_multiple_of(16);
        for b in image.chunks(BLOCK as usize) {
            offsets.push(base + data.len() as u64);
            if b.iter().all(|&x| x == 0) {
                data.resize(data.len() + BLOCK as usize + 1, 0xEE); // a gap: longer than a block
            } else if compressed {
                let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                z.write_all(b).unwrap();
                let packed = z.finish().unwrap();
                assert!(packed.len() < BLOCK as usize);
                data.extend_from_slice(&packed);
            } else {
                data.extend_from_slice(b);
            }
        }
        offsets.push(base + data.len() as u64);
        let mut out = vec![0u8; base as usize];
        out[..4].copy_from_slice(PFSC_MAGIC);
        put32(&mut out, 0xC, BLOCK as u32);
        put64(&mut out, 0x10, BLOCK);
        put64(&mut out, 0x18, table);
        put64(&mut out, 0x28, n * BLOCK);
        for (i, o) in offsets.iter().enumerate() { put64(&mut out, table as usize + i * 8, *o); }
        out.extend_from_slice(&data);
        out
    }

    pub fn package(spec: &Spec) -> Vec<u8> {
        let tid = "CUSA12345";
        let (category, label, flags) = match spec.kind {
            Kind::Game => ("gd", "GAME000000000000", 0x0a00_0000u32),
            Kind::Update => ("gp", "GAME000000000000", 0x6230_0000),
            Kind::Dlc => ("ac", spec.label, 0x0a00_0000),
        };
        let content = format!("UP0000-{tid}_00-{label}");
        let sfo = crate::sfo::build(&[("CATEGORY", crate::sfo::Value::Text(category.into())), ("TITLE_ID", crate::sfo::Value::Text(tid.into())),
            ("CONTENT_ID", crate::sfo::Value::Text(content.clone())), ("TITLE", crate::sfo::Value::Text("Fixture".into()))]);
        let dk3 = [0x33u8; 32];
        let ekpfs = [0x44u8; 32];
        let seed = [0x55u8; 16];
        let license = b"license bytes, 37 long: a tail stays".to_vec();
        // Entries: keys, image key, license.dat (encrypted), param.sfo, icon0.png.
        let ids = [ENTRY_KEYS, IMAGE_KEY, 0x400, 0x1000, 0x1200];
        let table = 0x2000usize;
        let mut body = Vec::new();
        let mut records = Vec::new();
        let mut contents: Vec<Vec<u8>> = Vec::new();
        for &id in &ids {
            contents.push(match id {
                ENTRY_KEYS => {
                    let dk3_key = rsa_key(&data::DK3_N, &data::DK3_E, &data::DK3_D, &data::DK3_P, &data::DK3_Q);
                    let mut v = vec![0u8; 32 + 7 * 32 + 7 * 256];
                    let at = 32 + 7 * 32 + 3 * 256;
                    v[at..at + 256].copy_from_slice(&rsa_encrypt(&dk3_key, &dk3));
                    v
                }
                IMAGE_KEY => vec![0u8; 256], // filled once its record is known
                0x400 => license.clone(),
                0x1000 => sfo.clone(),
                _ => b"PNG fixture".to_vec(),
            });
        }
        let start = table + ids.len() * 32;
        for (i, &id) in ids.iter().enumerate() {
            let offset = (start + body.len()) as u32;
            let mut r = [0u8; 32];
            r[..4].copy_from_slice(&id.to_be_bytes());
            r[16..20].copy_from_slice(&offset.to_be_bytes());
            r[20..24].copy_from_slice(&(contents[i].len() as u32).to_be_bytes());
            records.push(r);
            match id {
                IMAGE_KEY => {
                    let key = if spec.retail { rsa_key(&data::DK3_N, &data::DK3_E, &data::DK3_D, &data::DK3_P, &data::DK3_Q) }
                        else { rsa_key(&data::FAKE_N, &data::FAKE_E, &data::FAKE_D, &data::FAKE_P, &data::FAKE_Q) };
                    let mut v = rsa_encrypt(&key, &ekpfs);
                    cbc_encrypt(entry_iv_key(&r, &dk3), &mut v);
                    contents[i] = v;
                }
                0x400 => cbc_encrypt(entry_iv_key(&r, &dk3), &mut contents[i][..32]),
                _ => {}
            }
            body.extend_from_slice(&contents[i]);
            body.resize(body.len().next_multiple_of(16), 0);
        }
        // Outer image: header block in the clear, an empty block, then the PFSC container.
        let mut outer = vec![0u8; 2 * BLOCK as usize];
        put64(&mut outer, 0, 1);
        put64(&mut outer, 8, PFS_MAGIC);
        put32(&mut outer, 0x20, BLOCK as u32);
        outer[0x370..0x380].copy_from_slice(&seed);
        outer.extend_from_slice(&pfsc(&inner(spec), spec.compressed));
        outer.resize(outer.len().next_multiple_of(SECTOR as usize), 0);
        let (data_key, tweak_key) = pfs_keys(&ekpfs, &seed);
        for (i, sector) in outer.chunks_exact_mut(SECTOR as usize).enumerate().skip((BLOCK / SECTOR) as usize) {
            xts_encrypt(&data_key, &tweak_key, i as u64, sector);
        }
        let pfs_offset = (start + body.len()).next_multiple_of(0x10000);
        let mut out = vec![0u8; pfs_offset];
        out[..4].copy_from_slice(CNT_MAGIC);
        out[0x10..0x14].copy_from_slice(&(ids.len() as u32).to_be_bytes());
        out[0x18..0x1C].copy_from_slice(&(table as u32).to_be_bytes());
        out[0x40..0x40 + content.len()].copy_from_slice(content.as_bytes());
        out[0x78..0x7C].copy_from_slice(&flags.to_be_bytes());
        out[0x410..0x418].copy_from_slice(&(pfs_offset as u64).to_be_bytes());
        out[0x418..0x420].copy_from_slice(&(outer.len() as u64).to_be_bytes());
        for (i, r) in records.iter().enumerate() { out[table + i * 32..table + i * 32 + 32].copy_from_slice(r); }
        out[start..start + body.len()].copy_from_slice(&body);
        out.extend_from_slice(&outer);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use build::{package, Spec};

    fn open(bytes: &[u8]) -> Result<Package> {
        let t = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(t.path(), bytes).unwrap();
        Package::open(File::open(t.path()).unwrap(), &|| Ok(()))
    }

    fn read(pkg: &mut Package, nodes: &[Node], path: &str) -> Vec<u8> {
        let node = nodes.iter().find(|n| n.path == path).unwrap_or_else(|| panic!("missing {path}"));
        let mut out = Vec::new();
        pkg.copy(node, &mut |c| { out.extend_from_slice(c); Ok(()) }).unwrap();
        assert_eq!(out.len() as u64, node.size, "{path}");
        out
    }

    #[test]
    fn game_files_unpack_from_raw_compressed_and_zero_blocks() {
        let big: Vec<u8> = (0..2 * BLOCK as usize + 5).map(|i| (i * 7 % 251) as u8).collect();
        let zeros = vec![0u8; BLOCK as usize];
        let files: [(&str, &[u8]); 4] = [("eboot.bin", b"\x4f\x15\x3d\x1dfixture"), ("sce_module/a.prx", b"module"), ("data/big.bin", &big), ("data/zeros.bin", &zeros)];
        for compressed in [false, true] {
            let mut pkg = open(&package(&Spec { files: &files, compressed, ..Default::default() })).unwrap();
            assert_eq!((pkg.kind(), pkg.root().unwrap().as_str(), pkg.title_id.as_str()), (Kind::Game, "CUSA12345", "CUSA12345"));
            let nodes = pkg.list(&|| Ok(())).unwrap();
            for (path, bytes) in files { assert_eq!(read(&mut pkg, &nodes, path), bytes, "{path}, compressed {compressed}"); }
            assert!(nodes.iter().any(|n| n.path == "sce_module" && n.directory));
            assert!(crate::sfo::parse(&read(&mut pkg, &nodes, "sce_sys/param.sfo")).is_some());
            assert_eq!(read(&mut pkg, &nodes, "sce_sys/icon0.png"), b"PNG fixture");
            assert_eq!(read(&mut pkg, &nodes, "sce_sys/license.dat"), b"license bytes, 37 long: a tail stays", "decrypted; the short tail as stored");
            assert!(nodes.iter().any(|n| n.path == "sce_sys/entry_keys"), "every entry is kept, as the extractor did");
        }
    }

    #[test]
    fn updates_and_addons_unpack_where_shadps4_looks() {
        let files: [(&str, &[u8]); 1] = [("eboot.bin", b"x")];
        let update = open(&package(&Spec { files: &files, kind: Kind::Update, ..Default::default() })).unwrap();
        assert_eq!((update.kind(), update.root().unwrap()), (Kind::Update, "CUSA12345-patch".to_string()));
        let addon = open(&package(&Spec { files: &files, kind: Kind::Dlc, ..Default::default() })).unwrap();
        assert_eq!((addon.kind(), addon.root().unwrap()), (Kind::Dlc, "CUSA12345/ADDON00000000001".to_string()));
        let nested = open(&package(&Spec { files: &files, kind: Kind::Dlc, label: "A/B", ..Default::default() })).unwrap();
        assert!(nested.root().is_err(), "a label is one folder name");
    }

    #[test]
    fn a_file_system_file_replaces_the_entry_of_the_same_name() {
        let files: [(&str, &[u8]); 2] = [("eboot.bin", b"x"), ("sce_sys/icon0.png", b"from the file system")];
        let mut pkg = open(&package(&Spec { files: &files, ..Default::default() })).unwrap();
        let nodes = pkg.list(&|| Ok(())).unwrap();
        assert_eq!(nodes.iter().filter(|n| n.path == "sce_sys/icon0.png").count(), 1);
        assert_eq!(nodes.iter().filter(|n| n.path == "sce_sys").count(), 1);
        assert_eq!(read(&mut pkg, &nodes, "sce_sys/icon0.png"), b"from the file system");
    }

    #[test]
    fn a_folder_listing_can_span_several_blocks() {
        let files: [(&str, &[u8]); 1] = [("eboot.bin", b"x")];
        let mut pkg = open(&package(&Spec { files: &files, many: 3000, ..Default::default() })).unwrap();
        let nodes = pkg.list(&|| Ok(())).unwrap();
        assert_eq!(nodes.iter().filter(|n| n.path.starts_with("many/")).count(), 3000);
    }

    #[test]
    fn retail_and_damaged_packages_are_refused() {
        let files: [(&str, &[u8]); 1] = [("eboot.bin", b"x")];
        let e = open(&package(&Spec { files: &files, retail: true, ..Default::default() })).err().unwrap();
        assert!(format!("{e:#}").contains("isn't a fake package"), "{e:#}");
        let mut cut = package(&Spec { files: &files, ..Default::default() });
        cut.truncate(cut.len() - 0x1000);
        assert!(open(&cut).is_err(), "a cut-off download");
        let mut bad = package(&Spec { files: &files, ..Default::default() });
        bad[0x10..0x14].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(open(&bad).is_err(), "an impossible entry count");
        assert!(open(b"\x7fCNT").is_err());
        let escape: [(&str, &[u8]); 2] = [("eboot.bin", b"x"), ("..", b"pwned")];
        let e = open(&package(&Spec { files: &escape, ..Default::default() })).unwrap().list(&|| Ok(())).unwrap_err();
        assert!(format!("{e:#}").contains("Unsafe file name"), "{e:#}");
    }

    /// A real package: PS4_TEST_PKG=/path/to/game.pkg cargo test --release real_ps4_package -- --ignored --nocapture
    /// Lists the package and copies param.sfo and, for a game, eboot.bin (a PS4 SELF file).
    #[test]
    #[ignore = "needs a real package in PS4_TEST_PKG"]
    fn real_ps4_package_lists_and_copies() {
        let path = std::env::var("PS4_TEST_PKG").expect("set PS4_TEST_PKG");
        let mut pkg = Package::open(File::open(path).unwrap(), &|| Ok(())).unwrap();
        let nodes = pkg.list(&|| Ok(())).unwrap();
        let total: u64 = nodes.iter().map(|n| n.size).sum();
        eprintln!("{} {:?} into {}: {} nodes, {total} bytes", pkg.title_id, pkg.kind(), pkg.root().unwrap(), nodes.len());
        for n in nodes.iter().take(12) { eprintln!("  {} {}", if n.directory { "dir " } else { "file" }, n.path); }
        let read = |pkg: &mut Package, p: &str| {
            let node = nodes.iter().find(|n| n.path == p).unwrap_or_else(|| panic!("missing {p}")).clone();
            let mut out = Vec::new();
            pkg.copy(&node, &mut |c| { out.extend_from_slice(c); Ok(()) }).unwrap();
            assert_eq!(out.len() as u64, node.size);
            out
        };
        assert!(crate::sfo::parse(&read(&mut pkg, "sce_sys/param.sfo")).is_some());
        if pkg.kind() == Kind::Game {
            assert_eq!(&read(&mut pkg, "eboot.bin")[..4], &[0x4F, 0x15, 0x3D, 0x1D], "eboot.bin is a PS4 SELF file");
        }
    }
}
