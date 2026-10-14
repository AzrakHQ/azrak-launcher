//! Small shared helpers: paths, logging, HTTP, text cleanup.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const APP_NAME: &str = "ps5-launcher";
pub const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36";

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        let t = $crate::util::now_secs() as u64;
        eprintln!("[{:02}:{:02}:{:02}] {}", (t / 3600 + $crate::util::tz_offset_hours()) % 24, t / 60 % 60, t % 60, format!($($arg)*));
    }};
}

pub fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Local UTC offset in hours (for log timestamps only).
pub fn tz_offset_hours() -> u64 {
    static OFF: LazyLock<u64> = LazyLock::new(|| unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        ((tm.tm_gmtoff / 3600).rem_euclid(24)) as u64
    });
    *OFF
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join(APP_NAME)
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local").join("share")).join(APP_NAME)
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache")).join(APP_NAME)
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else if p == "~" {
        home()
    } else {
        PathBuf::from(p)
    }
}

/// Show paths under the home folder as "~/…" (shorter, and no user names on screen).
pub fn display_path(p: &str) -> String {
    let home = home();
    let home = home.to_string_lossy();
    match p.strip_prefix(home.as_ref()) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => p.to_string(),
    }
}

pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}.{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

pub fn sha1_hex(s: &str) -> String {
    sha1_smol::Sha1::from(s).digest().to_string()
}

// ------------------------------------------------------------------ HTTP

pub static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(40))
        .max_idle_connections_per_host(8)
        .build()
});

/// Percent-encode characters that are not valid in a URL (e.g. en dashes in file names).
pub fn iri_to_uri(url: &str) -> String {
    const KEEP: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`').add(b'{').add(b'}').add(b'|').add(b'\\').add(b'^');
    percent_encoding::utf8_percent_encode(url, KEEP).to_string()
}

/// An HTTP error that means "no network" (the name didn't resolve, the connection failed),
/// rather than an answer from the server.
pub fn is_network_error(error: &str) -> bool {
    ["Dns Failed", "Connection Failed", "Connect error", "timed out", "Network is unreachable"].iter().any(|m| error.contains(m))
}

/// The network is up: GitHub's API name resolves.
pub fn network_up() -> bool {
    use std::net::ToSocketAddrs;
    ("api.github.com", 443).to_socket_addrs().is_ok_and(|mut a| a.next().is_some())
}

pub fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let resp = AGENT.get(&iri_to_uri(url)).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        other => other.to_string(),
    })?;
    let mut buf = Vec::with_capacity(64 * 1024);
    resp.into_reader().take(64 * 1024 * 1024).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

pub fn http_json(url: &str) -> Result<serde_json::Value, String> {
    let bytes = http_get(url)?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

pub fn query_escape(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

// ------------------------------------------------------------------ text

static TAG_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?s)<(script|style)[^>]*>.*?</(script|style)>|<[^>]+>").unwrap());
static BR_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)<br\s*/?>").unwrap());

/// Strip HTML tags and decode entities, keeping line breaks from <br>.
pub fn clean_multiline(s: &str) -> String {
    let with_nl = BR_RE.replace_all(s, "\n");
    let no_tags = TAG_RE.replace_all(&with_nl, "");
    html_escape::decode_html_entities(&no_tags).trim().to_string()
}

/// Lowercase, strip accents/punctuation: used for search and matching.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        let c = fold_accent(c);
        if c == '\u{0}' {
            continue; // apostrophes join words: "Assassin's" -> "assassins"
        }
        if c.is_alphanumeric() {
            for l in c.to_lowercase() {
                out.push(l);
            }
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim_end().to_string()
}

fn fold_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
        'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'ñ' | 'Ñ' => 'n',
        '’' | '\'' => '\u{0}',
        other => other,
    }
}

// ------------------------------------------------------------------ dates

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTHS_LONG: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december",
];

/// "2026-09-22T13:14:02" or "2026-09-22" -> (y, m, d)
pub fn parse_iso_date(s: &str) -> Option<(i32, u32, u32)> {
    let s = s.get(..10)?;
    let mut it = s.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    Some((y, m, d))
}

/// "February 13, 2026" / "March 05, 2021" -> (y, m, d)
pub fn parse_long_date(s: &str) -> Option<(i32, u32, u32)> {
    let s = s.trim().to_lowercase().replace(',', " ");
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 3 {
        return parse_iso_date(&s);
    }
    let m = MONTHS_LONG.iter().position(|x| x.starts_with(parts[0]) || parts[0].starts_with(&x[..3]))? as u32 + 1;
    let d = parts[1].parse().ok()?;
    let y = parts[2].parse().ok()?;
    Some((y, m, d))
}

pub fn fmt_date(ymd: Option<(i32, u32, u32)>) -> String {
    match ymd {
        Some((y, m, d)) if (1..=12).contains(&m) => format!("{} {}, {}", MONTHS[m as usize - 1], d, y),
        _ => String::new(),
    }
}

/// Days since the Unix epoch (civil calendar), for sorting and "new" badges.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s < 60 {
        return format!("{s}s");
    }
    let (h, m) = (s / 3600, s / 60 % 60);
    if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

/// "16 sec", "12 min", "2 hr 5 min" — for places written in capitals, where "16S" reads badly.
pub fn fmt_duration_words(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m) = (s / 3600, s / 60 % 60);
    match (h, m) {
        (0, 0) => format!("{s} sec"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} hr"),
        (h, m) => format!("{h} hr {m} min"),
    }
}

pub fn fmt_clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, x) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{x:02}") } else { format!("{m}:{x:02}") }
}

pub fn local_time() -> libc::tm {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

pub fn fmt_last_played(ts: f64) -> String {
    if ts <= 0.0 {
        return String::new();
    }
    let off = unsafe {
        let t = ts as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm.tm_gmtoff as f64
    };
    let day = |t: f64| ((t + off) / 86400.0).floor() as i64;
    let diff = day(now_secs()) - day(ts);
    match diff {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => format!("{diff} days ago"),
        _ => {
            let days = day(ts);
            fmt_date(Some(civil_from_days(days)))
        }
    }
}

pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dates_roundtrip() {
        let d = days_from_civil(2026, 9, 29);
        assert_eq!(civil_from_days(d), (2026, 9, 29));
        assert_eq!(parse_long_date("February 13, 2026"), Some((2026, 2, 13)));
        assert_eq!(parse_long_date("March 05, 2021"), Some((2021, 3, 5)));
    }
    #[test]
    fn norm_works() {
        assert_eq!(norm("Pokémon: Legends™ Z-A"), "pokemon legends z a");
        assert_eq!(norm("Assassin's Creed"), "assassins creed");
    }
}
