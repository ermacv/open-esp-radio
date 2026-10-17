use core::ops::{BitAnd, BitOr, BitOrAssign};

use crate::TxMode;

/// A capability image contained bits unknown to this API version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityBitsError {
    /// Complete rejected image.
    pub bits: u32,
    /// Unknown bits from the rejected image.
    pub unknown: u32,
}

/// Portable controller capability bitset.
///
/// The private image and validating [`RadioCapabilities::from_bits`] prevent
/// adapters from silently publishing capabilities unknown to this contract.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct RadioCapabilities(u32);

impl RadioCapabilities {
    /// No optional operation is implemented.
    pub const NONE: Self = Self(0);
    /// Standalone clear-channel assessment.
    pub const CLEAR_CHANNEL_ASSESSMENT: Self = Self(1 << 0);
    /// Bounded CSMA-CA transmission.
    pub const CSMA_CA: Self = Self(1 << 1);
    /// Standalone energy detection scan.
    pub const ENERGY_SCAN: Self = Self(1 << 2);
    /// Hardware acknowledgement wait and correlation.
    pub const HARDWARE_ACKNOWLEDGEMENT: Self = Self(1 << 3);
    /// Scheduled monotonic-time transmission.
    pub const SCHEDULED_TRANSMIT: Self = Self(1 << 4);
    /// Promiscuous receive mode.
    pub const PROMISCUOUS: Self = Self(1 << 5);
    /// Receive timestamps in a monotonic radio epoch.
    pub const RECEIVE_TIMESTAMP: Self = Self(1 << 6);
    /// Automatic acknowledgement generation.
    pub const AUTOMATIC_ACKNOWLEDGEMENT: Self = Self(1 << 7);
    /// MAC security processing offload.
    pub const SECURITY_OFFLOAD: Self = Self(1 << 8);
    /// Hardware source matching and frame-pending selection.
    pub const SOURCE_MATCH: Self = Self(1 << 9);
    /// Retransmission after a transmission without acknowledgement or
    /// channel access.
    pub const TRANSMIT_RETRIES: Self = Self(1 << 10);
    /// Reception in a window that opens at a monotonic radio time.
    pub const SCHEDULED_RECEIVE: Self = Self(1 << 11);
    /// More than one addressing interface ([`Interface`](crate::Interface)).
    pub const MULTI_PAN: Self = Self(1 << 12);
    /// Network time written into a transmitted frame's Time IE at its SFD
    /// ([`TimeSync`](crate::TimeSync)).
    pub const TIME_SYNC: Self = Self(1 << 13);

    /// The bits this contract publishes; the image has room for more.
    const KNOWN: u32 = (1 << 14) - 1;

    /// Validate a serialized capability image.
    pub const fn from_bits(bits: u32) -> Result<Self, CapabilityBitsError> {
        let unknown = bits & !Self::KNOWN;
        if unknown == 0 {
            Ok(Self(bits))
        } else {
            Err(CapabilityBitsError { bits, unknown })
        }
    }

    /// Return the stable serialized image.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Return the union of two capability sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit in `required` is present.
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    /// Whether this controller supports the requested transmit mode.
    pub const fn supports_tx_mode(self, mode: TxMode) -> bool {
        match mode {
            TxMode::Direct => true,
            TxMode::ClearChannelAssessment => self.contains(Self::CLEAR_CHANNEL_ASSESSMENT),
            TxMode::CsmaCa { .. } => self.contains(Self::CSMA_CA),
            TxMode::Scheduled { cca, .. } => {
                self.contains(Self::SCHEDULED_TRANSMIT)
                    && (!cca || self.contains(Self::CLEAR_CHANNEL_ASSESSMENT))
            }
        }
    }
}

impl BitOr for RadioCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RadioCapabilities {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for RadioCapabilities {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

#[cfg(test)]
mod tests;
