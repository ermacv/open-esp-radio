//! Bind the CLI to the chip facts it was built alongside.

use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let chips = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../chips")
        .canonicalize()?;
    // Also notice a newly supported chip, without maintaining a second list.
    println!("cargo::rerun-if-changed={}", chips.display());
    let mut profiles = Vec::new();
    for entry in fs::read_dir(chips)? {
        let entry = entry?;
        let path = entry.path().join("platform/chip.toml");
        if path.is_file() {
            profiles.push((entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    profiles.sort();
    let mut source = String::from("const PROFILES: &[(&str, &[u8])] = &[\n");
    for (chip, path) in profiles {
        source.push_str(&format!("({chip:?}, include_bytes!({path:?})),\n"));
    }
    source.push_str("];\n");
    let output = env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("Cargo sets OUT_DIR"))?;
    fs::write(PathBuf::from(output).join("chip_facts.rs"), source)
}
