//! Presentation-only grouping. Release records, topic IDs and magnets stay untouched.
//!
//! One card per game: its releases (regions, editions, updates) and its versions for each
//! platform (PS5, PS4, PC). A name alone doesn't make two releases the same game when they came
//! out years apart: Doom (2016) isn't the 1993 Doom re-released, Prey (2017) isn't Prey (2006).
use crate::{catalog::Game, util::norm};
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Default)]
pub struct Groups {
    pub members: Vec<Vec<usize>>,
    by_game: Vec<usize>,
}

static EDITION: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)\b(?:digital\s+)?(?:deluxe|ultimate|standard|premium|gold|complete|collector'?s|game of the year|goty|definitive)\s+edition\b|\b(?:all\s+dlc)\b|\bdirector['’]?s\s+cut\b|\s\+\s[^+]*\b(?:dlc|pre-?purchase|bonus(?:es)?|entitlements?|season\s+pass|soundtrack|updates?)\b.*$"
).unwrap());

/// Releases on one platform with the same name this many years apart or more are different games.
const SAME_PLATFORM_YEARS: i32 = 2;
/// A game on another platform comes out later (God of War: PS4 2018, PC 2022) or earlier (PC
/// first, as many indies), but not this far apart: Tomb Raider (1996, 2013) are two games.
const PORT_YEARS: i32 = 7;

/// The year a release came out, from the catalog's date: "2021", "20.08.2021",
/// "4 September 2025", "2013-2018" (the first).
pub fn release_year(release: &str) -> Option<i32> {
    static YEAR: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\b(19[7-9]\d|20\d\d)\b").unwrap());
    YEAR.find(release).and_then(|m| m.as_str().parse().ok())
}

/// Close enough in years to be the same game; an unknown year doesn't say otherwise.
fn close(a: Option<i32>, b: Option<i32>, within: i32) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= within,
        _ => true,
    }
}

fn name_key(name: &str) -> String {
    norm(&EDITION.replace_all(name, ""))
}

/// Looser than `name_key`, for artwork only: bracketed notes and a bare "Game of the Year" go too,
/// so "Bloodborne: Game of the Year" (PC) finds "Bloodborne Game of the Year Edition (Complete
/// Edition)" (PS4).
pub fn art_key(name: &str) -> String {
    static NOTES: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\([^()]*\)|\b(?:game of the year|goty)\b").unwrap());
    norm(&NOTES.replace_all(&EDITION.replace_all(name, ""), ""))
}

impl Groups {
    pub fn new<'a>(games: impl IntoIterator<Item = (&'a Game, &'a str)>) -> Self {
        let games: Vec<_> = games.into_iter().collect();
        let mut parent: Vec<usize> = (0..games.len()).collect();
        fn root(parent: &mut [usize], index: usize) -> usize {
            if parent[index] != index { parent[index] = root(parent, parent[index]); }
            parent[index]
        }
        fn join(parent: &mut [usize], a: usize, b: usize) {
            let (a, b) = (root(parent, a), root(parent, b));
            parent[a] = b;
        }
        let years: Vec<Option<i32>> = games.iter().map(|(game, _)| release_year(&game.release)).collect();
        let names = |game: &Game, display: &str| -> Vec<String> {
            let mut names = vec![name_key(&game.name), name_key(display)];
            names.retain(|name| !name.is_empty());
            names.dedup();
            names
        };
        // First each platform's releases: same title ID, or same name a few years apart at most.
        let mut aliases = HashMap::<String, Vec<usize>>::new();
        for (index, (game, display)) in games.iter().enumerate() {
            let ids = std::iter::once(&game.title_id).chain(game.title_ids.iter())
                .filter(|id| crate::psn::valid_title_id(id)).map(|id| (format!("id:{}", id.to_ascii_uppercase()), true));
            let console = game.platform.label();
            let keys = ids.chain(names(game, display).into_iter().map(|name| (format!("name:{name}"), false)))
                .map(|(key, id)| (format!("{console}:{key}"), id));
            for (key, id) in keys {
                let seen = aliases.entry(key).or_default();
                match seen.iter().copied().find(|&other| id || close(years[index], years[other], SAME_PLATFORM_YEARS)) {
                    Some(other) => join(&mut parent, index, other),
                    None => seen.push(index),
                }
            }
        }
        // Then the same game on other platforms: same name, and each the other's nearest in
        // years (PS4's Doom 2016 and PC's DOOM 2016, not PS4's classic Doom re-release).
        let roots: Vec<usize> = (0..games.len()).map(|i| root(&mut parent, i)).collect();
        let mut year_of = HashMap::<usize, i32>::new();
        let mut by_name = HashMap::<String, Vec<usize>>::new();
        for (index, (game, display)) in games.iter().enumerate() {
            let r = roots[index];
            if let Some(y) = years[index] {
                year_of.entry(r).and_modify(|e| *e = (*e).min(y)).or_insert(y);
            }
            // Looser names here: "God of War (2018)" on PS4 is "God of War" on PC; the years
            // keep "Resident Evil 4 (Remake)" from the 2007 one.
            let mut loose = vec![art_key(&game.name), art_key(display)];
            loose.retain(|name| !name.is_empty());
            loose.dedup();
            for name in loose {
                let list = by_name.entry(name).or_default();
                if !list.contains(&r) { list.push(r); }
            }
        }
        let platform = |r: usize| games[r].0.platform;
        let mut links = Vec::new();
        for list in by_name.values() {
            // The other platform's card nearest in years to `from`'s, if any is close enough. An
            // unknown year only links a name that's one card on each side.
            let nearest = |from: usize, on: crate::platform::Platform| -> Option<usize> {
                let there: Vec<usize> = list.iter().copied().filter(|&r| platform(r) == on).collect();
                let here = list.iter().filter(|&&r| platform(r) == platform(from)).count();
                match (year_of.get(&from), there.as_slice()) {
                    (None, &[only]) if here == 1 => Some(only),
                    (Some(_), &[only]) if here == 1 && !year_of.contains_key(&only) => Some(only),
                    (Some(&y), _) => there.iter().filter_map(|&r| year_of.get(&r).map(|&ry| ((ry - y).abs(), r)))
                        .filter(|(gap, _)| *gap <= PORT_YEARS).min().map(|(_, r)| r),
                    _ => None,
                }
            };
            for &a in list {
                for &b in list {
                    if a < b && platform(a) != platform(b) && nearest(a, platform(b)) == Some(b) && nearest(b, platform(a)) == Some(a) {
                        links.push((a, b));
                    }
                }
            }
        }
        for (a, b) in links {
            join(&mut parent, a, b);
        }
        let mut groups = Self::default();
        let mut roots = HashMap::new();
        for index in 0..games.len() {
            let r = root(&mut parent, index);
            let group = *roots.entry(r).or_insert_with(|| {
                groups.members.push(Vec::new()); groups.members.len() - 1
            });
            groups.members[group].push(index);
            groups.by_game.push(group);
        }
        groups
    }
    pub fn releases(&self, index: usize) -> &[usize] {
        self.by_game.get(index).map(|group| self.members[*group].as_slice()).unwrap_or(&[])
    }
}

pub fn version_key(version: &str) -> Vec<u64> {
    let mut parts: Vec<u64> = version.split(|c: char| !c.is_ascii_digit()).filter_map(|s| s.parse().ok()).collect();
    while parts.last() == Some(&0) { parts.pop(); }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    fn groups(games: &[Game]) -> Groups { Groups::new(games.iter().map(|game| (game, game.name.as_str()))) }
    fn game(name: &str, id: &str) -> Game { Game { name: name.into(), title_id: id.into(), ..Default::default() } }
    #[test]
    fn groups_regions_editions_and_transitive_aliases_without_losing_records() {
        let mut games = vec![game("Dead Cells", "PPSA15552"), game("Dead Cells: Return to Castlevania Edition", "PPSA15552"),
            game("Dead Cells", "PPSA15553"), game("Other title", "PPSA15554")];
        games[2].title_ids.push("PPSA15554".into());
        games[0].magnet = "first magnet".into(); games[1].magnet = "second magnet".into();
        let grouped = groups(&games);
        assert_eq!(grouped.members.len(), 1);
        assert_eq!(grouped.releases(0), &[0, 1, 2, 3]);
        assert_eq!(games[0].magnet, "first magnet"); assert_eq!(games[1].magnet, "second magnet");
    }
    fn on(platform: crate::platform::Platform, name: &str, release: &str) -> Game {
        Game { name: name.into(), platform, release: release.into(), ..Default::default() }
    }

    #[test]
    fn a_games_versions_for_each_platform_are_one_card() {
        use crate::platform::Platform::{Pc, Ps4, Ps5};
        let mut ps4 = game("Firewatch", "CUSA04118");
        ps4.platform = Ps4;
        let mut ps4_eur = game("Firewatch", "CUSA04987");
        ps4_eur.platform = Ps4;
        let games = vec![game("Firewatch", "PPSA01234"), ps4, ps4_eur, on(Pc, "Firewatch", "")];
        assert_eq!(groups(&games).members, vec![vec![0, 1, 2, 3]]);
        // A port years later is the same game; a Director's Cut is an edition.
        let games = [on(Ps4, "God of War (2018)", "2018"), on(Pc, "God of War", "2022"),
            on(Ps4, "Ghost of Tsushima", "2020"), on(Ps5, "Ghost of Tsushima: Director's Cut", "20.08.2021"), on(Pc, "Ghost of Tsushima", "2024")];
        assert_eq!(groups(&games).members, vec![vec![0, 1], vec![2, 3, 4]]);
        // What came with a release ("+ DLC") is no part of its name; a second game is.
        assert_eq!(name_key("Ghost of Tsushima DIRECTOR'S CUT + Pre-purchase Entitlements"), "ghost of tsushima");
        assert_eq!(name_key("Final Fantasy VII Remake + 6 DLC + BP 5."), "final fantasy vii remake");
        assert_eq!(name_key("DOOM + DOOM II"), "doom doom ii");
    }

    #[test]
    fn same_names_years_apart_are_different_games() {
        use crate::platform::Platform::{Pc, Ps4};
        let games = [on(Pc, "Prey", "2006"), on(Pc, "Prey", "2017"), on(Ps4, "Prey", "2017"),
            on(Ps4, "Doom", "2016"), on(Ps4, "Doom", "2021"), on(Pc, "DOOM", "2016"),
            on(Pc, "Tomb Raider", "1996"), on(Ps4, "Tomb Raider: Definitive Edition", "2014")];
        let grouped = groups(&games);
        let together = |a: usize, b: usize| grouped.releases(a).contains(&b);
        assert!(together(1, 2) && !together(0, 1) && !together(0, 2));
        assert!(together(3, 5) && !together(3, 4) && !together(4, 5));
        assert!(!together(6, 7));
        assert_eq!(release_year("4 September 2025"), Some(2025));
        assert_eq!(release_year("2013-2018"), Some(2013));
        assert_eq!(release_year(""), None);
    }

    #[test]
    fn keeps_sequels_remasters_and_standalone_expansions_distinct() {
        let games = vec![game("Alan Wake", ""), game("Alan Wake 2", ""), game("Alan Wake Remastered", ""),
            game("Alan Wake 2 Deluxe Edition", ""), game("Alan Wake: American Nightmare", "")];
        let grouped = groups(&games);
        assert_eq!(grouped.members.len(), 4);
        assert_eq!(grouped.releases(1), &[1, 3]);
        assert_eq!(version_key("v1.05"), version_key("01.005.000"));
    }
    #[test]
    fn shared_artwork_names_group_cards_and_empty_names_do_not() {
        let games = [game("Catalog region A", ""), game("Catalog region B", ""), game("", ""), game("", "")];
        let grouped = Groups::new(games.iter().zip(["Same Game", "Same Game", "", ""]));
        assert_eq!(grouped.members.len(), 3);
        assert_eq!(grouped.releases(0), &[0, 1]);
    }
    #[test]
    fn bundled_releases_group_dead_cells_and_preserve_every_topic() {
        let file = crate::catalog::import(include_bytes!("../assets/rutracker/ps5-topics.json")).unwrap();
        let grouped = groups(&file.games);
        assert!(grouped.members.len() < 500);
        assert_eq!(grouped.members.iter().map(Vec::len).sum::<usize>(), 614);
        let index = file.games.iter().position(|game| game.id == 6744687).unwrap();
        assert!(grouped.releases(index).iter().any(|i| file.games[*i].id == 6744684));
    }
}
