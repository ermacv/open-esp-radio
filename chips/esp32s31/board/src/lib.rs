#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! Electrical and memory configuration for ESP32-S31-Function-CoreBoard-1.
//!
//! This board profile is shared by every image for this board; it does not
//! describe every ESP32-S31 board. The bootstrap brings the board's PSRAM up
//! through the HAL's boot sequences; stage two adopts the mapping through
//! [`BootstrapPsram`], which the staged runtime's handoff takes: the board
//! does not link the runtime.

use core::ops::Range;

use oer_esp32s31_layout::LAYOUT;

pub const PSRAM_BASE_ADDRESS: usize = LAYOUT.psram.origin as usize;
pub const PSRAM_SIZE_BYTES: usize = LAYOUT.psram.length as usize;
pub const FLASH_MMU_PAGE_SIZE_BYTES: u32 = 64 * 1024;

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

/// The board's PSRAM as stage two finds it: mapped by the bootstrap, with
/// code, data and stacks already running from it. The staged runtime's
/// handoff adopts it:
///
/// ```ignore
/// let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, controller, BootstrapPsram);
/// ```
///
/// Stage two owns no PSRAM controller, cache or MMU register: the root
/// drops the bootstrap's partitions, so adoption writes nothing.
pub struct BootstrapPsram;

/// The adopted PSRAM mapping: the epoch in which code, data and stacks run
/// from PSRAM. It lasts for the image's lifetime, and so does the MPLL
/// reference the mapping holds.
#[derive(Debug)]
pub struct PsramMapping {
    range: Range<usize>,
}

impl PsramMapping {
    /// The mapped window's virtual addresses.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }
}

#[allow(unsafe_code, reason = "adopting the bootstrap's PSRAM mapping")]
// SAFETY: `adopt` writes no register: it records the window the bootstrap
// mapped and leaves PSRAM, the MMU and interrupts as it found them.
unsafe impl oer_espressif_staged_layout::handoff::AdoptsBootstrapMapping for BootstrapPsram {
    type Mapping = PsramMapping;

    fn adopt(
        self,
        _handoff: &oer_espressif_staged_layout::handoff::StageTwoHandoff,
    ) -> PsramMapping {
        PsramMapping {
            range: PSRAM_BASE_ADDRESS..PSRAM_BASE_ADDRESS + PSRAM_SIZE_BYTES,
        }
    }
}
