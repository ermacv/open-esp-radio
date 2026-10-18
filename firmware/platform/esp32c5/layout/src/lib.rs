#![no_std]
#![forbid(unsafe_code)]

//! Staged-boot address map of the ESP32-C5 devkit with the N16R8 module
//! (16-MiB flash, 8-MiB quad PSRAM).
//!
//! The chip-neutral contract (the [`Layout`] type, the stage-two header, the
//! zeroed sections and the shared linker scripts) is
//! `oer-espressif-staged-layout`; this crate states the board's map once, for
//! the bootstrap, the board, and the build scripts of every image. With the
//! `build` feature, [`build`] links a binary with the shared scripts and this
//! chip's ROM script (`platform/esp32c5/linker`).
//!
//! The ESP32-C5 has one MMU whose 32-MiB cache window serves flash and PSRAM
//! alike. The board splits it: the bootstrap's flash pages in the lower half,
//! PSRAM at the fixed start of the upper half, where the bootstrap maps it and
//! stage two is linked.

use oer_espressif_staged_layout::{Layout, Region};

/// The cache window flash and PSRAM share.
///
/// SOURCE: ESP-IDF 4d59230d `components/soc/esp32c5/include/soc/soc.h:144-149`
/// (`SOC_IROM_LOW 0x42000000`, `SOC_IROM_HIGH 0x44000000`, DROM the same) and
/// `soc_caps.h:309` (`SOC_MMU_DI_VADDR_SHARED`); esp-hal's metadata places
/// PSRAM in it (`esp-metadata/devices/esp32c5/soc.toml` `extmem_origin`).
const CACHE_WINDOW: Region = Region::new(0x4200_0000, 0x0200_0000);

/// The board's staged-boot layout.
pub const LAYOUT: Layout = Layout {
    // Application-owned internal SRAM, `SRAM_START..SRAM_SEG_END`; the
    // second-stage loader, the ROM's download buffers, its stack and its data
    // lie above it.
    //
    // SOURCE: ESP-IDF 4d59230d `components/esp_system/ld/esp32c5/ld.hp_mem_defs:9`
    // (`SRAM_SEG_END 0x4084E5A0`) and `components/soc/esp32c5/include/soc/soc.h:156`
    // (`SOC_DRAM_LOW 0x40800000`).
    sram: Region::new(0x4080_0000, 0x0004_e5a0),
    // SOURCE: ESP-IDF 4d59230d
    // `components/bootloader/subproject/main/ld/esp32c5/bootloader.memory.ld.in:26-51`
    // (usable end 0x4085c5a0 minus the 0x2000 stack, 0x5000 `dram_seg` and 0x7000
    // `iram_loader_seg`; asserted as `bootloader_iram_loader_seg_start`).
    second_stage_loader_start: 0x4084_e5a0,
    // The lower half of the cache window: the bootstrap's text and read-only
    // data, mapped by the second-stage loader from the window's start.
    flash_xip: CACHE_WINDOW.first(0x0100_0000),
    // The module's 8-MiB PSRAM at the start of the window's upper half, a
    // multiple of its 64-KiB MMU pages (esp-hal `PsramOrigin::Fixed`).
    psram: Region::new(0x4300_0000, 0x0080_0000),
    // LP RAM: the panic record survives every reset but a power loss.
    // ESP-IDF keeps its tail as lp_reserved_seg: the RTC timer data
    // (RTC_TIMER_RESERVE_RTC, 24 bytes) + the bootloader's retained data; the
    // espflash bootloader this platform boots through reserves none.
    //
    // SOURCE: ESP-IDF 4d59230d `components/soc/esp32c5/include/soc/soc.h:160-161`
    // (`SOC_RTC_DRAM_LOW 0x50000000`, `SOC_RTC_DRAM_HIGH 0x50004000`),
    // `components/esp_system/ld/esp32c5/memory.ld.in:86-136` (lp_reserved_seg at
    // 0x50004000 - RESERVE_RTC_MEM) and `components/esp_system/ld/ld.common:64`.
    retained: Region::new(0x5000_0000, 0x0000_4000),
    retained_reserved_tail: 24,
    cpu0_task_stack_bytes: 0x3_0000,
    irq_stack_bytes: 0x8000,
    // One RISC-V core.
    harts: 1,
    // The bootstrap does not tune Flash: esp-hal runs the ESP32-C5 MSPI at
    // the image header's 80 MHz.
    flash_tuning_reference_bytes: 0,
    rom_script: "rom/esp32c5-eco2.x",
};

const _: () = {
    LAYOUT.check();
    assert!(CACHE_WINDOW.contains(LAYOUT.flash_xip));
    assert!(CACHE_WINDOW.contains(LAYOUT.psram));
    assert!(LAYOUT.flash_xip.end() <= LAYOUT.psram.origin);
};

#[cfg(feature = "build")]
pub mod build {
    //! Link a binary of this board with the shared linker scripts.
    extern crate std;

    use std::path::Path;

    fn linker_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../linker")
    }

    /// Link binary `bin` as a stage-two runtime of this board.
    pub fn configure_runtime(bin: &str) {
        oer_espressif_staged_layout::build::configure_runtime(bin, &super::LAYOUT, &linker_dir());
    }

    /// Link binary `bin` as this board's Flash-resident bootstrap.
    pub fn configure_bootstrap(bin: &str) {
        oer_espressif_staged_layout::build::configure_bootstrap(bin, &super::LAYOUT, &linker_dir());
    }
}
