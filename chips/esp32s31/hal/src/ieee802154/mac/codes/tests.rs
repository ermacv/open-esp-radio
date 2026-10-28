use super::super::host::HostMemory;
use super::*;
use core::cmp::Ordering;
use oer_espressif_ieee802154_mac_values::Ieee802154RxStatus;

#[test]
fn every_named_rx_abort_reason_retains_its_semantic_identity() {
    use Ieee802154RxAbortReason as Reason;
    use rx_status::AbortReason as Code;
    for (code, reason) in [
        (Code::RxStop, Reason::RxStop),
        (Code::SfdTimeout, Reason::SfdTimeout),
        (Code::CrcError, Reason::CrcError),
        (Code::InvalidLength, Reason::InvalidLength),
        (Code::FilterFail, Reason::FilterFail),
        (Code::NoRss, Reason::NoRss),
        (Code::CoexBreak, Reason::CoexistenceBreak),
        (Code::UnexpectedAck, Reason::UnexpectedAck),
        (Code::RxRestart, Reason::RxRestart),
        (Code::TxAckTimeout, Reason::TxAckTimeout),
        (Code::TxAckStop, Reason::TxAckStop),
        (Code::TxAckCoexBreak, Reason::TxAckCoexistenceBreak),
        (
            Code::EnhancedAckSecurityError,
            Reason::EnhancedAckSecurityError,
        ),
        (Code::EdAbort, Reason::EdAbort),
        (Code::EdStop, Reason::EdStop),
        (Code::EdCoexReject, Reason::EdCoexistenceReject),
    ] {
        let mut memory = HostMemory::filled(code.register_bits());
        let (task, setup) = memory.owners();
        assert_eq!(
            task.rx_status().abort_reason(),
            Ieee802154RxAbortReasonObservation::Named(reason)
        );
        let (_startup, interrupt) = setup.arm(task);
        assert_eq!(
            interrupt.rx_abort_reason(),
            Ieee802154RxAbortReasonObservation::Named(reason)
        );
    }
    assert_eq!(
        rx_abort_reason(None),
        Ieee802154RxAbortReasonObservation::Unclassified
    );
}

#[test]
fn every_named_tx_abort_reason_retains_its_semantic_identity() {
    use Ieee802154TxAbortReason as Reason;
    use tx_status::AbortReason as Code;
    for (code, reason) in [
        (Code::RxAckStop, Reason::RxAckStop),
        (Code::RxAckSfdTimeout, Reason::RxAckSfdTimeout),
        (Code::RxAckCrcError, Reason::RxAckCrcError),
        (Code::RxAckInvalidLength, Reason::RxAckInvalidLength),
        (Code::RxAckFilterFail, Reason::RxAckFilterFail),
        (Code::RxAckNoRss, Reason::RxAckNoRss),
        (Code::RxAckCoexBreak, Reason::RxAckCoexistenceBreak),
        (Code::RxAckTypeNotAck, Reason::RxAckTypeNotAck),
        (Code::RxAckRestart, Reason::RxAckRestart),
        (Code::RxAckTimeout, Reason::RxAckTimeout),
        (Code::TxStop, Reason::TxStop),
        (Code::TxCoexBreak, Reason::TxCoexistenceBreak),
        (Code::TxSecurityError, Reason::TxSecurityError),
        (Code::CcaFailed, Reason::CcaFailed),
        (Code::CcaBusy, Reason::CcaBusy),
    ] {
        let mut memory = HostMemory::filled(code.register_bits());
        let (task, setup) = memory.owners();
        assert_eq!(
            task.tx_status().abort_reason(),
            Ieee802154TxAbortReasonObservation::Named(reason)
        );
        let (_startup, interrupt) = setup.arm(task);
        assert_eq!(
            interrupt.tx_abort_reason(),
            Ieee802154TxAbortReasonObservation::Named(reason)
        );
    }
    assert_eq!(
        tx_abort_reason(None),
        Ieee802154TxAbortReasonObservation::Unclassified
    );
}

#[test]
fn receive_state_classification_preserves_the_frame_in_progress_boundary() {
    let sfd = rx_status::State::ReceiveSfd.bits();
    for raw in 0..=rx_status::STATE_MAX {
        let state = rx_state(Bits::new(raw).expect("within the RX field"));
        let expected = raw.cmp(&sfd);
        let status = Ieee802154RxStatus::new(
            0,
            Ieee802154RxAbortReasonObservation::Unclassified,
            state,
            false,
            false,
        );
        assert_eq!(state.is_receive_sfd(), expected == Ordering::Equal);
        assert_eq!(status.frame_in_progress(), expected == Ordering::Greater);
        assert_eq!(u32::from(state.value()), raw);
        assert_eq!(state.is_zero(), raw == 0);
    }
}

#[test]
fn security_observations_distinguish_named_zero_and_unclassified_fields() {
    use Ieee802154TxSecurityError as Reason;
    use tx_status::SecurityError as Code;
    for (code, reason) in [
        (Code::FrameControlNotSet, Reason::FrameControlNotSet),
        (Code::ReservedSecurityLevel, Reason::ReservedSecurityLevel),
        (Code::HeaderParse, Reason::HeaderParse),
        (Code::PayloadError, Reason::PayloadError),
        (
            Code::FrameCounterSuppression,
            Reason::FrameCounterSuppression,
        ),
    ] {
        let mut memory = HostMemory::filled(code.register_bits());
        let (task, _setup) = memory.owners();
        assert_eq!(
            task.tx_status().security_error(),
            Ieee802154TxSecurityErrorObservation::Named(reason)
        );
    }
    assert_eq!(
        tx_security_error(None, true),
        Ieee802154TxSecurityErrorObservation::None
    );
    assert_eq!(
        tx_security_error(None, false),
        Ieee802154TxSecurityErrorObservation::Unclassified
    );
}
