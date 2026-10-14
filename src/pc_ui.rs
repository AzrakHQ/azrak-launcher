//! This PC in the launcher: the Game Hub's "ON THIS PC" fact, the Settings row, the download
//! dialog's warning and a one-time notice when a console's games can't run here.

use crate::app::*;
use crate::pc::{Specs, Verdict};
use crate::platform::Platform;

/// The reason behind a verdict, as a sentence.
pub fn explain(v: &Verdict, platform: Platform) -> String {
    let ps5 = platform == Platform::Ps5;
    let console = platform.label();
    match v {
        Verdict::Good => format!("{console} games should run well"),
        Verdict::No(why) if why == "no AVX2" => "PS4 games need a processor from 2013 or later (with AVX2)".into(),
        Verdict::No(why) if why == "no Vulkan 1.3" => "Games need a graphics card with Vulkan 1.3: update the graphics driver, or the card is too old".into(),
        Verdict::No(why) if why == "missing features" => format!(
            "Your graphics card lacks Vulkan features {} needs: update its driver, or the card is too old", platform.emulator()),
        Verdict::No(_) => "No graphics driver found: install the driver for your graphics card".into(),
        Verdict::Slow(why) if why == "GPU at low clock" => "On Linux this NVIDIA card runs on the open-source NVK driver, which keeps GTX 600–1000 series cards at a low clock speed, so games run slowly".into(),
        Verdict::Slow(why) if why == "integrated GPU" => format!("{console} games run best on a graphics card; built-in graphics may be slow"),
        Verdict::Slow(why) if why.ends_with("GPU") => format!("{console} games run best with {} GB of graphics memory", if ps5 { 8 } else { 4 }),
        Verdict::Slow(why) if why.ends_with("CPU") => format!("{console} games run best on {} or more processor cores", if ps5 { 6 } else { 4 }),
        Verdict::Slow(_) => format!("{console} games run best with {} GB of RAM", if ps5 { 16 } else { 8 }),
    }
}

impl App {
    /// Read this PC in the background (at start, and from Settings).
    pub fn pc_check(&mut self) {
        if self.pc_checking {
            return;
        }
        self.pc_checking = true;
        std::thread::spawn(|| {
            let specs = crate::pc::detect();
            let _ = slint::invoke_from_event_loop(move || with_app(move |app| app.pc_detected(specs)));
        });
    }

    fn pc_detected(&mut self, specs: Specs) {
        self.pc_checking = false;
        self.pc = Some(specs);
        match self.overlay {
            Overlay::Hub => self.push_hub(),
            Overlay::Settings => self.refresh_settings(),
            _ => {}
        }
        self.pc_notice();
    }

    pub fn pc_verdict(&self, platform: Platform) -> Option<Verdict> {
        self.pc.as_ref()?.verdict(platform)
    }

    /// Once per PC: say so when a console's games can't start here, before anyone downloads one.
    fn pc_notice(&mut self) {
        let Some(specs) = self.pc.clone() else { return };
        let blocked: Vec<(Platform, Verdict)> = [Platform::Ps5, Platform::Ps4].into_iter()
            .filter_map(|p| specs.verdict(p).filter(|v| matches!(v, Verdict::No(_))).map(|v| (p, v)))
            .collect();
        let summary = specs.summary();
        if blocked.is_empty() || self.cfg.lock().unwrap().pc_noticed == summary {
            return;
        }
        let (title, why) = match &blocked[..] {
            [(p, v)] => (format!("This PC can't run {} games", p.label()), explain(v, *p)),
            [(_, v), ..] => ("This PC can't run games yet".to_string(), explain(v, Platform::Ps5)),
            [] => return,
        };
        self.toast_with(&title, &format!("{why}. Select to see this PC's details."), 2, None, false, "this-pc");
        self.cfg.lock().unwrap().pc_noticed = summary;
        self.cfg.lock().unwrap().save();
    }

    /// The Hub's "ON THIS PC" fact: (label, value, kind 1 good · 2 warning · 3 bad).
    pub fn pc_fact(&self, t: Target) -> Option<(String, String, i32)> {
        let verdict = self.pc_verdict(self.target_platform(t))?;
        // Room for it, when it's still to be downloaded.
        if !matches!(verdict, Verdict::No(_)) && t.local.is_none() && self.download_for_target(t).is_none() {
            let need = t.game.and_then(|g| self.games[g].g.size_gb);
            let dir = crate::util::expand_home(&self.cfg.lock().unwrap().download_path(self.target_platform(t)));
            if let (Some(need), Some(free)) = (need, crate::pc::free_gb(&dir)) {
                if need > free {
                    return Some((format!("ON THIS PC · {free:.0} GB FREE"), "Not enough space".into(), 3));
                }
            }
        }
        let key = match verdict.why() {
            Some(why) => format!("ON THIS PC · {}", why.to_uppercase()),
            None => "ON THIS PC".into(),
        };
        Some((key, verdict.headline().into(), verdict.kind()))
    }

    /// For the download dialog: a warning when this game won't start or may be slow here.
    pub fn pc_download_warning(&self, platform: Platform) -> String {
        match self.pc_verdict(platform) {
            Some(v @ Verdict::No(_)) => format!("This PC can't run {} games yet. {}.", platform.label(), explain(&v, platform)),
            Some(v @ Verdict::Slow(_)) => format!("It may run slowly on this PC. {}.", explain(&v, platform)),
            _ => String::new(),
        }
    }
}
