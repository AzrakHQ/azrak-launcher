//! Headless scenarios. They run in every cell of the removal matrix, so each must hold with ANY set of
//! extensions, including none. Feature-specific scenarios are gated on the extension they need.
use launcher::{run, App};
use std::fs;

fn app() -> (tempfile::TempDir, App) {
    let d = tempfile::tempdir().unwrap();
    let app = App::open(d.path().to_path_buf()).unwrap();
    (d, app)
}
fn cli(app: &App, args: &[&str]) -> Result<String, String> {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    run(&a, app, &mut out).map_err(|e| e.to_string())?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn every_build_starts_and_answers_basic_questions() {
    let (_d, app) = app();
    cli(&app, &["plugins"]).unwrap();
    cli(&app, &["systems"]).unwrap();
    assert_eq!(cli(&app, &["list"]).unwrap(), "");
    cli(&app, &["settings"]).unwrap();
    cli(&app, &["catalog-manifest"]).unwrap();
    let diag = cli(&app, &["diagnose"]).unwrap();
    assert!(diag.contains("plugins:"));
    let lib: serde_json::Value =
        serde_json::from_str(&cli(&app, &["ui", "library"]).unwrap()).unwrap();
    let empty = &lib["Library"]["empty"];
    if app.registry.systems().is_empty() {
        assert_eq!(empty, "NoSystems");
    } else {
        assert_eq!(empty, "NoGames");
    }
    cli(&app, &["ui", "settings"]).unwrap();
}

#[test]
fn unrecognised_input_is_a_clean_error_never_a_panic() {
    let (d, app) = app();
    let junk = d.path().join("junk");
    fs::create_dir_all(&junk).unwrap();
    fs::write(junk.join("whatever"), "1").unwrap();
    let e = cli(&app, &["install", junk.to_str().unwrap()]).unwrap_err();
    assert!(e.contains("recognised"), "{e}");
    assert!(cli(&app, &["install", "/does/not/exist"]).is_err());
    assert!(cli(&app, &["plan", "no/such"]).is_err());
    assert!(cli(&app, &["set", "nope", "x", "1"]).is_err());
    assert!(cli(&app, &["bogus"]).is_err());
}

#[test]
fn data_of_an_absent_plugin_is_kept_and_reported() {
    let (d, app) = app();
    fs::write(
        d.path().join("config.json"),
        r#"{"version":2,"plugins":{"gone":{"settings":{"x":1}}},"mystery":true}"#,
    )
    .unwrap();
    fs::create_dir_all(d.path().join("games/gone-system/g1/.launcher")).unwrap();
    fs::write(
        d.path().join("games/gone-system/g1/.launcher/game.json"),
        "{}",
    )
    .unwrap();
    let diag = cli(&app, &["diagnose"]).unwrap();
    assert!(
        diag.contains("kept settings of absent plugin gone"),
        "{diag}"
    );
    assert!(diag.contains("gone-system"), "{diag}");
    // Changing an unrelated thing must not touch what we do not understand.
    let _ = cli(&app, &["choose", "a", "b"]).unwrap();
    let saved = fs::read_to_string(d.path().join("config.json")).unwrap();
    assert!(
        saved.contains("\"gone\"") && saved.contains("mystery"),
        "{saved}"
    );
    assert!(d
        .path()
        .join("games/gone-system/g1/.launcher/game.json")
        .exists());
}

#[test]
fn a_broken_tier0_manifest_never_stops_the_app() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("plugins")).unwrap();
    fs::write(d.path().join("plugins/broken.toml"), "this is not [valid").unwrap();
    let app = App::open(d.path().to_path_buf()).unwrap();
    assert!(cli(&app, &["diagnose"])
        .unwrap()
        .contains("skipped manifest"));
}

#[cfg(feature = "native-pc")]
#[test]
fn native_pc_install_list_plan_end_to_end() {
    let (d, app) = app();
    let src = d.path().join("hello");
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(
        src.join("game.toml"),
        "name = \"Hello Game\"\nexe = \"bin/run\"",
    )
    .unwrap();
    fs::write(src.join("bin/run"), "#!/bin/sh\n").unwrap();
    assert!(cli(&app, &["install", src.to_str().unwrap()])
        .unwrap()
        .contains("Hello Game"));
    assert_eq!(
        cli(&app, &["list"]).unwrap().trim(),
        "native-pc/hello\tHello Game"
    );
    cli(&app, &["set", "native", "args", "-v"]).unwrap();
    let plan = cli(&app, &["plan", "native-pc/hello"]).unwrap();
    assert!(
        plan.contains("bin/run") && plan.trim_end().ends_with("-v"),
        "{plan}"
    );
    let ui: serde_json::Value =
        serde_json::from_str(&cli(&app, &["ui", "native-pc/hello"]).unwrap()).unwrap();
    assert_eq!(ui["Hub"]["can_play"], true);
}

#[cfg(feature = "toy")]
#[test]
fn toy_plugin_scenario() {
    let (d, app) = app();
    let src = d.path().join("alpha");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join(testkit::toy::MARKER), "Alpha").unwrap();
    cli(&app, &["install", src.to_str().unwrap()]).unwrap();
    assert!(cli(&app, &["list"]).unwrap().contains("toy-sys/alpha"));
    // The toy runner is unmanaged-with-executable but has no provision, so it is honestly "not installed".
    assert!(cli(&app, &["plan", "toy-sys/alpha"])
        .unwrap_err()
        .contains("not installed"));
    assert!(cli(&app, &["catalog-manifest"])
        .unwrap()
        .contains("toy-feed"));
}

#[test]
fn the_shipped_example_manifest_loads_and_works_with_no_code() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("plugins")).unwrap();
    fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/example.toml"),
        d.path().join("plugins/example.toml"),
    )
    .unwrap();
    let app = App::open(d.path().to_path_buf()).unwrap();
    assert!(cli(&app, &["systems"]).unwrap().contains("example-sys"));
    let src = d.path().join("mygame");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("boot.bin"), "").unwrap();
    assert!(cli(&app, &["install", src.to_str().unwrap()])
        .unwrap()
        .contains("example-sys/mygame"));
    // a declared setting is validated and persisted with no plugin code involved
    cli(&app, &["set", "example-run", "scale", "3"]).unwrap();
    assert!(cli(&app, &["set", "example-run", "scale", "9"]).is_err());
    assert!(cli(&app, &["settings"])
        .unwrap()
        .contains("example-run.scale = 3"));
}
