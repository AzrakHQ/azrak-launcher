//! Vocabulary lint: no console, emulator or store names outside the extension that owns them.
//! Scans kernel, engines, application, ui-model, host, testkit, shells and any UI files.
use std::fs;
use std::path::{Path, PathBuf};

const DENY: &[&str] = &[
    "ps5",
    "ps4",
    "ps3",
    "playstation",
    "kyty",
    "shadps4",
    "rutracker",
    "nintendo",
    "xbox",
];
const SCANNED_DIRS: &[&str] = &["crates", "shells", "ui"];
const EXT: &[&str] = &["rs", "slint", "toml", "yml", "yaml", "md", "json"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else if p
            .extension()
            .map(|x| EXT.contains(&x.to_string_lossy().as_ref()))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

pub fn offences(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    DENY.iter().copied().filter(|w| lower.contains(w)).collect()
}

#[test]
fn no_vocabulary_outside_extensions() {
    let mut files = vec![];
    for d in SCANNED_DIRS {
        walk(&root().join(d), &mut files);
    }
    let mut bad = vec![];
    for f in files {
        let Ok(text) = fs::read_to_string(&f) else {
            continue;
        };
        let hits = offences(&text);
        if !hits.is_empty() {
            bad.push(format!(
                "{}: {:?}",
                f.strip_prefix(root()).unwrap().display(),
                hits
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "vocabulary lint failed:\n{}",
        bad.join("\n")
    );
}

#[test]
fn lint_catches_a_leak() {
    assert_eq!(offences("let s = \"PS5 games\";"), vec!["ps5"]);
    assert!(offences("Console A and Runner One").is_empty());
}
