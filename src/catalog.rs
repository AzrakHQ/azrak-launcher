//! Offline RuTracker release catalogs (PS5 and PS4 forums), merged into one Library.
//! Browser collection is a separate tool.

use crate::platform::Platform;
use crate::util::{atomic_write, cache_dir, now_secs};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

pub const CATALOG_TTL: f64 = 6.0 * 3600.0;
pub const CATALOG_SCHEMA: u32 = 5;
pub const SOURCE: &str = "https://rutracker.net/forum/viewforum.php?f=546";
pub const PS4_SOURCE: &str = "https://rutracker.net/forum/viewforum.php?f=973";
/// PC games: native Linux games, and Windows games packed with Wine, DOSBox and the like.
pub const LINUX_SOURCE: &str = "https://rutracker.net/forum/viewforum.php?f=1992";
pub const WINE_SOURCE: &str = "https://rutracker.net/forum/viewforum.php?f=2059";
const BUNDLED: &[u8] = include_bytes!("../assets/rutracker/ps5-topics.json");
/// Empty until the PS4 snapshot is collected and bundled (see build.rs).
const BUNDLED_PS4: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ps4-topics.json"));

/// One console's RuTracker forum snapshot and where it's cached.
struct Source {
    platform: Platform,
    url: &'static str,
    file: &'static str,
    cache: &'static str,
    env: &'static str,
    bundled: &'static [u8],
}

// The PC catalogs aren't built in: they arrive with the daily published catalog.
const SOURCES: [Source; 4] = [
    Source { platform: Platform::Ps5, url: SOURCE, file: "ps5-topics.json", cache: "catalog-rutracker.json", env: "AZRAK_CATALOG_PATH", bundled: BUNDLED },
    Source { platform: Platform::Ps4, url: PS4_SOURCE, file: "ps4-topics.json", cache: "catalog-rutracker-ps4.json", env: "AZRAK_PS4_CATALOG_PATH", bundled: BUNDLED_PS4 },
    Source { platform: Platform::Pc, url: LINUX_SOURCE, file: "linux-topics.json", cache: "catalog-rutracker-linux.json", env: "AZRAK_LINUX_CATALOG_PATH", bundled: &[] },
    Source { platform: Platform::Pc, url: WINE_SOURCE, file: "wine-topics.json", cache: "catalog-rutracker-wine.json", env: "AZRAK_WINE_CATALOG_PATH", bundled: &[] },
];

/// A PC release's Steam app (from the published catalog: scripts/rutracker-list/steam.mjs).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Steam {
    pub appid: u64,
    pub name: String,
    pub short: String,
    pub genres: Vec<String>,
    pub release: Option<String>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    /// Steam sells a Linux version.
    pub linux: bool,
    pub metacritic: Option<u32>,
    pub header: Option<String>,
    pub shots: Vec<String>,
    pub video: Option<String>,
    pub assets: SteamAssets,
    /// ProtonDB's tier for the Windows version ("platinum", "gold", …).
    pub protondb: Option<String>,
}

impl Steam {
    /// The artwork and details the Library and Game Hub show, in the shape PSN data has: Steam's
    /// library portrait as the cover, its hero image as the background, its logo over it.
    pub fn info(&self) -> crate::psn::Info {
        let hero = self.assets.hero.clone().or_else(|| self.header.clone()).unwrap_or_default();
        crate::psn::Info {
            source: Some("steam".into()),
            name: self.name.clone(),
            publisher: self.publishers.first().or(self.developers.first()).cloned().unwrap_or_default(),
            genres: self.genres.clone(),
            release: self.release.as_deref().and_then(steam_date).unwrap_or_default(),
            short: self.short.clone(),
            long: self.short.clone(),
            master: hero.clone(),
            hub: hero.clone(),
            bg: hero,
            logo: self.assets.logo.clone().unwrap_or_default(),
            // Only a real portrait: the wide header shrinks to a strip on a 2:3 card.
            portrait: self.assets.portrait.clone().unwrap_or_default(),
            banner: self.header.clone().unwrap_or_default(),
            shots: self.shots.clone(),
            video: self.video.clone().unwrap_or_default(),
            store: format!("https://store.steampowered.com/app/{}/", self.appid),
            ..Default::default()
        }
    }
}

/// Steam's "Jan 25, 2018" or "25 Jan, 2018" as "2018-01-25".
fn steam_date(date: &str) -> Option<String> {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let parts: Vec<&str> = date.split(|c: char| c == ' ' || c == ',').filter(|p| !p.is_empty()).collect();
    let [a, b, year] = parts[..] else { return None };
    let (month, day) = if a.chars().next()?.is_ascii_digit() { (b, a) } else { (a, b) };
    let month = MONTHS.iter().position(|m| month.to_ascii_lowercase().starts_with(m))? + 1;
    let (day, year): (u32, u32) = (day.parse().ok()?, year.parse().ok()?);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct SteamAssets {
    pub portrait: Option<String>,
    pub hero: Option<String>,
    pub logo: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Game {
    pub id: i64,
    pub name: String,
    pub title: String,
    pub date: String,
    pub link: String,
    pub cover: String,
    pub trailer: String,
    pub genres: Vec<String>,
    pub mode: String,
    pub release: String,
    pub size: String,
    pub size_gb: Option<f64>,
    pub version: String,
    pub title_id: String,
    pub update: String,
    pub excerpt: String,
    pub description: Vec<String>,
    pub title_ids: Vec<String>,
    pub region: String,
    pub magnet: String,
    pub seeders: Option<u64>,
    pub leechers: Option<u64>,
    pub peers_observed: String,
    pub game_info: Value,
    /// The console this release is for (from the forum it was listed in).
    pub platform: Platform,
    /// PC: a Windows game packed to run with Wine (or DOSBox…), from that forum.
    pub wine: bool,
    /// PC: the game on Steam, for artwork and ratings.
    pub steam: Option<Steam>,
}

impl Game {
    pub fn detail(&self, key: &str) -> String { text(&self.game_info, key) }

    /// Its valid title IDs, upper-case: the main one and any others it carries.
    pub fn valid_title_ids(&self) -> Vec<String> {
        std::iter::once(&self.title_id).chain(&self.title_ids).filter(|id| crate::psn::valid_title_id(id))
            .map(|id| id.to_ascii_uppercase()).collect()
    }

    /// A PS5 release shipped as a PKG package, which the launcher can't install (PS5 games
    /// install from folders, archives and exFAT images). PS4 PKGs are extracted on install.
    pub fn is_pkg(&self) -> bool {
        static PKG: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\bf?pkg\b").unwrap());
        self.platform == Platform::Ps5 && (PKG.is_match(&self.detail("format")) || PKG.is_match(&self.title))
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CatalogFile {
    pub updated: f64,
    pub schema: u32,
    pub source: String,
    pub fingerprint: String,
    pub snapshot_at: String,
    pub games: Vec<Game>,
}

impl CatalogFile {
    /// Every console's catalog, merged: PS5 and, once collected, PS4.
    pub fn load() -> Self {
        merge(SOURCES.iter().map(load_one).collect())
    }

    pub fn stale(&self) -> bool {
        self.games.is_empty() || now_secs() - last_check() > CATALOG_TTL || SOURCES.iter().any(|src| {
            let Ok(bytes) = source_bytes(src) else { return false };
            if bytes.is_empty() { return false; }
            let cache = read_cache(src);
            cache.schema != CATALOG_SCHEMA || cache.source != src.url || cache.games.is_empty()
                || now_secs() - cache.updated > CATALOG_TTL || fingerprint(&bytes) != cache.fingerprint
        })
    }
}

fn read_cache(src: &Source) -> CatalogFile {
    std::fs::read(cache_dir().join(src.cache)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// One console's catalog: the cache when it matches the source, else a fresh import.
fn load_one(src: &Source) -> CatalogFile {
    let cache = read_cache(src);
    let valid = cache.schema == CATALOG_SCHEMA && cache.source == src.url && !cache.games.is_empty();
    let bundled = || if src.bundled.is_empty() { CatalogFile::default() } else { import_for(src, src.bundled).unwrap_or_default() };
    match source_bytes(src) {
        Ok(bytes) if bytes.is_empty() => CatalogFile::default(), // not collected yet (PS4)
        Ok(bytes) if valid && cache.fingerprint == fingerprint(&bytes) => cache,
        Ok(bytes) => match import_for(src, &bytes).and_then(|file| save(src, file)) {
            Ok(file) => file,
            Err(error) => {
                crate::log!("RuTracker {} catalog import failed: {error}", src.platform.label());
                if valid { cache } else { bundled() }
            }
        },
        Err(error) => {
            crate::log!("RuTracker {} catalog source unavailable: {error}", src.platform.label());
            if valid { cache } else { bundled() }
        }
    }
}

/// One Library from every console's catalog. PS5's snapshot details describe the result.
fn merge(mut files: Vec<CatalogFile>) -> CatalogFile {
    let mut merged = std::mem::take(&mut files[0]);
    for file in files {
        merged.games.extend(file.games);
    }
    merged.games.sort_by(|a, b| b.id.cmp(&a.id));
    merged
}

/// Explicit override, then user data, then a source checkout's generated JSON.
/// Installed binaries always have a bundled snapshot if none of those exist.
fn source_path(src: &Source) -> Option<PathBuf> {
    if let Some(path) = override_path(src) {
        return Some(path);
    }
    let data = data_path(src);
    if data.is_file() { return Some(data); }
    let generated = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist/rutracker").join(src.file);
    generated.is_file().then_some(generated)
}

fn override_path(src: &Source) -> Option<PathBuf> {
    std::env::var_os(src.env).filter(|path| !path.is_empty()).map(PathBuf::from)
}

/// Where downloaded catalogs live (and where a hand-collected one can be put).
fn data_path(src: &Source) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")).join(".local/share")
    }).join(crate::util::APP_NAME).join("rutracker").join(src.file)
}

/// The catalog the daily workflow publishes (.github/workflows/catalog.yml): new releases and
/// fresh seeder counts without a launcher update.
fn remote_base() -> String {
    std::env::var("AZRAK_CATALOG_URL").unwrap_or_else(|_| format!("https://raw.githubusercontent.com/{}/catalog/", crate::update::REPO))
}

/// Download published catalogs that differ from the local ones, into the data folder. A file is
/// only replaced by one that matches the published checksum and imports cleanly; any failure
/// leaves the local catalog as it was. Returns how many catalogs changed.
fn fetch_remote(progress: &dyn Fn(String)) -> Result<usize, String> {
    use sha2::{Digest, Sha256};
    let base = remote_base();
    let manifest = crate::util::http_json(&format!("{base}catalog.json"))?;
    let _ = atomic_write(&cache_dir().join("catalog-checked"), now_secs().to_string().as_bytes());
    let mut changed = 0;
    for src in SOURCES.iter().filter(|src| override_path(src).is_none()) {
        let Some(sha) = manifest["files"][src.file]["sha256"].as_str() else { continue };
        let path = data_path(src);
        let local = std::fs::read(&path).ok().map(|bytes| format!("{:x}", Sha256::digest(&bytes)));
        if local.as_deref() == Some(sha) {
            continue;
        }
        progress(format!("Downloading the {} catalog", src.platform.label()));
        let packed = crate::util::http_get(&format!("{base}{}.gz", src.file))?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(flate2::read::GzDecoder::new(&packed[..]), 256 << 20), &mut bytes)
            .map_err(|error| format!("{} catalog: {error}", src.platform.label()))?;
        if format!("{:x}", Sha256::digest(&bytes)) != sha {
            return Err(format!("The downloaded {} catalog doesn't match its checksum", src.platform.label()));
        }
        import_for(src, &bytes)?;
        atomic_write(&path, &bytes).map_err(|error| format!("Could not save the {} catalog: {error}", src.platform.label()))?;
        changed += 1;
    }
    Ok(changed)
}

/// Whether a published catalog differs from the local one. It reads only the 1 KB manifest, so
/// it's asked at every start and every hour; the full sync (which imports every catalog) runs only
/// when it says so. A new catalog is then in the Library minutes after it's published.
pub fn published_changed() -> Result<bool, String> {
    use sha2::{Digest, Sha256};
    let manifest = crate::util::http_json(&format!("{}catalog.json", remote_base()))?;
    Ok(SOURCES.iter().filter(|src| override_path(src).is_none()).any(|src| {
        manifest["files"][src.file]["sha256"].as_str().is_some_and(|sha| {
            std::fs::read(data_path(src)).ok().map(|bytes| format!("{:x}", Sha256::digest(&bytes))).as_deref() != Some(sha)
        })
    }))
}

/// When the published catalog was last checked (0: never).
fn last_check() -> f64 {
    std::fs::read_to_string(cache_dir().join("catalog-checked")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0.0)
}

fn source_bytes(src: &Source) -> Result<Vec<u8>, String> {
    match source_path(src) {
        Some(path) => std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display())),
        None => Ok(src.bundled.to_vec()),
    }
}

fn fingerprint(bytes: &[u8]) -> String { sha1_smol::Sha1::from(bytes).digest().to_string() }
fn text(value: &Value, key: &str) -> String { value[key].as_str().unwrap_or("").trim().to_string() }

static SIZE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)([\d.,]+)\s*(TB|GB|MB)").unwrap());
static TITLE_ID_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)(?:^|[^A-Z])(?:P\s*)+S\s*A\s*(\d{5})|(?:^|[^A-Z])C\s*U\s*S\s*A\s*(\d{5})"
).unwrap());
static CLEAN_TITLE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^(?:\[[^\]]*\]\s*)+").unwrap());
static MAGNET_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)[?&]xt=urn:btih:(?:[a-f0-9]{40}|[a-z2-7]{32})(?:&|$)"
).unwrap());

pub fn find_title_id(texts: &[&str]) -> String {
    texts.iter().flat_map(|value| title_ids(value)).next().unwrap_or_default()
}

fn title_ids(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    TITLE_ID_RE.captures_iter(value).filter_map(|captures| {
        // Do not consume the separator: adjacent IDs like PPSA12345/PPSA54321
        // must both survive. Reject a prefix of an invalid six-digit ID.
        if value.as_bytes().get(captures.get(0)?.end()).is_some_and(u8::is_ascii_digit) { return None; }
        let id = captures.get(1).map(|id| format!("PPSA{}", id.as_str()))
            .or_else(|| captures.get(2).map(|id| format!("CUSA{}", id.as_str())))?;
        seen.insert(id.clone()).then_some(id)
    }).collect()
}

fn parse_size_gb(value: &str) -> Option<f64> {
    let captures = SIZE_RE.captures(value)?;
    let size: f64 = captures[1].replace(',', ".").parse().ok()?;
    Some(size * match captures[2].to_uppercase().as_str() { "TB" => 1024.0, "MB" => 1.0 / 1024.0, _ => 1.0 })
}

fn clean_name(value: &str) -> String {
    let name = CLEAN_TITLE_RE.replace(value, "");
    let name = name.split(" [").next().unwrap_or("").trim();
    name.strip_prefix("PSVR2 only ").or_else(|| name.strip_prefix("PSVR only ")).unwrap_or(name).to_string()
}

/// Import a PS5 forum snapshot.
#[cfg(test)]
pub fn import(bytes: &[u8]) -> Result<CatalogFile, String> {
    import_for(&SOURCES[0], bytes)
}

/// The PS4 forum also carries apps (YouTube, Media Player), system software and forum
/// threads (rules, chat); the Library lists games only.
fn is_game(title: &str, genre: &str, title_id: &str, magnet: &str) -> bool {
    static NOT_GAME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
        r"(?i)\b(firmware|exploits?|ps4hen|theme pack|psnow|ps now|shadps4)\b").unwrap());
    let app = ["app", "apps", "application", "applications", "program", "programs", "software", "media", "utility", "utilities"]
        .iter().any(|g| genre.eq_ignore_ascii_case(g));
    let thread = magnet.is_empty() && title_id.is_empty() && genre.is_empty();
    !app && !thread && !NOT_GAME.is_match(title)
}

fn import_for(src: &Source, bytes: &[u8]) -> Result<CatalogFile, String> {
    let report: Value = serde_json::from_slice(bytes).map_err(|error| format!("Invalid RuTracker JSON: {error}"))?;
    if report["complete"].as_bool() != Some(true) || report["source"].as_str() != Some(src.url) {
        return Err(format!("Expected a complete {} forum listing ({}); partial results are not imported", src.platform.label(), src.url));
    }
    if report["translation"]["language"].as_str() != Some("en") {
        return Err("Translate the RuTracker snapshot to English before importing it".into());
    }
    let topics = report["topics"].as_array().ok_or("RuTracker topics array is missing")?;
    if topics.is_empty() || report["topic_count"].as_u64() != Some(topics.len() as u64) {
        return Err("RuTracker topic count is empty or does not match the listing".into());
    }
    // Seeder counts are as fresh as the last daily refresh, else the original collection.
    let snapshot_at = Some(text(&report, "refreshed_at")).filter(|at| !at.is_empty()).unwrap_or_else(|| text(&report, "collected_at"));
    let mut seen = HashSet::new();
    let mut games = Vec::with_capacity(topics.len());
    for topic in topics {
        let id = topic["id"].as_str().and_then(|id| id.parse::<i64>().ok())
            .filter(|id| *id > 0).ok_or("Invalid RuTracker topic ID")?;
        if !seen.insert(id) { return Err(format!("Duplicate RuTracker topic ID: {id}")); }
        let info = topic["game_info"].clone();
        let title = text(topic, "title");
        let named = text(&info, "name");
        let pc = src.platform == Platform::Pc;
        let name = if pc { crate::pcgame::clean_name(&title) } else { clean_name(if named.is_empty() { &title } else { &named }) };
        if name.is_empty() && pc { continue; }
        if name.is_empty() { return Err(format!("Topic {id} has no game name")); }
        let steam = topic.get("steam").filter(|s| s["appid"].as_u64().is_some())
            .and_then(|s| serde_json::from_value::<Steam>(s.clone()).ok());
        let raw_id = text(&info, "title_id");
        let ids = title_ids(&raw_id);
        let primary_id = ids.first().cloned().unwrap_or_else(|| find_title_id(&[&title]));
        let genre = text(&info, "genre");
        let size = text(topic, "size");
        let summary = text(&info, "release_summary");
        let images: Vec<String> = info["image_urls"].as_array().into_iter().flatten()
            .filter_map(Value::as_str).filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            .map(str::to_string).collect();
        let candidate = text(topic, "magnet");
        if !is_game(&title, &genre, &raw_id, &candidate) { continue; }
        let magnet = if candidate.starts_with("magnet:?") && MAGNET_RE.is_match(&candidate) { candidate } else { String::new() };
        let multiplayer = text(&info, "multiplayer");
        let mode = match multiplayer.as_str() { "No" | "None" => "Single player".to_string(), "Yes" => "Multiplayer".to_string(), _ => multiplayer };
        // Release dates are not posting dates: do not invent "added" timestamps.
        let release_date = text(&info, "release_date");
        games.push(Game {
            id, name, title, date: text(topic, "published_at"), link: format!("https://rutracker.net/forum/viewtopic.php?t={id}"),
            cover: images.first().cloned().unwrap_or_default(), trailer: String::new(),
            genres: genre.split([',', '/', '|']).map(str::trim).filter(|genre| !genre.is_empty()).map(str::to_string).collect(),
            mode, release: if release_date.is_empty() { text(&info, "release_year") } else { release_date },
            size_gb: parse_size_gb(&size), size, version: text(&info, "version"), title_id: primary_id,
            update: text(&info, "release_update"), excerpt: summary.clone(),
            description: (!summary.is_empty()).then_some(summary).into_iter().collect(),
            title_ids: ids, region: text(&info, "region"), magnet,
            seeders: topic["seeders"].as_u64(), leechers: topic["leechers"].as_u64(),
            peers_observed: Some(text(topic, "peers_observed")).filter(|at| !at.is_empty()).unwrap_or_else(|| snapshot_at.clone()), game_info: info, platform: src.platform,
            wine: src.url == WINE_SOURCE, steam,
        });
    }
    // Topics, not title IDs, are the identity: retain regional/version variants.
    games.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(CatalogFile { updated: now_secs(), schema: CATALOG_SCHEMA, source: src.url.into(),
        fingerprint: fingerprint(bytes), snapshot_at, games })
}

fn save(src: &Source, file: CatalogFile) -> Result<CatalogFile, String> {
    let bytes = serde_json::to_vec(&file).map_err(|error| error.to_string())?;
    atomic_write(&cache_dir().join(src.cache), &bytes).map_err(|error| format!("Could not cache RuTracker catalog: {error}"))?;
    Ok(file)
}

/// Download the published catalog when it changed, then import the local catalogs. Never contacts
/// RuTracker. The PS5 catalog must import; a PS4 catalog is optional until it has been collected.
/// Also says whether the published catalog couldn't be reached (to try again once the network is).
pub fn sync(progress: &dyn Fn(String)) -> Result<(CatalogFile, bool), String> {
    progress("Checking for new releases".into());
    // Offline or not published yet: the local catalogs still load.
    let offline = match fetch_remote(progress) {
        Ok(changed) => { crate::log!("Published catalog: {changed} updated"); false }
        Err(error) => {
            crate::log!("Published catalog not downloaded: {error}");
            crate::util::is_network_error(&error)
        }
    };
    progress("Loading RuTracker snapshots".into());
    let mut files = Vec::new();
    for (i, src) in SOURCES.iter().enumerate() {
        let bytes = source_bytes(src)?;
        if bytes.is_empty() && i > 0 {
            files.push(CatalogFile::default());
            continue;
        }
        match import_for(src, &bytes).and_then(|file| save(src, file)) {
            Ok(file) => files.push(file),
            Err(error) if i > 0 => {
                crate::log!("RuTracker {} catalog import failed: {error}", src.platform.label());
                files.push(read_cache(src));
            }
            Err(error) => return Err(error),
        }
    }
    let file = merge(files);
    progress(format!("Loaded {} RuTracker releases", file.games.len()));
    crate::log!("RuTracker catalog loaded: {} release topics", file.games.len());
    Ok((file, offline))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(topics: Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({ "source": SOURCE, "complete": true,
            "translation": { "language": "en" }, "collected_at": "2026-09-30T00:56:18Z",
            "topic_count": topics.as_array().unwrap().len(), "topics": topics })).unwrap()
    }
    #[test]
    fn ps4_snapshot_imports_as_ps4_and_merges() {
        let ps4 = serde_json::to_vec(&serde_json::json!({ "source": PS4_SOURCE, "complete": true,
            "translation": { "language": "en" }, "collected_at": "2026-10-02T00:00:00Z", "topic_count": 1,
            "topics": [{ "id": "900", "title": "[PS4] Firewatch [CUSA04118] [EUR]", "game_info": { "title_id": "CUSA04118" } }] })).unwrap();
        let file = import_for(&SOURCES[1], &ps4).unwrap();
        assert_eq!(file.games[0].platform, Platform::Ps4);
        assert_eq!(file.games[0].name, "Firewatch");
        assert!(import(&ps4).is_err(), "a PS4 snapshot is not accepted as the PS5 catalog");
        let ps5 = fixture(serde_json::json!([{ "id": "100", "title": "[PS5] Example [EUR]", "game_info": { "title_id": "PPSA12345" } }]));
        let merged = merge(vec![import(&ps5).unwrap(), file]);
        assert_eq!(merged.games.iter().map(|g| (g.id, g.platform)).collect::<Vec<_>>(), vec![(900, Platform::Ps4), (100, Platform::Ps5)]);
    }

    #[test]
    fn pc_catalogs_import_as_pc_games_with_their_steam_data() {
        let wine = serde_json::to_vec(&serde_json::json!({ "source": WINE_SOURCE, "complete": true,
            "translation": { "language": "en" }, "collected_at": "2026-10-14T00:00:00Z", "topic_count": 2,
            "topics": [
                { "id": "6000", "title": "Celeste (v1.4.0.0) + 2 DLC [amd64] [Multi] [Wine]", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553A",
                  "game_info": { "genre": "Platformer" },
                  "steam": { "appid": 504230, "name": "Celeste", "short": "Help Madeline survive.", "genres": ["Action"], "release": "Jan 25, 2018",
                    "linux": true, "metacritic": 88, "header": "https://x/header.jpg", "shots": ["https://x/ss.jpg"], "video": null,
                    "assets": { "portrait": "https://x/p.jpg", "hero": null, "logo": "https://x/logo.png" }, "protondb": "platinum" } },
                { "id": "6001", "title": "[In development] Unknown Game [amd64] [ENG] [Wine]", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553B",
                  "game_info": { "genre": "RPG" }, "steam": { "appid": null, "checked_at": "2026-10-14T00:00:00Z" } }
            ] })).unwrap();
        let file = import_for(&SOURCES[3], &wine).unwrap();
        let celeste = file.games.iter().find(|g| g.id == 6000).unwrap();
        assert_eq!((celeste.platform, celeste.wine, celeste.name.as_str()), (Platform::Pc, true, "Celeste"));
        let info = celeste.steam.as_ref().unwrap().info();
        assert_eq!((info.portrait.as_str(), info.master.as_str(), info.release.as_str()), ("https://x/p.jpg", "https://x/header.jpg", "2018-01-25"));
        assert_eq!(info.store, "https://store.steampowered.com/app/504230/");
        let unknown = file.games.iter().find(|g| g.id == 6001).unwrap();
        assert_eq!((unknown.name.as_str(), unknown.steam.is_none()), ("Unknown Game", true));
        assert_eq!(steam_date("25 Jan, 2018").as_deref(), Some("2018-01-25"));
        assert_eq!(steam_date("Coming soon"), None);
        // A PC catalog isn't taken for another forum's.
        assert!(import_for(&SOURCES[2], &wine).is_err());
    }

    #[test]
    fn apps_system_software_and_forum_threads_are_not_games() {
        let bytes = fixture(serde_json::json!([
            { "id": "1", "title": "[PS5] YouTube [EUR] [2.23]", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553A", "game_info": { "genre": "App", "title_id": "PPSA01116" } },
            { "id": "2", "title": "[PS5] SHAREfactory [EUR]", "game_info": { "genre": "Media", "title_id": "PPSA00572" } },
            { "id": "3", "title": "[PS5] System firmware version 1.52 [RUS]", "game_info": { "genre": "Software" } },
            { "id": "4", "title": "[PS5] 4.05 + PS4HEN", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553B" },
            { "id": "5", "title": "[PS5] Fludilka" },
            { "id": "6", "title": "[PS5] Far Cry 5 [EUR/RUS] (v1.00)", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553C" },
            { "id": "7", "title": "[PS5] Patapon 3 [USA]", "game_info": { "genre": "Action Adventure, Rhythm, Music", "title_id": "PPSA12345" } }
        ]));
        let file = import(&bytes).unwrap();
        assert_eq!(file.games.iter().map(|g| g.id).collect::<Vec<_>>(), vec![7, 6]);
    }

    #[test]
    fn title_ids_and_sizes() {
        assert_eq!(find_title_id(&["P PSA16106 – EUR"]), "PPSA16106");
        assert_eq!(find_title_id(&["PPPSA30140 – EUR"]), "PPSA30140");
        assert_eq!(find_title_id(&["CUSA12345"]), "CUSA12345");
        assert_eq!(find_title_id(&["PS5 Digital Edition"]), "");
        assert_eq!(title_ids("PPSA02572 / PPSA02571* / PPSA02572"), vec!["PPSA02572", "PPSA02571"]);
        assert_eq!(title_ids("PPSA02572/PPSA02571"), vec!["PPSA02572", "PPSA02571"]);
        assert_eq!(find_title_id(&["PPSA123456"]), "");
        assert_eq!(parse_size_gb("57 GB"), Some(57.0));
        assert_eq!(parse_size_gb("100 MB"), Some(100.0 / 1024.0));
        assert_eq!(parse_size_gb("1.5 TB"), Some(1536.0));
    }
    #[test]
    fn missing_and_zero_peers_are_distinct_and_variants_survive() {
        let bytes = fixture(serde_json::json!([
            { "id": "100", "title": "[PS5] Example [USA]", "seeders": 0, "leechers": null,
              "game_info": { "title_id": "PPSA12345", "region": "USA", "version": "1.00" } },
            { "id": "101", "title": "[PS5] Example [EUR]", "game_info": { "title_id": "PPSA12345" } }
        ]));
        let file = import(&bytes).unwrap();
        assert_eq!(file.games.len(), 2);
        let game = file.games.iter().find(|game| game.id == 100).unwrap();
        assert_eq!(game.seeders, Some(0));
        assert_eq!(game.leechers, None);
        assert_eq!(game.title_id, "PPSA12345");
        assert_eq!(game.region, "USA");
        assert_eq!(game.name, "Example");
        assert!(game.date.is_empty());
    }
    #[test]
    fn rejects_partial_and_wrong_sources() {
        let mut report: Value = serde_json::from_slice(&fixture(serde_json::json!([
            { "id": "100", "title": "[PS5] Example" }
        ]))).unwrap();
        report["complete"] = Value::Bool(false);
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
        report["complete"] = Value::Bool(true);
        report["source"] = Value::String("https://example.com".into());
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
    }
    #[test]
    fn rejects_count_mismatch_duplicates_and_untranslated_sources() {
        let bytes = fixture(serde_json::json!([
            { "id": "100", "title": "[PS5] Example" },
            { "id": "100", "title": "[PS5] Duplicate" }
        ]));
        assert!(import(&bytes).is_err());
        let mut report: Value = serde_json::from_slice(&bytes).unwrap();
        report["topics"][1]["id"] = Value::String("101".into());
        report["topic_count"] = Value::from(3);
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
        report["topic_count"] = Value::from(2);
        report["translation"]["language"] = Value::String("ru".into());
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
    }
    #[test]
    fn bundled_snapshot_is_complete_and_keeps_magnets() {
        let file = import(BUNDLED).unwrap();
        assert_eq!(file.games.len(), 614);
        assert!(file.games.iter().all(|game| !game.magnet.is_empty()));
        assert!(file.games.iter().all(|game| game.seeders.is_some() && game.leechers.is_some()));
        assert!(file.games.iter().all(|game| !game.name.contains("[PS5]")));
        assert!(file.games.iter().all(|game| game.date.is_empty()));
    }
}