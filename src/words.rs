//! Players' words for technical things: what's on the main screens says what it means for the
//! player ("Getting PS4 support · 9%"), and the numbers and names behind it stay in Settings and
//! the Game Hub's technical details.

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
