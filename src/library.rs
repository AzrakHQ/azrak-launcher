//! Installed games: folders containing sce_sys/param.json (PS5) or sce_sys/param.sfo (PS4), or
//! the installer's manifest (PC games).

use crate::platform::Platform;
use crate::util::sha1_hex;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct LocalGame {
    pub id: String,
    pub name: String,
    pub title_id: String,
    pub content_id: String,
    pub version: String,
    pub path: PathBuf,
    pub icon0: Option<PathBuf>,
    pub pic0: Option<PathBuf>,
    pub platform: Platform,
    /// PC games: what Play starts, from the installer's manifest.
    pub pc: Option<crate::pcgame::Manifest>,
}

pub fn read_param(dir: &Path) -> Option<LocalGame> {
    if let Some(m) = crate::pcgame::Manifest::read(dir) {
        return Some(LocalGame {
            id: sha1_hex(&dir.to_string_lossy())[..12].to_string(),
            name: m.name.clone(),
            path: dir.to_path_buf(),
            platform: Platform::Pc,
            pc: Some(m),
            ..Default::default()
        });
    }
    let sce = dir.join("sce_sys");
    let Ok(bytes) = std::fs::read(sce.join("param.json")) else { return read_sfo(dir) };
    let p: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let lp = &p["localizedParameters"];
    let lang = lp["defaultLanguage"].as_str().unwrap_or("en-US");
    let tid = p["titleId"].as_str().unwrap_or("").to_string();
    let name = lp[lang]["titleName"]
        .as_str()
        .or(lp["en-US"]["titleName"].as_str())
        .map(String::from)
        .unwrap_or_else(|| if tid.is_empty() { dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { tid.clone() });
    let file = |n: &str| Some(sce.join(n)).filter(|p| p.is_file());
    Some(LocalGame {
        id: sha1_hex(&dir.to_string_lossy())[..12].to_string(),
        name,
        title_id: tid,
        content_id: p["contentId"].as_str().unwrap_or("").to_string(),
        version: p["contentVersion"].as_str().unwrap_or("").to_string(),
        path: dir.to_path_buf(),
        icon0: file("icon0.png"),
        pic0: file("pic0.png"),
        platform: Platform::Ps5,
        pc: None,
    })
}

/// A PS4 game folder. Only full games ("gd" category): updates and add-ons have their own
/// param.sfo but aren't games to list or launch.
fn read_sfo(dir: &Path) -> Option<LocalGame> {
    let sce = dir.join("sce_sys");
    let bytes = std::fs::read(sce.join("param.sfo")).ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    let p = crate::sfo::parse(&bytes)?;
    let get = |k: &str| p.get(k).map(|v| v.text().to_string()).unwrap_or_default();
    if !get("CATEGORY").starts_with("gd") {
        return None;
    }
    let tid = get("TITLE_ID");
    let name = Some(get("TITLE")).filter(|t| !t.is_empty())
        .unwrap_or_else(|| if tid.is_empty() { dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { tid.clone() });
    let file = |n: &str| Some(sce.join(n)).filter(|p| p.is_file());
    Some(LocalGame {
        id: sha1_hex(&dir.to_string_lossy())[..12].to_string(),
        name,
        title_id: tid,
        content_id: get("CONTENT_ID"),
        version: get("APP_VER"),
        path: dir.to_path_buf(),
        icon0: file("icon0.png"),
        pic0: file("pic1.png").or_else(|| file("pic0.png")),
        platform: Platform::Ps4,
        pc: None,
    })
}

/// Scan each folder, its children, and grandchildren (for "Games/<Title>/<PPSAxxxxx-app0>", or a
/// PC game a folder deeper).
pub fn scan(dirs: &[PathBuf]) -> Vec<LocalGame> {
    let mut found: Vec<LocalGame> = Vec::new();
    let mut push = |g: LocalGame| {
        if !found.iter().any(|x| x.id == g.id) {
            found.push(g);
        }
    };
    for root in dirs {
        if let Some(g) = read_param(root) {
            push(g);
        }
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let mut children: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        children.sort();
        for c in children {
            if c.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                continue;
            }
            if let Some(g) = read_param(&c) {
                push(g);
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&c) {
                let game = |p: &Path| p.join("sce_sys").is_dir() || p.join(crate::pcgame::MANIFEST).is_file();
                for gc in rd.flatten().map(|e| e.path()).filter(|p| game(p)) {
                    if let Some(g) = read_param(&gc) {
                        push(g);
                    }
                }
            }
        }
    }
    found.sort_by_key(|g| g.name.to_lowercase());
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfo::{build, Value};

    fn ps4(dir: &Path, category: &str) {
        std::fs::create_dir_all(dir.join("sce_sys")).unwrap();
        std::fs::write(dir.join("sce_sys/param.sfo"), build(&[
            ("CATEGORY", Value::Text(category.into())), ("TITLE", Value::Text("Firewatch".into())),
            ("TITLE_ID", Value::Text("CUSA04118".into())), ("APP_VER", Value::Text("01.08".into())),
        ])).unwrap();
        std::fs::write(dir.join("eboot.bin"), b"x").unwrap();
    }

    #[test]
    fn finds_pc_games_a_folder_deeper() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("Celeste/Celeste-0123456789ab");
        std::fs::create_dir_all(&dir).unwrap();
        let m = crate::pcgame::Manifest { version: 1, name: "Celeste".into(), topic: 5, launch: "start.sh".into(), kind: crate::pcgame::Kind::Script, alternatives: vec![] };
        std::fs::write(dir.join(crate::pcgame::MANIFEST), serde_json::to_vec(&m).unwrap()).unwrap();
        let found = scan(&[t.path().to_path_buf()]);
        assert_eq!(found.iter().map(|g| (g.name.as_str(), g.platform)).collect::<Vec<_>>(), vec![("Celeste", Platform::Pc)]);
    }

    #[test]
    fn finds_ps4_games_but_not_updates() {
        let t = tempfile::tempdir().unwrap();
        ps4(&t.path().join("CUSA04118"), "gd");
        ps4(&t.path().join("CUSA04118-patch"), "gp");
        let found = scan(&[t.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        let g = &found[0];
        assert_eq!((g.platform, g.title_id.as_str(), g.name.as_str(), g.version.as_str()), (Platform::Ps4, "CUSA04118", "Firewatch", "01.08"));
    }
}
