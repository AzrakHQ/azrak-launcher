//! Library search: the game a player means from what they typed. On a controller every letter
//! takes a few presses, so a search finds the game early and forgives how it was typed:
//! - words run together or apart: "spiderman" and "witcher3" find Spider-Man and The Witcher 3;
//! - numbers either way: "final fantasy 7" finds Final Fantasy VII;
//! - initials and nicknames: "tlou", "re4", "gow", "spidey";
//! - a genre: "racing" finds racing games, after the games with "racing" in their name;
//! - typos: when nothing matches, near spellings do ("assasins creed"), and the Library says
//!   what it searched for instead.
//!
//! Results come best match first; the chosen sort orders the games that match equally well.

use crate::util::norm;
use std::sync::LazyLock;

/// Nicknames that aren't a title's initials (initials work on their own: "gow", "ff7", "re4").
const NICKNAMES: [(&str, &str); 12] = [
    ("spidey", "spider man"),
    ("fifa", "ea sports fc"),
    ("pes", "efootball"),
    ("pubg", "playerunknowns battlegrounds"),
    ("bf", "battlefield"),
    ("ow", "overwatch"),
    ("lbp", "littlebigplanet"),
    ("dbfz", "dragon ball fighterz"),
    ("mc", "minecraft"),
    ("bb", "bloodborne"),
    ("cp", "cyberpunk"),
    ("re", "biohazard"),
];

/// What sites put between slashes besides other names: languages, regions, kinds of release.
const NOTE_WORDS: [&str; 10] = ["rus", "eng", "eur", "usa", "jpn", "asia", "multi", "retail", "fpkg", "pkg"];

/// Words initials may leave out: "tlou2" is The Last of Us Part II.
const SKIP: [&str; 6] = ["the", "a", "an", "of", "and", "part"];

const ROMAN: [&str; 20] = ["i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii", "xiii", "xiv", "xv", "xvi", "xvii", "xviii", "xix", "xx"];
const ARABIC: [&str; 20] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20"];

// How well a typed word matches a game, best last.
/// The publisher, a title ID, a genre, or (when nothing else matched) a near spelling.
const OTHER: u8 = 1;
/// Inside a word: "man" in Batman.
const INSIDE: u8 = 2;
/// Two-letter initials: "er" for Elden Ring.
const SHORT: u8 = 3;
/// The start of a word, or of words run together: "spiderm".
const START: u8 = 4;
/// A whole word, a nickname, or initials of three letters or more.
const WHOLE: u8 = 5;

/// 7 for VII and VII for 7, up to 20.
fn other_number(word: &str) -> Option<&'static str> {
    ROMAN.iter().position(|r| *r == word).map(|i| ARABIC[i]).or_else(|| ARABIC.iter().position(|a| *a == word).map(|i| ROMAN[i]))
}

/// One name of a game: the store's, or one of the site's alternatives ("GTA V / GTA 5").
#[derive(Default)]
struct Name {
    words: Vec<String>,
    /// The words run together, and where each starts in it.
    joined: String,
    starts: Vec<usize>,
    /// The words a name may be said to start at: the first, and after "The" or a possessive,
    /// so "spider man" starts Marvel's Spider-Man.
    leads: Vec<usize>,
}

impl Name {
    fn new(raw: &str, words: Vec<String>) -> Name {
        let mut starts = Vec::with_capacity(words.len());
        let mut joined = String::new();
        for w in &words {
            starts.push(joined.len());
            joined.push_str(w);
        }
        let possessive = raw.split_whitespace().take(2).position(|t| t.ends_with("'s") || t.ends_with("’s"))
            .map(|i| norm(&raw.split_whitespace().take(i + 1).collect::<Vec<_>>().join(" ")).split_whitespace().count());
        let leads = [Some(0), (words[0] == "the").then_some(1), possessive].into_iter().flatten().filter(|i| *i < words.len()).collect();
        Name { words, joined, starts, leads }
    }

    /// 2: the name is what was typed; 1: it starts with it; 0: neither. Numbers match either
    /// way, and the last word typed may be unfinished.
    fn lead(&self, q: &Query) -> u8 {
        let same = |a: &String, b: &String| a == b || other_number(a) == Some(b.as_str());
        let mut best = 0;
        for &at in &self.leads {
            let (words, joined) = (&self.words[at..], &self.joined[self.starts[at]..]);
            let typed = &q.words[..q.words.len().min(words.len())];
            let along = typed.len() == q.words.len() && words.iter().zip(typed).all(|(a, b)| same(a, b));
            if (along && words.len() == typed.len()) || joined == q.joined {
                return 2;
            }
            let (last, before) = q.words.split_last().unwrap_or((&q.joined, &[]));
            let starts = q.words.len() <= words.len()
                && words.iter().zip(before).all(|(a, b)| same(a, b))
                && (same(&words[before.len()], last) || words[before.len()].starts_with(last.as_str()));
            if starts || joined.starts_with(&q.joined) {
                best = 1;
            }
        }
        best
    }

    fn quality(&self, w: &str) -> u8 {
        if self.words.iter().any(|x| x == w || other_number(x) == Some(w)) {
            return WHOLE;
        }
        let letters = w.chars().count();
        let initials = letters >= 2 && (0..self.words.len()).any(|i| initials(w, &self.words[i..]));
        if initials && letters >= 3 {
            WHOLE
        } else if self.starts.iter().any(|&s| self.joined[s..].starts_with(w)) {
            START
        } else if initials {
            SHORT
        } else if self.joined.contains(w) {
            INSIDE
        } else {
            0
        }
    }
}

/// `q` is the initials of `words` from the first: a letter a word, a number whole (either way),
/// and "the", "of" and the like may be left out.
fn initials(q: &str, words: &[String]) -> bool {
    if q.is_empty() {
        return true;
    }
    let Some((first, rest)) = words.split_first() else { return false };
    let initial = &first[..first.chars().next().map_or(0, char::len_utf8)];
    let keys = match other_number(first) {
        Some(n) => [first.as_str(), n],
        None if first.chars().all(|c| c.is_ascii_digit()) => [first.as_str(), first.as_str()],
        None => [initial, initial],
    };
    keys.iter().any(|k| q.strip_prefix(k).is_some_and(|r| initials(r, rest)))
        || (SKIP.contains(&first.as_str()) && initials(q, rest))
}

/// Edit distance (with swapped letters as one edit) from `typed` to the closest start of
/// `word`: "assasin" is one edit from "assassin", the start of "assassins".
fn prefix_distance(typed: &[char], word: &str) -> usize {
    let word: Vec<char> = word.chars().collect();
    let (n, m) = (typed.len(), word.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(typed[i - 1] != word[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && typed[i - 1] == word[j - 2] && typed[i - 2] == word[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n].iter().copied().min().unwrap_or(n)
}

/// What the search looks in for one game, built when the catalog loads.
#[derive(Default)]
pub struct Doc {
    names: Vec<Name>,
    nicknames: Vec<&'static str>,
    /// Title IDs, publisher, region, version: found, after the names.
    other: String,
    genres: Vec<String>,
}

impl Doc {
    /// `name` is the store's name, `site_name` the site's (alternatives split by "/", notes in
    /// brackets); `other` and `genres` are found too, after the names.
    pub fn new(name: &str, site_name: &str, other: &str, genres: &str) -> Doc {
        static NOTES: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\[[^\]]*\]").unwrap());
        let site = NOTES.replace_all(site_name, " ");
        // Alternatives are split by " / " ("Split/Second" is one name), and a piece that's a
        // note ("EN", "v1.01") is no name: typing "en" shouldn't find it as one.
        let is_name = |words: &[String]| words.iter()
            .any(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3 && !NOTE_WORDS.contains(&w.as_str()));
        let mut names: Vec<Name> = Vec::new();
        for (i, raw) in name.split(" / ").chain(site.split(" / ")).enumerate() {
            let words: Vec<String> = norm(raw).split_whitespace().map(String::from).collect();
            if !words.is_empty() && (i == 0 || is_name(&words)) && !names.iter().any(|n| n.words == words) {
                names.push(Name::new(raw, words));
            }
        }
        let nicknames = NICKNAMES.iter().filter(|(_, title)| {
            let title: Vec<&str> = title.split(' ').collect();
            names.iter().any(|n| n.words.windows(title.len()).any(|w| w.iter().zip(&title).all(|(a, b)| a == b)))
        }).map(|(nick, _)| *nick).collect();
        let mut genres: Vec<String> = norm(genres).split_whitespace().map(String::from).collect();
        genres.sort();
        genres.dedup();
        Doc { names, nicknames, other: norm(&format!("{site_name} {other}")), genres }
    }

    fn quality(&self, w: &str) -> u8 {
        if self.nicknames.contains(&w) {
            return WHOLE;
        }
        let mut best = self.names.iter().map(|n| n.quality(w)).max().unwrap_or(0);
        if best == 0 {
            // Letters and a number typed together, each the start of a word: "bf1", "tekken8".
            let parts = split_digits(w);
            let part = |p: &str| if self.nicknames.contains(&p) { WHOLE } else { self.names.iter().map(|n| n.quality(p)).max().unwrap_or(0) };
            if parts.len() > 1 && parts.iter().all(|p| part(p) >= START) {
                best = START;
            }
        }
        if best == 0 && (self.other.contains(w) || (w.chars().count() >= 3 && self.genres.iter().any(|g| g.starts_with(w)))) {
            best = OTHER;
        }
        best
    }

    /// The word of a name `w` is a near spelling of, if any.
    fn near(&self, w: &str) -> Option<&str> {
        let typed: Vec<char> = w.chars().collect();
        let allowed = match typed.len() {
            0..=3 => return None,
            4..=6 => 1,
            _ => 2,
        };
        if typed.iter().any(|c| c.is_ascii_digit()) {
            return None;
        }
        self.names.iter().flat_map(|n| &n.words)
            .map(|x| (prefix_distance(&typed, x), x))
            .filter(|(d, _)| *d <= allowed)
            .min_by_key(|(d, _)| *d)
            .map(|(_, x)| x.as_str())
    }

    /// 2: a name is what was typed; 1: a name starts with it, or with its initials ("cod" is
    /// Call of Duty before Code Vein); 0: neither.
    fn lead(&self, q: &Query) -> u8 {
        let initials = q.joined.chars().count() >= 3 && self.names.iter().any(|n| initials(&q.joined, &n.words));
        self.names.iter().map(|n| n.lead(q)).max().unwrap_or(0).max(u8::from(initials))
    }

    /// How well the game matches, higher is better; None when it doesn't. With `near`, a word
    /// that matches nothing may be a near spelling of a word of a name.
    pub fn score(&self, q: &Query, near: bool) -> Option<u8> {
        if q.words.is_empty() {
            return Some(0);
        }
        let lead = self.lead(q);
        let mut least = WHOLE;
        for w in &q.words {
            let mut x = self.quality(w);
            if x == 0 && near && self.near(w).is_some() {
                x = OTHER;
            }
            if x == 0 && lead == 0 {
                return None;
            }
            least = least.min(x);
        }
        Some(lead * 8 + least)
    }
}

/// "bf1" -> "bf", "1".
fn split_digits(w: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut prev: Option<bool> = None;
    for (i, c) in w.char_indices() {
        let digit = c.is_ascii_digit();
        if prev.is_some_and(|p| p != digit) {
            parts.push(&w[start..i]);
            start = i;
        }
        prev = Some(digit);
    }
    parts.push(&w[start..]);
    parts
}

/// What was typed, ready to match.
pub struct Query {
    words: Vec<String>,
    joined: String,
}

impl Query {
    pub fn new(text: &str) -> Query {
        let words: Vec<String> = norm(text).split_whitespace().map(String::from).collect();
        Query { joined: words.concat(), words }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// What was typed, each word that matched nothing in `doc` replaced by its near spelling:
    /// what a search that only found near spellings searched for.
    pub fn corrected(&self, doc: &Doc) -> String {
        self.words.iter().map(|w| if doc.quality(w) > 0 { w.as_str() } else { doc.near(w).unwrap_or(w) }).collect::<Vec<_>>().join(" ")
    }
}

/// Every game's score for a search (see `Doc::score`).
#[derive(Default)]
pub struct Hits {
    pub scores: Vec<Option<u8>>,
    /// Nothing matched what was typed, so these are near spellings.
    pub near: bool,
}

pub fn run<'a>(q: &Query, docs: impl Iterator<Item = &'a Doc> + Clone) -> Hits {
    let scores: Vec<Option<u8>> = docs.clone().map(|d| d.score(q, false)).collect();
    if q.is_empty() || scores.iter().any(Option::is_some) {
        return Hits { scores, near: false };
    }
    Hits { scores: docs.map(|d| d.score(q, true)).collect(), near: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real names from the catalogs (store name, site name), with look-alikes that used to come
    /// first or match by accident.
    const GAMES: [(&str, &str); 29] = [
        ("Code Vein II", "CODE VEIN II"),
        ("Code Violet", "Code Violet"),
        ("Call of Duty: Black Ops 6", "Call of Duty: Black Ops 6"),
        ("Just Shapes & Beats", "Just Shapes & Beats"),
        ("eFootball 2026", "eFootball PES 2026 Monster Patch [RUS / v7.04]"),
        ("Marvel's Spider-Man 2", "Marvel's Spider-Man 2"),
        ("Marvel's Spider-Man: Miles Morales", "Marvel's Spider-Man: Miles Morales"),
        ("Final Fantasy VII Remake", "Final Fantasy VII Remake Intergrade"),
        ("Crisis Core: Final Fantasy VII Reunion", "Crisis Core: Final Fantasy VII Reunion"),
        ("Final Fantasy XVI", "Final Fantasy XVI"),
        ("Resident Evil 4", "Resident Evil 4 (Remake) (Gold Edition) / BIOHAZARD RE:4 Gold Edition"),
        ("Resident Evil Village", "Resident Evil Village"),
        ("The Last of Us Part I", "The Last of Us Part I : Deluxe Edition[RUS tex / RUS sound / EUR / v02.002.000 / Retail / FPKG]"),
        ("The Last of Us Part II Remastered", "The Last of Us Part II (Remastered)"),
        ("God of War Ragnarök", "God of War Ragnarök (Digital Deluxe Edition)"),
        ("Godfall", "Godfall"),
        ("The Witcher 3: Wild Hunt", "The Witcher 3: Wild Hunt - Complete Edition"),
        ("Assassin's Creed Mirage", "Assassin's Creed Mirage"),
        ("Ratchet & Clank: Rift Apart", "Ratchet & Clank: Rift Apart"),
        ("TEKKEN 8", "TEKKEN 8 (Ultimate Edition)"),
        ("NBA 2K25", "NBA 2K25"),
        ("Grand Theft Auto V", "Grand Theft Auto V / Grand Theft Auto 5 / GTA V / GTA 5"),
        ("Battlefield 1", "Battlefield 1 Revolution"),
        ("Gran Turismo 7", "Gran Turismo 7"),
        ("Need for Speed Unbound", "Need for Speed UnboundNFS Unbound"),
        ("Astro Bot", "Astro Bot (Digital Deluxe Edition¹)"),
        ("Patapon Remastered", "Patapon Remastered / EN / v1.01"),
        ("Enslaved: Odyssey to the West", "Enslaved: Odyssey to the West"),
        ("Split/Second", "Split/Second: Velocity"),
    ];

    fn docs() -> Vec<Doc> {
        GAMES.iter().map(|(name, site)| {
            let genres = if name.contains("Gran Turismo") || name.contains("Need for Speed") { "Racing, Simulation" } else { "Action" };
            Doc::new(name, site, "", genres)
        }).collect()
    }

    /// The games found, best first (equal scores in catalog order, as the chosen sort would).
    fn find(text: &str) -> (Vec<&'static str>, bool) {
        let docs = docs();
        let q = Query::new(text);
        let hits = run(&q, docs.iter());
        let mut found: Vec<(usize, u8)> = hits.scores.iter().enumerate().filter_map(|(i, s)| s.map(|s| (i, s))).collect();
        found.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
        (found.into_iter().map(|(i, _)| GAMES[i].0).collect(), hits.near)
    }

    fn first(text: &str) -> &'static str {
        find(text).0.first().copied().unwrap_or("(nothing)")
    }

    #[test]
    fn words_together_or_apart() {
        assert_eq!(first("spiderman"), "Marvel's Spider-Man 2");
        assert_eq!(first("spider man"), "Marvel's Spider-Man 2");
        assert_eq!(first("witcher3"), "The Witcher 3: Wild Hunt");
        assert_eq!(first("tekken8"), "TEKKEN 8");
        assert_eq!(first("nba2k"), "NBA 2K25");
    }

    #[test]
    fn numbers_either_way() {
        // Final Fantasy VII starts with it; Crisis Core has it later on.
        assert_eq!(find("final fantasy 7").0, ["Final Fantasy VII Remake", "Crisis Core: Final Fantasy VII Reunion"]);
        assert_eq!(first("final fantasy vii"), "Final Fantasy VII Remake");
        assert_eq!(first("final fantasy 16"), "Final Fantasy XVI");
        assert_eq!(first("gta 5"), "Grand Theft Auto V");
    }

    #[test]
    fn initials_and_nicknames() {
        assert_eq!(first("ff7"), "Final Fantasy VII Remake");
        assert_eq!(first("ffxvi"), "Final Fantasy XVI");
        assert_eq!(first("re4"), "Resident Evil 4");
        assert_eq!(first("tlou"), "The Last of Us Part I");
        assert_eq!(first("tlou2"), "The Last of Us Part II Remastered");
        assert_eq!(first("gow"), "God of War Ragnarök");
        assert_eq!(first("gt7"), "Gran Turismo 7");
        assert_eq!(first("nfs"), "Need for Speed Unbound");
        assert_eq!(first("spidey"), "Marvel's Spider-Man 2");
        assert_eq!(first("bf1"), "Battlefield 1");
        // Call of Duty's initials come before names that merely start with "cod".
        assert_eq!(find("cod").0[..3], ["Call of Duty: Black Ops 6", "Code Vein II", "Code Violet"]);
    }

    #[test]
    fn best_match_first() {
        // A whole word before the same letters inside another word.
        assert_eq!(find("pes").0, ["eFootball 2026", "Just Shapes & Beats"]);
        // A name that starts with what was typed before one that only contains it.
        assert_eq!(first("god"), "God of War Ragnarök");
        assert_eq!(first("god of w"), "God of War Ragnarök");
        assert_eq!(first("astro bot"), "Astro Bot");
    }

    #[test]
    fn notes_between_slashes_are_not_names() {
        // "EN" in "Patapon Remastered / EN / v1.01" isn't a name the game is called.
        assert_eq!(first("en"), "Enslaved: Odyssey to the West");
        // A slash inside a name keeps it whole.
        assert_eq!(first("splitsecond"), "Split/Second");
        assert_eq!(first("gta 5"), "Grand Theft Auto V");
    }

    #[test]
    fn genres_after_names() {
        assert_eq!(find("racing").0, ["Gran Turismo 7", "Need for Speed Unbound"]);
        // Too short to be a genre: "ac" isn't "Action".
        assert!(!find("ac").0.contains(&"TEKKEN 8"));
    }

    #[test]
    fn typos_find_near_spellings_only_when_nothing_matches() {
        assert_eq!(find("assasins creed"), (vec!["Assassin's Creed Mirage"], true));
        assert_eq!(find("rachet"), (vec!["Ratchet & Clank: Rift Apart"], true));
        assert_eq!(find("resident evli").0, ["Resident Evil 4", "Resident Evil Village"]);
        assert!(!find("creed").1);
        // Nothing near either: still nothing.
        assert!(find("zzzz qqqq").0.is_empty());
        let docs = docs();
        let mirage = &docs[GAMES.iter().position(|g| g.0 == "Assassin's Creed Mirage").unwrap()];
        assert_eq!(Query::new("Assasins creed").corrected(mirage), "assassins creed");
    }

    #[test]
    fn helpers() {
        assert_eq!(split_digits("bf1"), ["bf", "1"]);
        assert_eq!(split_digits("nba2k25"), ["nba", "2", "k", "25"]);
        assert_eq!(prefix_distance(&"assasin".chars().collect::<Vec<_>>(), "assassins"), 1);
        assert_eq!(prefix_distance(&"evli".chars().collect::<Vec<_>>(), "evil"), 1);
        assert_eq!((other_number("vii"), other_number("16")), (Some("7"), Some("xvi")));
    }
}
