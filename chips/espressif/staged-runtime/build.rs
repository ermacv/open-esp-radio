//! Generates the runtime's vector entries, vector tables, every line's
//! default handler and the interrupt table's CPU lines from the facts of the
//! chip its feature selects (`oer_espressif_facts::generate::Vectors`):
//! `vectors.rs`, `vector-tables.rs`, `vector-markers.rs`, `vectors.s` and the linker script fragment
//! `oer-staged-runtime-vectors.x`, which the runtime linker script includes
//! from this build directory, a library search path of every image that
//! links the runtime. Without a chip feature (host tests) it generates
//! nothing.

use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let chips: Vec<&str> = oer_espressif_facts::tracked::ids()
        .into_iter()
        .filter(|id| std::env::var_os(format!("CARGO_FEATURE_{}", id.to_uppercase())).is_some())
        .collect();
    let id = match chips.as_slice() {
        [] => return Ok(()),
        [id] => *id,
        _ => {
            return Err(format!("an image selects exactly one chip feature, not {chips:?}").into());
        }
    };
    let (facts, files) = oer_espressif_facts::generate::tracked(id)?;
    let vectors = oer_espressif_facts::generate::Vectors::of(&facts)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    for (name, text) in [
        ("vectors.rs", vectors.rust()?),
        ("vector-markers.rs", vectors.vector_markers()),
        ("vector-tables.rs", vectors.vector_tables()?),
        ("stack-symbols.rs", vectors.stack_symbols()),
        ("vectors.s", vectors.assembly()),
        ("oer-staged-runtime-vectors.x", vectors.linker_defaults()),
    ] {
        std::fs::write(out.join(name), text)?;
    }
    println!("cargo:rustc-link-search={}", out.display());
    Ok(())
}
