fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    oer_esp32s31_platform_layout::build::configure_runtime("oer-esp32s31-hil-agent");
    enabled_features(manifest);
}

/// Lists the manifest features this build enables, as `ENABLED_FEATURES`,
/// for the image's capability set.
fn enabled_features(manifest: &std::path::Path) {
    let path = manifest.join("Cargo.toml");
    println!("cargo::rerun-if-changed={}", path.display());
    let text = std::fs::read_to_string(&path).expect("the runtime manifest reads");
    let mut in_features = false;
    let mut enabled = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_features = line == "[features]";
            continue;
        }
        let Some((name, _)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !in_features || name.is_empty() || name.starts_with('#') || name.contains(' ') {
            continue;
        }
        let variable = format!("CARGO_FEATURE_{}", name.to_uppercase().replace('-', "_"));
        if std::env::var_os(variable).is_some() {
            enabled.push(format!("{name:?}"));
        }
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        out.join("enabled_features.rs"),
        format!(
            "/// The Cargo features this image was built with.\npub(crate) const ENABLED_FEATURES: &[&str] = &[{}];\n",
            enabled.join(", ")
        ),
    )
    .expect("the feature list writes");
}
