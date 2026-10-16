fn main() {
    let mut cfg = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    // Without a window backend there are no image decoders; pre-render resources for the software renderer instead.
    if std::env::var_os("CARGO_FEATURE_WINDOW").is_none() {
        cfg = cfg.embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    }
    slint_build::compile_with_config("ui/app.slint", cfg).expect("slint compile");
}
