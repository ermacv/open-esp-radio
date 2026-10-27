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

/// Why one PPDU is protected; several sources can apply at once.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxProtectionReasons(u8);

impl TxProtectionReasons {
    pub const ERP: Self = Self(1 << 0);
    pub const HT: Self = Self(1 << 1);
    pub const HE_TXOP_DURATION: Self = Self(1 << 2);
    pub const LENGTH: Self = Self(1 << 3);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
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
    pub reasons: TxProtectionReasons,
}

impl TxProtectionDecision {
    pub const UNPROTECTED: Self = Self {
        protection: TxProtection::None,
        reasons: TxProtectionReasons(0),
    };
}
