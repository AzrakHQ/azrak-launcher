fn main() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().embed_resources(slint_build::EmbedResourcesKind::EmbedFiles),
    )
    .expect("Slint UI failed to compile");

    // The PS4 RuTracker snapshot is bundled when it has been collected; until then the
    // launcher simply has no PS4 catalog (an empty file here).
    let source = std::path::Path::new("assets/rutracker/ps4-topics.json");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ps4-topics.json");
    std::fs::write(&out, std::fs::read(source).unwrap_or_default()).expect("could not stage the PS4 catalog");
    println!("cargo:rerun-if-changed=assets/rutracker/ps4-topics.json");

    println!("cargo:rerun-if-changed=build.rs");
}
