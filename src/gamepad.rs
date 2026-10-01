//! Controller input straight from evdev (/dev/input/event*). No udev/SDL dependency.
//! Works for DualSense, DualShock 4, Xbox and most pads with the standard Linux mapping,
//! and keeps working while a game has focus (needed for the PS button toggle).
//!
//! On macOS, controllers come from the system through gilrs instead (see the macOS items below).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))] // the evdev code is Linux-only

use std::collections::HashMap;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pad {
    Up,
    Down,
    Left,
    Right,
    Confirm,  // ✕ / A
    Back,     // ○ / B
    Square,   // □ / X
    Triangle, // △ / Y
    L1,
    R1,
    L2,
    R2,
    Options,
    Ps,
}

const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const BTN_SOUTH: u16 = 0x130;
const BTN_MODE: u16 = 0x13c;

fn map_button(code: u16) -> Option<Pad> {
    Some(match code {
        0x130 => Pad::Confirm,
        0x131 => Pad::Back,
        0x133 => Pad::Triangle,
        0x134 => Pad::Square,
        0x136 => Pad::L1,
        0x137 => Pad::R1,
        0x138 => Pad::L2,
        0x139 => Pad::R2,
        0x13b => Pad::Options,
        0x13c => Pad::Ps,
        0x220 => Pad::Up,
        0x221 => Pad::Down,
        0x222 => Pad::Left,
        0x223 => Pad::Right,
        _ => return None,
    })
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

fn abs_info(fd: RawFd, axis: u16) -> Option<AbsInfo> {
    // EVIOCGABS(axis) = _IOR('E', 0x40 + axis, struct input_absinfo)
    let req = (2u64 << 30) | ((std::mem::size_of::<AbsInfo>() as u64) << 16) | ((b'E' as u64) << 8) | (0x40 + axis as u64);
    let mut info = AbsInfo::default();
    let r = unsafe { libc::ioctl(fd, req as _, &mut info as *mut AbsInfo) };
    (r >= 0 && info.maximum > info.minimum).then_some(info)
}

struct Device {
    fd: RawFd,
    path: String,
    ranges: HashMap<u16, (i32, i32)>,
    axes: HashMap<u16, f32>,
    buttons: [bool; 4], // dpad buttons up/down/left/right
}

/// evdev nodes of gamepads (devices with a joystick handler and a south face button).
fn gamepad_paths() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else { return Vec::new() };
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let (mut handlers, mut keys) = ("", "");
        for line in block.lines() {
            if let Some(h) = line.strip_prefix("H: Handlers=") {
                handlers = h;
            } else if let Some(k) = line.strip_prefix("B: KEY=") {
                keys = k;
            }
        }
        if keys.is_empty() || !handlers.split_whitespace().any(|h| h.starts_with("js")) {
            continue;
        }
        // KEY bitmap: space-separated 64-bit words, most significant first.
        let words: Vec<u64> = keys.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
        let bit = |n: u16| {
            let (w, b) = ((n / 64) as usize, n % 64);
            words.len() > w && words[words.len() - 1 - w] >> b & 1 == 1
        };
        if bit(BTN_SOUTH) || bit(BTN_MODE) {
            if let Some(ev) = handlers.split_whitespace().find(|h| h.starts_with("event")) {
                out.push(format!("/dev/input/{ev}"));
            }
        }
    }
    out
}

/// Names of the connected controllers, for showing whether games will get one. Reads the same
/// device list as `gamepad_paths`, so it works even without permission to open the devices.
#[cfg(target_os = "linux")]
pub fn connected() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    for block in text.split("\n\n") {
        let (mut name, mut handlers, mut keys) = ("", "", "");
        for line in block.lines() {
            if let Some(n) = line.strip_prefix("N: Name=") {
                name = n.trim_matches('"');
            } else if let Some(h) = line.strip_prefix("H: Handlers=") {
                handlers = h;
            } else if let Some(k) = line.strip_prefix("B: KEY=") {
                keys = k;
            }
        }
        if keys.is_empty() || !handlers.split_whitespace().any(|h| h.starts_with("js")) {
            continue;
        }
        let words: Vec<u64> = keys.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
        let bit = |n: u16| {
            let (w, b) = ((n / 64) as usize, n % 64);
            words.len() > w && words[words.len() - 1 - w] >> b & 1 == 1
        };
        if (bit(BTN_SOUTH) || bit(BTN_MODE)) && !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Names of the controllers macOS reports, kept current by the `run` loop below.
#[cfg(target_os = "macos")]
static MAC_PADS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Names of the connected controllers, for showing whether games will get one.
#[cfg(target_os = "macos")]
pub fn connected() -> Vec<String> {
    MAC_PADS.lock().map(|n| n.clone()).unwrap_or_default()
}

/// "DualSense Wireless Controller connected" or "No controller connected".
pub fn status() -> String {
    match connected().first() {
        Some(name) => format!("{name} connected"),
        None => "No controller connected".into(),
    }
}

fn open_device(path: &str) -> Option<Device> {
    let c = std::ffi::CString::new(path).ok()?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let mut ranges = HashMap::new();
    for axis in [0u16, 1, 16, 17] {
        if let Some(i) = abs_info(fd, axis) {
            ranges.insert(axis, (i.minimum, i.maximum));
        }
    }
    crate::log!("controller connected: {path}");
    Some(Device { fd, path: path.to_string(), ranges, axes: HashMap::new(), buttons: [false; 4] })
}

/// Spawns the input thread. `emit` receives presses (directions auto-repeat while held).
pub fn spawn(emit: impl Fn(Pad) + Send + 'static) {
    std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(move || run(emit))
        .ok();
}

/// Turns "which directions are held" into presses with auto-repeat, like a keyboard: one press
/// at once, a pause, then a steady repeat. Shared by the Linux and macOS input backends.
#[derive(Default)]
struct Repeater {
    /// Direction -> when its next repeat is due.
    next: HashMap<Pad, Instant>,
}

const REPEAT_AFTER: Duration = Duration::from_millis(380);
const REPEAT_EVERY: Duration = Duration::from_millis(85);

impl Repeater {
    /// `held` is [up, down, left, right]. Calls `emit` for every press that is due at `now`.
    fn update(&mut self, held: [bool; 4], now: Instant, mut emit: impl FnMut(Pad)) {
        for (i, p) in [Pad::Up, Pad::Down, Pad::Left, Pad::Right].into_iter().enumerate() {
            if !held[i] {
                self.next.remove(&p);
            } else if let Some(next) = self.next.get_mut(&p) {
                if now >= *next {
                    *next = now + REPEAT_EVERY;
                    emit(p);
                }
            } else {
                self.next.insert(p, now + REPEAT_AFTER);
                emit(p);
            }
        }
    }

    /// How long until the next repeat is due (zero if overdue), or None if nothing is held.
    fn next_due(&self, now: Instant) -> Option<Duration> {
        self.next.values().map(|t| t.saturating_duration_since(now)).min()
    }
}

#[cfg(target_os = "linux")]
fn run(emit: impl Fn(Pad)) {
    let mut devices: Vec<Device> = Vec::new();
    let mut last_scan = Instant::now() - Duration::from_secs(60);
    let mut repeater = Repeater::default();
    let mut buf = [0u8; 24 * 64];
    loop {
        if last_scan.elapsed() > Duration::from_secs(3) {
            last_scan = Instant::now();
            for p in gamepad_paths() {
                if !devices.iter().any(|d| d.path == p) {
                    if let Some(d) = open_device(&p) {
                        devices.push(d);
                    }
                }
            }
        }
        if devices.is_empty() {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        // Wait for input, or until the next key-repeat is due.
        let now = Instant::now();
        let timeout = repeater.next_due(now).unwrap_or(Duration::from_millis(3000));
        let mut fds: Vec<libc::pollfd> = devices.iter().map(|d| libc::pollfd { fd: d.fd, events: libc::POLLIN, revents: 0 }).collect();
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout.as_millis().min(3000) as i32) };

        let mut gone = Vec::new();
        for (i, pfd) in fds.iter().enumerate() {
            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                gone.push(i);
                continue;
            }
            if pfd.revents & libc::POLLIN == 0 {
                continue;
            }
            let dev = &mut devices[i];
            let n = unsafe { libc::read(dev.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n <= 0 {
                if n < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EAGAIN) {
                    gone.push(i);
                }
                continue;
            }
            for ev in buf[..n as usize].chunks_exact(24) {
                let typ = u16::from_ne_bytes([ev[16], ev[17]]);
                let code = u16::from_ne_bytes([ev[18], ev[19]]);
                let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                match typ {
                    EV_KEY => match map_button(code) {
                        Some(p @ (Pad::Up | Pad::Down | Pad::Left | Pad::Right)) => {
                            dev.buttons[p as usize] = value != 0;
                        }
                        Some(p) if value == 1 => emit(p),
                        _ => {}
                    },
                    EV_ABS if [0, 1, 16, 17].contains(&code) => {
                        let v = match dev.ranges.get(&code) {
                            Some(&(lo, hi)) if code < 16 => (value - lo) as f32 / (hi - lo) as f32 * 2.0 - 1.0,
                            _ => value.signum() as f32,
                        };
                        dev.axes.insert(code, v);
                    }
                    _ => {}
                }
            }
        }
        for i in gone.into_iter().rev() {
            crate::log!("controller disconnected: {}", devices[i].path);
            unsafe { libc::close(devices[i].fd) };
            devices.remove(i);
        }

        // Combine d-pad (hat or buttons) and left stick into directions with auto-repeat.
        let mut dirs = [false; 4];
        for d in &devices {
            let ax = |c: u16| d.axes.get(&c).copied().unwrap_or(0.0);
            dirs[0] |= d.buttons[0] || ax(17) < -0.5 || ax(1) < -0.55;
            dirs[1] |= d.buttons[1] || ax(17) > 0.5 || ax(1) > 0.55;
            dirs[2] |= d.buttons[2] || ax(16) < -0.5 || ax(0) < -0.55;
            dirs[3] |= d.buttons[3] || ax(16) > 0.5 || ax(0) > 0.55;
        }
        repeater.update(dirs, Instant::now(), |p| emit(p));
    }
}

/// macOS: controllers through `gilrs` (IOKit underneath). It maps pads to one standard layout, so
/// buttons arrive by position (South = ✕ on a PlayStation pad, A on an Xbox pad).
#[cfg(target_os = "macos")]
fn map_gilrs_button(button: gilrs::Button) -> Option<Pad> {
    use gilrs::Button::*;
    Some(match button {
        South => Pad::Confirm,
        East => Pad::Back,
        North => Pad::Triangle,
        West => Pad::Square,
        LeftTrigger => Pad::L1,
        RightTrigger => Pad::R1,
        LeftTrigger2 => Pad::L2,
        RightTrigger2 => Pad::R2,
        Start => Pad::Options,
        Mode => Pad::Ps,
        DPadUp => Pad::Up,
        DPadDown => Pad::Down,
        DPadLeft => Pad::Left,
        DPadRight => Pad::Right,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
fn run(emit: impl Fn(Pad)) {
    use gilrs::{Axis, Event, EventType, GamepadId, Gilrs};

    /// What one controller is holding: d-pad buttons [up, down, left, right] and the left stick.
    #[derive(Default)]
    struct State {
        dpad: [bool; 4],
        stick: [f32; 2], // x, y (gilrs reports up as positive y)
    }

    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            crate::log!("controllers unavailable: {e}");
            return;
        }
    };
    let mut states: HashMap<GamepadId, State> = HashMap::new();
    let mut repeater = Repeater::default();
    loop {
        // Sleep until input arrives or a direction repeat is due (never long: pads can appear).
        let wait = repeater.next_due(Instant::now()).unwrap_or(Duration::from_millis(500)).min(Duration::from_millis(500));
        let mut first = gilrs.next_event_blocking(Some(wait));
        while let Some(Event { id, event, .. }) = first.take().or_else(|| gilrs.next_event()) {
            let st = states.entry(id).or_default();
            match event {
                // Directions are tracked as held state; the Repeater turns them into presses.
                EventType::ButtonPressed(b, _) => match map_gilrs_button(b) {
                    Some(p) if (p as usize) < 4 => st.dpad[p as usize] = true,
                    Some(p) => emit(p),
                    None => {}
                },
                EventType::ButtonReleased(b, _) => {
                    if let Some(p) = map_gilrs_button(b) {
                        if (p as usize) < 4 {
                            st.dpad[p as usize] = false;
                        }
                    }
                }
                EventType::AxisChanged(Axis::LeftStickX, v, _) => st.stick[0] = v,
                EventType::AxisChanged(Axis::LeftStickY, v, _) => st.stick[1] = v,
                EventType::Disconnected => {
                    crate::log!("controller disconnected");
                    states.remove(&id);
                }
                _ => {}
            }
        }
        // Keep the list of connected pads current for the Settings and launch screens.
        let mut names: Vec<String> = Vec::new();
        for (_, pad) in gilrs.gamepads() {
            if pad.is_connected() && !names.iter().any(|n| n == pad.name()) {
                names.push(pad.name().to_string());
            }
        }
        if let Ok(mut shared) = MAC_PADS.lock() {
            if *shared != names {
                *shared = names;
            }
        }
        let mut dirs = [false; 4];
        for st in states.values() {
            dirs[0] |= st.dpad[0] || st.stick[1] > 0.55;
            dirs[1] |= st.dpad[1] || st.stick[1] < -0.55;
            dirs[2] |= st.dpad[2] || st.stick[0] < -0.55;
            dirs[3] |= st.dpad[3] || st.stick[0] > 0.55;
        }
        repeater.update(dirs, Instant::now(), |p| emit(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the repeater at `at` ms after `t0` and collect what it presses.
    fn step(r: &mut Repeater, held: [bool; 4], t0: Instant, at: u64) -> Vec<Pad> {
        let mut out = Vec::new();
        r.update(held, t0 + Duration::from_millis(at), |p| out.push(p));
        out
    }
    const NONE: [bool; 4] = [false; 4];
    const DOWN: [bool; 4] = [false, true, false, false];

    #[test]
    fn a_held_direction_presses_at_once_then_pauses_then_repeats() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        assert_eq!(step(&mut r, DOWN, t0, 0), [Pad::Down], "immediate press");
        assert!(step(&mut r, DOWN, t0, 100).is_empty(), "still in the initial pause");
        assert!(step(&mut r, DOWN, t0, 379).is_empty());
        assert_eq!(step(&mut r, DOWN, t0, 380), [Pad::Down], "first repeat");
        assert!(step(&mut r, DOWN, t0, 400).is_empty());
        assert_eq!(step(&mut r, DOWN, t0, 465), [Pad::Down], "then every 85 ms");
        assert_eq!(step(&mut r, DOWN, t0, 550), [Pad::Down]);
    }

    #[test]
    fn releasing_stops_the_repeat_and_a_new_press_starts_over() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        step(&mut r, DOWN, t0, 0);
        assert!(step(&mut r, NONE, t0, 100).is_empty());
        assert_eq!(r.next_due(t0 + Duration::from_millis(100)), None);
        assert!(step(&mut r, NONE, t0, 500).is_empty(), "nothing repeats after release");
        assert_eq!(step(&mut r, DOWN, t0, 600), [Pad::Down], "pressed again: immediate");
        assert!(step(&mut r, DOWN, t0, 700).is_empty(), "with the initial pause again");
    }

    #[test]
    fn directions_repeat_independently() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        assert_eq!(step(&mut r, DOWN, t0, 0), [Pad::Down]);
        // Right joins while Down is mid-pause: Right presses at once, Down keeps its own clock.
        assert_eq!(step(&mut r, [false, true, false, true], t0, 200), [Pad::Right]);
        assert_eq!(step(&mut r, [false, true, false, true], t0, 380), [Pad::Down]);
        // Right's initial pause ends at 580, and Down (repeating every 85 ms since 380) is due too.
        assert_eq!(step(&mut r, [false, true, false, true], t0, 580), [Pad::Down, Pad::Right]);
    }

    #[test]
    fn next_due_says_when_to_wake_for_a_repeat() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        assert_eq!(r.next_due(t0), None);
        step(&mut r, DOWN, t0, 0);
        assert_eq!(r.next_due(t0 + Duration::from_millis(100)), Some(Duration::from_millis(280)));
        assert_eq!(r.next_due(t0 + Duration::from_millis(900)), Some(Duration::ZERO), "overdue means now");
    }
}
