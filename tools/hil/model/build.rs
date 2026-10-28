//! Embeds the features every chip's HIL image manifest declares, found
//! through the chip profiles, so the host builds the classes of the tree it
//! was built from.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()?;
    // A chip's platform directory appears below `chips/`.
    println!("cargo::rerun-if-changed={}", root.join("chips").display());
    let mut entries = String::new();
    for profile in oer_chip_profile::Profile::all(&root).map_err(|error| error.to_string())? {
        println!(
            "cargo::rerun-if-changed={}",
            root.join(oer_chip_profile::platform_dir(&profile.facts.id))
                .display()
        );
        let Some(hil) = &profile.hil else { continue };
        for (class, recipe) in &hil.images {
            let manifest = root.join(&recipe.manifest);
            println!("cargo::rerun-if-changed={}", manifest.display());
            let text = std::fs::read_to_string(&manifest)
                .map_err(|error| format!("HIL manifest {}: {error}", manifest.display()))?;
            let table: toml::Table = toml::from_str(&text)?;
            let features: Vec<&String> = table
                .get("features")
                .and_then(toml::Value::as_table)
                .ok_or_else(|| format!("{} declares no features", manifest.display()))?
                .keys()
                .collect();
            entries.push_str(&format!(
                "    ({:?}, {:?}, &{:?}),\n",
                profile.facts.id, class, features
            ));
        }
    }
    let out = Path::new(&std::env::var("OUT_DIR")?).join("runtime_features.rs");
    std::fs::write(
        out,
        format!(
            "/// The features each runtime manifest of this tree declares, by chip and class.\n\
             const RUNTIME_FEATURES: &[(&str, &str, &[&str])] = &[\n{entries}];\n"
        ),
    )?;
    Ok(())
}
