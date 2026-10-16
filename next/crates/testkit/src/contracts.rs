//! Contract suites: run against every registered extension.
use kernel::*;
use std::collections::BTreeSet;

pub fn check_registry(reg: &Registry) {
    let mut ids = BTreeSet::new();
    for s in reg.systems() {
        let d = s.descriptor();
        assert!(ids.insert(format!("system:{}", d.id)), "duplicate system id");
        assert!(!d.id.as_str().is_empty() && !d.folder.is_empty(), "system id and folder must be set");
        assert!(!d.folder.contains('/') && !d.folder.contains(".."), "folder must be a single safe name");
        assert!(is_hex_color(&d.style.color), "style colour must be #rrggbb: {}", d.style.color);
        assert!(d.style.cover.w > 0 && d.style.cover.h > 0);
        check_recognise_total(s.as_ref());
    }
    for r in reg.runners() {
        let d = r.descriptor();
        assert!(ids.insert(format!("runner:{}", d.id)), "duplicate runner id");
        assert!(!d.systems.is_empty(), "runner {} runs no system", d.id);
        let mut keys = BTreeSet::new();
        for s in &d.settings {
            assert!(keys.insert(s.key.clone()), "duplicate setting key {}", s.key);
            assert!(s.validate(&s.default).is_ok(), "default of {} must validate", s.key);
        }
        check_plan_pure(r.as_ref());
    }
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// `recognise` must never panic, whatever the tree holds.
pub fn check_recognise_total(s: &dyn System) {
    let empty = crate::MemTree::new("empty");
    let _ = s.recognise(&empty);
    let odd = crate::MemTree::new("odd")
        .with("a/b/c/d/e/f", "x")
        .with("é/ü", "")
        .with_entry(Entry { path: RelPath::new("link").unwrap(), size: 0, kind: EntryKind::Symlink });
    let _ = s.recognise(&odd);
}

/// `plan` is pure: same inputs, same output, and defaults alone are enough to plan.
pub fn check_plan_pure(r: &dyn Runner) {
    if r.descriptor().launch.is_none() { return; }
    let settings = effective(&r.descriptor().settings, &Settings::new());
    let game = InstalledGame {
        game: GameRef { system: r.descriptor().systems[0].clone(), key: GameKey::new("k") },
        name: "n".into(), root: "/games/k".into(), extra: serde_json::Value::Null,
    };
    let a = r.plan(std::path::Path::new("/r/exe"), &game, &settings);
    let b = r.plan(std::path::Path::new("/r/exe"), &game, &settings);
    assert_eq!(a.is_ok(), b.is_ok());
    if let (Ok(a), Ok(b)) = (a, b) { assert_eq!(a, b); } else { panic!("default settings must be enough to plan"); }
}
