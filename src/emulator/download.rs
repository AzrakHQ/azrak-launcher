//! Downloading an emulator build (or the launcher itself): streamed to disk, SHA-256 checked.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

/// Large downloads can take minutes: no overall deadline, only a stall timeout per read.
static DOWNLOADER: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| {
    ureq::AgentBuilder::new()
        .user_agent(crate::util::USER_AGENT)
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(60))
        .build()
});

/// Stream `url` to `dest`, verifying its SHA-256 when one is given.
pub fn file(url: &str, size: u64, sha256: &str, dest: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let resp = DOWNLOADER.get(url).call().map_err(|e| format!("download failed: {e}"))?;
    let total = resp.header("Content-Length").and_then(|s| s.parse().ok()).unwrap_or(size);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    if !sha256.is_empty() {
        let got = format!("{:x}", hasher.finalize());
        if !got.eq_ignore_ascii_case(sha256) {
            return Err("download is corrupted (checksum mismatch)".into());
        }
    }
    Ok(())
}
