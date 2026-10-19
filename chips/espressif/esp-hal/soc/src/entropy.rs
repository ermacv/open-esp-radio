//! Independent SoC entropy ownership, unrelated to radio power or PHY epochs.

use esp_hal::{
    peripherals::RNG,
    rng::{TrngError, TrngSource},
};

/// Owns the ESP32-S31 independent LP TRNG for its entire service lifetime.
///
/// Reads borrow this owner; a temporary HAL reader is released before the
/// borrow ends. No clonable HAL reader escapes, so dropping this owner cannot
/// disable entropy underneath one of its clients. External HAL TRNG readers
/// must independently obey esp-hal's source-lifetime contract.
pub struct Entropy<'d> {
    source: TrngSource<'d>,
}

impl<'d> Entropy<'d> {
    /// Enable the independent source using the unique RNG peripheral witness.
    pub fn new(rng: RNG<'d>) -> Self {
        Self {
            source: TrngSource::new(rng),
        }
    }

    /// Fill a fixed-size value while the source owner remains borrowed.
    ///
    /// The HAL spaces reads using the running CPU cycle counter. This is a
    /// synchronous operation proportional to `N`, not an async timeout or a
    /// bound under a stopped clock. No radio state is required for entropy.
    /// An unavailable HAL source produces no output and no PRNG fallback.
    pub fn random_bytes<const N: usize>(&mut self) -> Result<[u8; N], TrngError> {
        let mut bytes = [0; N];
        self.source.read(&mut bytes)?;
        Ok(bytes)
    }
}

/// The source's read-only health observation.
pub use esp_hal::rng::SourceStatus;

impl Entropy<'_> {
    /// Observe health while retaining the unique source witness.
    pub fn source_status(&self) -> SourceStatus {
        self.source.status()
    }
}
