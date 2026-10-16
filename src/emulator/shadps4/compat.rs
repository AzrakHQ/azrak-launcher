//! shadPS4's community compatibility list for PS4 games (one file per update) and its game
//! compatibility report form.

use crate::compat::{log_excerpt, system_info, Db, Entry, Report, Source, Status, THIS_OS};

const URL: &str = "https://github.com/shadps4-compatibility/shadps4-game-compatibility/releases/latest/download/compatibility_data.json";
const PAGE: &str = "https://github.com/shadps4-compatibility/shadps4-game-compatibility/issues";
const REPORT_FORM: &str = "https://github.com/shadps4-compatibility/shadps4-game-compatibility/issues/new?template=game_compatibility.yml";

pub const SOURCE: Source = Source { url: URL, cache: "compatibility-shadps4.json", page: PAGE, parse: parse_shad, report };

fn shad_status(s: &str) -> Option<Status> {
    Some(match s {
        "status-playable" | "status-ingame" => Status::InGame,
        "status-menus" => Status::MainMenu,
        "status-boots" => Status::Logo,
        "status-nothing" => Status::DoesntBoot,
        _ => return None,
    })
}

/// shadPS4's list: { "CUSA04118": { "os-linux": { "status": "status-ingame", "last_tested": … }, … } }.
fn parse_shad(bytes: &[u8]) -> Option<Db> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut db = Db::new();
    for (tid, per_os) in v.as_object()? {
        let Some(per_os) = per_os.as_object() else { continue };
        let on = |os: &str| per_os.get(os).and_then(|e| e["status"].as_str()).and_then(shad_status);
        let Some(best) = per_os.values().filter_map(|e| e["status"].as_str().and_then(shad_status)).min() else { continue };
        let linux = on("os-linux");
        let mut platforms: Vec<String> = per_os.keys().filter_map(|k| match k.as_str() {
            "os-linux" => Some("Linux".to_string()),
            "os-windows" => Some("Windows".to_string()),
            "os-macOS" => Some("macOS".to_string()),
            _ => None,
        }).collect();
        platforms.sort();
        // The Linux report's test date, else the newest one.
        let tested = per_os.get("os-linux").and_then(|e| e["last_tested"].as_str())
            .or_else(|| per_os.values().filter_map(|e| e["last_tested"].as_str()).max())
            .unwrap_or_default().to_string();
        db.insert(tid.to_uppercase(), Entry {
            status: linux.unwrap_or(best),
            on_linux: linux.is_some(),
            mine: false,
            linux,
            windows: on("os-windows"),
            reports: per_os.len() as u64,
            version: tested,
            platforms,
        });
    }
    Some(db)
}


fn report(r: &Report) -> String {
    let q = crate::util::query_escape;
    let sys = system_info();
    let label = match r.status {
        Status::InGame => "Ingame",
        Status::MainMenu => "Menus",
        Status::Logo => "Boots",
        Status::DoesntBoot => "Nothing",
    };
    let details = format!("{} on {THIS_OS}.", r.status.meaning());
    let log = log_excerpt(r.log);
    let mut url = format!("{REPORT_FORM}&title={}", q(&format!("{} - {}", r.title_id, r.name)));
    for (field, value) in [
        ("game-name", r.name), ("game-serial", r.title_id), ("game-version", r.game_version),
        ("emulator-version", r.emulator_version), ("emulation-status", label), ("Operating-System", THIS_OS),
        ("processor", &sys.cpu), ("graphics-card", &sys.gpu), ("emulation-description", &details), ("log", &log),
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

    fn report_url_for(name: &str, title_id: &str, emulator_version: &str, game_version: &str, status: Status, log: &str) -> String {
        report(&Report { name, title_id, emulator_version, game_version, status, log })
    }

    #[test]
    fn shadps4_list_and_ps4_report() {
        let json = br#"{"CUSA04118":{"os-linux":{"status":"status-ingame","last_tested":"2026-10-01T22:30:38Z"},"os-windows":{"status":"status-playable","last_tested":"2026-09-01T00:00:00Z"}},
            "CUSA00001":{"os-windows":{"status":"status-boots","last_tested":"2026-09-02T00:00:00Z"}},"CUSA00002":{"os-linux":{"status":"status-unknown"}}}"#;
        let db = parse_shad(json).unwrap();
        let fw = &db["CUSA04118"];
        assert_eq!((fw.status, fw.on_linux, fw.windows), (Status::InGame, true, Some(Status::InGame)));
        assert_eq!(fw.tested_date(), "2026-10-01");
        assert_eq!(fw.platforms, vec!["Linux", "Windows"]);
        assert_eq!(db["CUSA00001"].tag(), "Win · Boots");
        assert!(!db.contains_key("CUSA00002"), "unknown statuses are skipped");
        let url = report_url_for("Firewatch", "CUSA04118", "0.18.0", "01.08", Status::MainMenu, "");
        assert!(url.starts_with(REPORT_FORM));
        assert!(url.contains("&game-serial=CUSA04118") && url.contains("&emulation-status=Menus") && url.contains("&game-version=01%2E08"));
        assert!(url.contains(&format!("&Operating-System={THIS_OS}")), "the report names the system it comes from");
        assert_eq!(crate::compat::list_page(crate::platform::Platform::Ps4), PAGE);
    }
}
