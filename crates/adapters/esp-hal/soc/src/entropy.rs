//! Independent SoC entropy ownership, unrelated to radio power or PHY epochs.

use esp_hal::{
    peripherals::RNG,
    rng::{Trng, TrngError, TrngSource},
};

/// Owns the ESP32-S31 independent LP TRNG for its entire service lifetime.
///
/// Reads borrow this owner; a temporary HAL reader is released before the
/// borrow ends. No clonable HAL reader escapes, so dropping this owner cannot
/// disable entropy underneath one of its clients. External HAL TRNG readers
/// must independently obey esp-hal's source-lifetime contract.
pub struct Entropy<'d> {
    _source: TrngSource<'d>,
}

impl<'d> Entropy<'d> {
    /// Enable the independent source using the unique RNG peripheral witness.
    pub fn new(rng: RNG<'d>) -> Self {
        Self {
            _source: TrngSource::new(rng),
        }
    }

    /// Fill a fixed-size value while the source owner remains borrowed.
    ///
    /// The HAL spaces reads using the running CPU cycle counter. This is a
    /// synchronous operation proportional to `N`, not an async timeout or a
    /// bound under a stopped clock. No radio state is required for entropy.
    /// An unavailable HAL source produces no output and no PRNG fallback.
    pub fn random_bytes<const N: usize>(&self) -> Result<[u8; N], TrngError> {
        let reader = Trng::try_new()?;
        let mut bytes = [0; N];
        reader.read(&mut bytes);
        Ok(bytes)
    }
}

/// What the LP TRNG's registers say about the entropy source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceStatus {
    /// The LP peripheral clock runs and the block is out of reset.
    pub clocked: bool,
    /// Sampling, the CRC noise conditioner and the standard 256-bit output
    /// mode are on, as ESP-IDF's `rng_ll_enable` leaves them.
    pub sampling: bool,
    /// The health tests run (not bypassed) on the selected noise source.
    pub health_tested: bool,
    /// A startup or continuous health test has failed since the last clear.
    pub health_error: bool,
}

impl SourceStatus {
    /// Whether the source produces health-tested entropy.
    pub const fn is_healthy(self) -> bool {
        self.clocked && self.sampling && self.health_tested && !self.health_error
    }
}

/// Read the LP TRNG's state. esp-hal enables the source at startup and keeps
/// it running after a `TrngSource` is dropped, because `Rng` reads it too.
pub fn source_status() -> SourceStatus {
    let clock = esp_hal::peripherals::LP_PERI::regs().rng_ctrl().read();
    let trng = RNG::regs();
    let conf = trng.conf().read();
    let debug = trng.debug_conf().read();
    SourceStatus {
        // ESP-IDF's `rng_ll_is_enabled`: `TRNG.DATE.CLK_EN` reads zero after
        // `rng_ll_enable`, whose block reset follows the write that sets it.
        clocked: clock.lp_rng_clk_en().bit_is_set() && clock.lp_rng_rst_en().bit_is_clear(),
        sampling: conf.sample_enable().bit_is_set()
            && conf.noise_crc_en().bit_is_set()
            && conf.random_output_mode().bit_is_set(),
        health_tested: debug.health_test_bypass().bit_is_clear()
            && conf.noise_source_sel().bits() != 0,
        health_error: trng.int_raw().read().error_int_raw().bit_is_set(),
    }
}
