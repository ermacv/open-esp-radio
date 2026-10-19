//! Unslotted CSMA-CA backoff, ported from OpenThread `SubMac`
//! (`src/core/mac/sub_mac.cpp` and `sub_mac.hpp` at OpenThread
//! `43cc05a9bcf780bd758bad53d897e2d88cf8cb75`, the submodule of ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`; sha256
//! `14a28df2416d94b11bdd7684552f9aba9eaea355538a3567334d98f775c9b3f0` and
//! `cba6e34f18d2e8deb8e037820a0a83c20ae9cb854cb106b581521b1249bfab5b`).
//!
//! ESP-IDF's radio performs one CCA per transmission; its OpenThread port
//! does not claim the CSMA backoff capability, so `SubMac` waits a random
//! backoff before every CCA attempt and retries while the channel is busy.
//! Each backoff is a uniform number of unit backoff periods below
//! `2^BE`, where `BE` starts at `macMinBE` and grows by one per busy
//! channel up to `macMaxBE`. A request with no backoffs transmits at once
//! with one CCA.

/// `macMinBE` (`SubMac::kCsmaMinBe`).
pub const CSMA_MIN_BACKOFF_EXPONENT: u8 = 3;
/// `macMaxBE` (`SubMac::kCsmaMaxBe`).
pub const CSMA_MAX_BACKOFF_EXPONENT: u8 = 5;
/// Symbols per unit backoff period (`SubMac::kUnitBackoffPeriod`).
pub const UNIT_BACKOFF_PERIOD_SYMBOLS: u32 = 20;
/// Microseconds per 2.4 GHz O-QPSK symbol (`Radio::kSymbolTime`).
pub const SYMBOL_MICROS: u32 = 16;

/// The backoff state of one CSMA-CA transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CsmaCa {
    max_backoffs: u8,
    backoffs: u8,
}

impl CsmaCa {
    /// A transmission that may back off `max_backoffs` times after a busy
    /// channel.
    pub const fn new(max_backoffs: u8) -> Self {
        Self {
            max_backoffs,
            backoffs: 0,
        }
    }

    /// The busy channels seen so far.
    pub const fn backoffs(&self) -> u8 {
        self.backoffs
    }

    /// `SubMac::StartCsmaBackoff`: the backoff before the next CCA attempt
    /// in microseconds, drawn from the uniform word `random` as
    /// `Random::NonCrypto::GenerateUpToExcluding` draws it (`random % 2^BE`).
    /// `None` when the request allows no backoff: the frame goes out at once.
    pub const fn backoff_micros(&self, random: u32) -> Option<u32> {
        if self.max_backoffs == 0 {
            return None;
        }
        let exponent = CSMA_MIN_BACKOFF_EXPONENT.saturating_add(self.backoffs);
        let exponent = if exponent > CSMA_MAX_BACKOFF_EXPONENT {
            CSMA_MAX_BACKOFF_EXPONENT
        } else {
            exponent
        };
        let periods = random % (1 << exponent);
        Some(periods * UNIT_BACKOFF_PERIOD_SYMBOLS * SYMBOL_MICROS)
    }

    /// `SubMac::HandleTransmitDone` after a channel-access failure: count
    /// one backoff and return whether another attempt follows. Otherwise the
    /// transmission ends with a busy channel.
    pub fn channel_busy(&mut self) -> bool {
        if self.backoffs < self.max_backoffs {
            self.backoffs += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests;
