#![no_std]
#![forbid(unsafe_code)]

//! Staged-boot address map of ESP32-S31-Function-CoreBoard-1.
//!
//! The chip-neutral contract (the [`Layout`] type, the stage-two header, the
//! zeroed sections and the shared linker scripts) is
//! `oer-firmware-platform-espressif-staged-layout`; this crate states the board's map once, for
//! the bootstrap, the board, and the build scripts of every image. With the
//! `build` feature, [`build`] links a binary with the shared scripts and this
//! chip's ROM script (`firmware/platform/esp32s31/linker`).
//!
//! Sizes describe this board's fitted memories and the staged-boot contract;
//! they are not universal ESP32-S31 capabilities.

use oer_firmware_platform_espressif_staged_layout::{Layout, Region};

/// The board's staged-boot layout.
pub const LAYOUT: Layout = Layout {
    // Application-owned internal SRAM, `SRAM_START..SRAM_SEG_END` of ESP-IDF's
    // application memory map; the ROM boot stack side lies above it.
    //
    // SOURCE: ESP-IDF 4d59230d `components/esp_system/ld/esp32s31/ld.hp_mem_defs:9`
    // (`SRAM_SEG_END 0x2F07AFC0`) and `memory.ld.in:21-26`
    // (`SRAM_SIZE = SRAM_SEG_END - SRAM_START`, without the ULP HP-memory reserve).
    sram: Region::new(0x2f00_0000, 0x0007_afc0),
    // SOURCE: ESP-IDF 4d59230d
    // `components/bootloader/subproject/main/ld/esp32s31/bootloader.memory.ld.in:39-64`
    // (usable end 0x2f07afb0 minus the 0x2000 stack, 0x5000 `dram_seg` and 0x7000
    // `iram_loader_seg`; asserted as `bootloader_iram_loader_seg_start`).
    second_stage_loader_start: 0x2f06_cfb0,
    // The unified 64-MiB Flash XIP instruction/data window.
    flash_xip: Region::new(0x4000_0000, 0x0400_0000),
    // Cached external RAM aperture of the fitted 16-MiB PSRAM.
    psram: Region::new(0x5000_0000, 0x0100_0000),
    // LP RAM (RTC fast memory). ESP-IDF keeps its tail as lp_reserved_seg:
    // the RTC timer data (RTC_TIMER_RESERVE_RTC, 24 bytes) + the bootloader's
    // retained data + the secure-boot fast-wake digest; the espflash
    // bootloader this platform boots through enables neither of the last two.
    //
    // SOURCE: ESP-IDF 4d59230d `components/esp_system/ld/esp32s31/memory.ld.in`
    // (lp_reserved_seg at 0x2E008000 - RESERVE_RTC_MEM).
    retained: Region::new(0x2e00_0000, 0x0000_8000),
    retained_reserved_tail: 24,
    cpu0_task_stack_bytes: 0x3_0000,
    irq_stack_bytes: 0x8000,
    harts: 2,
    // `Flash::tune_120mhz` needs at least fifteen distinct 4-KiB XIP pages.
    flash_tuning_reference_bytes: 0x1_0000,
    // SOURCE: ESP-IDF 4d59230d `components/esp_rom/esp32s31/ld/esp32s31.rom.ld`
    // (the ECO0 ROM of the board's revision 0 chip).
    rom_script: "rom/esp32s31-eco0.x",
};

const _: () = LAYOUT.check();

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
        oer_firmware_platform_espressif_staged_layout::build::configure_runtime(
            bin,
            &super::LAYOUT,
            &linker_dir(),
        );
    }

    /// Link binary `bin` as this board's Flash-resident bootstrap.
    pub fn configure_bootstrap(bin: &str) {
        oer_firmware_platform_espressif_staged_layout::build::configure_bootstrap(
            bin,
            &super::LAYOUT,
            &linker_dir(),
        );
    }
}
