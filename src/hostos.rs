//! The few places where Linux and macOS differ at the OS level. Everything else is shared.
//!
//! Each helper has one Linux and one macOS implementation behind `cfg`, and a single test
//! that runs on whichever OS builds it, so the contract is the same on both.

use std::ffi::{CStr, CString, OsString};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

#[cfg(target_os = "linux")]
fn clear_errno() {
    unsafe { *libc::__errno_location() = 0 };
}

#[cfg(target_os = "macos")]
fn clear_errno() {
    unsafe { *libc::__error() = 0 };
}

/// Names in the directory open as `fd` (no "." or ".."), in no particular order.
///
/// Works from the descriptor, not a path, so a directory swapped in later can't redirect the
/// listing. It re-opens "." for a private read position instead of sharing `fd`'s.
pub fn dir_names(fd: RawFd) -> io::Result<Vec<OsString>> {
    let dot = CString::new(".").expect("no NUL in a literal");
    let own = unsafe { libc::openat(fd, dot.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if own < 0 {
        return Err(last_error());
    }
    // fdopendir takes ownership of `own`; closedir closes it.
    let dir = unsafe { libc::fdopendir(own) };
    if dir.is_null() {
        let e = last_error();
        unsafe { libc::close(own) };
        return Err(e);
    }
    let mut names = Vec::new();
    let result = loop {
        clear_errno();
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            // NULL is both "end" and "error"; errno tells them apart.
            let e = last_error();
            break if e.raw_os_error().unwrap_or(0) == 0 { Ok(()) } else { Err(e) };
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(OsString::from_vec(name.to_vec()));
        }
    };
    unsafe { libc::closedir(dir) };
    result.map(|()| names)
}

/// Rename `from` (in `from_dir`) to `to` (in `to_dir`), failing if `to` already exists.
/// Atomic: nothing is ever overwritten.
pub fn rename_noreplace(from_dir: RawFd, from: &CStr, to_dir: RawFd, to: &CStr) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    let r = unsafe { libc::renameat2(from_dir, from.as_ptr(), to_dir, to.as_ptr(), libc::RENAME_NOREPLACE) };
    #[cfg(target_os = "macos")]
    let r = unsafe { libc::renameatx_np(from_dir, from.as_ptr(), to_dir, to.as_ptr(), libc::RENAME_EXCL) };
    if r == 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

/// A path that opens the same file as the already-open descriptor `fd`, with its own offset.
/// Used to hand an open file to libraries that only accept a path.
pub fn fd_open_path(fd: RawFd) -> String {
    #[cfg(target_os = "linux")]
    return format!("/proc/self/fd/{fd}");
    // `/dev/fd/N` on macOS is a dup of `fd`: it shares the file offset, so a library that opens
    // a volume, seeks to its end, closes it and reopens it later (libarchive does, for split
    // archives) would resume at the end. Reopen by the file's real path instead, which gives a
    // fresh offset; the installer re-checks the file's identity after reading.
    #[cfg(target_os = "macos")]
    return fd_real_path(fd).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| format!("/dev/fd/{fd}"));
}

/// The real, canonical location of the open file or directory `fd`.
pub fn fd_real_path(fd: RawFd) -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    return std::fs::canonicalize(format!("/proc/self/fd/{fd}"));
    #[cfg(target_os = "macos")]
    {
        // F_GETPATH fills a buffer of at least MAXPATHLEN (1024) bytes with the file's path.
        let mut buf = [0u8; 1024];
        if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
            return Err(last_error());
        }
        let path = CStr::from_bytes_until_nul(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::canonicalize(PathBuf::from(OsString::from_vec(path.to_bytes().to_vec())))
    }
}

/// Whether a process with this id exists (a zombie still counts).
pub fn pid_exists(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    return std::path::Path::new(&format!("/proc/{pid}")).exists();
    #[cfg(target_os = "macos")]
    // Signal 0 only checks. EPERM means it exists but belongs to someone else.
    return unsafe { libc::kill(pid as i32, 0) } == 0 || last_error().raw_os_error() == Some(libc::EPERM);
}

/// Raise the open-file limit as far as the system allows.
///
/// A torrent download opens every file in the torrent up front. macOS starts apps with a limit of
/// 256 open files, so a game with a few hundred files failed part-way with "Too many open files"
/// (reported as `error opening <file> in read/write mode`). Returns the limit now in effect.
pub fn raise_open_file_limit() -> Option<u64> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return None;
    }
    // macOS refuses anything above OPEN_MAX (10240) even when the hard limit says "unlimited".
    let ceiling: u64 = if cfg!(target_os = "macos") { 10_240 } else { 1 << 20 };
    let target = (lim.rlim_max as u64).min(ceiling);
    if (lim.rlim_cur as u64) < target {
        let want = libc::rlimit { rlim_cur: target as libc::rlim_t, rlim_max: lim.rlim_max };
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &want) } == 0 {
            return Some(target);
        }
    }
    Some(lim.rlim_cur as u64)
}

/// Resolve symlinks in the part of `path` that already exists, keeping the rest as written.
///
/// The installer refuses symlinks anywhere in a path it opens, even in ancestors. A path the
/// user chose is resolved once with this, then opened strictly: on macOS `/tmp` and `/var` are
/// links, so `/tmp/games` would otherwise be refused. Paths with `..` are left for the installer
/// to reject.
pub fn resolve_existing(path: &Path) -> PathBuf {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return path.to_path_buf();
    }
    let mut existing = path.to_path_buf();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&existing) {
            return missing.iter().rev().fold(real, |p, part| p.join(part));
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// A temp folder whose path has no symlinks, so tests can hand it to the installer. (macOS's temp
/// folder is `/var/folders/...`, and `/var` is a link to `/private/var`.)
#[cfg(test)]
pub fn real_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new().tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

/// Library names to try, in order, when loading libarchive at run time.
pub fn libarchive_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    return &["libarchive.so.13", "libarchive.so"];
    // Homebrew's keg-only libarchive isn't on the default search path.
    #[cfg(target_os = "macos")]
    return &[
        "/opt/homebrew/opt/libarchive/lib/libarchive.13.dylib",
        "/usr/local/opt/libarchive/lib/libarchive.13.dylib",
        "libarchive.13.dylib",
        "libarchive.dylib",
    ];
}

/// Library names to try, in order, when loading libmpv (trailers) at run time.
pub fn libmpv_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    return &["libmpv.so.2", "libmpv.so.1", "libmpv.so"];
    // Homebrew's mpv formula installs libmpv outside the default search path.
    #[cfg(target_os = "macos")]
    return &[
        "/opt/homebrew/lib/libmpv.2.dylib",
        "/usr/local/lib/libmpv.2.dylib",
        "libmpv.2.dylib",
        "libmpv.dylib",
    ];
}

/// How to install libarchive on this OS, for the error shown when it's missing.
pub fn libarchive_install_hint() -> &'static str {
    #[cfg(target_os = "linux")]
    return "libarchive13 on Debian/Ubuntu";
    #[cfg(target_os = "macos")]
    return "`brew install libarchive`";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn dir_names_lists_entries_without_dot_entries() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.txt"), b"a").unwrap();
        std::fs::create_dir(t.path().join("sub")).unwrap();
        std::fs::write(t.path().join(".hidden"), b"h").unwrap();
        let dir = std::fs::File::open(t.path()).unwrap();
        let mut names: Vec<String> = dir_names(dir.as_raw_fd()).unwrap().into_iter().map(|n| n.into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, [".hidden", "a.txt", "sub"]);
    }

    #[test]
    fn dir_names_can_be_called_twice_on_one_descriptor() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a"), b"").unwrap();
        let dir = std::fs::File::open(t.path()).unwrap();
        assert_eq!(dir_names(dir.as_raw_fd()).unwrap().len(), 1);
        assert_eq!(dir_names(dir.as_raw_fd()).unwrap().len(), 1);
    }

    #[test]
    fn dir_names_of_empty_directory_is_empty() {
        let t = tempfile::tempdir().unwrap();
        let dir = std::fs::File::open(t.path()).unwrap();
        assert!(dir_names(dir.as_raw_fd()).unwrap().is_empty());
    }

    #[test]
    fn dir_names_rejects_a_file() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("f"), b"").unwrap();
        let file = std::fs::File::open(t.path().join("f")).unwrap();
        assert!(dir_names(file.as_raw_fd()).is_err());
    }

    #[test]
    fn rename_noreplace_moves_but_never_overwrites() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("src"), b"new").unwrap();
        std::fs::write(t.path().join("taken"), b"old").unwrap();
        let dir = std::fs::File::open(t.path()).unwrap();
        let c = |s: &str| CString::new(s).unwrap();
        let fd = dir.as_raw_fd();

        let err = rename_noreplace(fd, &c("src"), fd, &c("taken")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(t.path().join("taken")).unwrap(), b"old");
        assert!(t.path().join("src").exists());

        rename_noreplace(fd, &c("src"), fd, &c("free")).unwrap();
        assert_eq!(std::fs::read(t.path().join("free")).unwrap(), b"new");
        assert!(!t.path().join("src").exists());
    }

    #[test]
    fn fd_open_path_reopens_the_same_file() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("data.bin");
        std::fs::write(&path, b"payload").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        assert_eq!(std::fs::read(fd_open_path(file.as_raw_fd())).unwrap(), b"payload");
    }

    #[test]
    fn fd_open_path_gives_a_fresh_offset_even_after_the_descriptor_moved() {
        use std::io::{Read, Seek, SeekFrom};
        let t = real_tempdir();
        let path = t.path().join("data.bin");
        std::fs::write(&path, b"payload").unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        let mut again = std::fs::File::open(fd_open_path(file.as_raw_fd())).unwrap();
        let mut got = Vec::new();
        again.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"payload");
    }

    #[test]
    fn fd_real_path_is_the_canonical_location() {
        let t = tempfile::tempdir().unwrap();
        let dir = std::fs::File::open(t.path()).unwrap();
        assert_eq!(fd_real_path(dir.as_raw_fd()).unwrap(), std::fs::canonicalize(t.path()).unwrap());
    }

    #[test]
    fn pid_exists_sees_this_process_and_not_a_missing_one() {
        assert!(pid_exists(std::process::id()));
        assert!(!pid_exists(2_000_000_000));
    }

    #[test]
    fn resolve_existing_follows_links_in_the_part_that_exists() {
        let t = real_tempdir();
        std::fs::create_dir(t.path().join("real")).unwrap();
        std::os::unix::fs::symlink("real", t.path().join("link")).unwrap();
        let real = t.path().join("real");
        // A link in the middle, with folders that don't exist yet below it.
        assert_eq!(resolve_existing(&t.path().join("link/new/sub")), real.join("new/sub"));
        assert_eq!(resolve_existing(&t.path().join("link")), real);
        // Already real, or entirely missing: unchanged.
        assert_eq!(resolve_existing(&real), real);
        assert_eq!(resolve_existing(&t.path().join("nothing/here")), t.path().join("nothing/here"));
    }

    #[test]
    fn resolve_existing_leaves_dangling_links_and_dotdot_alone() {
        let t = real_tempdir();
        std::os::unix::fs::symlink("missing", t.path().join("dangling")).unwrap();
        assert_eq!(resolve_existing(&t.path().join("dangling/x")), t.path().join("dangling/x"));
        let dotdot = t.path().join("a/../b");
        assert_eq!(resolve_existing(&dotdot), dotdot);
        assert_eq!(resolve_existing(Path::new("/")), Path::new("/"));
    }

    #[test]
    fn real_tempdir_has_no_symlinks_in_its_path() {
        let t = real_tempdir();
        assert_eq!(std::fs::canonicalize(t.path()).unwrap(), t.path());
    }

    #[test]
    fn open_file_limit_is_raised_and_actually_allows_more_files() {
        let before = {
            let mut l = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) };
            l.rlim_cur as u64
        };
        let now = raise_open_file_limit().unwrap();
        assert!(now >= before.min(10_240), "{now} < {before}");
        // Prove it by really holding more files open than the old macOS default of 256.
        let t = real_tempdir();
        let want = 300usize.min(now.saturating_sub(64) as usize);
        let files: Vec<_> = (0..want).map(|i| std::fs::File::create(t.path().join(format!("f{i}"))).unwrap()).collect();
        assert_eq!(files.len(), want);
    }

    #[test]
    fn libarchive_has_candidates_and_a_hint() {
        assert!(!libarchive_candidates().is_empty());
        assert!(!libmpv_candidates().is_empty());
        assert!(!libarchive_install_hint().is_empty());
    }
}
