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
    /// Has the Vulkan extensions and features KytyPS5 (PS5) and shadPS4 (PS4) refuse to start
    /// without; `None` when the check couldn't run.
    pub ps5_ok: Option<bool>,
    pub ps4_ok: Option<bool>,
}

impl Gpu {
    /// An NVIDIA card from before Turing (GTX 600–10 series: Kepler GK, Maxwell GM, Pascal GP) on
    /// the open-source NVK driver, which can't raise its clock above the start-up speed, so games
    /// run very slowly. Mesa names the chip: "NVIDIA GeForce GTX 1050 Ti (NVK GP107)".
    pub fn nvk_low_clock(&self) -> bool {
        self.name.split("(NVK ").nth(1).is_some_and(|chip| ["GK", "GM", "GP"].iter().any(|g| chip.starts_with(g)))
    }

    fn meets(&self, platform: Platform) -> Option<bool> {
        if platform == Platform::Ps4 { self.ps4_ok } else { self.ps5_ok }
    }
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

/// This PC as read, with its verdicts for PS5 and PS4 games worked out once (the Library asks
/// for every release on every keystroke). Read-only, so the verdicts always match the specs: a
/// new reading is a new `Reading`.
#[derive(Clone, Debug)]
pub struct Reading {
    specs: Specs,
    verdicts: [Option<Verdict>; 2],
}

impl Reading {
    pub fn new(specs: Specs) -> Reading {
        let verdicts = [specs.verdict(Platform::Ps5), specs.verdict(Platform::Ps4)];
        Reading { specs, verdicts }
    }

    pub fn specs(&self) -> &Specs {
        &self.specs
    }

    /// `Specs::verdict`, worked out when read; none for PC games.
    pub fn verdict(&self, platform: Platform) -> Option<&Verdict> {
        match platform {
            Platform::Ps5 => self.verdicts[0].as_ref(),
            Platform::Ps4 => self.verdicts[1].as_ref(),
            Platform::Pc => None,
        }
    }
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

    /// The reason in a few words; the launcher explains it in sentences (pc_ui::explain).
    #[cfg(test)]
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
}

impl Specs {

    /// The graphics card games will use: the strongest one that isn't software.
    pub fn gpu(&self) -> Option<&Gpu> {
        self.best_gpu(|_| true)
    }

    /// The graphics card this console's emulator would use, picked the way it picks:
    /// - KytyPS5 only considers devices that pass its checks, preferring a discrete one; if none
    ///   does, the strongest card says why;
    /// - shadPS4 sorts every device (Vulkan 1.3, then discrete, then not software, then the most
    ///   graphics memory), takes the first and only then checks it, so a discrete card it can't
    ///   use wins over an integrated one it could.
    fn gpu_for(&self, platform: Platform) -> Option<&Gpu> {
        if platform == Platform::Ps4 {
            let key = |g: &Gpu| (g.vulkan >= (1, 3), g.discrete, !g.software, g.vram_gb);
            return self.gpus.as_ref()?.iter()
                .max_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
        }
        self.best_gpu(|g| g.meets(platform) == Some(true))
    }

    fn best_gpu(&self, preferred: impl Fn(&Gpu) -> bool) -> Option<&Gpu> {
        let key = |g: &Gpu| (preferred(g), g.vulkan >= (1, 3), g.discrete, g.vram_gb);
        self.gpus.as_ref()?.iter().filter(|g| !g.software)
            .max_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Whether games of this console run here. `None`: unknown (the graphics check failed).
    pub fn verdict(&self, platform: Platform) -> Option<Verdict> {
        // PC games have their own requirements, which the catalog doesn't carry.
        if platform == Platform::Pc {
            return None;
        }
        let gpus = self.gpus.as_ref()?;
        let Some(gpu) = self.gpu_for(platform).filter(|g| !g.software) else {
            return Some(Verdict::No(if gpus.is_empty() { "no GPU driver" } else { "no GPU" }.into()));
        };
        if gpu.vulkan < (1, 3) {
            return Some(Verdict::No("no Vulkan 1.3".into()));
        }
        let ps4 = platform == Platform::Ps4;
        if ps4 && !self.x86_64_v3 {
            return Some(Verdict::No("no AVX2".into()));
        }
        // Vulkan 1.3 isn't all: each emulator checks specific extensions and features. Without
        // that answer there's no honest "Should run well".
        match gpu.meets(platform) {
            Some(false) => return Some(Verdict::No("missing features".into())),
            None => return None,
            Some(true) => {}
        }
        // What the README asks for, a step up for PS5 games: (VRAM, cores, threads, RAM).
        let (vram, cores, threads, ram) = if ps4 { (3.5, 4, 8, 7.0) } else { (7.5, 6, 12, 14.5) };
        let slow = if !gpu.discrete {
            Some("integrated GPU".to_string())
        } else if gpu.nvk_low_clock() {
            Some("GPU at low clock".to_string())
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
    if let Some(specs) = std::env::var("AZRAK_PRETEND_PC").ok().and_then(|s| pretend(&s)) {
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
/// "name,1.3,discrete,8,1,1" (the last two: meets KytyPS5's and shadPS4's requirements, 1 or 0)
/// separated by "|".
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

/// Runs `azrak --probe-gpu` and reads its lines. `None` when it crashed or hung.
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

/// Lines of `name<TAB>major.minor<TAB>type<TAB>vram GiB<TAB>PS5 ok<TAB>PS4 ok` (1, 0 or ? for
/// unknown), as [`print_gpus`] writes them.
fn parse_probe(text: &str) -> Vec<Gpu> {
    text.lines().filter_map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        let (name, version, kind, vram) = (*f.first()?, *f.get(1)?, *f.get(2)?, *f.get(3)?);
        let flag = |i: usize| match f.get(i).copied() { Some("1") => Some(true), Some("0") => Some(false), _ => None };
        let (major, minor) = version.split_once('.')?;
        Some(Gpu {
            name: name.to_string(),
            vulkan: (major.parse().ok()?, minor.parse().ok()?),
            discrete: kind == "discrete",
            software: kind == "cpu",
            vram_gb: vram.parse().ok()?,
            ps5_ok: flag(4),
            ps4_ok: flag(5),
        })
    }).collect()
}

/// `--probe-gpu`: list the Vulkan devices on stdout. Runs in its own process (see [`probe_gpus`]).
/// No Vulkan loader or devices prints nothing; that's a real answer, not a failure.
pub fn print_gpus() {
    let flag = |ok: Option<bool>| match ok { Some(true) => "1", Some(false) => "0", None => "?" };
    for d in unsafe { vulkan_devices() } {
        let (major, minor) = d.vulkan;
        println!("{}\t{major}.{minor}\t{}\t{:.1}\t{}\t{}", d.name, d.kind, d.vram_gb, flag(d.ps5_ok), flag(d.ps4_ok));
    }
}

struct Probed {
    name: String,
    vulkan: (u32, u32),
    kind: &'static str,
    vram_gb: f32,
    ps5_ok: Option<bool>,
    ps4_ok: Option<bool>,
}

/// Device extensions each emulator refuses to start without (KytyPS5's vulkanWindow.cpp,
/// shadPS4's vk_instance.cpp). Alternatives are separated by '|' (an extension promoted to KHR).
const PS5_EXTENSIONS: [&str; 8] = ["VK_KHR_swapchain", "VK_EXT_depth_clip_control", "VK_EXT_image_view_min_lod",
    "VK_KHR_push_descriptor", "VK_KHR_maintenance1", "VK_EXT_depth_clip_enable", "VK_EXT_color_write_enable",
    "VK_KHR_fragment_shader_barycentric"];
const PS4_EXTENSIONS: [&str; 4] = ["VK_KHR_swapchain", "VK_KHR_push_descriptor",
    "VK_EXT_vertex_attribute_divisor|VK_KHR_vertex_attribute_divisor", "VK_EXT_robustness2|VK_KHR_robustness2"];

/// A Vulkan feature struct: sType, pNext, then its VkBool32 fields (at most 60 read here).
#[repr(C)]
struct Features {
    s_type: u32,
    next: *mut std::ffi::c_void,
    values: [u32; 60],
}

impl Features {
    fn new(s_type: u32) -> Box<Self> {
        Box::new(Features { s_type, next: std::ptr::null_mut(), values: [0; 60] })
    }
}

/// The few Vulkan calls needed, through the system's loader (libvulkan.so.1).
unsafe fn vulkan_devices() -> Vec<Probed> {
    use std::ffi::{c_char, c_void, CStr};
    type Handle = *mut c_void;
    type GetProc = unsafe extern "C" fn(Handle, *const c_char) -> *const c_void;
    type CreateInstance = unsafe extern "C" fn(*const InstanceInfo, *const c_void, *mut Handle) -> i32;
    type Enumerate = unsafe extern "C" fn(Handle, *mut u32, *mut Handle) -> i32;
    type Props = unsafe extern "C" fn(Handle, *mut u8);
    type Destroy = unsafe extern "C" fn(Handle, *const c_void);
    type EnumerateExtensions = unsafe extern "C" fn(Handle, *const c_char, *mut u32, *mut u8) -> i32;
    type GetFeatures2 = unsafe extern "C" fn(Handle, *mut Features);
    type GetFormat = unsafe extern "C" fn(Handle, i32, *mut [u32; 3]);
    type GetQueues = unsafe extern "C" fn(Handle, *mut u32, *mut [u32; 6]);
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
    // Vulkan 1.1 for vkGetPhysicalDeviceFeatures2; a 1.0-only loader gets an instance without it,
    // and then the emulators' requirements are reported as unknown.
    let mut instance: Handle = std::ptr::null_mut();
    for api in [(1 << 22) | (1 << 12), 1 << 22] {
        let app = AppInfo { s_type: 0, next: std::ptr::null(), app: c"azrak".as_ptr(), app_version: 1, engine: std::ptr::null(), engine_version: 0, api };
        let info = InstanceInfo { s_type: 1, next: std::ptr::null(), flags: 0, app: &app, layers: 0, layer_names: std::ptr::null(), exts: 0, ext_names: std::ptr::null() };
        if create(&info, std::ptr::null(), &mut instance) == 0 && !instance.is_null() {
            break;
        }
        instance = std::ptr::null_mut();
    }
    if instance.is_null() {
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
    let extensions_of = get(c"vkEnumerateDeviceExtensionProperties");
    let extensions_of: Option<EnumerateExtensions> = (!extensions_of.is_null()).then(|| std::mem::transmute(extensions_of));
    let format_of = get(c"vkGetPhysicalDeviceFormatProperties");
    let format_of: Option<GetFormat> = (!format_of.is_null()).then(|| std::mem::transmute(format_of));
    let queues_of = get(c"vkGetPhysicalDeviceQueueFamilyProperties");
    let queues_of: Option<GetQueues> = (!queues_of.is_null()).then(|| std::mem::transmute(queues_of));
    let features2 = get(c"vkGetPhysicalDeviceFeatures2");
    let features2: Option<GetFeatures2> = (!features2.is_null()).then(|| std::mem::transmute(features2));
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

        // Extensions (VkExtensionProperties: name[256], specVersion: 260 bytes each), then the
        // features the emulators check, chained only for extensions the device has.
        let (mut ps5_ok, mut ps4_ok) = (None, None);
        let vulkan = (api >> 22, (api >> 12) & 0x3ff);
        if let (Some(extensions_of), Some(features2), Some(format_of), Some(queues_of)) = (extensions_of, features2, format_of, queues_of) {
            let mut n = 0u32;
            extensions_of(device, std::ptr::null(), &mut n, std::ptr::null_mut());
            let mut raw = vec![0u8; n as usize * 260];
            extensions_of(device, std::ptr::null(), &mut n, raw.as_mut_ptr());
            let names: std::collections::HashSet<String> = raw.chunks_exact(260).take(n as usize)
                .map(|e| CStr::from_bytes_until_nul(e).map(|c| c.to_string_lossy().into_owned()).unwrap_or_default()).collect();
            let has = |want: &str| want.split('|').any(|alt| names.contains(alt));
            // Feature structs, chained after VkPhysicalDeviceFeatures2. sType values and field
            // positions from vulkan_core.h; the Vulkan 1.2/1.3 structs only on a 1.3 device.
            let mut base = Features::new(1000059000); // PHYSICAL_DEVICE_FEATURES_2
            let mut chain: Vec<(&str, Box<Features>)> = vec![
                ("core 1.1", Features::new(49)), // PHYSICAL_DEVICE_VULKAN_1_1_FEATURES
                ("core 1.2", Features::new(51)),
                ("core 1.3", Features::new(53)),
                ("VK_KHR_fragment_shader_barycentric", Features::new(1000203000)),
                ("VK_EXT_image_view_min_lod", Features::new(1000391000)),
                ("VK_EXT_depth_clip_control", Features::new(1000355000)),
                ("VK_EXT_depth_clip_enable", Features::new(1000102000)),
                ("VK_EXT_color_write_enable", Features::new(1000381000)),
                ("VK_EXT_robustness2|VK_KHR_robustness2", Features::new(1000286000)),
            ];
            chain.retain(|(ext, _)| if ext.starts_with("core") { vulkan >= (1, 3) } else { has(ext) });
            for i in (0..chain.len()).rev() {
                let next: *mut c_void = if i + 1 < chain.len() { &mut *chain[i + 1].1 as *mut Features as *mut c_void } else { std::ptr::null_mut() };
                chain[i].1.next = next;
            }
            base.next = chain.first_mut().map_or(std::ptr::null_mut(), |(_, f)| &mut **f as *mut Features as *mut c_void);
            features2(device, &mut *base);
            let feature = |ext: &str, index: usize| chain.iter().find(|(e, _)| *e == ext).is_some_and(|(_, f)| f.values[index] == 1);
            let core = |index: usize| base.values[index] == 1;

            // KytyPS5 (vulkanWindow.cpp, VulkanFindPhysicalDevice): every check it rejects a
            // device for, except the window surface's, which needs a window.
            // VkPhysicalDeviceFeatures: robustBufferAccess, independentBlend, tessellationShader,
            // sampleRateShading, depthClamp, depthBiasClamp, fillModeNonSolid, depthBounds,
            // largePoints, multiViewport, samplerAnisotropy, fragmentStoresAndAtomics,
            // shaderImageGatherExtended, shaderStorageImageWriteWithoutFormat, shaderClipDistance,
            // shaderCullDistance.
            let core_ok = [0, 3, 5, 6, 11, 12, 13, 14, 16, 18, 19, 26, 28, 32, 37, 38].iter().all(|i| core(*i));
            // 1.1 storageBuffer16BitAccess; 1.2 samplerMirrorClampToEdge, storageBuffer8BitAccess,
            // shaderBufferInt64Atomics, shaderSampledImageArrayNonUniformIndexing,
            // timelineSemaphore, bufferDeviceAddress, shaderOutputViewportIndex, shaderOutputLayer;
            // 1.3 robustImageAccess, synchronization2, dynamicRendering.
            let versions_ok = feature("core 1.1", 0)
                && [0, 2, 5, 14, 37, 38, 44, 45].iter().all(|i| feature("core 1.2", *i))
                && [0, 9, 12].iter().all(|i| feature("core 1.3", *i));
            let extensions_ok = PS5_EXTENSIONS.iter().all(|e| has(e))
                && ["VK_KHR_fragment_shader_barycentric", "VK_EXT_image_view_min_lod", "VK_EXT_depth_clip_control",
                    "VK_EXT_depth_clip_enable", "VK_EXT_color_write_enable"].iter().all(|e| feature(e, 0));
            // A queue family that does graphics and compute.
            let mut families = 0u32;
            queues_of(device, &mut families, std::ptr::null_mut());
            let mut queues = vec![[0u32; 6]; families as usize];
            queues_of(device, &mut families, queues.as_mut_ptr());
            let queue_ok = queues.iter().take(families as usize).any(|q| q[0] & 0x3 == 0x3);
            // Formats with optimal tiling (VkFormatProperties.optimalTilingFeatures).
            let format = |f: i32, need: u32| { let mut p = [0u32; 3]; format_of(device, f, &mut p); p[1] & need == need };
            let (depth, texture, storage) = (0x200, 0x1 | 0x8000, 0x2 | 0x8000);
            let formats_ok = [126, 130, 124].iter().all(|f| format(*f, depth)) // D32, D32S8, D16
                && [138, 43, 9, 16].iter().all(|f| format(*f, texture)) // BC3 sRGB, RGBA8 sRGB, R8, RG8
                && (format(43, storage) || format(37, storage)) // RGBA8 sRGB or UNORM
                && (format(50, storage) || format(44, storage)); // BGRA8 sRGB or UNORM
            // VkPhysicalDeviceLimits.maxSamplerAnisotropy at 568 in VkPhysicalDeviceProperties.
            let anisotropy_ok = *(p.add(568) as *const f32) >= 16.0;
            ps5_ok = Some(vulkan >= (1, 3) && core_ok && versions_ok && extensions_ok && queue_ok && formats_ok && anisotropy_ok);

            // shadPS4 (vk_instance.cpp): the extensions it asserts, and robustness2's features.
            let robust = |i| feature("VK_EXT_robustness2|VK_KHR_robustness2", i);
            ps4_ok = Some(PS4_EXTENSIONS.iter().all(|e| has(e)) && robust(0) && robust(1) && robust(2));
        }
        out.push(Probed { name, vulkan, kind, vram_gb: local as f32 / (1u64 << 30) as f32, ps5_ok, ps4_ok });
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
        Gpu { name: name.into(), vulkan, discrete, software: name.starts_with("llvmpipe"), vram_gb: vram, ps5_ok: Some(true), ps4_ok: Some(true) }
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
        let gpus = parse_probe("NVIDIA GeForce RTX 3090\t1.4\tdiscrete\t24.0\t1\t1\nllvmpipe (LLVM 20.1.2, 256 bits)\t1.4\tcpu\t31.0\t0\t1\nOld\t1.3\tdiscrete\t2.0\nbroken line\n");
        assert_eq!(gpus.len(), 3);
        assert_eq!((gpus[0].vulkan, gpus[0].discrete, gpus[0].vram_gb, gpus[0].ps5_ok), ((1, 4), true, 24.0, Some(true)));
        assert!(gpus[1].software && gpus[1].ps5_ok == Some(false) && gpus[1].ps4_ok == Some(true));
        assert_eq!((gpus[2].ps5_ok, gpus[2].ps4_ok), (None, None), "older probe output: unknown");
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
        // Older NVIDIA cards on NVK run at their start-up clock (the test laptop's GTX 1050 Ti).
        let pascal = pc(4, 8, true, 16.0, vec![gpu("NVIDIA GeForce GTX 1050 Ti (NVK GP107)", (1, 3), true, 4.0)]);
        assert_eq!(pascal.verdict(Platform::Ps4), Some(Verdict::Slow("GPU at low clock".into())));
        let turing = pc(6, 12, true, 16.0, vec![gpu("NVIDIA GeForce RTX 2060 (NVK TU106)", (1, 3), true, 6.0)]);
        assert_eq!(turing.verdict(Platform::Ps4), Some(Verdict::Good));
        // Every reason fits a Game Hub fact's label ("ON THIS PC · …").
        for v in [ivy.verdict(Platform::Ps4), deck.verdict(Platform::Ps4), small.verdict(Platform::Ps5), kepler.verdict(Platform::Ps4)] {
            assert!(v.unwrap().why().unwrap().chars().count() <= 16);
        }
        // Vulkan 1.3, but without what KytyPS5 checks for (e.g. fragment-shader barycentrics).
        let partial = pc(8, 16, true, 16.0, vec![Gpu { ps5_ok: Some(false), ..rtx.clone() }]);
        assert_eq!(partial.verdict(Platform::Ps5).unwrap().short(), "Won't start · missing features");
        assert_eq!(partial.verdict(Platform::Ps4), Some(Verdict::Good));
        // The emulator picks the card that has what it needs.
        let two = pc(8, 16, true, 16.0, vec![Gpu { ps5_ok: Some(false), vram_gb: 24.0, ..rtx.clone() }, gpu("AMD Radeon RX 6600", (1, 3), true, 8.0)]);
        assert_eq!(two.verdict(Platform::Ps5), Some(Verdict::Good));
        // shadPS4 takes the discrete card even when only the integrated one would work.
        let mixed = pc(8, 16, true, 16.0, vec![Gpu { ps4_ok: Some(false), ..rtx.clone() }, gpu("AMD Radeon 780M", (1, 3), false, 2.0)]);
        assert_eq!(mixed.verdict(Platform::Ps4).unwrap().short(), "Won't start · missing features");
        // …and an integrated card when there's no discrete one; software only as a last resort.
        let laptop = pc(8, 16, true, 16.0, vec![gpu("llvmpipe", (1, 4), false, 30.0), gpu("AMD Radeon 780M", (1, 3), false, 2.0)]);
        assert_eq!(laptop.verdict(Platform::Ps4), Some(Verdict::Slow("integrated GPU".into())));
        // Requirements unknown (the feature check couldn't run): no verdict rather than a guess.
        let unknown = pc(8, 16, true, 16.0, vec![Gpu { ps5_ok: None, ps4_ok: None, ..rtx.clone() }]);
        assert_eq!(unknown.verdict(Platform::Ps5), None);
        // A graphics check that crashed tells nothing.
        assert_eq!(Specs { gpus: None, ..big.clone() }.verdict(Platform::Ps5), None);
        // A reading's verdicts are the specs' own.
        let reading = Reading::new(big.clone());
        assert_eq!((reading.verdict(Platform::Ps5), reading.verdict(Platform::Pc)), (big.verdict(Platform::Ps5).as_ref(), None));
    }

    #[test]
    fn this_pc_answers() {
        let specs = detect();
        assert!(specs.threads > 0 && specs.ram_gb > 0.5, "{specs:?}");
    }
}
