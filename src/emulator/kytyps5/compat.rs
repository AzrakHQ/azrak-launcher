//! KytyPS5's community compatibility list (kytyps5.github.io) and its "Game Emulation Status
//! Report" form. Same data KytyPS5's own launcher uses.

use crate::compat::{log_excerpt, system_info, Db, Entry, Report, Source, Status, THIS_OS};

const URL: &str = "https://kytyps5.github.io/data/compatibility.json";
const PAGE: &str = "https://kytyps5.github.io/";
/// Reports there feed the list above.
const REPORT_FORM: &str = "https://github.com/KytyPS5/KytyPS5/issues/new?template=kytyps5-game-emulation.yaml";

pub const SOURCE: Source = Source { url: URL, cache: "compatibility.json", page: PAGE, parse, report };

fn parse(bytes: &[u8]) -> Option<Db> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut db = Db::new();
    for (tid, e) in v.as_object()? {
        let Some(best) = e["status"].as_str().and_then(Status::parse) else { continue };
        let plat = e["platforms"].as_object();
        let mut platforms: Vec<String> = plat
            .map(|p| {
                p.keys()
                    .map(|k| match k.as_str() {
                        "linux" => "Linux".into(),
                        "windows" => "Windows".into(),
                        "macos" => "macOS".into(),
                        o => o.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        platforms.sort();
        let on = |os: &str| plat.and_then(|p| p.get(os)).and_then(|l| l["status"].as_str()).and_then(Status::parse);
        let linux = on("linux");
        db.insert(
            tid.to_uppercase(),
            Entry {
                status: linux.unwrap_or(best),
                on_linux: linux.is_some(),
                mine: false,
                linux,
                windows: on("windows"),
                reports: e["reports"].as_u64().unwrap_or(1),
                version: plat
                    .and_then(|p| p.values().filter_map(|x| x["version"].as_str()).max_by_key(|s| s.len()).map(String::from))
                    .unwrap_or_default(),
                platforms,
            },
        );
    }
    Some(db)
}


fn report(r: &Report) -> String {
    let q = crate::util::query_escape;
    let mut url = format!("{REPORT_FORM}&title={}", q(&format!("[GAME STATUS]: {} ({})", r.name, THIS_OS.to_lowercase())));
    let sys = system_info();
    let log = log_excerpt(r.log);
    let details = format!("{} on {THIS_OS}.", r.status.meaning());
    for (field, value) in [
        ("game-title", r.name),
        ("game-id", r.title_id),
        ("kyty-version", r.emulator_version),
        ("compatibility-status", r.status.form_label()),
        ("what-happened", &details),
        ("steps-to-reproduce", "Start the game with KytyPS5."),
        ("expected-behavior", "The game plays normally."),
        ("os", &sys.os),
        ("cpu", &sys.cpu),
        ("gpu", &sys.gpu),
        ("ram-vram", &sys.ram),
        ("log-file", &log),
    ] {
        if !value.is_empty() {
            url.push_str(&format!("&{field}={}", q(value)));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::with_mine;

    fn report_url_for(name: &str, title_id: &str, emulator_version: &str, game_version: &str, status: Status, log: &str) -> String {
        report(&Report { name, title_id, emulator_version, game_version, status, log })
    }

    #[test]
    fn parses_feed() {
        let json = br#"{"PPSA02929":{"status":"InGame","reports":2,"platforms":{"windows":{"status":"InGame","version":"0.2.2 (KytyPS5-2026-08-18-6bf6929)"},"linux":{"status":"MainMenu","version":"v0.3.0"}}},"PPSA1":{"status":"Weird"}}"#;
        let db = parse(json).unwrap();
        assert_eq!(db.len(), 1);
        let e = &db["PPSA02929"];
        // Linux wins over a better Windows result.
        assert_eq!(e.status, Status::MainMenu);
        assert!(e.on_linux);
        assert_eq!(e.windows, Some(Status::InGame));
        assert_eq!(e.tag(), "Menus");
        assert_eq!(e.platforms, vec!["Linux", "Windows"]);
        assert_eq!(e.tested_date(), "2026-08-18");
        assert!(Status::InGame < Status::DoesntBoot);
    }
    #[test]
    fn windows_only_results_are_marked() {
        let json = br#"{"PPSA1":{"status":"InGame","platforms":{"windows":{"status":"InGame"}}},"PPSA2":{"status":"Logo"}}"#;
        let db = parse(json).unwrap();
        let w = &db["PPSA1"];
        assert!(!w.on_linux);
        assert_eq!(w.tag(), "Win · In-game");
        assert_eq!(w.chip_text(), "In-game on Windows · untested on Linux");
        assert_eq!(db["PPSA2"].tag(), "Other OS · Boots");
    }
    #[test]
    fn your_result_wins_on_this_pc() {
        let json = br#"{"PPSA1":{"status":"InGame","platforms":{"windows":{"status":"InGame"}}}}"#;
        let mut mine = crate::results::Results::default();
        mine.rate("PPSA1", "One", Status::Logo, "b", 1.0, "");
        mine.rate("PPSA9", "Nine", Status::InGame, "b", 1.0, "");
        let db = with_mine(parse(json).unwrap(), &mine);
        let e = &db["PPSA1"];
        assert!(e.mine && e.on_linux);
        assert_eq!(e.status, Status::Logo);
        assert_eq!(e.windows, Some(Status::InGame), "the community's results stay visible");
        assert_eq!(e.chip_text(), "Boots on Linux · your result");
        assert_eq!(db["PPSA9"].tag(), "In-game");
        assert!(e.in_game_anywhere(), "Windows says in-game, even though your result is Boots");
    }
    #[test]
    fn in_game_anywhere_checks_every_os() {
        let json = br#"{"A":{"status":"InGame","platforms":{"windows":{"status":"InGame"},"linux":{"status":"Logo"}}},"B":{"status":"MainMenu","platforms":{"linux":{"status":"MainMenu"}}},"C":{"status":"InGame"}}"#;
        let db = parse(json).unwrap();
        assert!(db["A"].in_game_anywhere() && db["A"].status == Status::Logo);
        assert!(!db["B"].in_game_anywhere());
        assert!(db["C"].in_game_anywhere());
    }
    #[test]
    fn report_url_prefills_the_form() {
        let url = report_url_for("Dreaming Sarah", "PPSA02929", "KytyPS5-2026-09-30-b7a1fac", "", Status::DoesntBoot, "boot\n\nCould not find suitable device\n");
        assert!(url.starts_with(REPORT_FORM));
        assert!(url.contains("&game-id=PPSA02929"));
        assert!(url.contains("&game-title=Dreaming%20Sarah"));
        assert!(url.contains("&kyty-version=KytyPS5%2D2026%2D09%2D30%2Db7a1fac"));
        assert!(url.contains("&compatibility-status=Doesn%27t%20boot"));
        assert!(url.contains("&log-file="));
        assert!(url.contains("Could%20not%20find%20suitable%20device"));
        assert!(report_url_for("A", "B", "C", "", Status::InGame, "").find("log-file").is_none());
        let long = "x".repeat(200) + "\n";
        assert!(log_excerpt(&long.repeat(100)).len() < 2600);
        let crash = format!("{}--- Build ---\nOfficial build X\n--- Error ---\nCould not find suitable device\n{}", "boot line\n".repeat(500), "after\n".repeat(3));
        let e = log_excerpt(&crash);
        assert!(e.starts_with("The error from the log") && e.contains("--- Error ---\nCould not find suitable device"));
        assert!(!e.contains("boot line"));
    }
}
