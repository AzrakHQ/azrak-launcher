//! UI sounds, synthesized and played through ALSA on Linux (PipeWire/PulseAudio via their ALSA
//! plugin) and through the system `afplay` on macOS.
//! libasound is loaded at runtime, so there is no build dependency; without it the
//! launcher is simply silent. The device is opened only while sounds play.
#![cfg_attr(target_os = "macos", allow(dead_code, unused_imports))] // the ALSA code below is Linux-only

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub enum Sound {
    Move,
    Select,
    Back,
    Error,
    Start,
}

/// Every sound, in the order of `Sound as usize`.
const ALL: [Sound; 5] = [Sound::Move, Sound::Select, Sound::Back, Sound::Error, Sound::Start];

static TX: OnceLock<Sender<Sound>> = OnceLock::new();
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn play(s: Sound) {
    if ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(s);
        }
    }
}

const RATE: u32 = 48_000;

type Open = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type SetParams = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type WriteI = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type Recover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type Drain = unsafe extern "C" fn(*mut c_void) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void) -> c_int;

struct Alsa {
    _lib: libloading::Library,
    open: Open,
    set_params: SetParams,
    writei: WriteI,
    recover: Recover,
    drain: Drain,
    close: Close,
}

impl Alsa {
    fn load() -> Option<Alsa> {
        unsafe {
            let lib = libloading::Library::new("libasound.so.2").ok()?;
            Some(Alsa {
                open: *lib.get(b"snd_pcm_open\0").ok()?,
                set_params: *lib.get(b"snd_pcm_set_params\0").ok()?,
                writei: *lib.get(b"snd_pcm_writei\0").ok()?,
                recover: *lib.get(b"snd_pcm_recover\0").ok()?,
                drain: *lib.get(b"snd_pcm_drain\0").ok()?,
                close: *lib.get(b"snd_pcm_close\0").ok()?,
                _lib: lib,
            })
        }
    }

    fn open_pcm(&self) -> Option<*mut c_void> {
        let mut pcm: *mut c_void = std::ptr::null_mut();
        unsafe {
            if (self.open)(&mut pcm, c"default".as_ptr(), 0, 0) < 0 {
                return None;
            }
            // S16_LE = 2, RW_INTERLEAVED = 3, mono, 48 kHz, 25 ms latency.
            if (self.set_params)(pcm, 2, 3, 1, RATE, 1, 25_000) < 0 {
                (self.close)(pcm);
                return None;
            }
        }
        Some(pcm)
    }

    fn write(&self, pcm: *mut c_void, samples: &[i16]) {
        let mut off = 0usize;
        while off < samples.len() {
            let n = unsafe { (self.writei)(pcm, samples[off..].as_ptr() as *const c_void, (samples.len() - off) as c_ulong) };
            if n < 0 {
                if unsafe { (self.recover)(pcm, n as c_int, 1) } < 0 {
                    return;
                }
            } else {
                off += n as usize;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Triangle,
    Square,
}

/// Exponential pitch glide with a fast attack and exponential decay (like the v1 Web Audio tones).
fn tone(out: &mut Vec<f32>, f1: f32, f2: f32, dur: f32, vol: f32, wave: Wave, delay: f32) {
    let start = (delay * RATE as f32) as usize;
    let len = (dur * RATE as f32) as usize;
    if out.len() < start + len {
        out.resize(start + len, 0.0);
    }
    let mut phase = 0.0f32;
    for i in 0..len {
        let t = i as f32 / len as f32;
        let f = f1 * (f2 / f1).powf(t);
        phase = (phase + f / RATE as f32).fract();
        let s = match wave {
            Wave::Sine => (phase * std::f32::consts::TAU).sin(),
            Wave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            Wave::Square => if phase < 0.5 { 0.6 } else { -0.6 },
        };
        let attack = (i as f32 / (0.008 * RATE as f32)).min(1.0);
        let env = attack * (0.0001f32 / 1.0).powf(t); // exponential decay to -80 dB
        out[start + i] += s * env * vol;
    }
}

fn render(s: Sound) -> Vec<i16> {
    let mut buf = Vec::new();
    match s {
        Sound::Move => tone(&mut buf, 1500.0, 1320.0, 0.045, 0.05, Wave::Triangle, 0.0),
        Sound::Select => tone(&mut buf, 680.0, 1020.0, 0.12, 0.09, Wave::Sine, 0.0),
        Sound::Back => tone(&mut buf, 880.0, 560.0, 0.1, 0.07, Wave::Sine, 0.0),
        Sound::Error => tone(&mut buf, 230.0, 170.0, 0.22, 0.06, Wave::Square, 0.0),
        Sound::Start => {
            tone(&mut buf, 392.0, 784.0, 0.45, 0.09, Wave::Sine, 0.0);
            tone(&mut buf, 587.0, 1175.0, 0.6, 0.06, Wave::Sine, 0.12);
        }
    }
    buf.iter().map(|x| (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect()
}

/// Mono 16-bit PCM as a WAV file, for players that want a file.
#[cfg(any(target_os = "macos", test))]
fn wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes per second
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// macOS: write each sound to a WAV file once, then play it with the system's `afplay`.
/// Starting a process costs a few tens of milliseconds, which is fine for UI sounds.
#[cfg(target_os = "macos")]
fn play_with_afplay(rx: std::sync::mpsc::Receiver<Sound>) {
    use std::process::{Command, Stdio};
    let dir = crate::util::cache_dir().join("sounds");
    let mut files = Vec::new();
    for sound in ALL {
        let path = dir.join(format!("{}.wav", format!("{sound:?}").to_lowercase()));
        if let Err(e) = crate::util::atomic_write(&path, &wav_bytes(&render(sound), RATE)) {
            crate::log!("audio: cannot write {}: {e}; sounds disabled", path.display());
            return;
        }
        files.push(path);
    }
    let mut last_move = Instant::now() - Duration::from_secs(1);
    while let Ok(sound) = rx.recv() {
        // Moving through a grid fires fast; a process per step would pile up.
        if matches!(sound, Sound::Move) {
            if last_move.elapsed() < Duration::from_millis(120) {
                continue;
            }
            last_move = Instant::now();
        }
        match Command::new("afplay").arg(&files[sound as usize]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            // Reap it when it ends, so finished players don't linger as zombies.
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => {
                crate::log!("audio: afplay not available ({e}); sounds disabled");
                return;
            }
        }
    }
}

/// Linux: play through ALSA (PipeWire/PulseAudio via their ALSA plugin).
#[cfg(not(target_os = "macos"))]
fn play_with_alsa(rx: std::sync::mpsc::Receiver<Sound>) {
    let Some(alsa) = Alsa::load() else {
        crate::log!("audio: libasound not available, sounds disabled");
        return;
    };
    let cache: Vec<Vec<i16>> = ALL.into_iter().map(render).collect();
    let mut pcm: Option<*mut c_void> = None;
    let mut last_move = Instant::now() - Duration::from_secs(1);
    loop {
        let msg = if pcm.is_some() { rx.recv_timeout(Duration::from_secs(3)) } else { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) };
        match msg {
            Ok(s) => {
                if matches!(s, Sound::Move) {
                    if last_move.elapsed() < Duration::from_millis(40) {
                        continue;
                    }
                    last_move = Instant::now();
                }
                if pcm.is_none() {
                    pcm = alsa.open_pcm();
                }
                if let Some(p) = pcm {
                    alsa.write(p, &cache[s as usize]);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Release the device after a few idle seconds.
                if let Some(p) = pcm.take() {
                    unsafe {
                        (alsa.drain)(p);
                        (alsa.close)(p);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

pub fn init() {
    let (tx, rx) = channel::<Sound>();
    if TX.set(tx).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            #[cfg(target_os = "macos")]
            play_with_afplay(rx);
            #[cfg(not(target_os = "macos"))]
            play_with_alsa(rx);
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }
    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    #[test]
    fn wav_has_a_valid_mono_16_bit_header() {
        let wav = wav_bytes(&[1, -2, 300], 48_000);
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32_at(&wav, 4), 36 + 6, "RIFF size excludes the first 8 bytes");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32_at(&wav, 16), 16, "fmt chunk size");
        assert_eq!(u16_at(&wav, 20), 1, "PCM");
        assert_eq!(u16_at(&wav, 22), 1, "mono");
        assert_eq!(u32_at(&wav, 24), 48_000, "sample rate");
        assert_eq!(u32_at(&wav, 28), 96_000, "bytes per second");
        assert_eq!(u16_at(&wav, 32), 2, "block align");
        assert_eq!(u16_at(&wav, 34), 16, "bits per sample");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32_at(&wav, 40), 6, "data size");
        let samples: Vec<i16> = wav[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(samples, [1, -2, 300]);
    }

    #[test]
    fn an_empty_wav_is_still_well_formed() {
        let wav = wav_bytes(&[], 48_000);
        assert_eq!(wav.len(), 44);
        assert_eq!(u32_at(&wav, 4), 36);
        assert_eq!(u32_at(&wav, 40), 0);
    }

    #[test]
    fn every_sound_renders_audible_samples() {
        for sound in ALL {
            let samples = render(sound);
            assert!(samples.len() > 1000, "{sound:?} is too short");
            assert!(samples.iter().any(|s| s.abs() > 100), "{sound:?} is silent");
        }
    }
}
