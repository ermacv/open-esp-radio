//! Caller-selected protection values consumed by TX hardware.
use crate::tx::LegacyRate;

/// Receiver class taken from the MPDU Address 1 field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxReceiver {
    Individual,
    Group,
}

impl TxReceiver {
    /// Classify the receiver address (RA). The Ethernet destination of a
    /// To-DS frame is Address 3 and does not select the exchange.
    pub const fn from_address1(address1: &[u8; 6]) -> Self {
        if address1[0] & 1 != 0 {
            Self::Group
        } else {
            Self::Individual
        }
    }
}

/// Control exchange that precedes one PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtection {
    None,
    CtsToSelf { rate: LegacyRate },
    RtsCts { rate: LegacyRate },
}

impl TxProtection {
    /// Transmit rate of the protection frame, if any.
    pub const fn control_rate(self) -> Option<LegacyRate> {
        match self {
            Self::None => None,
            Self::CtsToSelf { rate } | Self::RtsCts { rate } => Some(rate),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxProtectionDecision {
    pub protection: TxProtection,
}

impl TxProtectionDecision {
    pub const UNPROTECTED: Self = Self {
        protection: TxProtection::None,
    };
}
