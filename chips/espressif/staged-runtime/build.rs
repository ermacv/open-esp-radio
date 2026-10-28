//! Generates the runtime's vector entries, vector tables, every line's
//! default handler and the interrupt table's CPU lines from the facts of the
//! chip the HAL selects through the platform port, whose build publishes its
//! id as the port's `chip` metadata (`DEP_OER_ESPRESSIF_PLATFORM_PORT_CHIP`)
//! (`oer_espressif_facts::generate::Vectors`):
//! `vectors.rs`, `vector-tables.rs`, `vector-markers.rs`, `vectors.s` and the linker script fragment
//! `oer-staged-runtime-vectors.x`, which the runtime linker script includes
//! from this build directory, a library search path of every image that
//! links the runtime, and sets `chip_vectors`. Without a chip (host tests of
//! this package alone) it generates nothing.

use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_OER_ESPRESSIF_PLATFORM_PORT_CHIP");
    println!("cargo::rustc-check-cfg=cfg(chip_vectors)");
    let Some(id) = std::env::var_os("DEP_OER_ESPRESSIF_PLATFORM_PORT_CHIP") else {
        return Ok(());
    };
    let id = id
        .into_string()
        .map_err(|id| format!("the port's chip id {id:?} is not UTF-8"))?;
    let id = id.as_str();
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
    println!("cargo::rustc-cfg=chip_vectors");
    Ok(())
}
