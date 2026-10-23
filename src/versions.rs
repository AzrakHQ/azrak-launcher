//! A game's versions for each platform (PS5, PS4, PC) on one card, and which to play: the one
//! you have, else the one that plays best on this PC and can be downloaded. Only what's known is
//! said: emulator results, this PC's verdict, ProtonDB, how many share it. No promised frame rates.

use crate::app::*;
use crate::compat::Outlook;
use crate::platform::Platform;
use crate::words::Availability;

/// The order a game's platforms are shown in, the same on every game.
pub const PLATFORMS: [Platform; 3] = [Platform::Ps5, Platform::Ps4, Platform::Pc];

/// How well a release should play on this PC, from what's known about it. Cheap: the
/// Library works it out for every release on every keystroke.
pub enum Plays<'a> {
    /// A PS5 or PS4 release: the emulator's results and this PC's verdict.
    Console(Outlook),
    Native,
    /// A Windows game: ProtonDB's tier, whether Wine comes with the release, and what runs it
    /// here otherwise ("Proton", "Wine"; looked up once, so its words and colour agree).
    Wine(Option<&'a str>, bool, Option<&'static str>),
}

impl Plays<'_> {
    /// Higher plays better.
    pub fn score(&self) -> u8 {
        match self {
            Plays::Console(o) => match o {
                Outlook::PlaysHere => 6,
                Outlook::Plays => 5,
                Outlook::PlaysSlowly => 4,
                Outlook::Untested => 3,
                Outlook::StopsEarly => 2,
                Outlook::DoesntStart => 1,
                Outlook::CantRunHere => 0,
            },
            Plays::Native | Plays::Wine(Some("platinum" | "gold" | "native"), ..) => 6,
            Plays::Wine(Some("borked"), ..) => 1,
            Plays::Wine(Some(_), ..) => 4,
            // Nothing here to run it with: it won't start.
            Plays::Wine(None, false, None) => 1,
            Plays::Wine(None, ..) => 3,
        }
    }

    pub fn headline(&self) -> String {
        match self {
            Plays::Console(o) => o.headline().into(),
            Plays::Native => "Native Linux game".into(),
            Plays::Wine(Some("borked"), ..) => "Doesn't run with Wine yet".into(),
            Plays::Wine(Some(tier), ..) => format!("Runs with Wine · {}", title_case(tier)),
            Plays::Wine(None, true, _) => "Runs with Wine (included)".into(),
            Plays::Wine(None, false, Some(runner)) => format!("Runs with {runner}"),
            Plays::Wine(None, false, None) => "Needs Proton or Wine".into(),
        }
    }

    /// The Game Hub's "how it runs" box for a PC game: its label, its value and (as on its tile)
    /// its colour. None for console games, whose page says it in its verdict line.
    pub fn hub_fact(&self) -> Option<(&'static str, String, i32)> {
        let (label, value) = match self {
            Plays::Console(_) => return None,
            Plays::Native => ("ON LINUX", "Native game".into()),
            Plays::Wine(Some(tier), ..) => ("PROTONDB · RUNS WITH WINE", title_case(tier)),
            Plays::Wine(None, true, _) => ("RUNS WITH", "Wine, included".into()),
            Plays::Wine(None, false, Some(runner)) => ("WINDOWS GAME · RUNS WITH", (*runner).into()),
            Plays::Wine(None, false, None) => ("WINDOWS GAME", "Needs Proton or Wine".into()),
        };
        Some((label, value, self.kind()))
    }

    /// The Fact colours: 1 good, 2 warning, 3 bad, 0 unknown.
    pub fn kind(&self) -> i32 {
        match self {
            Plays::Console(o) => o.kind(),
            Plays::Native => 1,
            Plays::Wine(Some(tier), ..) => protondb_kind(tier),
            // Nothing to run it with is a game that won't start.
            Plays::Wine(None, false, None) => 3,
            Plays::Wine(None, ..) => 0,
        }
    }
}

/// What runs Windows games here ("Proton", "Wine"): the Game Hub and its tiles ask on every
/// redraw, so it's looked up again only after a few seconds (installing Proton shows soon).
fn windows_runner() -> Option<&'static str> {
    static RUNNER: std::sync::Mutex<Option<(std::time::Instant, Option<&'static str>)>> = std::sync::Mutex::new(None);
    let mut seen = RUNNER.lock().unwrap();
    match *seen {
        Some((at, runner)) if at.elapsed() < std::time::Duration::from_secs(5) => runner,
        _ => {
            let runner = crate::pcgame::windows_runner().map(|(_, runner)| runner);
            *seen = Some((std::time::Instant::now(), runner));
            runner
        }
    }
}

/// ProtonDB's tier as a Fact colour: 1 good, 2 warning, 3 bad.
pub fn protondb_kind(tier: &str) -> i32 {
    match tier {
        "platinum" | "gold" | "native" => 1,
        "borked" => 3,
        _ => 2,
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
    pub fn plays(&self, i: usize) -> Plays<'_> {
        self.plays_for(Target { game: Some(i), local: self.games[i].local })
    }

    /// How well a game should play here: a catalog release, or one installed (catalogued or
    /// not). The tiles and the Game Hub both say it from this.
    pub fn plays_for(&self, t: Target) -> Plays<'_> {
        let platform = self.target_platform(t);
        if platform != Platform::Pc {
            // A catalog release reads its entry directly (no copy of its title ID).
            let compat = match t.game {
                Some(i) if t.local.is_none() => self.game_compat(&self.games[i]),
                _ => self.target_compat(t),
            };
            return Plays::Console(Outlook::of(compat, self.pc_verdict(platform)));
        }
        if !self.pc_windows(t) {
            return Plays::Native;
        }
        let g = t.game.map(|i| &self.games[i].g);
        let tier = g.and_then(|g| g.steam.as_ref()).and_then(|s| s.protondb.as_deref()).filter(|tier| !tier.is_empty());
        let packed = g.is_some_and(|g| g.wine);
        // Only asked when it's what would run it.
        let runner = (tier.is_none() && !packed).then(windows_runner).flatten();
        Plays::Wine(tier, packed, runner)
    }

    /// Which version plays best here, the one you have aside: how well it plays, whether
    /// anyone shares it. "Best for this PC" and the release a card shows both start from it.
    fn play_rank(&self, i: usize) -> (u8, bool) {
        (self.plays(i).score(), Availability::of(self.games[i].g.seeders) != Availability::None)
    }

    /// Between versions that play alike: a PC game, then PS4, then PS5; then the most shared.
    fn tie_rank(&self, i: usize) -> (u8, u8) {
        let g = &self.games[i];
        (platform_preference(g.g.platform), availability_rank(Availability::of(g.g.seeders)))
    }

    /// What makes one release a better pick than another, best highest: the one you have, one
    /// you can get, how it plays (`play_rank`); then a picture for its card (no placeholder
    /// while a release has one), `tie_rank`, the newest version, the most shared.
    pub fn release_rank(&self, i: usize) -> impl Ord + use<> {
        let g = &self.games[i];
        let installed_version = g.local.is_some_and(|l| {
            let a = crate::game_groups::version_key(&g.g.version);
            !a.is_empty() && a == crate::game_groups::version_key(&self.locals[l].l.version)
        });
        (
            (installed_version, g.local.is_some(), !g.g.magnet.is_empty()),
            self.play_rank(i),
            (self.has_card_art(i), self.tie_rank(i), g.info.as_ref().is_some_and(|info| !info.master.is_empty())),
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
        // How it plays here and the platform, not which release has a picture.
        versions.iter().max_by_key(|(_, i)| (self.play_rank(*i), self.tie_rank(*i))).map(|(p, _)| *p)
    }

    /// Show the game on another platform: its best release there.
    pub fn switch_version(&mut self, i: usize) {
        let Some(t) = self.hub else { return };
        let Some(current) = t.game else { return };
        let versions = self.platform_versions(current);
        let Some(&(platform, release)) = versions.get(i) else { return };
        // The platform shown stays on the release picked with "Choose another release".
        if platform != self.games[current].g.platform {
            crate::audio::play(crate::audio::Sound::Select);
            self.open_hub(Target { game: Some(release), local: self.games[release].local });
        }
        self.set_focus(Z_VERSIONS, i as i32);
    }

    /// The year release `i` came out: the catalog's date, else the store's. Grouping and the
    /// search suggestions both go by it.
    pub fn release_year(&self, i: usize) -> Option<i32> {
        let g = &self.games[i];
        crate::game_groups::release_year(&g.g.release).or_else(|| (g.rel_day != 0).then(|| crate::util::civil_from_days(g.rel_day).0))
    }

    /// The year game `index` came out: its earliest release's, as grouping counts it (a 2020
    /// upload of Doom 2016 is still Doom 2016).
    pub fn game_year(&self, index: usize) -> Option<i32> {
        self.groups.releases(index).iter().filter_map(|&i| self.release_year(i)).min()
    }

    /// How many platforms game `index` is on.
    pub fn platform_count(&self, index: usize) -> usize {
        let members = self.groups.releases(index);
        PLATFORMS.iter().filter(|p| members.iter().any(|&i| self.games[i].g.platform == **p)).count()
    }

    /// The platforms game `index` is on, in the usual order.
    pub fn game_platforms(&self, index: usize) -> Vec<Platform> {
        let members = self.groups.releases(index);
        PLATFORMS.iter().copied().filter(|p| members.iter().any(|&i| self.games[i].g.platform == *p)).collect()
    }

    /// The game's name on its card: on several platforms, the plainest of its releases' names
    /// ("Hades", not "Hades + Original Soundtrack Bundle"); else the release's own.
    pub fn game_title(&self, index: usize) -> String {
        if self.platform_count(index) < 2 {
            return self.games[index].name.clone();
        }
        self.groups.releases(index).iter().map(|&i| &self.games[i].name).filter(|n| !n.is_empty())
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
        let shown = self.games[current].g.platform;
        let best_releases = self.platform_versions(current);
        if best_releases.len() < 2 {
            return Vec::new();
        }
        // "Best for this PC" is a platform's, judged by its best release, so it stays put while
        // you browse that platform's releases; the shown platform's tile is the release on screen.
        let best = self.best_platform(&best_releases);
        let versions: Vec<(Platform, usize)> = best_releases.into_iter().map(|(p, b)| (p, if p == shown { current } else { b })).collect();
        versions.iter().map(|&(p, i)| {
            let g = &self.games[i];
            // The shown platform's tile says what the page says (with the installed copy kept
            // across the release picker).
            let plays = if p == shown { self.plays_for(t) } else { self.plays(i) };
            let detail = if g.local.is_some() {
                "Installed".to_string()
            } else {
                [format!("Availability: {}", Availability::of(g.g.seeders).label()), crate::present::size_label(&g.g).unwrap_or_default()]
                    .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
            };
            crate::VersionData {
                platform: p.label().into(),
                verdict: plays.headline().into(),
                verdict_kind: plays.kind(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tile_and_the_game_hub_say_the_same() {
        for plays in [Plays::Native, Plays::Wine(Some("gold"), false, None), Plays::Wine(Some("silver"), true, None),
            Plays::Wine(Some("borked"), false, None), Plays::Wine(None, true, None), Plays::Wine(None, false, Some("Proton")), Plays::Wine(None, false, None)] {
            assert_eq!(Some(plays.kind()), plays.hub_fact().map(|f| f.2));
        }
        assert_eq!(Plays::Wine(Some("gold"), false, None).hub_fact().unwrap().1, "Gold");
        // Colours are what a version is, not its place in the ranking.
        assert_eq!((Plays::Native.kind(), Plays::Wine(Some("silver"), false, None).kind(), Plays::Wine(Some("borked"), false, None).kind()), (1, 2, 3));
        // Nothing to run a Windows game with: red, and ranked as won't start.
        assert_eq!((Plays::Wine(None, false, None).kind(), Plays::Wine(None, false, None).score()), (3, 1));
        assert!(Plays::Console(Outlook::Plays).hub_fact().is_none());
        // An empty tier can't reach a title (none is asked for) and wouldn't break one.
        assert_eq!(title_case(""), "");
    }
}
