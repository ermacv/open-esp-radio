//! Fixed software allocation budget and validated AP admission limits.
//!
//! Peer storage, status snapshots, association identities, and TIM construction
//! share this bound. Hardware key-slot availability is a separate chip policy;
//! this portable module has no dependency on radio registers or a chip profile.
//! Variable-capacity AP service storage is a separate API change.

use core::fmt;

/// Software peer-table ceiling for one AP service epoch.
///
/// This bounds both Open and WPA2 service storage. Chip composition must prove
/// that its hardware resources accommodate every admitted encrypted peer.
pub const AP_MAX_CLIENTS: usize = 15;
/// TIM storage covering the bounded association IDs, including bitmap bit zero.
pub const AP_TIM_VIRTUAL_BITMAP_OCTETS: usize = AP_MAX_CLIENTS / 8 + 1;

/// Validated runtime admission limit for one AP epoch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccessPointClientLimit(u8);

impl AccessPointClientLimit {
    pub const MAX: u8 = AP_MAX_CLIENTS as u8;

    pub const fn new(value: u8) -> Result<Self, AccessPointClientLimitError> {
        if value == 0 || value > Self::MAX {
            return Err(AccessPointClientLimitError { value });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointClientLimitError {
    value: u8,
}

impl AccessPointClientLimitError {
    pub const fn value(self) -> u8 {
        self.value
    }
}

impl fmt::Display for AccessPointClientLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "access-point client limit {} is outside 1..={}",
            self.value,
            AccessPointClientLimit::MAX,
        )
    }
}

impl core::error::Error for AccessPointClientLimitError {}

#[cfg(test)]
mod tests;
