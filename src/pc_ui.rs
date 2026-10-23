//! This PC in the launcher: the Game Hub's line about it, the Settings row, the download
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
        // Which version each Library card shows depends on what plays here (the highlighted
        // game stays highlighted: see `apply_filter`).
        self.apply_filter();
        if self.view == 1 && !self.boot.active {
            self.push_grid();
        }
        match self.overlay {
            Overlay::Hub => self.push_hub(),
            Overlay::Settings => self.refresh_settings(),
            _ => {}
        }
        self.pc_notice();
    }

    /// This PC's verdict for a platform's games (worked out once per reading of the PC).
    pub fn pc_verdict(&self, platform: Platform) -> Option<&Verdict> {
        self.pc.as_ref()?.verdict_for(platform)
    }

    /// Once per PC: say so when a console's games can't start here, before anyone downloads one.
    fn pc_notice(&mut self) {
        let Some(specs) = self.pc.clone() else { return };
        let blocked: Vec<(Platform, Verdict)> = [Platform::Ps5, Platform::Ps4].into_iter()
            .filter_map(|p| self.pc_verdict(p).filter(|v| matches!(v, Verdict::No(_))).map(|v| (p, v.clone())))
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
        self.toast_with(&title, &format!("{why}."), 2, None, false, "this-pc", "This PC's details");
        self.cfg.lock().unwrap().pc_noticed = summary;
        self.cfg.lock().unwrap().save();
    }

    /// The Hub's line about this PC, under its verdict: "This PC: PS5 games should run well".
    pub fn pc_line(&self, t: Target) -> Option<String> {
        let platform = self.target_platform(t);
        self.pc_verdict(platform).map(|v| format!("This PC: {}", explain(v, platform)))
    }

    /// Under the Hub's Download button: what pressing it means. The download's size, the folder
    /// it goes to and the next step, or a warning when that folder's disk is short of space.
    /// `None` once it's downloading or installed (the button says so itself).
    pub fn download_note(&self, t: Target) -> Option<(String, bool)> {
        let g = &self.games[t.game?];
        if t.local.is_some() || g.g.magnet.is_empty() || g.g.is_pkg() || self.download_for_target(t).is_some() {
            return None;
        }
        let dir = self.cfg.lock().unwrap().download_dir.clone();
        let shown = crate::util::display_path(&dir);
        let size = crate::present::size_label(&g.g);
        let free = crate::pc::free_gb(&crate::util::expand_home(&dir));
        let short = matches!((g.g.size_gb, free), (Some(need), Some(free)) if need > free);
        let mut parts: Vec<String> = size.into_iter().collect();
        parts.push(format!("goes to {shown}"));
        parts.push(match free {
            Some(free) if short => format!("only {free:.0} GB free there"),
            _ => "then press Install".into(),
        });
        Some((parts.join(" · "), short))
    }

    /// For the download dialog: a warning when this game won't start or may be slow here.
    pub fn pc_download_warning(&self, platform: Platform) -> String {
        match self.pc_verdict(platform) {
            Some(v @ Verdict::No(_)) => format!("This PC can't run {} games yet. {}.", platform.label(), explain(v, platform)),
            Some(v @ Verdict::Slow(_)) => format!("It may run slowly on this PC. {}.", explain(v, platform)),
            _ => String::new(),
        }
    }
}
