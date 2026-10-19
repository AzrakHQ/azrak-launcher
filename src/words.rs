//! Players' words for technical things: what's on the main screens says what it means for the
//! player ("Getting PS4 support · 9%"), and the numbers and names behind it stay in Settings and
//! the Game Hub's technical details.

/// How easy a game is to download, from how many people were sharing it (seeders) when the
/// catalog counted them. The counts aren't live, so this is a hint, not a promise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Availability {
    /// Several people share it: it should download steadily.
    Good,
    /// One or two: it may download slowly, or stall when they're away.
    Low,
    /// Nobody was sharing it.
    None,
    /// The catalog has no count for it.
    Unknown,
}

impl Availability {
    pub fn of(seeders: Option<u64>) -> Availability {
        match seeders {
            None => Availability::Unknown,
            Some(0) => Availability::None,
            Some(1..=2) => Availability::Low,
            Some(_) => Availability::Good,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Availability::Good => "Good",
            Availability::Low => "Low",
            Availability::None => "None",
            Availability::Unknown => "Unknown",
        }
    }

    /// 1 good (green), 2 low (amber), 4 none or unknown (grey): the Fact colours.
    pub fn kind(self) -> i32 {
        match self {
            Availability::Good => 1,
            Availability::Low => 2,
            Availability::None | Availability::Unknown => 4,
        }
    }
}

/// The status line under the Home screen, in players' words. Progress messages name emulators
/// and versions ("Downloading shadPS4 0.20.0 · 9%"), which Settings → Updates still shows; on
/// the Home screen it's what they're for that matters.
pub fn status(text: &str) -> String {
    let percent = |done: &str| -> Option<u64> {
        let (a, b) = done.split_once('/')?;
        let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        (b > 0.0).then(|| (a / b * 100.0).clamp(0.0, 100.0).floor() as u64)
    };
    // "… · 9%" at the end of a progress message.
    let tail = text.rsplit_once(" · ").map(|(_, p)| p).filter(|p| p.ends_with('%')).map(|p| format!(" · {p}")).unwrap_or_default();
    let support = |console: &str| {
        if text.starts_with("Installing") { format!("Setting up {console} support…") } else { format!("Getting {console} support{tail}") }
    };
    if text.starts_with("Downloading shadPS4") || text.starts_with("Installing shadPS4") {
        return support("PS4");
    }
    if text.starts_with("Downloading KytyPS5") || text.starts_with("Installing KytyPS5") {
        return support("PS5");
    }
    if text.starts_with("Downloading Azrak Launcher") {
        return format!("Getting the launcher's update{tail}");
    }
    if let Some(done) = text.strip_prefix("Downloading artwork ").or_else(|| text.strip_prefix("Downloading game art ")) {
        return match percent(done) {
            Some(p) => format!("Getting game pictures · {p}%"),
            None => "Getting game pictures…".into(),
        };
    }
    if text.starts_with("Loading RuTracker") || (text.starts_with("Loaded ") && text.ends_with("RuTracker releases")) {
        return "Loading the game list…".into();
    }
    if let Some(console) = text.strip_prefix("Downloading the ").and_then(|t| t.strip_suffix(" catalog")) {
        return format!("Updating the {console} game list…");
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_from_seeders() {
        assert_eq!(Availability::of(Some(46)), Availability::Good);
        assert_eq!(Availability::of(Some(3)), Availability::Good);
        assert_eq!(Availability::of(Some(2)), Availability::Low);
        assert_eq!(Availability::of(Some(1)), Availability::Low);
        assert_eq!(Availability::of(Some(0)), Availability::None);
        assert_eq!(Availability::of(None), Availability::Unknown, "a missing count is unknown, not zero");
        assert_eq!((Availability::Good.kind(), Availability::Low.kind(), Availability::None.kind()), (1, 2, 4));
    }

    #[test]
    fn status_line_in_players_words() {
        assert_eq!(status("Downloading shadPS4 0.20.0 · 9%"), "Getting PS4 support · 9%");
        assert_eq!(status("Installing shadPS4…"), "Setting up PS4 support…");
        assert_eq!(status("Downloading KytyPS5 2026-10-19 · 40%"), "Getting PS5 support · 40%");
        assert_eq!(status("Downloading artwork 1382/3602"), "Getting game pictures · 38%");
        assert_eq!(status("Downloading game art 10/20"), "Getting game pictures · 50%");
        assert_eq!(status("Downloading the PS4 catalog"), "Updating the PS4 game list…");
        assert_eq!(status("Downloading Azrak Launcher 1.18.0 · 5%"), "Getting the launcher's update · 5%");
        assert_eq!(status("Checking for new releases…"), "Checking for new releases…");
        assert_eq!(status(""), "");
    }
}
