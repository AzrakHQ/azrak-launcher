//! Monitors, window placement, focus, and external video playback.

use slint::winit_030::{winit, WinitWindowAccessor};
use slint::ComponentHandle;
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

/// Connected displays. Empty on failure.
#[cfg(target_os = "macos")]
pub fn monitors() -> Vec<Monitor> {
    Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .stderr(Stdio::null())
        .output()
        .map(|o| parse_system_profiler(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Displays from `system_profiler SPDisplaysDataType -json`. Names match the ones winit reports
/// (the localized display name), which is how the window is put on the chosen one; positions
/// aren't in this output, so the window code reads them from the monitor itself.
#[cfg(any(target_os = "macos", test))]
fn parse_system_profiler(json: &str) -> Vec<Monitor> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let mut out = Vec::new();
    for gpu in v["SPDisplaysDataType"].as_array().into_iter().flatten() {
        for d in gpu["spdisplays_ndrvs"].as_array().into_iter().flatten() {
            let Some(name) = d["_name"].as_str() else { continue };
            // "3024 x 1964" (pixels); fall back to the scaled resolution "1512 x 982 @ 120.00Hz".
            let size = [d["_spdisplays_pixels"].as_str(), d["_spdisplays_resolution"].as_str()]
                .into_iter()
                .flatten()
                .find_map(|t| {
                    let mut n = t.split(['x', '@']).filter_map(|p| p.split_whitespace().next()?.parse::<u32>().ok());
                    Some((n.next()?, n.next()?))
                });
            let Some((w, h)) = size else { continue };
            out.push(Monitor { name: name.to_string(), w, h, x: 0, y: 0, primary: d["spdisplays_main"].as_str() == Some("spdisplays_yes") });
        }
    }
    out
}

/// Connected displays from `xrandr --listmonitors` (X11). Empty on failure.
#[cfg(not(target_os = "macos"))]
pub fn monitors() -> Vec<Monitor> {
    let Ok(out) = Command::new("xrandr").arg("--listmonitors").stderr(Stdio::null()).output() else { return Vec::new() };
    let re = regex::Regex::new(r"^\s*\d+:\s*\+?(\*?)(\S+)\s+(\d+)/\d+x(\d+)/\d+([+-]\d+)([+-]\d+)").unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let c = re.captures(l)?;
            Some(Monitor {
                primary: !c[1].is_empty(),
                name: c[2].trim_start_matches(['+', '*']).to_string(),
                w: c[3].parse().ok()?,
                h: c[4].parse().ok()?,
                x: c[5].parse().ok()?,
                y: c[6].parse().ok()?,
            })
        })
        .collect()
}

pub fn pick(mons: &[Monitor], pref: &str) -> Option<Monitor> {
    mons.iter()
        .find(|m| !pref.is_empty() && m.name == pref)
        .or_else(|| mons.iter().find(|m| m.primary))
        .or_else(|| mons.first())
        .cloned()
}

/// Put the window on one monitor (never spanning two) and make it borderless fullscreen.
/// The native window only exists once the event loop runs, so this retries until the window
/// is there and the window manager has actually made it fullscreen.
pub fn place_window(ui: &crate::AppWindow, target: Option<&Monitor>, windowed: bool) {
    fn attempt(weak: slint::Weak<crate::AppWindow>, target: Option<Monitor>, windowed: bool, tries: u32) {
        let Some(ui) = weak.upgrade() else { return };
        let done = place_now(&ui, target.as_ref(), windowed);
        if !done && tries > 0 {
            slint::Timer::single_shot(std::time::Duration::from_millis(120), move || attempt(weak, target, windowed, tries - 1));
        }
    }
    attempt(ui.as_weak(), target.cloned(), windowed, 25);
}

/// Returns true once the window exists and is in the requested state.
fn place_now(ui: &crate::AppWindow, target: Option<&Monitor>, windowed: bool) -> bool {
    let name = target.map(|m| m.name.clone());
    let listed = target.map(|m| (m.x, m.y, m.w, m.h));
    ui.window().with_winit_window(move |w: &winit::window::Window| {
        if !windowed && w.fullscreen().is_some() {
            return true;
        }
        let handle = name.as_ref().and_then(|n| w.available_monitors().find(|m| m.name().as_deref() == Some(n.as_str())));
        // macOS can't list positions up front, so take them from the monitor itself; if winit names
        // the display differently than system_profiler, just fullscreen on the current one.
        let geom = if cfg!(target_os = "macos") {
            handle.as_ref().map(|m| (m.position().x, m.position().y, m.size().width, m.size().height))
        } else {
            listed
        };
        if windowed {
            if let Some((x, y, mw, mh)) = geom {
                let (ww, wh) = (1600.min(mw - 80), 900.min(mh - 80));
                let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(ww, wh));
                w.set_outer_position(winit::dpi::PhysicalPosition::new(x + (mw - ww) as i32 / 2, y + (mh - wh) as i32 / 2));
            }
        } else {
            if let Some((x, y, mw, mh)) = geom {
                w.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                // Fill the monitor even if the window manager ignores the fullscreen hint.
                let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(mw, mh));
            }
            w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(handle.or_else(|| w.current_monitor()))));
        }
        w.focus_window();
        windowed
    })
    .unwrap_or(false)
}

pub fn window_has_focus(ui: &crate::AppWindow) -> bool {
    ui.window().with_winit_window(|w: &winit::window::Window| w.has_focus()).unwrap_or(true)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_displays_from_system_profiler() {
        let json = r#"{"SPDisplaysDataType":[{"_name":"Apple M2 Pro","spdisplays_ndrvs":[
            {"_name":"Color LCD","_spdisplays_pixels":"3024 x 1964","_spdisplays_resolution":"1512 x 982 @ 120.00Hz","spdisplays_main":"spdisplays_yes"},
            {"_name":"DELL U2720Q","_spdisplays_resolution":"2560 x 1440 @ 60.00Hz"}]}]}"#;
        let m = parse_system_profiler(json);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].name.as_str(), m[0].w, m[0].h, m[0].primary), ("Color LCD", 3024, 1964, true));
        assert_eq!((m[1].name.as_str(), m[1].w, m[1].h, m[1].primary), ("DELL U2720Q", 2560, 1440, false));
        assert_eq!(pick(&m, "DELL U2720Q").unwrap().name, "DELL U2720Q");
        assert_eq!(pick(&m, "").unwrap().name, "Color LCD");
    }

    #[test]
    fn bad_system_profiler_output_gives_no_displays() {
        assert!(parse_system_profiler("").is_empty());
        assert!(parse_system_profiler("{}").is_empty());
        assert!(parse_system_profiler(r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[{"_name":"x"}]}]}"#).is_empty());
    }
}
