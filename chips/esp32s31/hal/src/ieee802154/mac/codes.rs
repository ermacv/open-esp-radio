//! Decode generated hardware names into the chip-independent MAC vocabulary.
//!
//! SOURCE: esp-idf-7b9cc1ac-ieee802154-common-ll, abort/security reason enums
//! and ieee802154_ll_is_current_rx_frame. Numeric code definitions stay in
//! the register model; unknown abort/security values remain unclassified.

use oer_esp32s31_pac::ieee802154::{rx_status, tx_status};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode,
    Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation, Ieee802154TxSecurityError,
    Ieee802154TxSecurityErrorObservation,
};
use oer_mmio::Bits;

use super::task::byte;

pub(super) fn rx_state(state: Bits<{ rx_status::STATE_WIDTH }>) -> Ieee802154RxStateCode {
    Ieee802154RxStateCode::from_parts(
        byte(state),
        state.get().cmp(&rx_status::State::ReceiveSfd.bits()),
    )
}

pub(super) const fn rx_abort_reason(
    reason: Option<rx_status::AbortReason>,
) -> Ieee802154RxAbortReasonObservation {
    use Ieee802154RxAbortReason as Reason;
    use rx_status::AbortReason as Code;
    Ieee802154RxAbortReasonObservation::Named(match reason {
        Some(Code::RxStop) => Reason::RxStop,
        Some(Code::SfdTimeout) => Reason::SfdTimeout,
        Some(Code::CrcError) => Reason::CrcError,
        Some(Code::InvalidLength) => Reason::InvalidLength,
        Some(Code::FilterFail) => Reason::FilterFail,
        Some(Code::NoRss) => Reason::NoRss,
        Some(Code::CoexBreak) => Reason::CoexistenceBreak,
        Some(Code::UnexpectedAck) => Reason::UnexpectedAck,
        Some(Code::RxRestart) => Reason::RxRestart,
        Some(Code::TxAckTimeout) => Reason::TxAckTimeout,
        Some(Code::TxAckStop) => Reason::TxAckStop,
        Some(Code::TxAckCoexBreak) => Reason::TxAckCoexistenceBreak,
        Some(Code::EnhancedAckSecurityError) => Reason::EnhancedAckSecurityError,
        Some(Code::EdAbort) => Reason::EdAbort,
        Some(Code::EdStop) => Reason::EdStop,
        Some(Code::EdCoexReject) => Reason::EdCoexistenceReject,
        None => return Ieee802154RxAbortReasonObservation::Unclassified,
    })
}

pub(super) const fn tx_abort_reason(
    reason: Option<tx_status::AbortReason>,
) -> Ieee802154TxAbortReasonObservation {
    use Ieee802154TxAbortReason as Reason;
    use tx_status::AbortReason as Code;
    Ieee802154TxAbortReasonObservation::Named(match reason {
        Some(Code::RxAckStop) => Reason::RxAckStop,
        Some(Code::RxAckSfdTimeout) => Reason::RxAckSfdTimeout,
        Some(Code::RxAckCrcError) => Reason::RxAckCrcError,
        Some(Code::RxAckInvalidLength) => Reason::RxAckInvalidLength,
        Some(Code::RxAckFilterFail) => Reason::RxAckFilterFail,
        Some(Code::RxAckNoRss) => Reason::RxAckNoRss,
        Some(Code::RxAckCoexBreak) => Reason::RxAckCoexistenceBreak,
        Some(Code::RxAckTypeNotAck) => Reason::RxAckTypeNotAck,
        Some(Code::RxAckRestart) => Reason::RxAckRestart,
        Some(Code::RxAckTimeout) => Reason::RxAckTimeout,
        Some(Code::TxStop) => Reason::TxStop,
        Some(Code::TxCoexBreak) => Reason::TxCoexistenceBreak,
        Some(Code::TxSecurityError) => Reason::TxSecurityError,
        Some(Code::CcaFailed) => Reason::CcaFailed,
        Some(Code::CcaBusy) => Reason::CcaBusy,
        None => return Ieee802154TxAbortReasonObservation::Unclassified,
    })
}

pub(super) const fn tx_security_error(
    reason: Option<tx_status::SecurityError>,
    is_zero: bool,
) -> Ieee802154TxSecurityErrorObservation {
    use Ieee802154TxSecurityError as Reason;
    use tx_status::SecurityError as Code;
    Ieee802154TxSecurityErrorObservation::Named(match reason {
        Some(Code::FrameControlNotSet) => Reason::FrameControlNotSet,
        Some(Code::ReservedSecurityLevel) => Reason::ReservedSecurityLevel,
        Some(Code::HeaderParse) => Reason::HeaderParse,
        Some(Code::PayloadError) => Reason::PayloadError,
        Some(Code::FrameCounterSuppression) => Reason::FrameCounterSuppression,
        // This is only a numeric-zero observation, not a sourced success code.
        None if is_zero => {
            return Ieee802154TxSecurityErrorObservation::None;
        }
        None => return Ieee802154TxSecurityErrorObservation::Unclassified,
    })
}

#[cfg(test)]
mod tests;
