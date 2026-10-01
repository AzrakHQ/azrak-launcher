//! Trailers, played inside the launcher window. libmpv (loaded at runtime when installed, like
//! libarchive) decodes the video and plays the sound; its software render API draws each frame
//! into memory on a render thread, and the UI shows the frame as an image (the GPU scales it).
//! Nothing shares OpenGL state with the UI renderer. Without libmpv there is no trailer button.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;

type Handle = *mut c_void;
type RenderCtx = *mut c_void;

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

// From mpv/client.h and mpv/render.h (stable ABI).
const PARAM_END: c_int = 0;
const PARAM_API_TYPE: c_int = 1;
const PARAM_SW_SIZE: c_int = 17;
const PARAM_SW_FORMAT: c_int = 18;
const PARAM_SW_STRIDE: c_int = 19;
const PARAM_SW_POINTER: c_int = 20;
const UPDATE_FRAME: u64 = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const END_FILE_ERROR: c_int = 4;
/// Frames are drawn at the video's own size, at most this big; the GPU scales them to the screen.
const MAX_SIZE: (i64, i64) = (1920, 1080);

struct Lib {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut Event,
    terminate_destroy: unsafe extern "C" fn(Handle),
    render_create: unsafe extern "C" fn(*mut RenderCtx, Handle, *mut RenderParam) -> c_int,
    render_set_update_callback: unsafe extern "C" fn(RenderCtx, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    render_update: unsafe extern "C" fn(RenderCtx) -> u64,
    render: unsafe extern "C" fn(RenderCtx, *mut RenderParam) -> c_int,
    render_free: unsafe extern "C" fn(RenderCtx),
}

fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| {
        let lib = crate::hostos::libmpv_candidates().iter().find_map(|n| unsafe { libloading::Library::new(n) }.ok())?;
        // Each signature follows libmpv's public C API; the library outlives every pointer.
        unsafe {
            macro_rules! sym { ($name:literal) => { *lib.get(concat!($name, "\0").as_bytes()).ok()? }; }
            Some(Lib {
                create: sym!("mpv_create"), initialize: sym!("mpv_initialize"),
                set_option_string: sym!("mpv_set_option_string"), command: sym!("mpv_command"),
                set_property_string: sym!("mpv_set_property_string"), get_property: sym!("mpv_get_property"),
                wait_event: sym!("mpv_wait_event"), terminate_destroy: sym!("mpv_terminate_destroy"),
                render_create: sym!("mpv_render_context_create"),
                render_set_update_callback: sym!("mpv_render_context_set_update_callback"),
                render_update: sym!("mpv_render_context_update"), render: sym!("mpv_render_context_render"),
                render_free: sym!("mpv_render_context_free"), _lib: lib,
            })
        }
    }).as_ref()
}

/// Whether trailers can play (libmpv is installed).
pub fn available() -> bool {
    lib().is_some()
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

/// Wakes the render thread when mpv has a new frame, or when the player closes.
#[derive(Default)]
struct Signal {
    pending: Mutex<bool>,
    cv: Condvar,
}

impl Signal {
    fn notify(&self) {
        *self.pending.lock().unwrap() = true;
        self.cv.notify_one();
    }
}

unsafe extern "C" fn on_update(ctx: *mut c_void) {
    (*(ctx as *const Signal)).notify();
}

/// Raw mpv pointers handed to the render thread. mpv's API is thread-safe; the render context
/// is only used by that thread, and the player waits for it before destroying anything.
struct Ptrs(Handle, RenderCtx);
unsafe impl Send for Ptrs {}

struct Player {
    mpv: Handle,
    signal: Arc<Signal>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    loaded: bool,
    ended: Option<Result<(), String>>,
}

thread_local! {
    static PLAYER: RefCell<Option<Player>> = const { RefCell::new(None) };
}

/// Start playing `url`. `frame` receives each new frame (from the render thread).
pub fn open(url: &str, frame: impl Fn(slint::SharedPixelBuffer<slint::Rgba8Pixel>) + Send + 'static) -> Result<(), String> {
    let lib = lib().ok_or("Trailers need libmpv (install mpv)")?;
    close();
    unsafe {
        let mpv = (lib.create)();
        if mpv.is_null() {
            return Err("Could not start the video player".into());
        }
        for (k, v) in [
            ("vo", "libmpv"), ("hwdec", "no"), ("terminal", "no"), ("osc", "no"), ("ytdl", "no"),
            ("input-default-bindings", "no"), ("input-vo-keyboard", "no"), ("keep-open", "no"),
            ("audio-client-name", "PS5 Launcher"), ("cache", "yes"), ("idle", "yes"),
        ] {
            (lib.set_option_string)(mpv, cstr(k).as_ptr(), cstr(v).as_ptr());
        }
        if (lib.initialize)(mpv) < 0 {
            (lib.terminate_destroy)(mpv);
            return Err("Could not start the video player".into());
        }
        let api = cstr("sw");
        let mut params = [
            RenderParam { kind: PARAM_API_TYPE, data: api.as_ptr() as *mut c_void },
            RenderParam { kind: PARAM_END, data: std::ptr::null_mut() },
        ];
        let mut ctx: RenderCtx = std::ptr::null_mut();
        if (lib.render_create)(&mut ctx, mpv, params.as_mut_ptr()) < 0 || ctx.is_null() {
            (lib.terminate_destroy)(mpv);
            return Err("This libmpv can't draw video for the launcher".into());
        }
        let signal = Arc::new(Signal::default());
        (lib.render_set_update_callback)(ctx, Some(on_update), Arc::as_ptr(&signal) as *mut c_void);
        // Only now load the file: mpv drops the video track if it starts before a renderer exists.
        let (load, target) = (cstr("loadfile"), cstr(url));
        let mut args = [load.as_ptr(), target.as_ptr(), std::ptr::null()];
        (lib.command)(mpv, args.as_mut_ptr());
        let stop = Arc::new(AtomicBool::new(false));
        let ptrs = Ptrs(mpv, ctx);
        let (thread_signal, thread_stop) = (signal.clone(), stop.clone());
        let thread = std::thread::Builder::new().name("trailer".into()).spawn(move || {
            let ptrs = ptrs;
            render_loop(lib, ptrs.0, ptrs.1, &thread_signal, &thread_stop, &frame);
            (lib.render_set_update_callback)(ptrs.1, None, std::ptr::null_mut());
            (lib.render_free)(ptrs.1);
        }).map_err(|e| e.to_string())?;
        PLAYER.with(|p| *p.borrow_mut() = Some(Player { mpv, signal, stop, thread: Some(thread), loaded: false, ended: None }));
    }
    Ok(())
}

/// Draw each new frame into a fresh buffer and hand it to the UI.
fn render_loop(lib: &Lib, mpv: Handle, ctx: RenderCtx, signal: &Signal, stop: &AtomicBool, frame: &dyn Fn(slint::SharedPixelBuffer<slint::Rgba8Pixel>)) {
    loop {
        {
            let mut pending = signal.pending.lock().unwrap();
            while !*pending && !stop.load(Ordering::Acquire) {
                pending = signal.cv.wait(pending).unwrap();
            }
            *pending = false;
        }
        if stop.load(Ordering::Acquire) {
            return;
        }
        unsafe {
            if (lib.render_update)(ctx) & UPDATE_FRAME == 0 {
                continue;
            }
            let (w, h) = (get_int(lib, mpv, "dwidth"), get_int(lib, mpv, "dheight"));
            if w <= 0 || h <= 0 {
                continue;
            }
            let scale = (MAX_SIZE.0 as f64 / w as f64).min(MAX_SIZE.1 as f64 / h as f64).min(1.0);
            let (w, h) = (((w as f64 * scale) as u32).max(2), ((h as f64 * scale) as u32).max(2));
            let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
            let mut size = [w as c_int, h as c_int];
            let format = cstr("rgb0");
            let mut stride = w as usize * 4;
            let mut params = [
                RenderParam { kind: PARAM_SW_SIZE, data: size.as_mut_ptr() as *mut c_void },
                RenderParam { kind: PARAM_SW_FORMAT, data: format.as_ptr() as *mut c_void },
                RenderParam { kind: PARAM_SW_STRIDE, data: &mut stride as *mut usize as *mut c_void },
                RenderParam { kind: PARAM_SW_POINTER, data: buffer.make_mut_bytes().as_mut_ptr() as *mut c_void },
                RenderParam { kind: PARAM_END, data: std::ptr::null_mut() },
            ];
            if (lib.render)(ctx, params.as_mut_ptr()) < 0 {
                continue;
            }
            // "rgb0" leaves the fourth byte undefined: make every pixel opaque.
            for px in buffer.make_mut_slice() {
                px.a = 255;
            }
            frame(buffer);
        }
    }
}

/// Stop playback and free the player.
pub fn close() {
    let Some(mut p) = PLAYER.with(|p| p.borrow_mut().take()) else { return };
    let Some(lib) = lib() else { return };
    p.stop.store(true, Ordering::Release);
    p.signal.notify();
    if let Some(thread) = p.thread.take() {
        let _ = thread.join();
    }
    unsafe { (lib.terminate_destroy)(p.mpv) };
}

fn with_player<T>(f: impl FnOnce(&Lib, &mut Player) -> T) -> Option<T> {
    let lib = lib()?;
    PLAYER.with(|p| p.borrow_mut().as_mut().map(|p| f(lib, p)))
}

pub fn toggle_pause() {
    with_player(|lib, p| unsafe {
        let paused = get_flag(lib, p.mpv, "pause");
        (lib.set_property_string)(p.mpv, cstr("pause").as_ptr(), cstr(if paused { "no" } else { "yes" }).as_ptr());
    });
}

pub fn seek(seconds: f64, absolute: bool) {
    with_player(|lib, p| unsafe {
        let (cmd, amount, how) = (cstr("seek"), cstr(&format!("{seconds:.2}")), cstr(if absolute { "absolute" } else { "relative" }));
        let mut args = [cmd.as_ptr(), amount.as_ptr(), how.as_ptr(), std::ptr::null()];
        (lib.command)(p.mpv, args.as_mut_ptr());
    });
}

unsafe fn get_flag(lib: &Lib, mpv: Handle, name: &str) -> bool {
    let mut v: c_int = 0;
    (lib.get_property)(mpv, cstr(name).as_ptr(), FORMAT_FLAG, &mut v as *mut c_int as *mut c_void);
    v != 0
}

unsafe fn get_int(lib: &Lib, mpv: Handle, name: &str) -> i64 {
    let mut v = 0i64;
    (lib.get_property)(mpv, cstr(name).as_ptr(), FORMAT_INT64, &mut v as *mut i64 as *mut c_void);
    v
}

unsafe fn get_double(lib: &Lib, mpv: Handle, name: &str) -> f64 {
    let mut v = 0f64;
    (lib.get_property)(mpv, cstr(name).as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void);
    v
}

pub struct Status {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    /// Still opening or buffering.
    pub loading: bool,
    /// Playback finished (Ok) or failed (Err), and the player should close.
    pub ended: Option<Result<(), String>>,
}

/// Current playback state; also handles mpv's events. Call a few times a second.
pub fn poll() -> Option<Status> {
    with_player(|lib, p| unsafe {
        loop {
            let e = &*(lib.wait_event)(p.mpv, 0.0);
            match e.id {
                EVENT_NONE => break,
                EVENT_FILE_LOADED => p.loaded = true,
                EVENT_END_FILE => {
                    let reason = if e.data.is_null() { 0 } else { *(e.data as *const c_int) };
                    p.ended = Some(if reason == END_FILE_ERROR { Err("The trailer couldn't be played".into()) } else { Ok(()) });
                }
                _ => {}
            }
        }
        Status {
            position: get_double(lib, p.mpv, "time-pos"),
            duration: get_double(lib, p.mpv, "duration"),
            paused: get_flag(lib, p.mpv, "pause"),
            loading: !p.loaded || get_flag(lib, p.mpv, "paused-for-cache"),
            ended: p.ended.clone(),
        }
    })
}
