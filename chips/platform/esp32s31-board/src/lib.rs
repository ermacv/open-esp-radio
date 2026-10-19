#![no_std]

//! Electrical and memory configuration for ESP32-S31-Function-CoreBoard-1.
//!
//! This board profile is shared by every image for this board; it does not
//! describe every ESP32-S31 board. The bootstrap initializes the
//! board's PSRAM with [`initialize_psram`]; stage two adopts the mapping with
//! [`adopt_psram_mapping`], inside the staged runtime's `adopt`, which the
//! image calls itself: the board does not link the runtime.

use oer_platform_esp32s31_layout::LAYOUT;

use esp_hal::{
    peripherals::PSRAM,
    psram::{Psram, PsramConfig, PsramSize, PsramTimingParams},
};

pub const BOARD_NAME: &str = "ESP32-S31-Function-CoreBoard-1";
pub const PSRAM_BASE_ADDRESS: usize = LAYOUT.psram.origin as usize;
pub const PSRAM_SIZE_BYTES: usize = LAYOUT.psram.length as usize;
pub const PSRAM_CLOCK_MHZ: u32 = 250;
pub const PSRAM_DATA_LINES: u8 = 8;
pub const FLASH_CLOCK_MHZ: u32 = 120;
pub const FLASH_SIZE_BYTES: usize = 16 * 1024 * 1024;
pub const FLASH_MMU_PAGE_SIZE_BYTES: u32 = 64 * 1024;
pub const FLASH_DATA_LINES: u8 = 4;

pub const fn psram_config() -> PsramConfig {
    PsramConfig {
        size: PsramSize::AutoDetect,
        timing: PsramTimingParams::MHZ_250,
    }
}

pub fn initialize_psram(peripheral: PSRAM<'static>) -> Psram {
    Psram::new(peripheral, psram_config())
}

/// Adopt the PSRAM mapping the bootstrap left, without resetting or
/// remapping it, and check that the adoption left the PSRAM PHY supply as it
/// was.
///
/// Stage two calls it as the mapping closure of the staged runtime's `adopt`:
///
/// ```ignore
/// unsafe {
///     oer_platform_staged_runtime::adopt(INTERRUPT_TABLE, || {
///         board::adopt_psram_mapping(peripherals.PSRAM)
///     })
/// }
/// ```
///
/// Code, data and task stacks run from PSRAM from here on, so the adopted
/// mapping keeps the PSRAM function clock (and through it MPLL) referenced in
/// esp-hal's clock tree for the lifetime of the image: a driver that requests
/// and releases MPLL must never power it down under PSRAM.
///
/// # Safety
/// Call once per boot, in stage two after the bootstrap's handoff, while the
/// bootstrap's mapping of the complete board PSRAM is intact: nothing may
/// have reset or remapped PSRAM since, because code and stacks already run
/// from it.
pub unsafe fn adopt_psram_mapping(peripheral: PSRAM<'static>) -> Psram {
    let supply = psram_supply();
    let started = esp_hal::time::Instant::now();
    // SAFETY: the bootstrap initialized and mapped the complete board PSRAM
    // at `PSRAM_BASE_ADDRESS` and handed over without resetting or remapping
    // it (the caller's contract).
    let psram = unsafe {
        Psram::from_existing_mapping(
            peripheral,
            PSRAM_BASE_ADDRESS..PSRAM_BASE_ADDRESS + PSRAM_SIZE_BYTES,
        )
    };
    // Bringing the PHY LDO up waits 1 ms for its rail, so an adoption that
    // takes half of that rewrote the supply even with equal values.
    assert!(
        started.elapsed() < esp_hal::time::Duration::from_micros(500),
        "PSRAM adoption reprogrammed the PSRAM PHY LDO"
    );
    // Code and stacks already run from PSRAM: adopting the mapping must leave
    // the PHY supply's configuration as the bootstrap left it.
    assert_eq!(
        psram_supply(),
        supply,
        "PSRAM adoption reprogrammed the PSRAM PHY supply"
    );
    psram
}

/// The PMU words that configure the PSRAM PHY supply: the external LDO
/// (inrush limit, voltage, enable) and the PSRAM power-down control.
fn psram_supply() -> (u32, u32) {
    let pmu = esp_hal::peripherals::PMU::regs();
    (
        pmu.ext_ldo_ctrl().read().bits(),
        pmu.psram_cfg().read().bits(),
    )
}

pub fn has_expected_psram_capacity(psram: &Psram) -> bool {
    psram.raw_parts().1 == PSRAM_SIZE_BYTES
}
