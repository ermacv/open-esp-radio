//! Receive and transmit state observations and the public LL enable sets.

use crate::Ieee802154TxAbortReasonObservation;

/// Opaque three-bit receive-state observation.
///
/// Only the comparison around the publicly identified `RECEIVE_SFD` value is
/// exposed. Zero is intentionally not named `idle` until lifecycle evidence
/// proves that interpretation for the chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStateCode(u8);

impl Ieee802154RxStateCode {
    pub const MAX: u8 = 0x07;
    pub const RECEIVE_SFD: u8 = 1;

    pub const fn is_receive_sfd(self) -> bool {
        self.0 == Self::RECEIVE_SFD
    }

    pub const fn is_after_receive_sfd(self) -> bool {
        self.0 > Self::RECEIVE_SFD
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    #[doc(hidden)]
    pub const fn from_field(value: u8) -> Self {
        Self(value)
    }

    /// A three-bit state observation, as a register model reports it.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// Opaque four-bit transmit-state observation.
///
/// No individual value is assigned a lifecycle meaning by this foundation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TxStateCode(u8);

impl Ieee802154TxStateCode {
    pub const MAX: u8 = 0x0f;

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    #[doc(hidden)]
    pub const fn from_field(value: u8) -> Self {
        Self(value)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// One paired receive/transmit state sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154StateSnapshot {
    rx: Ieee802154RxStateCode,
    tx: Ieee802154TxStateCode,
}

impl Ieee802154StateSnapshot {
    pub const fn new(rx: Ieee802154RxStateCode, tx: Ieee802154TxStateCode) -> Self {
        Self { rx, tx }
    }

    pub const fn rx(self) -> Ieee802154RxStateCode {
        self.rx
    }

    pub const fn tx(self) -> Ieee802154TxStateCode {
        self.tx
    }

    /// Test only the observed numeric state codes.
    ///
    /// This is not a reset-readiness or quiescence claim. Those semantic
    /// predicates require a reviewed lifecycle and shared-reset model.
    pub const fn all_codes_zero(self) -> bool {
        self.rx.is_zero() && self.tx.is_zero()
    }
}

/// Energy-detection sample reduction (`ieee802154_ll_ed_sample_mode_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EdSampleMode {
    /// Report the maximum sample.
    Maximum,
    /// Report the average sample.
    Average,
}

/// A receive-abort enable set named by the public LL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154RxAbortEnableSet {
    /// `TX_ACK_TIMEOUT` and `TX_ACK_COEX_BREAK`, as enabled by MAC init.
    RuntimeBaseline,
    /// `IEEE802154_RX_ABORT_ALL`.
    All,
}

impl Ieee802154RxAbortEnableSet {
    /// Bit `reason - 1` of every reason in the set.
    #[doc(hidden)]
    pub const fn mask(self) -> u32 {
        match self {
            Self::RuntimeBaseline => (1 << (16 - 1)) | (1 << (18 - 1)),
            Self::All => 0x7fff_ffff,
        }
    }
}

/// A transmit-abort enable set named by the public LL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154TxAbortEnableSet {
    /// `RX_ACK_TIMEOUT`, `TX_COEX_BREAK`, `TX_SECURITY_ERROR`, `CCA_FAILED`
    /// and `CCA_BUSY`, as enabled by MAC init.
    RuntimeBaseline,
    /// `IEEE802154_TX_ABORT_ALL`.
    All,
}

impl Ieee802154TxAbortEnableSet {
    /// Bit `reason - 1` of every reason in the set.
    #[doc(hidden)]
    pub const fn mask(self) -> u32 {
        match self {
            Self::RuntimeBaseline => {
                (1 << (16 - 1))
                    | (1 << (18 - 1))
                    | (1 << (19 - 1))
                    | (1 << (24 - 1))
                    | (1 << (25 - 1))
            }
            Self::All => 0x7fff_ffff,
        }
    }
}

/// Transmit-security failure (`ieee802154_ll_tx_security_failed_reason_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154TxSecurityError {
    /// `IEEE802154_TX_SEC_FRAME_CTRL_NOT_SET`.
    FrameControlNotSet,
    /// `IEEE802154_TX_SEC_RESERVED_SEC_LEVEL`.
    ReservedSecurityLevel,
    /// `IEEE802154_TX_SEC_HEADER_PARSE`.
    HeaderParse,
    /// `IEEE802154_TX_SEC_PAYLOAD_ERROR`.
    PayloadError,
    /// `IEEE802154_TX_SEC_FRAME_COUNTER_SUP`.
    FrameCounterSuppression,
}

/// Observed transmit-security error field.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154TxSecurityErrorObservation {
    /// The field is zero.
    None,
    /// The field matched a public-LL reason.
    Named(Ieee802154TxSecurityError),
    /// The field value has no public-LL identity.
    Unclassified,
}

impl Ieee802154TxSecurityErrorObservation {
    /// Classify one sampled error field.
    #[doc(hidden)]
    pub const fn from_field(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Named(Ieee802154TxSecurityError::FrameControlNotSet),
            2 => Self::Named(Ieee802154TxSecurityError::ReservedSecurityLevel),
            3 => Self::Named(Ieee802154TxSecurityError::HeaderParse),
            4 => Self::Named(Ieee802154TxSecurityError::PayloadError),
            5 => Self::Named(Ieee802154TxSecurityError::FrameCounterSuppression),
            _ => Self::Unclassified,
        }
    }
}

/// Complete `TX_STATUS` observation (`ieee802154_ll_get_tx_status`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TxStatus {
    state: Ieee802154TxStateCode,
    abort_reason: Ieee802154TxAbortReasonObservation,
    security_error: Ieee802154TxSecurityErrorObservation,
}

impl Ieee802154TxStatus {
    /// One observation, as a register model reports it. Observations grant
    /// no write authority.
    pub const fn new(
        state: Ieee802154TxStateCode,
        abort_reason: Ieee802154TxAbortReasonObservation,
        security_error: Ieee802154TxSecurityErrorObservation,
    ) -> Self {
        Self {
            state,
            abort_reason,
            security_error,
        }
    }

    /// Transmitter state code.
    pub const fn state(&self) -> Ieee802154TxStateCode {
        self.state
    }

    /// Transmit-abort reason of the last abort.
    pub const fn abort_reason(&self) -> Ieee802154TxAbortReasonObservation {
        self.abort_reason
    }

    /// `ieee802154_ll_get_tx_security_failed_reason`.
    pub const fn security_error(&self) -> Ieee802154TxSecurityErrorObservation {
        self.security_error
    }
}

#[cfg(test)]
mod tests;
