//! Cargo build-script helpers that bind a chip's [`Layout`] to the shared
//! linker scripts.
//!
//! The scripts under `chips/espressif/staged-layout/linker` read every address, size and
//! header constant through `--defsym` symbols, so the linked images, the
//! bootstrap and the host auditor share one definition. The chip's own
//! `linker` directory holds only its ROM symbol script
//! ([`Layout::rom_script`]).

extern crate std;

use crate::{Layout, stage_two};
use std::{path::Path, println};

/// The shared linker scripts, in this crate.
fn shared_linker_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("linker")
}

fn defsym(bin: &str, name: &str, value: u32) {
    println!("cargo:rustc-link-arg-bin={bin}=--defsym={name}={value}");
}

fn link(bin: &str, layout: &Layout, chip_linker_dir: &Path, scripts: &[&str], entry: &str) {
    let shared = shared_linker_dir();
    let rom = chip_linker_dir.join(layout.rom_script);
    assert!(
        rom.is_file(),
        "the chip's ROM script {} is missing; pass the chip's `linker` directory",
        rom.display()
    );
    println!("cargo:rerun-if-changed={}", rom.display());
    for script in scripts {
        let path = shared.join(script);
        assert!(
            path.is_file(),
            "shared linker script {} is missing",
            path.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rustc-link-search={}", shared.display());
    println!("cargo:rustc-link-search={}", chip_linker_dir.display());
    // `--emit-relocs` keeps the link's relocations in the ELF: the stack
    // analysis reads jump tables, function addresses and calls from them
    // instead of reconstructing them from machine code.
    let rom = std::format!("-T{}", layout.rom_script);
    for argument in [rom.as_str(), entry, "--nmagic", "--emit-relocs"] {
        println!("cargo:rustc-link-arg-bin={bin}={argument}");
    }
    for (name, region) in [
        ("SRAM", layout.sram),
        ("PSRAM", layout.psram),
        ("RUNTIME_PSRAM", layout.runtime_psram()),
        ("RETAINED", layout.retained),
    ] {
        defsym(bin, &std::format!("{name}_ORIGIN"), region.origin);
        defsym(bin, &std::format!("{name}_LENGTH"), region.length);
    }
}

/// Links binary `bin` as a stage-two runtime of `layout`: code, data and task
/// stacks in PSRAM, interrupt entries, stacks and DMA state in internal SRAM.
pub fn configure_runtime(bin: &str, layout: &Layout, chip_linker_dir: &Path) {
    println!("cargo:rerun-if-env-changed=OPEN_RADIO_IRQ_STACK_BYTES");
    let irq_stack_bytes = match std::env::var("OPEN_RADIO_IRQ_STACK_BYTES") {
        Ok(value) => image_irq_stack_bytes(&value).expect("valid image IRQ stack allocation"),
        Err(std::env::VarError::NotPresent) => layout.irq_stack_bytes,
        Err(std::env::VarError::NotUnicode(_)) => panic!("IRQ stack allocation is not Unicode"),
    };
    link(
        bin,
        layout,
        chip_linker_dir,
        &["runtime/link.x", "runtime/memory.x", "runtime/sections.x"],
        "-Truntime/link.x",
    );
    // The DMA window and address width the runtime's `sections.x` includes,
    // generated from the chip's register model.
    let window = chip_linker_dir.join("dma-window.x");
    assert!(
        window.is_file(),
        "the chip's generated {} is missing; run `cargo registers generate`",
        window.display()
    );
    println!("cargo:rerun-if-changed={}", window.display());
    // A section no rule places is an error, not an orphan the linker puts
    // somewhere: a mistyped `.dma.bss.*` name would otherwise leave a DMA
    // static outside the checked range.
    println!("cargo:rustc-link-arg-bin={bin}=--orphan-handling=error");
    for (name, value) in [
        ("STAGE_TWO_MAGIC", stage_two::MAGIC),
        ("STAGE_TWO_ABI_VERSION", stage_two::ABI_VERSION),
        ("STAGE_TWO_HEADER_BYTES", stage_two::HEADER_BYTES as u32),
        ("CPU0_PSRAM_TASK_STACK_BYTES", layout.cpu0_task_stack_bytes),
        ("IRQ_STACK_BYTES", irq_stack_bytes),
        ("HART_COUNT", layout.harts),
        ("RETAINED_RESERVED_TAIL", layout.retained_reserved_tail),
    ] {
        defsym(bin, name, value);
    }
    println!("cargo:rerun-if-env-changed={LAYOUT_SEED_ENV}");
    let seed = std::env::var(LAYOUT_SEED_ENV).ok();
    let seed = layout_seed(seed.as_deref()).unwrap_or_else(|error| panic!("{error}"));
    for argument in shuffle_arguments(seed) {
        println!("cargo:rustc-link-arg-bin={bin}={argument}");
    }
}

fn image_irq_stack_bytes(value: &str) -> Result<u32, &'static str> {
    let bytes: u32 = value
        .parse()
        .map_err(|_| "IRQ stack allocation is not a u32")?;
    if bytes <= crate::interrupts::IRQ_STACK_GUARD_BYTES || bytes % 16 != 0 {
        return Err("IRQ stack allocation must exceed the guard and be aligned to 16 bytes");
    }
    Ok(bytes)
}

/// Environment variable that selects a reproducible alternative placement of
/// ordinary code and read-only data.
///
/// Throughput of code that runs from cached external memory depends on where
/// the linker happens to put each function: an unrelated dependency change
/// re-sorts them and moves the cost per byte by tens of percent. A seed makes
/// the linker shuffle the ordinary `.text.*` and `.rodata.*` input sections,
/// so a performance comparison can measure several placements of the same
/// source instead of one accidental one. The explicitly placed ISR, hot and
/// critical sections keep their names and so their placement. Unset, the
/// linker keeps its natural order.
pub const LAYOUT_SEED_ENV: &str = "OER_LAYOUT_SEED";

/// Parse a layout seed; zero is rejected because the linker reads it as
/// "random", which no build record could reproduce.
pub fn layout_seed(value: Option<&str>) -> Result<Option<u32>, std::string::String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value.parse::<u32>() {
        Ok(0) | Err(_) => Err(std::format!(
            "{LAYOUT_SEED_ENV}={value:?} must be a nonzero decimal u32"
        )),
        Ok(seed) => Ok(Some(seed)),
    }
}

/// Linker arguments that shuffle ordinary code and read-only data by `seed`.
pub fn shuffle_arguments(seed: Option<u32>) -> std::vec::Vec<std::string::String> {
    seed.into_iter()
        .flat_map(|seed| {
            [".text.*", ".rodata.*"].map(|glob| std::format!("--shuffle-sections={glob}={seed}"))
        })
        .collect()
}

/// Links binary `bin` as the Flash-resident bootstrap of `layout`.
pub fn configure_bootstrap(bin: &str, layout: &Layout, chip_linker_dir: &Path) {
    link(
        bin,
        layout,
        chip_linker_dir,
        &[
            "bootstrap/link.x",
            "bootstrap/memory.x",
            "bootstrap/sections.x",
            "bootstrap/flash-sections.x",
        ],
        "-Tbootstrap/link.x",
    );
    let text = layout.bootstrap_flash_text();
    defsym(bin, "BOOTSTRAP_FLASH_TEXT_ORIGIN", text.origin);
    defsym(bin, "BOOTSTRAP_FLASH_TEXT_LENGTH", text.length);
    defsym(
        bin,
        "SECOND_STAGE_LOADER_START",
        layout.second_stage_loader_start,
    );
    defsym(
        bin,
        "FLASH_TUNING_REFERENCE_BYTES",
        layout.flash_tuning_reference_bytes,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_irq_stack_allocation_never_silently_falls_back() {
        assert_eq!(image_irq_stack_bytes("65536"), Ok(65536));
        for value in ["", "0", "1024", "65537", "-1", "4294967296"] {
            assert!(image_irq_stack_bytes(value).is_err(), "{value}");
        }
    }

    #[test]
    fn only_a_nonzero_seed_shuffles_and_it_is_reproducible() {
        assert_eq!(layout_seed(None), Ok(None));
        assert!(shuffle_arguments(None).is_empty());
        assert!(layout_seed(Some("0")).is_err());
        assert!(layout_seed(Some("-1")).is_err());
        assert!(layout_seed(Some("seven")).is_err());
        let seed = layout_seed(Some("7")).unwrap();
        assert_eq!(
            shuffle_arguments(seed),
            [
                "--shuffle-sections=.text.*=7",
                "--shuffle-sections=.rodata.*=7"
            ]
        );
    }
}
