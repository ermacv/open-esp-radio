//! Embeds every chip's HIL agent manifest, found through the chip profiles,
//! so the host's expectation of a flashed image is the tree it was built
//! from.

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = root.canonicalize().expect("the repository root exists");
    println!(
        "cargo::rerun-if-changed={}",
        root.join("platform").display()
    );
    let mut entries = String::new();
    for profile in oer_chip_profile::Profile::all(&root).expect("the chip profiles load") {
        let manifest = profile.hil_agent_manifest(&root);
        println!(
            "cargo::rerun-if-changed={}",
            profile.hil_agent_workspace(&root).display()
        );
        if !manifest.is_file() {
            continue;
        }
        println!("cargo::rerun-if-changed={}", manifest.display());
        entries.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            profile.id,
            manifest.display().to_string()
        ));
    }
    let out =
        Path::new(&std::env::var("OUT_DIR").expect("Cargo sets OUT_DIR")).join("manifests.rs");
    std::fs::write(
        out,
        format!("/// The runtime manifests of this tree, by chip.\nconst RUNTIME_MANIFESTS: &[(&str, &str)] = &[\n{entries}];\n"),
    )
    .expect("the manifest table is written");
}
