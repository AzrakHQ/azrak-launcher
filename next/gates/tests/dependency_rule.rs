//! The dependency rule, enforced from `cargo metadata`:
//!   kernel            -> no workspace crate
//!   extensions/*      -> kernel only (dev-dependencies may use the test kit and engines)
//!   engines, application, ui-model, host, testkit -> never an extension, never a shell
//!   only the `launcher` binary may depend on extensions
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

struct Pkg {
    name: String,
    layer: &'static str,
    deps: Vec<(String, Option<String>)>,
}

fn packages() -> Vec<Pkg> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .current_dir(root())
        .output()
        .expect("cargo metadata");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ws: Vec<String> = v["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    v["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let manifest = p["manifest_path"].as_str().unwrap();
            let rel = manifest
                .strip_prefix(root().to_str().unwrap())
                .unwrap()
                .trim_start_matches('/');
            let layer = if rel.starts_with("crates/kernel/") {
                "kernel"
            } else if rel.starts_with("extensions/") {
                "extension"
            } else if rel.starts_with("shells/") {
                "shell"
            } else if rel.starts_with("gates/") {
                "gates"
            } else {
                "internal"
            };
            let deps = p["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| ws.contains(&d["name"].as_str().unwrap().to_string()))
                .map(|d| {
                    (
                        d["name"].as_str().unwrap().to_string(),
                        d["kind"].as_str().map(String::from),
                    )
                })
                .collect();
            Pkg {
                name: p["name"].as_str().unwrap().to_string(),
                layer,
                deps,
            }
        })
        .collect()
}

fn violations(pkgs: &[Pkg]) -> Vec<String> {
    let layer: BTreeMap<_, _> = pkgs.iter().map(|p| (p.name.clone(), p.layer)).collect();
    let mut out = vec![];
    for p in pkgs {
        for (dep, kind) in &p.deps {
            let normal = kind.is_none();
            let dl = layer[dep];
            let bad = match p.layer {
                "kernel" => true,
                "extension" => normal && dep != "kernel",
                "internal" => dl == "extension" || dl == "shell",
                _ => false,
            };
            if bad {
                out.push(format!(
                    "{} ({}) must not depend on {} ({})",
                    p.name, p.layer, dep, dl
                ));
            }
        }
    }
    out
}

#[test]
fn dependency_rule_holds() {
    let v = violations(&packages());
    assert!(v.is_empty(), "dependency rule violated:\n{}", v.join("\n"));
}

#[test]
fn rule_catches_each_kind_of_violation() {
    let mk = |name: &str, layer: &'static str, deps: &[(&str, Option<&str>)]| Pkg {
        name: name.into(),
        layer,
        deps: deps
            .iter()
            .map(|(d, k)| (d.to_string(), k.map(String::from)))
            .collect(),
    };
    let base = || {
        vec![
            mk("kernel", "kernel", &[]),
            mk("engines", "internal", &[("kernel", None)]),
            mk("ext", "extension", &[("kernel", None)]),
            mk("app", "shell", &[]),
        ]
    };
    assert!(violations(&base()).is_empty());
    let mut v = base();
    v[0].deps.push(("engines".into(), None));
    assert_eq!(
        violations(&v).len(),
        1,
        "kernel depending on a workspace crate"
    );
    let mut v = base();
    v[1].deps.push(("ext".into(), None));
    assert_eq!(violations(&v).len(), 1, "engines depending on an extension");
    let mut v = base();
    v[2].deps.push(("engines".into(), None));
    assert_eq!(violations(&v).len(), 1, "extension depending on engines");
    let mut v = base();
    v[2].deps.push(("engines".into(), Some("dev".into())));
    assert!(violations(&v).is_empty(), "dev-dependencies are allowed");
}
