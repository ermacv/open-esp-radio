//! Embeds every chip's HIL agent manifest, found through the chip profiles,
//! so the host's expectation of a flashed image is the tree it was built
//! from.

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = root.canonicalize().expect("the repository root exists");
    // A chip's platform directory appears below `chips/`.
    println!("cargo::rerun-if-changed={}", root.join("chips").display());
    let mut entries = String::new();
    for profile in oer_chip_profile::Profile::all(&root).expect("the chip profiles load") {
        println!(
            "cargo::rerun-if-changed={}",
            root.join(oer_chip_profile::platform_dir(&profile.id))
                .display()
        );
        let Some(hil) = &profile.hil else { continue };
        for (class, recipe) in &hil.images {
            let manifest = root.join(&recipe.manifest);
            println!("cargo::rerun-if-changed={}", manifest.display());
            if !manifest.is_file() {
                panic!("missing HIL manifest {}", manifest.display());
            }
            entries.push_str(&format!(
                "    ({:?}, {:?}, include_str!({:?})),\n",
                profile.id,
                class,
                manifest.display().to_string()
            ));
        }
    }
    let out =
        Path::new(&std::env::var("OUT_DIR").expect("Cargo sets OUT_DIR")).join("manifests.rs");
    std::fs::write(
        out,
        format!("/// The runtime manifests of this tree, by chip.\nconst RUNTIME_MANIFESTS: &[(&str, &str, &str)] = &[\n{entries}];\n"),
    )
    .expect("the manifest table is written");
}
