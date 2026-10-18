#![no_std]

//! Memory configuration of the ESP32-C5 devkit with the N16R8 module
//! (16-MiB quad flash, 8-MiB quad PSRAM).
//!
//! The bootstrap initializes the module's PSRAM with [`initialize_psram`] at
//! the layout's fixed address in the cache window flash and PSRAM share;
//! stage two adopts the mapping with [`adopt_psram`]. This profile does not
//! describe every ESP32-C5 board.

use oer_esp32c5_platform_layout::LAYOUT;

use esp_hal::{
    peripherals::PSRAM,
    psram::{FlashFreq, Psram, PsramConfig, PsramOrigin, PsramSize, SpiRamFreq},
};

pub const BOARD_NAME: &str = "ESP32-C5 devkit (ESP32-C5-WROOM-1-N16R8)";
pub const PSRAM_BASE_ADDRESS: usize = LAYOUT.psram.origin as usize;
pub const PSRAM_SIZE_BYTES: usize = LAYOUT.psram.length as usize;
pub const FLASH_SIZE_BYTES: usize = 16 * 1024 * 1024;
pub const FLASH_MMU_PAGE_SIZE_BYTES: u32 = 64 * 1024;

/// The module's PSRAM: quad, 8 MiB, at the layout's fixed address, at the
/// fastest clock esp-hal drives the ESP32-C5 MSPI at (80 MHz, with flash at
/// the image header's 80 MHz).
///
/// The chip's PSRAM is quad only; ESP-IDF offers 40, 80 and 120 MHz, the
/// last behind MSPI timing tuning, which esp-hal does not implement for this
/// chip.
///
/// SOURCE(esp32c5): ESP-IDF 4d59230d `components/esp_psram/esp32c5/Kconfig.spiram:11-38`
/// (`SPIRAM_MODE_QUAD` only; `SPIRAM_SPEED_120M`, `_80M`, `_40M`) and
/// `components/spi_flash/esp32c5/Kconfig.flash_freq` (120, 80, 40, 20 MHz;
/// `components/esptool_py/Kconfig.projbuild:104-109` writes 80m into the
/// image header for both 120 and 80 MHz).
pub fn psram_config() -> PsramConfig {
    PsramConfig {
        size: PsramSize::Size(PSRAM_SIZE_BYTES),
        origin: PsramOrigin::Fixed(PSRAM_BASE_ADDRESS),
        flash_frequency: FlashFreq::FlashFreq80m,
        ram_frequency: SpiRamFreq::Freq80m,
        ..PsramConfig::default()
    }
}

pub fn initialize_psram(peripheral: PSRAM<'static>) -> Psram {
    Psram::new(peripheral, psram_config())
}

/// Adopt the board mapping and install the stage-two interrupt context
/// (`oer_espressif_staged_runtime::adopt`).
///
/// `interrupt_table` is the image's `INTERRUPT_TABLE`
/// (`oer_espressif_staged_runtime::interrupt_table!`).
///
/// Code, data and task stacks run from PSRAM from here on, so the adopted
/// mapping keeps the PLL behind the MSPI clock referenced in esp-hal's clock
/// tree for the lifetime of the image.
///
/// # Safety
/// Call once after `_runtime_start`, with interrupts disabled and the
/// bootstrap's board mapping intact. Keep interrupts disabled until the
/// application's timer and executor handlers have been bound.
#[cfg(feature = "stage-two")]
pub unsafe fn adopt_psram(
    peripheral: PSRAM<'static>,
    interrupt_table: &'static oer_espressif_staged_runtime::InterruptTable,
) -> Psram {
    let adopt = || {
        // SAFETY: the bootstrap initialized and mapped the complete module
        // PSRAM at `PSRAM_BASE_ADDRESS` and handed over without resetting or
        // remapping it (the caller's contract).
        unsafe {
            Psram::from_existing_mapping(
                peripheral,
                PSRAM_BASE_ADDRESS..PSRAM_BASE_ADDRESS + PSRAM_SIZE_BYTES,
            )
        }
    };
    // SAFETY: the caller's contract is `adopt`'s; the closure adopts the
    // bootstrap's mapping without resetting or remapping it.
    unsafe { oer_espressif_staged_runtime::adopt(interrupt_table, adopt) }
}
