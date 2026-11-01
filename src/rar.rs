//! RAR archives, read by RARLAB's own decoder (unrar, built in). libarchive refuses RAR5 packed
//! with a dictionary over 64 MiB, and can't find a RAR inside a YAD Simple Installer.
//! Only names, sizes and file contents come out of it: unrar "tests" each file and hands its bytes
//! to a callback, so it never creates, names or writes a file itself.

use anyhow::{ensure, Context, Result};
use std::{
    ffi::CString,
    fs::File,
    os::{raw::{c_char, c_int, c_long, c_uint}, unix::fs::FileExt},
    panic::AssertUnwindSafe,
    sync::{Mutex, MutexGuard},
};
use unrar_sys as ffi;

/// unrar finds an archive inside a self-extractor only this far in (its MAXSFXSIZE).
pub const MAX_SFX: u64 = 0x200000;
const SIGNATURE: &[u8] = b"Rar!\x1a\x07";
/// unrar keeps its error state in globals: one archive open at a time.
static LOCK: Mutex<()> = Mutex::new(());
/// A stored RAR 4 holding `a`, "azrak". unrar builds its checksum tables in static constructors,
/// which never run when it was compiled to use `.ctors` (conda's GCC does) and linked by
/// rust-lld; every archive then looks damaged. Reading this one first tells the two apart.
const PROBE: &[u8] = b"Rar!\x1a\x07\x00\xcf\x90\x73\x00\x00\x0d\x00\x00\x00\x00\x00\x00\x00\x63\x56\x74\x00\x80\x21\x00\x05\x00\x00\x00\x05\x00\x00\x00\x03\x10\xb7\x01\x97\x00\x00\x21\x00\x14\x30\x01\x00\xa4\x81\x00\x00\x61azrak\x04\xb0\x7b\x00\x00\x07\x00";

// unrar.dll's structures are packed to 1 byte (dll.hpp); unrar_sys's aren't, so every field after
// the first pointer of its header is misplaced on 64-bit. These follow dll.hpp exactly.
#[repr(C, packed)]
struct OpenData {
    arc_name: *const c_char,
    arc_name_w: *const libc::wchar_t,
    open_mode: c_uint,
    open_result: c_uint,
    cmt_buf: *mut c_char,
    cmt_buf_size: c_uint,
    cmt_size: c_uint,
    cmt_state: c_uint,
    flags: c_uint,
    callback: Option<extern "C" fn(c_uint, c_long, c_long, c_long) -> c_int>,
    user_data: c_long,
    op_flags: c_uint,
    cmt_buf_w: *mut libc::wchar_t,
    reserved: [c_uint; 25],
}

#[repr(C, packed)]
struct HeaderData {
    arc_name: [c_char; 1024],
    arc_name_w: [libc::wchar_t; 1024],
    file_name: [c_char; 1024],
    file_name_w: [libc::wchar_t; 1024],
    flags: c_uint,
    pack_size: c_uint,
    pack_size_high: c_uint,
    unp_size: c_uint,
    unp_size_high: c_uint,
    host_os: c_uint,
    file_crc: c_uint,
    file_time: c_uint,
    unp_ver: c_uint,
    method: c_uint,
    file_attr: c_uint,
    cmt_buf: *mut c_char,
    cmt_buf_size: c_uint,
    cmt_size: c_uint,
    cmt_state: c_uint,
    dict_size: c_uint,
    hash_type: c_uint,
    hash: [c_char; 32],
    redir_type: c_uint,
    redir_name: *mut libc::wchar_t,
    redir_name_size: c_uint,
    dir_target: c_uint,
    mtime: [c_uint; 2],
    ctime: [c_uint; 2],
    atime: [c_uint; 2],
    arc_name_ex: *mut libc::wchar_t,
    arc_name_ex_size: c_uint,
    file_name_ex: *mut libc::wchar_t,
    file_name_ex_size: c_uint,
    reserved: [c_uint; 982],
}

const UCM_PROCESSDATA: c_uint = 1;
const UCM_NEEDPASSWORD: c_uint = 2;
const UCM_CHANGEVOLUMEW: c_uint = 3;
const UCM_NEEDPASSWORDW: c_uint = 4;
const UCM_LARGEDICT: c_uint = 5;
const HOST_UNIX: c_uint = 3;
const FSREDIR_UNIXSYMLINK: c_uint = 1;
const MAX_NAME: usize = 1024;

/// Whether a RAR starts at `at` in `file`.
pub fn starts_at(file: &File, at: u64) -> Result<bool> {
    let mut magic = [0u8; SIGNATURE.len()];
    match file.read_exact_at(&mut magic, at) {
        Ok(()) => Ok(magic == SIGNATURE),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// What an archive entry is. Paths are as stored: the installer checks them.
pub enum Kind { File, Directory, Symlink(String), Unsupported(&'static str) }
pub struct Header { pub path: String, pub kind: Kind, pub size: u64, pub encrypted: bool, pub exec: bool }

type Sink<'a> = dyn FnMut(&[u8]) -> Result<()> + 'a;
struct State {
    /// Where the current file's bytes go, while it's being read.
    sink: Option<*mut Sink<'static>>,
    error: Option<anyhow::Error>,
    /// The only volumes unrar may move on to.
    volumes: Vec<Vec<libc::wchar_t>>,
}

extern "C" fn callback(msg: c_uint, user: c_long, p1: c_long, p2: c_long) -> c_int {
    // SAFETY: `user` is the State boxed by Archive, which outlives its handle.
    let state = unsafe { &mut *(user as *mut State) };
    let fail = |state: &mut State, error: anyhow::Error| { state.error.get_or_insert(error); -1 };
    match msg {
        UCM_PROCESSDATA => {
            let Some(sink) = state.sink else { return fail(state, anyhow::anyhow!("RAR data outside a file")) };
            // SAFETY: unrar passes its own buffer of p2 bytes for the duration of this call.
            let data = unsafe { std::slice::from_raw_parts(p1 as *const u8, p2.max(0) as usize) };
            // A panic must not unwind into C++.
            match std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*sink)(data) })) {
                Ok(Ok(())) => 1,
                Ok(Err(error)) => fail(state, error),
                Err(_) => fail(state, anyhow::anyhow!("RAR data handler panicked")),
            }
        }
        UCM_CHANGEVOLUMEW => {
            // SAFETY: a NUL-terminated wide string from unrar.
            let name = unsafe { wide(p1 as *const libc::wchar_t, 4096) };
            if p2 == 1 && state.volumes.contains(&name) { return 1; }
            fail(state, anyhow::anyhow!("Missing RAR volume {}", text(&name).unwrap_or_default().rsplit('/').next().unwrap_or("")))
        }
        UCM_NEEDPASSWORD | UCM_NEEDPASSWORDW => fail(state, anyhow::anyhow!("Password-protected archives are unsupported")),
        UCM_LARGEDICT => fail(state, anyhow::anyhow!("RAR needs a {} GiB dictionary; more memory than an install may take", p1 / (1024 * 1024))),
        _ => 0,
    }
}

unsafe fn wide(p: *const libc::wchar_t, max: usize) -> Vec<libc::wchar_t> {
    let mut out = Vec::new();
    if p.is_null() { return out; }
    while out.len() < max {
        let c = *p.add(out.len());
        if c == 0 { break; }
        out.push(c);
    }
    out
}
fn text(w: &[libc::wchar_t]) -> Result<String> {
    w.iter().map(|&c| char::from_u32(c as u32).context("Invalid character in RAR name")).collect()
}

/// One RAR set, read start to end: `next` a header, then `read` or `skip` its file.
pub struct Archive {
    handle: *const ffi::Handle,
    state: Box<State>,
    _lock: MutexGuard<'static, ()>,
}

impl Archive {
    /// `volumes` in order; unrar finds each next volume by name and may open only these.
    pub fn open(volumes: &[String]) -> Result<Self> {
        static WORKS: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        WORKS.get_or_init(|| probe().map_err(|e| format!("{e:#}"))).clone().map_err(|e| anyhow::anyhow!(
            "This build of the launcher can't read RAR archives ({e}): its C++ compiler left unrar's start-up code out. Rebuild it with the system compiler (CXX=c++)"))?;
        Self::open_unchecked(volumes)
    }

    fn open_unchecked(volumes: &[String]) -> Result<Self> {
        ensure!(!volumes.is_empty(), "No RAR volume");
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let wide: Vec<Vec<libc::wchar_t>> = volumes.iter().map(|v| v.chars().map(|c| c as libc::wchar_t).collect()).collect();
        let mut state = Box::new(State { sink: None, error: None, volumes: wide });
        let mut name = state.volumes[0].clone();
        name.push(0);
        let narrow = CString::new(volumes[0].as_str())?;
        // SAFETY: zero is valid for every field (null pointers, no callback).
        let mut data: OpenData = unsafe { std::mem::zeroed() };
        data.arc_name = narrow.as_ptr();
        data.arc_name_w = name.as_ptr();
        data.open_mode = ffi::RAR_OM_EXTRACT;
        data.callback = Some(callback);
        data.user_data = &mut *state as *mut State as c_long;
        let handle = unsafe { ffi::RAROpenArchiveEx(&data as *const OpenData as *const ffi::OpenArchiveDataEx) };
        let result = data.open_result;
        let archive = Self { handle, state, _lock: lock };
        // A file that isn't a RAR still gets a handle, with the reason in open_result.
        if result != 0 || handle.is_null() { return Err(archive.error(result as c_int, "Open RAR archive")); }
        Ok(archive)
    }

    fn error(&self, code: c_int, action: &str) -> anyhow::Error {
        let reason = match code {
            ffi::ERAR_NO_MEMORY => "not enough memory",
            ffi::ERAR_BAD_DATA => "damaged data (checksum mismatch)",
            ffi::ERAR_BAD_ARCHIVE | ffi::ERAR_UNKNOWN_FORMAT => "not a RAR archive, or a damaged one",
            ffi::ERAR_EOPEN => "a volume is missing or can't be opened",
            ffi::ERAR_EREAD => "read error",
            ffi::ERAR_MISSING_PASSWORD | ffi::ERAR_BAD_PASSWORD => "password-protected archives are unsupported",
            25 => "its dictionary is larger than an install may use",
            _ => "unknown error",
        };
        anyhow::anyhow!("{action}: {reason} (unrar {code})")
    }

    fn take(&mut self, code: c_int, action: &str) -> Result<()> {
        if let Some(error) = self.state.error.take() { return Err(error); }
        if code != 0 { return Err(self.error(code, action)); }
        Ok(())
    }

    pub fn next(&mut self) -> Result<Option<Header>> {
        loop {
            let mut link = vec![0 as libc::wchar_t; 4096];
            // SAFETY: zero is valid for every field; unrar fills it.
            let mut h: Box<HeaderData> = Box::new(unsafe { std::mem::zeroed() });
            h.redir_name = link.as_mut_ptr();
            h.redir_name_size = link.len() as c_uint;
            let code = unsafe { ffi::RARReadHeaderEx(self.handle, &*h as *const HeaderData as *const ffi::HeaderDataEx) };
            if code == ffi::ERAR_END_ARCHIVE && self.state.error.is_none() { return Ok(None); }
            self.take(code, "Read RAR header")?;
            // Fields are copied out: a packed field can't be borrowed.
            let (flags, attr, host, redir) = (h.flags, h.file_attr, h.host_os, h.redir_type);
            // The rest of a file begun in the previous volume (skipping it moved on to this one).
            if flags & ffi::RHDF_SPLITBEFORE != 0 {
                self.skip()?;
                continue;
            }
            let name_w = h.file_name_w;
            let len = name_w.iter().position(|&c| c == 0).unwrap_or(MAX_NAME);
            ensure!(len < MAX_NAME - 1, "RAR entry name is too long");
            let path = text(&name_w[..len])?;
            let unix = host == HOST_UNIX;
            let size = (h.unp_size_high as u64) << 32 | h.unp_size as u64;
            let kind = match redir {
                0 if flags & ffi::RHDF_DIRECTORY != 0 => Kind::Directory,
                0 if unix && !matches!(attr & libc::S_IFMT, 0 | libc::S_IFREG) => Kind::Unsupported("special file"),
                0 => Kind::File,
                FSREDIR_UNIXSYMLINK => {
                    let target = text(&link[..link.iter().position(|&c| c == 0).unwrap_or(0)])?;
                    // RAR 4 keeps a link's target as its data, which unrar never hands out.
                    if target.is_empty() { Kind::Unsupported("RAR 4 symlink") } else { Kind::Symlink(target) }
                }
                2 | 3 => Kind::Unsupported("Windows link"),
                4 => Kind::Unsupported("hardlink"),
                _ => Kind::Unsupported("file copy reference"),
            };
            return Ok(Some(Header { path, kind, size, encrypted: flags & ffi::RHDF_ENCRYPTED != 0, exec: unix && attr & 0o111 != 0 }));
        }
    }

    /// Pass the current file's bytes to `sink`, in order; unrar checks its checksum at the end.
    pub fn read(&mut self, sink: &mut Sink<'_>) -> Result<()> {
        // SAFETY: the pointer is cleared before this returns, so it never outlives `sink`.
        self.state.sink = Some(unsafe { std::mem::transmute::<*mut Sink<'_>, *mut Sink<'static>>(sink as *mut Sink<'_>) });
        let code = unsafe { ffi::RARProcessFileW(self.handle, ffi::RAR_TEST, std::ptr::null(), std::ptr::null()) };
        self.state.sink = None;
        self.take(code, "Read RAR data")
    }

    pub fn skip(&mut self) -> Result<()> {
        let code = unsafe { ffi::RARProcessFileW(self.handle, ffi::RAR_SKIP, std::ptr::null(), std::ptr::null()) };
        self.take(code, "Skip RAR data")
    }

    /// The first `max` bytes of the current file; the rest is read and checked, then dropped.
    pub fn head(&mut self, max: usize) -> Result<Vec<u8>> {
        let mut head = Vec::new();
        self.read(&mut |chunk| {
            let room = max - head.len();
            head.extend_from_slice(&chunk[..chunk.len().min(room)]);
            Ok(())
        })?;
        Ok(head)
    }

    #[cfg(test)]
    pub fn version() -> String {
        let v = unsafe { ffi::RARGetDllVersion() };
        format!("unrar (RARLAB, API {v})")
    }
}

fn probe() -> Result<()> {
    use std::{io::Write, os::fd::{AsRawFd, FromRawFd}};
    let fd = unsafe { libc::memfd_create(c"azrak-rar-probe".as_ptr(), libc::MFD_CLOEXEC) };
    ensure!(fd >= 0, "memfd: {}", std::io::Error::last_os_error());
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(PROBE)?;
    let mut archive = Archive::open_unchecked(&[format!("/proc/self/fd/{}", file.as_raw_fd())])?;
    ensure!(archive.next()?.is_some_and(|h| h.path == "a" && h.size == 5), "probe entry missing");
    ensure!(archive.head(16)? == b"azrak", "probe data differs");
    ensure!(archive.next()?.is_none(), "probe has extra entries");
    Ok(())
}

impl Drop for Archive {
    fn drop(&mut self) {
        if !self.handle.is_null() { unsafe { ffi::RARCloseArchive(self.handle); } }
    }
}

