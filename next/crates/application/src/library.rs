use engines::install::GAME_FILE;
use kernel::*;
use std::fs;
use std::path::Path;

/// Installed games of registered systems. Folders of systems nobody registered are left alone.
pub fn scan(reg: &Registry, games_dir: &Path) -> Vec<InstalledGame> {
    let mut out = vec![];
    for s in reg.systems() {
        let folder = games_dir.join(&s.descriptor().folder);
        let Ok(rd) = fs::read_dir(&folder) else {
            continue;
        };
        let mut dirs: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            let Ok(bytes) = fs::read(dir.join(GAME_FILE)) else {
                continue;
            };
            let Ok(mut g) = serde_json::from_slice::<InstalledGame>(&bytes) else {
                continue;
            };
            // Trust the location over the file: a moved library still resolves.
            g.root = dir;
            if g.game.system == s.descriptor().id {
                out.push(g);
            }
        }
    }
    out
}

pub struct Kept {
    pub folder: String,
    pub games: usize,
}

/// Folders under the games directory that no installed system owns (their plugin is absent). Never deleted.
pub fn kept_for_absent_systems(reg: &Registry, games_dir: &Path) -> Vec<Kept> {
    let owned: Vec<String> = reg
        .systems()
        .iter()
        .map(|s| s.descriptor().folder.clone())
        .collect();
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(games_dir) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !e.path().is_dir() || name.starts_with('.') || owned.contains(&name) {
            continue;
        }
        let games = fs::read_dir(e.path())
            .map(|r| {
                r.flatten()
                    .filter(|x| x.path().join(GAME_FILE).exists())
                    .count()
            })
            .unwrap_or(0);
        out.push(Kept {
            folder: name,
            games,
        });
    }
    out.sort_by(|a, b| a.folder.cmp(&b.folder));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use engines::install::{install, Limits};
    use testkit::{toy, ToyPlugin};

    #[test]
    fn scan_finds_installed_and_removal_keeps_files() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("alpha");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(toy::MARKER), "Alpha").unwrap();
        let games = d.path().join("games");
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        install(&reg, &src, &games, &Limits::default(), &CancelToken::new()).unwrap();
        let found = scan(&reg, &games);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Alpha");

        // The plugin is removed: nothing is listed, nothing is deleted, and the folder is reported as kept.
        let none = Registry::build(&[]).unwrap();
        assert!(scan(&none, &games).is_empty());
        let kept = kept_for_absent_systems(&none, &games);
        assert_eq!((kept[0].folder.as_str(), kept[0].games), ("toy", 1));
        assert!(games.join("toy/alpha").join(GAME_FILE).exists());
        assert!(kept_for_absent_systems(&reg, &games).is_empty());
    }
}
