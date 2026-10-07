//! This PC: what the emulators need and whether this computer has it. Reads the processor and
//! memory from /proc, and the graphics cards from Vulkan (the API both emulators draw with), in
//! a child process so a broken graphics driver can't take the launcher down.

use crate::platform::Platform;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    /// Vulkan version the driver supports, as (major, minor).
    pub vulkan: (u32, u32),
    pub discrete: bool,
    /// Drawn by the processor (llvmpipe and friends), not a graphics card.
    pub software: bool,
    /// Graphics memory in GiB (for integrated graphics: what it may share).
    pub vram_gb: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Specs {
    pub cpu: String,
    pub cores: u32,
    pub threads: u32,
    /// AVX2, BMI2, FMA… (x86-64-v3): shadPS4 is built for these and stops at once without them.
    pub x86_64_v3: bool,
    pub ram_gb: f32,
    /// `None` when the graphics check didn't finish (a driver crash or hang).
    pub gpus: Option<Vec<Gpu>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Good,
    /// Runs, but slower than the game needs, and the main reason (a few words, e.g. "integrated GPU").
    Slow(String),
    /// Won't start, and why (e.g. "no Vulkan 1.3").
    No(String),
}

impl Verdict {
    pub fn headline(&self) -> &'static str {
        match self {
            Verdict::Good => "Should run well",
            Verdict::Slow(_) => "May run slowly",
            Verdict::No(_) => "Won't start",
        }
    }

    pub fn why(&self) -> Option<&str> {
        match self {
            Verdict::Good => None,
            Verdict::Slow(why) | Verdict::No(why) => Some(why),
        }
    }

    /// "May run slowly · integrated GPU".
    #[cfg(test)]
    pub fn short(&self) -> String {
        match self.why() {
            Some(why) => format!("{} · {why}", self.headline()),
            None => self.headline().into(),
        }
    }

    /// 1 good, 2 warning, 3 bad (the Fact and Settings colors).
    pub fn kind(&self) -> i32 {
        match self {
            Verdict::Good => 1,
            Verdict::Slow(_) => 2,
            Verdict::No(_) => 3,
        }
    }
}

impl Specs {
    /// The graphics card games will use: the strongest one that isn't software.
    pub fn gpu(&self) -> Option<&Gpu> {
        self.gpus.as_ref()?.iter().filter(|g| !g.software).max_by(|a, b| {
            (a.vulkan >= (1, 3), a.discrete, a.vram_gb).partial_cmp(&(b.vulkan >= (1, 3), b.discrete, b.vram_gb)).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Whether games of this console run here. `None`: unknown (the graphics check failed).
    pub fn verdict(&self, platform: Platform) -> Option<Verdict> {
        let gpus = self.gpus.as_ref()?;
        let Some(gpu) = self.gpu() else {
            return Some(Verdict::No(if gpus.is_empty() { "no GPU driver" } else { "no GPU" }.into()));
        };
        if gpu.vulkan < (1, 3) {
            return Some(Verdict::No("no Vulkan 1.3".into()));
        }
        let ps4 = platform == Platform::Ps4;
        if ps4 && !self.x86_64_v3 {
            return Some(Verdict::No("no AVX2".into()));
        }
        // What the README asks for, a step up for PS5 games: (VRAM, cores, threads, RAM).
        let (vram, cores, threads, ram) = if ps4 { (3.5, 4, 8, 7.0) } else { (7.5, 6, 12, 14.5) };
        let slow = if !gpu.discrete {
            Some("integrated GPU".to_string())
        } else if gpu.vram_gb < vram {
            Some(format!("{} GB GPU", gpu.vram_gb.round()))
        } else if self.cores < cores || self.threads < threads {
            Some(format!("{}-core CPU", self.cores))
        } else if self.ram_gb < ram {
            Some(format!("{} GB RAM", self.ram_gb.round()))
        } else {
            None
        };
        Some(slow.map_or(Verdict::Good, Verdict::Slow))
    }

    /// One line for Settings, e.g. "Ryzen 9 7950X (16 cores) · GeForce RTX 3090, 24 GB · 32 GB RAM".
    pub fn summary(&self) -> String {
        let gpu = match (self.gpus.as_ref(), self.gpu()) {
            (None, _) => "graphics unknown".to_string(),
            (_, None) => "no graphics card found".to_string(),
            (_, Some(g)) if g.discrete => format!("{}, {} GB", tidy(&g.name), g.vram_gb.round()),
            (_, Some(g)) => tidy(&g.name),
        };
        format!("{} ({} cores) · {gpu} · {} GB RAM", tidy(&self.cpu), self.cores, self.ram_gb.round())
    }
}

/// Drop brand noise from hardware names: "AMD Ryzen 9 7950X 16-Core Processor" → "Ryzen 9 7950X".
fn tidy(name: &str) -> String {
    let mut s = name.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    for cut in [" with Radeon", " CPU @", " Processor", " w/ Radeon", " ("] {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    let words: Vec<&str> = s.split_whitespace()
        .filter(|w| !matches!(*w, "AMD" | "Intel" | "NVIDIA" | "Corporation" | "Graphics" if s.split_whitespace().count() > 2))
        .filter(|w| !w.ends_with("-Core"))
        .collect();
    words.join(" ")
}

/// Read this PC. Takes up to a few seconds (the graphics check); call off the UI thread.
pub fn detect() -> Specs {
    if let Some(specs) = std::env::var("PS5_LAUNCHER_PRETEND_PC").ok().and_then(|s| pretend(&s)) {
        return specs;
    }
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut specs = parse_cpu(&cpuinfo);
    specs.ram_gb = meminfo.lines().find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f32>().ok())
        .map(|kb| kb / 1024.0 / 1024.0).unwrap_or(0.0);
    specs.gpus = probe_gpus();
    specs
}

/// For screenshots of other PCs: "CPU;cores;threads;avx2 0 or 1;RAM GB;GPUs" where GPUs are
/// "name,1.3,discrete,8" separated by "|".
fn pretend(text: &str) -> Option<Specs> {
    let f: Vec<&str> = text.split(';').collect();
    let [cpu, cores, threads, v3, ram, gpus] = f[..] else { return None };
    Some(Specs {
        cpu: cpu.into(),
        cores: cores.parse().ok()?,
        threads: threads.parse().ok()?,
        x86_64_v3: v3 == "1",
        ram_gb: ram.parse().ok()?,
        gpus: Some(parse_probe(&gpus.replace(',', "\t").replace('|', "\n"))),
    })
}

fn parse_cpu(text: &str) -> Specs {
    let mut specs = Specs::default();
    let mut cores = std::collections::HashSet::new();
    let (mut physical, mut flags) = (String::new(), String::new());
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match key.trim() {
            "processor" => specs.threads += 1,
            "model name" if specs.cpu.is_empty() => specs.cpu = value.to_string(),
            "physical id" => physical = value.to_string(),
            "core id" => {
                cores.insert((physical.clone(), value.to_string()));
            }
            "flags" if flags.is_empty() => flags = value.to_string(),
            _ => {}
        }
    }
    specs.cores = if cores.is_empty() { specs.threads } else { cores.len() as u32 };
    let have: std::collections::HashSet<&str> = flags.split_whitespace().collect();
    specs.x86_64_v3 = ["avx", "avx2", "bmi1", "bmi2", "fma", "f16c", "movbe", "abm"].iter().all(|f| have.contains(f));
    specs
}

/// Runs `ps5-launcher --probe-gpu` and reads its lines. `None` when it crashed or hung.
fn probe_gpus() -> Option<Vec<Gpu>> {
    let exe = std::env::current_exe().ok()?;
    let mut child = std::process::Command::new(exe).arg("--probe-gpu")
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    for _ in 0..100 {
        if let Some(status) = child.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            let mut out = String::new();
            std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
            return Some(parse_probe(&out));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// Lines of `name<TAB>major.minor<TAB>type<TAB>vram GiB`, as [`print_gpus`] writes them.
fn parse_probe(text: &str) -> Vec<Gpu> {
    text.lines().filter_map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        let [name, version, kind, vram] = f[..] else { return None };
        let (major, minor) = version.split_once('.')?;
        Some(Gpu {
            name: name.to_string(),
            vulkan: (major.parse().ok()?, minor.parse().ok()?),
            discrete: kind == "discrete",
            software: kind == "cpu",
            vram_gb: vram.parse().ok()?,
        })
    }).collect()
}

/// `--probe-gpu`: list the Vulkan devices on stdout. Runs in its own process (see [`probe_gpus`]).
/// No Vulkan loader or devices prints nothing; that's a real answer, not a failure.
pub fn print_gpus() {
    for (name, (major, minor), kind, vram) in unsafe { vulkan_devices() } {
        println!("{name}\t{major}.{minor}\t{kind}\t{vram:.1}");
    }
}

/// The few Vulkan calls needed, through the system's loader (libvulkan.so.1).
unsafe fn vulkan_devices() -> Vec<(String, (u32, u32), &'static str, f32)> {
    use std::ffi::{c_char, c_void, CStr};
    type Handle = *mut c_void;
    type GetProc = unsafe extern "C" fn(Handle, *const c_char) -> *const c_void;
    type CreateInstance = unsafe extern "C" fn(*const InstanceInfo, *const c_void, *mut Handle) -> i32;
    type Enumerate = unsafe extern "C" fn(Handle, *mut u32, *mut Handle) -> i32;
    type Props = unsafe extern "C" fn(Handle, *mut u8);
    type Destroy = unsafe extern "C" fn(Handle, *const c_void);
    #[repr(C)]
    struct AppInfo { s_type: u32, next: *const c_void, app: *const c_char, app_version: u32, engine: *const c_char, engine_version: u32, api: u32 }
    #[repr(C)]
    struct InstanceInfo { s_type: u32, next: *const c_void, flags: u32, app: *const AppInfo, layers: u32, layer_names: *const c_void, exts: u32, ext_names: *const c_void }

    let mut out = Vec::new();
    let lib = libc::dlopen(c"libvulkan.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
    if lib.is_null() {
        return out;
    }
    let gipa = libc::dlsym(lib, c"vkGetInstanceProcAddr".as_ptr());
    if gipa.is_null() {
        return out;
    }
    let gipa: GetProc = std::mem::transmute(gipa);
    let create = gipa(std::ptr::null_mut(), c"vkCreateInstance".as_ptr());
    if create.is_null() {
        return out;
    }
    let create: CreateInstance = std::mem::transmute(create);
    let app = AppInfo { s_type: 0, next: std::ptr::null(), app: c"ps5-launcher".as_ptr(), app_version: 1, engine: std::ptr::null(), engine_version: 0, api: 1 << 22 };
    let info = InstanceInfo { s_type: 1, next: std::ptr::null(), flags: 0, app: &app, layers: 0, layer_names: std::ptr::null(), exts: 0, ext_names: std::ptr::null() };
    let mut instance: Handle = std::ptr::null_mut();
    if create(&info, std::ptr::null(), &mut instance) != 0 || instance.is_null() {
        return out;
    }
    let get = |name: &CStr| gipa(instance, name.as_ptr());
    let (enumerate, props, memory, destroy) = (get(c"vkEnumeratePhysicalDevices"), get(c"vkGetPhysicalDeviceProperties"),
        get(c"vkGetPhysicalDeviceMemoryProperties"), get(c"vkDestroyInstance"));
    if [enumerate, props, memory, destroy].iter().any(|p| p.is_null()) {
        return out;
    }
    let (enumerate, props, memory, destroy): (Enumerate, Props, Props, Destroy) =
        (std::mem::transmute(enumerate), std::mem::transmute(props), std::mem::transmute(memory), std::mem::transmute(destroy));
    let mut count = 0u32;
    enumerate(instance, &mut count, std::ptr::null_mut());
    let mut devices = vec![std::ptr::null_mut(); count as usize];
    enumerate(instance, &mut count, devices.as_mut_ptr());
    for device in devices.into_iter().take(count as usize) {
        // VkPhysicalDeviceProperties (824 bytes): apiVersion @0, deviceType @16, deviceName[256] @20.
        let mut p = [0u64; 256];
        props(device, p.as_mut_ptr() as *mut u8);
        let p = p.as_ptr() as *const u8;
        let api = *(p as *const u32);
        let kind = match *(p.add(16) as *const u32) { 1 => "integrated", 2 => "discrete", 4 => "cpu", _ => "other" };
        let name = CStr::from_ptr(p.add(20) as *const c_char).to_string_lossy().replace('\t', " ");
        // VkPhysicalDeviceMemoryProperties (520 bytes): heapCount @260, heaps[16] {size u64, flags u32} @264.
        let mut m = [0u64; 80];
        memory(device, m.as_mut_ptr() as *mut u8);
        let m = m.as_ptr() as *const u8;
        let heaps = (*(m.add(260) as *const u32)).min(16) as usize;
        let local: u64 = (0..heaps).map(|i| m.add(264 + i * 16))
            .filter(|h| *(h.add(8) as *const u32) & 1 != 0) // DEVICE_LOCAL
            .map(|h| *(h as *const u64)).max().unwrap_or(0);
        out.push((name, (api >> 22, (api >> 12) & 0x3ff), kind, local as f32 / (1u64 << 30) as f32));
    }
    destroy(instance, std::ptr::null());
    out
}

/// Free space where games go, in GiB (the nearest existing folder).
pub fn free_gb(dir: &Path) -> Option<f64> {
    let mut dir = dir;
    while !dir.exists() {
        dir = dir.parent()?;
    }
    let path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(path.as_ptr(), &mut st) } == 0).then(|| st.f_bavail as f64 * st.f_frsize as f64 / (1u64 << 30) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(cores: u32, threads: u32, v3: bool, ram: f32, gpus: Vec<Gpu>) -> Specs {
        Specs { cpu: "Test CPU".into(), cores, threads, x86_64_v3: v3, ram_gb: ram, gpus: Some(gpus) }
    }

    fn gpu(name: &str, vulkan: (u32, u32), discrete: bool, vram: f32) -> Gpu {
        Gpu { name: name.into(), vulkan, discrete, software: name.starts_with("llvmpipe"), vram_gb: vram }
    }

    #[test]
    fn reads_the_processor() {
        let text = "processor\t: 0\nmodel name\t: AMD Ryzen 5 5600X 6-Core Processor\nphysical id\t: 0\ncore id\t: 0\n\
            flags\t\t: fpu sse4_2 avx avx2 bmi1 bmi2 fma f16c movbe abm\n\n\
            processor\t: 1\nmodel name\t: AMD Ryzen 5 5600X 6-Core Processor\nphysical id\t: 0\ncore id\t: 0\n\n\
            processor\t: 2\nphysical id\t: 0\ncore id\t: 1\n";
        let s = parse_cpu(text);
        assert_eq!((s.cores, s.threads, s.x86_64_v3), (2, 3, true));
        assert_eq!(tidy(&s.cpu), "Ryzen 5 5600X");
        let old = parse_cpu("processor\t: 0\nmodel name\t: Intel(R) Core(TM) i7-3770 CPU @ 3.40GHz\nflags\t\t: fpu sse4_2 avx\n");
        assert!(!old.x86_64_v3);
        assert_eq!(tidy(&old.cpu), "Core i7-3770");
    }

    #[test]
    fn reads_the_graphics_probe() {
        let gpus = parse_probe("NVIDIA GeForce RTX 3090\t1.4\tdiscrete\t24.0\nllvmpipe (LLVM 20.1.2, 256 bits)\t1.4\tcpu\t31.0\nbroken line\n");
        assert_eq!(gpus.len(), 2);
        assert_eq!((gpus[0].vulkan, gpus[0].discrete, gpus[0].vram_gb), ((1, 4), true, 24.0));
        assert!(gpus[1].software);
    }

    #[test]
    fn verdicts_say_what_holds_a_pc_back() {
        let rtx = gpu("NVIDIA GeForce RTX 3090", (1, 4), true, 24.0);
        let big = pc(16, 32, true, 31.3, vec![rtx.clone(), gpu("llvmpipe", (1, 4), false, 31.0)]);
        assert_eq!(big.verdict(Platform::Ps5), Some(Verdict::Good));
        assert_eq!(big.verdict(Platform::Ps4), Some(Verdict::Good));
        assert_eq!(big.summary(), "Test CPU (16 cores) · GeForce RTX 3090, 24 GB · 31 GB RAM");

        // Only software rendering, or none at all.
        assert_eq!(pc(8, 16, true, 16.0, vec![gpu("llvmpipe", (1, 4), false, 16.0)]).verdict(Platform::Ps5), Some(Verdict::No("no GPU".into())));
        assert_eq!(pc(8, 16, true, 16.0, vec![]).verdict(Platform::Ps4), Some(Verdict::No("no GPU driver".into())));
        // Too old for the emulators.
        let kepler = pc(8, 16, true, 16.0, vec![gpu("GeForce GTX 680", (1, 2), true, 2.0)]);
        assert_eq!(kepler.verdict(Platform::Ps5).unwrap().short(), "Won't start · no Vulkan 1.3");
        let ivy = pc(4, 8, false, 16.0, vec![rtx.clone()]);
        assert_eq!(ivy.verdict(Platform::Ps4).unwrap().short(), "Won't start · no AVX2");
        assert_eq!(ivy.verdict(Platform::Ps5).unwrap().short(), "May run slowly · 4-core CPU", "KytyPS5 runs without AVX2");
        // Runs, slowly.
        let deck = pc(4, 8, true, 15.0, vec![gpu("AMD Custom GPU 0405", (1, 3), false, 4.0)]);
        assert_eq!(deck.verdict(Platform::Ps4), Some(Verdict::Slow("integrated GPU".into())));
        let mid = pc(6, 12, true, 15.6, vec![gpu("AMD Radeon RX 6600", (1, 3), true, 8.0)]);
        assert_eq!(mid.verdict(Platform::Ps4), Some(Verdict::Good));
        assert_eq!(mid.verdict(Platform::Ps5), Some(Verdict::Good));
        let small = pc(6, 12, true, 7.7, vec![gpu("GeForce GTX 1650", (1, 3), true, 4.0)]);
        assert_eq!(small.verdict(Platform::Ps4), Some(Verdict::Good));
        assert_eq!(small.verdict(Platform::Ps5).unwrap().short(), "May run slowly · 4 GB GPU");
        // Every reason fits a Game Hub fact's label ("ON THIS PC · …").
        for v in [ivy.verdict(Platform::Ps4), deck.verdict(Platform::Ps4), small.verdict(Platform::Ps5), kepler.verdict(Platform::Ps4)] {
            assert!(v.unwrap().why().unwrap().chars().count() <= 16);
        }
        // A graphics check that crashed tells nothing.
        assert_eq!(Specs { gpus: None, ..big }.verdict(Platform::Ps5), None);
    }

    #[test]
    fn this_pc_answers() {
        let specs = detect();
        assert!(specs.threads > 0 && specs.ram_gb > 0.5, "{specs:?}");
    }
}
