//! A game's versions for each platform (PS5, PS4, PC) on one card, and which to play: the one
//! you have, else the one that plays best on this PC and can be downloaded. Only what's known is
//! said: emulator results, this PC's verdict, ProtonDB, how many share it. No promised frame rates.

use crate::app::*;
use crate::compat::Outlook;
use crate::platform::Platform;
use crate::words::Availability;

/// The order a game's platforms are shown in, the same on every game.
pub const PLATFORMS: [Platform; 3] = [Platform::Ps5, Platform::Ps4, Platform::Pc];

/// How well a release should play on this PC: a score (higher is better) and what it means.
pub struct Fit {
    pub score: u8,
    pub headline: String,
    /// The Fact colours: 1 good, 2 warning, 3 bad, 0 unknown.
    pub kind: i32,
}

impl Fit {
    fn new(score: u8, headline: &str, kind: i32) -> Fit {
        Fit { score, headline: headline.into(), kind }
    }
}

/// On a tie, a PC game (no emulator) before PS4 (the more mature emulator) before PS5.
fn platform_preference(p: Platform) -> u8 {
    match p {
        Platform::Pc => 2,
        Platform::Ps4 => 1,
        Platform::Ps5 => 0,
    }
}

fn availability_rank(a: Availability) -> u8 {
    match a {
        Availability::Good => 3,
        Availability::Low => 2,
        Availability::Unknown => 1,
        Availability::None => 0,
    }
}

impl App {
    /// How well release `i` should play here.
    pub fn fit(&self, i: usize) -> Fit {
        let g = &self.games[i];
        let t = Target { game: Some(i), local: g.local };
        if g.g.platform != Platform::Pc {
            let outlook = self.outlook(t).unwrap_or(Outlook::Untested);
            let score = match outlook {
                Outlook::PlaysHere => 6,
                Outlook::Plays => 5,
                Outlook::PlaysSlowly => 4,
                Outlook::Untested => 3,
                Outlook::StopsEarly => 2,
                Outlook::DoesntStart => 1,
                Outlook::CantRunHere => 0,
            };
            return Fit::new(score, outlook.headline(), outlook.kind());
        }
        if !self.pc_windows(t) {
            return Fit::new(6, "Native Linux game", 1);
        }
        match g.g.steam.as_ref().and_then(|s| s.protondb.as_deref()) {
            Some(tier @ ("platinum" | "gold" | "native")) => Fit::new(6, &format!("Runs with Wine · {}", title_case(tier)), 1),
            Some("borked") => Fit::new(1, "Doesn't run with Wine yet", 3),
            Some(tier) => Fit::new(4, &format!("Runs with Wine · {}", title_case(tier)), 2),
            None if g.g.wine => Fit::new(3, "Runs with Wine (included)", 0),
            None => Fit::new(3, "Needs Proton or Wine", 0),
        }
    }

    /// What makes one release a better pick than another, best highest: the one you have, one
    /// you can get, how well it plays here, whether anyone shares it, then the platform; then
    /// a picture for its card, the newest version, the most shared.
    pub fn release_rank(&self, i: usize) -> impl Ord + use<> {
        let g = &self.games[i];
        let installed_version = g.local.is_some_and(|l| {
            let a = crate::game_groups::version_key(&g.g.version);
            !a.is_empty() && a == crate::game_groups::version_key(&self.locals[l].l.version)
        });
        let availability = Availability::of(g.g.seeders);
        (
            (installed_version, g.local.is_some(), !g.g.magnet.is_empty()),
            (self.fit(i).score, availability != Availability::None, platform_preference(g.g.platform), availability_rank(availability)),
            (self.card_req(i).is_some(), g.info.as_ref().is_some_and(|info| !info.master.is_empty())),
            (crate::game_groups::version_key(&g.g.version), g.g.seeders.unwrap_or(0), g.g.id),
        )
    }

    /// The release of `members` to show and play.
    pub fn best_release(&self, members: impl IntoIterator<Item = usize>) -> Option<usize> {
        members.into_iter().max_by_key(|&i| self.release_rank(i))
    }

    /// The game's platforms in the usual order, each with its best release.
    pub fn platform_versions(&self, index: usize) -> Vec<(Platform, usize)> {
        let members = self.groups.releases(index);
        PLATFORMS.iter().filter_map(|&p| {
            self.best_release(members.iter().copied().filter(|&i| self.games[i].g.platform == p)).map(|i| (p, i))
        }).collect()
    }

    /// The platform that should play best here, leaving aside which one is installed: for
    /// "Best for this PC". None with a single platform.
    pub fn best_platform(&self, versions: &[(Platform, usize)]) -> Option<Platform> {
        if versions.len() < 2 {
            return None;
        }
        versions.iter().max_by_key(|(p, i)| {
            let availability = Availability::of(self.games[*i].g.seeders);
            (self.fit(*i).score, availability != Availability::None, platform_preference(*p), availability_rank(availability))
        }).map(|(p, _)| *p)
    }

    /// Show the game on another platform: its best release there.
    pub fn switch_version(&mut self, i: usize) {
        let Some(t) = self.hub else { return };
        let Some(current) = t.game else { return };
        let versions = self.platform_versions(current);
        let Some(&(_, release)) = versions.get(i) else { return };
        if release != current {
            crate::audio::play(crate::audio::Sound::Select);
            self.open_hub(Target { game: Some(release), local: self.games[release].local });
        }
        self.set_focus(Z_VERSIONS, i as i32);
    }

    /// The game's name on its card: on several platforms, the plainest of its releases' names
    /// ("Hades", not "Hades + Original Soundtrack Bundle"); else the release's own.
    pub fn game_title(&self, index: usize) -> String {
        let members = self.groups.releases(index);
        let platforms = PLATFORMS.iter().filter(|p| members.iter().any(|&i| self.games[i].g.platform == **p)).count();
        if platforms < 2 {
            return self.games[index].name.clone();
        }
        members.iter().map(|&i| &self.games[i].name).filter(|n| !n.is_empty())
            .min_by_key(|n| n.chars().count()).cloned().unwrap_or_else(|| self.games[index].name.clone())
    }

    /// How many platforms the Game Hub's game has (its "Play it on" row shows from two).
    pub fn hub_version_count(&self) -> usize {
        let n = self.hub.and_then(|t| t.game).map(|i| self.platform_versions(i).len()).unwrap_or(0);
        if n > 1 { n } else { 0 }
    }

    /// Down (1) or up (-1) the Game Hub's rows above its details: the buttons, "Play it on"
    /// (the platform shown highlighted), the other releases, the details.
    pub fn hub_step(&mut self, d: i32) {
        let current = self.hub.and_then(|t| t.game);
        let releases = current.is_some_and(|i| self.release_order(i).len() > 1);
        let rows: Vec<i32> = [Some(Z_HUB), (self.hub_version_count() > 0).then_some(Z_VERSIONS), releases.then_some(Z_RELEASES), Some(Z_HUB_DETAILS)]
            .into_iter().flatten().collect();
        let Some(at) = rows.iter().position(|z| *z == self.zone) else { return };
        let Some(&to) = usize::try_from(at as i32 + d).ok().and_then(|n| rows.get(n)) else { return };
        let idx = if to == Z_VERSIONS {
            let shown = current.map(|i| self.games[i].g.platform);
            current.map(|i| self.platform_versions(i)).unwrap_or_default().iter().position(|(p, _)| Some(*p) == shown).unwrap_or(0) as i32
        } else {
            0
        };
        self.move_focus(to, idx);
    }

    /// The Game Hub's "Play it on" row: one tile per platform.
    pub fn version_tiles(&self, t: Target) -> Vec<crate::VersionData> {
        let Some(current) = t.game else { return Vec::new() };
        let versions = self.platform_versions(current);
        if versions.len() < 2 {
            return Vec::new();
        }
        let best = self.best_platform(&versions);
        let shown = self.games[current].g.platform;
        versions.iter().map(|&(p, i)| {
            let g = &self.games[i];
            let fit = self.fit(i);
            let detail = if g.local.is_some() {
                "Installed".to_string()
            } else {
                [format!("Availability: {}", Availability::of(g.g.seeders).label()), crate::present::size_label(&g.g).unwrap_or_default()]
                    .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
            };
            crate::VersionData {
                platform: p.label().into(),
                verdict: fit.headline.into(),
                verdict_kind: fit.kind,
                detail: detail.into(),
                best: best == Some(p),
                current: p == shown,
            }
        }).collect()
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
