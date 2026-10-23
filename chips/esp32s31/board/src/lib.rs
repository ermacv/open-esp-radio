#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Electrical and memory configuration for ESP32-S31-Function-CoreBoard-1.
//!
//! This board profile is shared by every image for this board; it does not
//! describe every ESP32-S31 board. The bootstrap initializes the
//! board's PSRAM with [`initialize_psram`]; stage two adopts the mapping
//! through [`BootstrapPsram`], which the staged runtime's handoff takes: the
//! board does not link the runtime.

use oer_esp32s31_layout::LAYOUT;

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

/// Antenna gain used by this board profile, in mBm.
///
/// The antenna gain is 2.33 dBi: the gain of the antenna on which the
/// ESP32-S31-WROOM-3U was certified, not a measured gain of this board's PCB
/// antenna.
/// SOURCE: ESP32-S31-WROOM-3 & WROOM-3U datasheet v0.7 (preliminary),
/// section 10.
pub const WIFI_ANTENNA_GAIN_MBM: i32 = 233;

/// Conducted power limits: board antenna gain, chip maximum (the PHY
/// calibration ceiling), and an optional installation cap. The stand cap
/// is an installation setting, not a board fact.
pub const fn wifi_power_limits(
    chip_maximum_mbm: i32,
    owner_cap_mbm: Option<i32>,
) -> oer_radio_regulatory::PowerLimits {
    oer_radio_regulatory::PowerLimits {
        antenna_gain_mbm: WIFI_ANTENNA_GAIN_MBM,
        chip_maximum_mbm,
        owner_cap_mbm,
    }
}

pub const fn psram_config() -> PsramConfig {
    PsramConfig {
        size: PsramSize::AutoDetect,
        timing: PsramTimingParams::MHZ_250,
    }
}

pub fn initialize_psram(peripheral: PSRAM<'static>) -> Psram {
    Psram::new(peripheral, psram_config())
}

/// The board's PSRAM as stage two finds it: mapped by the bootstrap, with
/// code, data and stacks already running from it. The staged runtime's
/// handoff adopts it:
///
/// ```ignore
/// let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, BootstrapPsram(peripherals.PSRAM));
/// ```
///
/// The adoption checks that it left the PSRAM PHY supply as it was. The
/// adopted mapping keeps the PSRAM function clock (and through it MPLL)
/// referenced in esp-hal's clock tree for the lifetime of the image: a driver
/// that requests and releases MPLL must never power it down under PSRAM.
pub struct BootstrapPsram(pub PSRAM<'static>);

#[allow(unsafe_code, reason = "adopting the bootstrap's PSRAM mapping")]
// SAFETY: `adopt` takes over the bootstrap's mapping through
// `Psram::from_existing_mapping`, which neither resets nor remaps PSRAM, and
// checks that the PSRAM PHY supply kept its configuration; it leaves
// interrupts as it found them.
unsafe impl oer_espressif_staged_layout::handoff::AdoptsBootstrapMapping for BootstrapPsram {
    type Mapping = Psram;

    fn adopt(mut self, _handoff: &oer_espressif_staged_layout::handoff::StageTwoHandoff) -> Psram {
        let supply = esp_hal::psram::existing_supply_configuration(&mut self.0);
        let started = esp_hal::time::Instant::now();
        // SAFETY: the handoff proves the bootstrap initialized and mapped the
        // complete board PSRAM at `PSRAM_BASE_ADDRESS` and that nothing reset
        // or remapped it since.
        let psram = unsafe {
            Psram::from_existing_mapping(
                self.0,
                PSRAM_BASE_ADDRESS..PSRAM_BASE_ADDRESS + PSRAM_SIZE_BYTES,
            )
        };
        // Bringing the PHY LDO up waits 1 ms for its rail, so an adoption that
        // takes half of that rewrote the supply even with equal values.
        assert!(
            started.elapsed() < esp_hal::time::Duration::from_micros(500),
            "PSRAM adoption reprogrammed the PSRAM PHY LDO"
        );
        // Code and stacks already run from PSRAM: adopting the mapping must
        // leave the PHY supply's configuration as the bootstrap left it.
        assert_eq!(
            psram.supply_configuration(),
            supply,
            "PSRAM adoption reprogrammed the PSRAM PHY supply"
        );
        psram
    }
}

pub fn has_expected_psram_capacity(psram: &Psram) -> bool {
    psram.raw_parts().1 == PSRAM_SIZE_BYTES
}
