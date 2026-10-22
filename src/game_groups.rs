//! Presentation-only grouping. Release records, topic IDs and magnets stay untouched.
use crate::{catalog::Game, util::norm};
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Default)]
pub struct Groups {
    pub members: Vec<Vec<usize>>,
    by_game: Vec<usize>,
}

static EDITION: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)\b(?:digital\s+)?(?:deluxe|ultimate|standard|premium|gold|complete|collector'?s|game of the year|goty|definitive)\s+edition\b|\b(?:all\s+dlc)\b"
).unwrap());

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
        let mut aliases = HashMap::<String, usize>::new();
        for (index, (game, display)) in games.iter().enumerate() {
            let names = [name_key(&game.name), name_key(display)];
            let ids = std::iter::once(&game.title_id).chain(game.title_ids.iter())
                .filter(|id| crate::psn::valid_title_id(id)).map(|id| format!("id:{}", id.to_ascii_uppercase()));
            // A PS4 and a PS5 release of the same game stay separate cards: different emulators.
            let console = game.platform.label();
            let keys = ids.chain(names.into_iter().filter(|name| !name.is_empty()).map(|name| format!("name:{name}")))
                .map(|key| format!("{console}:{key}"));
            for key in keys {
                if let Some(&other) = aliases.get(&key) {
                    let (a, b) = (root(&mut parent, index), root(&mut parent, other));
                    parent[a] = b;
                } else { aliases.insert(key, index); }
            }
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
    #[test]
    fn ps4_and_ps5_releases_of_a_game_stay_separate() {
        let mut ps4 = game("Firewatch", "CUSA04118");
        ps4.platform = crate::platform::Platform::Ps4;
        let mut ps4_eur = game("Firewatch", "CUSA04987");
        ps4_eur.platform = crate::platform::Platform::Ps4;
        let games = vec![game("Firewatch", "PPSA01234"), ps4, ps4_eur];
        let groups = Groups::new(games.iter().map(|g| (g, g.name.as_str())));
        assert_eq!(groups.members, vec![vec![0], vec![1, 2]]);
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