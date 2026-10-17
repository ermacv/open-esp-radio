//! ESP32-S31 ROM-backed delays used inside exclusive PHY transactions.

/// The one production implementation of a short blocking hardware settle.
///
/// `ets_delay_us` uses the chip's calibrated ROM delay. Interrupts remain
/// enabled, so preemption can only lengthen the required minimum interval.
#[derive(Clone, Copy, Debug, Default)]
pub struct RomShortDelay;

#[allow(unsafe_code)]
unsafe extern "C" {
    fn ets_delay_us(micros: u32);
}

impl RomShortDelay {
    /// Largest recovered settle currently admitted to the blocking path.
    pub const MAX_MICROS: u32 = 20;

    /// Complete one recovered minimum settle inside an exclusive PHY transaction.
    pub fn settle_micros(micros: u32) -> bool {
        if micros == 0 || micros > Self::MAX_MICROS {
            return false;
        }
        // SAFETY: `ets_delay_us` is the ESP32-S31 ROM routine with this fixed
        // C ABI. It accepts every `u32`; the wrapper additionally limits calls
        // to the reviewed short-settle interval.
        #[allow(unsafe_code)]
        unsafe {
            ets_delay_us(micros);
        }
        true
    }
}
