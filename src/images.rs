//! Image loading: fetch → decode → resize → cache, on a priority thread pool.
//!
//! Two disk caches under ~/.cache/ps5-launcher:
//!   img/<sha1[:2]>/<sha1(url)>   original downloads (same layout as v1)
//!   thumbs/<sha1(key)>.qoi        decoded + resized pixels; QOI decodes ~10x faster than PNG
//! A warm start therefore never touches the network or a JPEG/PNG decoder.

use crate::util::{cache_dir, http_get, sha1_hex};
use fast_image_resize as fr;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    Url(String),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Job {
    pub key: String,
    pub src: Src,
    pub max_w: u32,
    /// Fraction of the height to cut from the top (removes the PS5 banner on site covers).
    pub crop_top: f32,
    pub prio: u8,
    seq: u64,
}

impl PartialEq for Job {
    fn eq(&self, o: &Self) -> bool {
        self.prio == o.prio && self.seq == o.seq
    }
}
impl Eq for Job {}
impl PartialOrd for Job {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Job {
    // Highest priority first; newest request first within a priority (what the user looks at now).
    fn cmp(&self, o: &Self) -> Ordering {
        self.prio.cmp(&o.prio).then(self.seq.cmp(&o.seq))
    }
}

pub mod prio {
    pub const PREFETCH: u8 = 1;
    pub const CARD: u8 = 3;
    pub const TILE: u8 = 5;
    pub const DETAIL: u8 = 6;
    pub const HERO: u8 = 8;
}

struct Queue {
    heap: BinaryHeap<Job>,
    queued: HashSet<String>,
    seq: u64,
}

type Done = Arc<dyn Fn(String, Option<SharedPixelBuffer<Rgba8Pixel>>) + Send + Sync>;

type Lane = Arc<(Mutex<Queue>, Condvar)>;

/// Two lanes of workers: images already on disk never wait behind slow downloads.
#[derive(Clone)]
pub struct Pool {
    local: Lane,
    net: Lane,
}

fn lane(name: &str, threads: usize, done: &Done) -> Lane {
    let q: Lane = Arc::new((Mutex::new(Queue { heap: BinaryHeap::new(), queued: HashSet::new(), seq: 0 }), Condvar::new()));
    for i in 0..threads {
        let q = q.clone();
        let done = done.clone();
        std::thread::Builder::new()
            .name(format!("img-{name}-{i}"))
            .spawn(move || loop {
                let job = {
                    let (lock, cv) = &*q;
                    let mut st = lock.lock().unwrap();
                    loop {
                        if let Some(j) = st.heap.pop() {
                            break j;
                        }
                        st = cv.wait(st).unwrap();
                    }
                };
                let result = std::panic::catch_unwind(|| load(&job)).ok().flatten();
                q.0.lock().unwrap().queued.remove(&job.key);
                done(job.key, result);
            })
            .ok();
    }
    q
}

/// True if an image can be shown without touching the network.
pub fn on_disk(key: &str, src: &Src) -> bool {
    thumb_path(key).exists()
        || match src {
            Src::Url(u) => raw_cache_path(u).exists(),
            Src::File(p) => p.exists(),
        }
}

impl Pool {
    pub fn new(local_threads: usize, net_threads: usize, done: Done) -> Pool {
        Pool { local: lane("disk", local_threads, &done), net: lane("net", net_threads, &done) }
    }

    pub fn request(&self, key: String, src: Src, max_w: u32, crop_top: f32, prio: u8) {
        // UI audits run without artwork, so only interface elements are on screen.
        if *NO_ART {
            return;
        }
        // Already queued in either lane: bump its priority.
        for q in [&self.local, &self.net] {
            let mut st = q.0.lock().unwrap();
            if st.queued.contains(&key) {
                let old: Vec<Job> = st.heap.drain().collect();
                st.seq += 1;
                let seq = st.seq;
                for mut j in old {
                    if j.key == key {
                        j.prio = j.prio.max(prio);
                        j.seq = seq;
                    }
                    st.heap.push(j);
                }
                return;
            }
        }
        let q = if on_disk(&key, &src) { &self.local } else { &self.net };
        let (lock, cv) = &**q;
        let mut st = lock.lock().unwrap();
        st.queued.insert(key.clone());
        st.seq += 1;
        let seq = st.seq;
        st.heap.push(Job { key, src, max_w, crop_top, prio, seq });
        cv.notify_one();
    }

    /// Drop queued (not yet started) jobs that are no longer wanted.
    pub fn retain(&self, keep: impl Fn(&Job) -> bool) -> Vec<String> {
        let mut dropped = Vec::new();
        for q in [&self.local, &self.net] {
            let mut st = q.0.lock().unwrap();
            let old: Vec<Job> = st.heap.drain().collect();
            for j in old {
                if keep(&j) {
                    st.heap.push(j);
                } else {
                    st.queued.remove(&j.key);
                    dropped.push(j.key);
                }
            }
        }
        dropped
    }
}

fn raw_cache_path(url: &str) -> PathBuf {
    let h = sha1_hex(url);
    cache_dir().join("img").join(&h[..2]).join(h)
}

fn thumb_path(key: &str) -> PathBuf {
    cache_dir().join("thumbs").join(format!("{}.qoi", sha1_hex(key)))
}

fn fetch(url: &str) -> Option<Vec<u8>> {
    let path = raw_cache_path(url);
    if let Ok(b) = std::fs::read(&path) {
        return Some(b);
    }
    let mut candidates = vec![url.to_string()];
    // YouTube: maxres art is missing for some videos; fall back gracefully.
    if url.contains("i.ytimg.com") && url.ends_with("/maxresdefault.jpg") {
        let base = url.rsplit_once('/').map(|x| x.0).unwrap_or(url);
        candidates.push(format!("{base}/sddefault.jpg"));
        candidates.push(format!("{base}/hqdefault.jpg"));
    }
    for c in candidates {
        if let Ok(data) = http_get(&c) {
            // ytimg serves a tiny grey placeholder for missing art.
            if !data.is_empty() && (data.len() > 1500 || !c.contains("i.ytimg.com")) {
                let _ = crate::util::atomic_write(&path, &data);
                return Some(data);
            }
        }
    }
    None
}

/// Download (but don't decode) images into the disk cache, `threads` at a time. Already
/// cached files are skipped instantly. Already-cached files
/// are skipped instantly. `progress(done, total)` is called as files complete.
pub fn warm(urls: Vec<String>, threads: usize, progress: &(dyn Fn(usize, usize) + Sync)) {
    if *NO_ART {
        return;
    }
    let todo: Vec<String> = urls.into_iter().filter(|u| !raw_cache_path(u).exists()).collect();
    let total = todo.len();
    if total == 0 {
        return;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(u) = todo.get(i) else { break };
                let _ = fetch(u);
                progress(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, total);
            });
        }
    });
}

const BIG_PIXELS: u32 = 1000 * 700;

/// Remove oversized decoded copies left by older versions (they could add up to gigabytes).
pub fn trim_thumbs() {
    let Ok(rd) = std::fs::read_dir(cache_dir().join("thumbs")) else { return };
    let mut freed = 0u64;
    for e in rd.flatten() {
        if let Ok(m) = e.metadata() {
            if m.len() > (BIG_PIXELS as u64) * 4 {
                freed += m.len();
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    if freed > 0 {
        crate::log!("removed {} MB of oversized decoded images", freed >> 20);
    }
}

pub static NO_ART: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("PS5_LAUNCHER_NO_ART").is_some());

pub static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
pub static DEBUG: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("PS5_LAUNCHER_DEBUG").is_some());

fn load(job: &Job) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let t0 = std::time::Instant::now();
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f32() * 1000.0;
    let tp = thumb_path(&job.key);
    if let Ok(bytes) = std::fs::read(&tp) {
        if let Ok((h, px)) = qoi::decode_to_vec(&bytes) {
            if px.len() == (h.width * h.height * 4) as usize {
                if *DEBUG {
                    crate::log!("t={:>6} img thumb {:>6.1}ms {}x{} p{} {}", START.elapsed().as_millis(), ms(t0), h.width, h.height, job.prio, short(&job.key));
                }
                return Some(SharedPixelBuffer::clone_from_slice(&px, h.width, h.height));
            }
        }
    }
    let was_cached = matches!(&job.src, Src::Url(u) if raw_cache_path(u).exists());
    let bytes = match &job.src {
        Src::Url(u) => fetch(u)?,
        Src::File(p) => std::fs::read(p).ok()?,
    };
    let t_fetch = ms(t0);
    let t1 = std::time::Instant::now();
    let mut img = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let t_decode = ms(t1);
    // A negative crop means "find it": the blue PS4 band printed across PS4 box art.
    let crop_top = if job.crop_top < 0.0 { ps4_band(&img) } else { job.crop_top };
    if crop_top > 0.0 {
        let cut = (img.height() as f32 * crop_top) as u32;
        img = image::imageops::crop_imm(&img, 0, cut, img.width(), img.height() - cut).to_image();
    }
    let t2 = std::time::Instant::now();
    let (w, h) = img.dimensions();
    let (px, w, h) = if job.max_w > 0 && w > job.max_w {
        let nh = ((h as u64 * job.max_w as u64) / w as u64).max(1) as u32;
        let src = fr::images::Image::from_vec_u8(w, h, img.into_raw(), fr::PixelType::U8x4).ok()?;
        let mut dst = fr::images::Image::new(job.max_w, nh, fr::PixelType::U8x4);
        let opts = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom));
        fr::Resizer::new().resize(&src, &mut dst, &opts).ok()?;
        (dst.into_vec(), job.max_w, nh)
    } else {
        (img.into_raw(), w, h)
    };
    let t_resize = ms(t2);
    let t3 = std::time::Instant::now();
    // Keep a decoded copy only for small images (tiles, covers, screenshots). Full-screen art
    // would be ~4 MB each; it decodes from its download in ~20 ms anyway.
    if w * h <= BIG_PIXELS {
        if let Ok(enc) = qoi::encode_to_vec(&px, w, h) {
            let _ = crate::util::atomic_write(&tp, &enc);
        }
    }
    if *DEBUG {
        crate::log!("t={:>6} img {} {:>6.1}ms fetch {:>6.1} decode {:>5.1} resize {:>5.1} save {:>5.1} {}KB {}x{} p{} {}",
            START.elapsed().as_millis(), if was_cached { "disk" } else { "NET " }, ms(t0), t_fetch, t_decode, t_resize, ms(t3), bytes.len() / 1024, w, h, job.prio, short(&job.key));
    }
    Some(SharedPixelBuffer::clone_from_slice(&px, w, h))
}

fn short(key: &str) -> String {
    let tail: String = key.chars().rev().take(34).collect::<Vec<_>>().into_iter().rev().collect();
    tail
}

/// UI-thread side: decoded images by key with an LRU byte budget.
pub struct Store {
    map: HashMap<String, (Image, usize, u64)>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    total: usize,
    tick: u64,
    budget: usize,
    pub pool: Pool,
}

impl Store {
    pub fn new(pool: Pool, budget_mb: usize) -> Store {
        Store { map: HashMap::new(), pending: HashSet::new(), failed: HashSet::new(), total: 0, tick: 0, budget: budget_mb << 20, pool }
    }

    pub fn get(&mut self, key: &str) -> Option<Image> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(key).map(|e| {
            e.2 = t;
            e.0.clone()
        })
    }

    /// Returns the image if ready, otherwise queues it and returns None.
    pub fn want(&mut self, key: &str, src: Src, max_w: u32, crop_top: f32, prio: u8) -> Option<Image> {
        if let Some(i) = self.get(key) {
            return Some(i);
        }
        if !self.failed.contains(key) {
            self.pending.insert(key.to_string());
            self.pool.request(key.to_string(), src, max_w, crop_top, prio);
        }
        None
    }

    /// True if the image will show within a few ms (downloaded, and decodes fine).
    /// This image couldn't be downloaded (it won't be shown).
    pub fn has_failed(&self, key: &str) -> bool {
        self.failed.contains(key)
    }

    pub fn on_disk(&self, key: &str, src: &Src) -> bool {
        !*NO_ART && !self.failed.contains(key) && on_disk(key, src)
    }

    pub fn is_pending(&self, key: &str) -> bool {
        self.pending.contains(key)
    }

    pub fn insert(&mut self, key: String, buf: Option<SharedPixelBuffer<Rgba8Pixel>>) {
        self.pending.remove(&key);
        let Some(buf) = buf else {
            self.failed.insert(key);
            return;
        };
        let bytes = (buf.width() * buf.height() * 4) as usize;
        self.tick += 1;
        if let Some(old) = self.map.insert(key, (Image::from_rgba8(buf), bytes, self.tick)) {
            self.total -= old.1;
        }
        self.total += bytes;
        if self.total > self.budget {
            let mut entries: Vec<(u64, String)> = self.map.iter().map(|(k, v)| (v.2, k.clone())).collect();
            entries.sort_unstable();
            for (_, k) in entries {
                if self.total <= self.budget * 8 / 10 {
                    break;
                }
                if let Some(e) = self.map.remove(&k) {
                    self.total -= e.1;
                }
            }
        }
    }

    /// Forget queued-but-unstarted requests the UI no longer needs.
    pub fn cancel_unless(&mut self, keep: impl Fn(&Job) -> bool) {
        for k in self.pool.retain(keep) {
            self.pending.remove(&k);
        }
    }

    pub fn clear_failures(&mut self) {
        self.failed.clear();
    }
}

/// Ask for this crop to remove the blue "PS4" band from PS4 box art, when it has one.
pub const CROP_PS4_BAND: f32 = -1.0;

/// Height (as a fraction) of the blue "PS4" band across the top of PS4 box art, or 0 when
/// there is none: rows of mostly PlayStation blue, with the white "PS4" lettering.
pub fn ps4_band(img: &image::RgbaImage) -> f32 {
    let (w, h) = img.dimensions();
    if w < 32 || h < w {
        return 0.0; // only portrait box art has the band
    }
    let mut rows = 0;
    for y in 0..h / 5 {
        let (mut blue, mut white, mut n) = (0u32, 0u32, 0u32);
        for x in (0..w).step_by(2) {
            let [r, g, b, _] = img.get_pixel(x, y).0;
            let (r, g, b) = (r as i32, g as i32, b as i32);
            if b > r + 60 && b > 100 {
                blue += 1;
            } else if r > 200 && g > 200 && b > 200 {
                white += 1;
            }
            n += 1;
        }
        if blue * 2 >= n && (blue + white) * 100 >= n * 85 {
            rows = y + 1;
        } else if rows > 0 || y as f32 > h as f32 * 0.02 {
            break; // the band ended, or there is none at the very top
        }
    }
    let fraction = rows as f32 / h as f32;
    if (0.04..=0.16).contains(&fraction) { fraction + 0.004 } else { 0.0 }
}

#[cfg(test)]
mod ps4_band_tests {
    use super::*;
    #[test]
    fn finds_the_band_only_on_ps4_box_art() {
        // 100x160 box art: 16 rows of PlayStation blue with a white "logo", then artwork.
        let mut boxart = image::RgbaImage::from_pixel(100, 160, image::Rgba([180, 60, 40, 255]));
        for y in 0..16 {
            for x in 0..100 {
                let logo = (10..30).contains(&x) && (5..11).contains(&y);
                boxart.put_pixel(x, y, if logo { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([10, 80, 190, 255]) });
            }
        }
        assert!((ps4_band(&boxart) - (16.0 / 160.0 + 0.004)).abs() < 1e-4);
        // A blue sky that fills the whole top is not a band: it doesn't end in time.
        let sky = image::RgbaImage::from_pixel(100, 160, image::Rgba([20, 90, 200, 255]));
        assert_eq!(ps4_band(&sky), 0.0);
        // Square store art and landscape logos are left alone.
        assert_eq!(ps4_band(&image::RgbaImage::from_pixel(100, 100, image::Rgba([10, 80, 190, 255]))), 0.0);
        assert_eq!(ps4_band(&image::RgbaImage::from_pixel(160, 100, image::Rgba([10, 80, 190, 255]))), 0.0);
    }
}
