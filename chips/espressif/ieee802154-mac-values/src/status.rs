// SPDX-License-Identifier: Apache-2.0
//! Receive and transmit state observations and the public LL enable sets.

use core::cmp::Ordering;

use crate::Ieee802154TxAbortReasonObservation;

/// Opaque receive-state image and its chip-classified relation to RECEIVE_SFD.
///
/// The HAL owns field geometry and the RECEIVE_SFD code. Zero remains a numeric
/// observation, without a claim that it means idle or reset readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStateCode {
    value: u8,
    receive_sfd: Ordering,
}

impl Ieee802154RxStateCode {
    /// Combine a diagnostic image with the HAL's comparison to RECEIVE_SFD.
    pub const fn from_parts(value: u8, receive_sfd: Ordering) -> Self {
        Self { value, receive_sfd }
    }

    pub const fn is_receive_sfd(self) -> bool {
        matches!(self.receive_sfd, Ordering::Equal)
    }

    pub const fn is_after_receive_sfd(self) -> bool {
        matches!(self.receive_sfd, Ordering::Greater)
    }

    pub const fn is_zero(self) -> bool {
        self.value == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.value
    }
}

/// Opaque transmit-state observation.
///
/// No individual value is assigned a lifecycle meaning by this foundation.
/// The HAL owns field geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TxStateCode(u8);

impl Ieee802154TxStateCode {
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    pub const fn from_field(value: u8) -> Self {
        Self(value)
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

/// A transmit-abort enable set named by the public LL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154TxAbortEnableSet {
    /// `RX_ACK_TIMEOUT`, `TX_COEX_BREAK`, `TX_SECURITY_ERROR`, `CCA_FAILED`
    /// and `CCA_BUSY`, as enabled by MAC init.
    RuntimeBaseline,
    /// `IEEE802154_TX_ABORT_ALL`.
    All,
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
