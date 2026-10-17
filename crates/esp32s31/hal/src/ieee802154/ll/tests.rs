//! The conversions between the engine's values and the ESP32-S31 PAC types
//! lose nothing: every named event, abort reason and context survives the
//! round trip, and an unclassified PAC sample stays unclassified.

use super::*;
use std::format;

#[test]
fn every_event_and_event_set_reaches_the_pac_unchanged() {
    for (engine, pac) in EVENTS {
        assert_eq!(event_into_pac(engine), pac);
        let observed = event_observation_from_pac(PacEventObservation::from_named(
            event_mask_into_pac(engine.mask()),
        ));
        assert_eq!(observed.classification(), Ok(engine.mask()), "{engine:?}");
    }
    let all = event_mask_into_pac(Ieee802154EventMask::NAMED);
    assert_eq!(
        event_observation_from_pac(PacEventObservation::from_named(all)).classification(),
        Ok(Ieee802154EventMask::NAMED)
    );
}

/// Each PAC reason maps to the engine reason of the same name.
#[test]
fn abort_reasons_map_by_name() {
    use PacRxAbortReason as R;
    for reason in [
        R::RxStop,
        R::SfdTimeout,
        R::CrcError,
        R::InvalidLength,
        R::FilterFail,
        R::NoRss,
        R::CoexistenceBreak,
        R::UnexpectedAck,
        R::RxRestart,
        R::TxAckTimeout,
        R::TxAckStop,
        R::TxAckCoexistenceBreak,
        R::EnhancedAckSecurityError,
        R::EdAbort,
        R::EdStop,
        R::EdCoexistenceReject,
    ] {
        let converted = rx_abort_reason_from_pac(PacRxAbortReasonObservation::Named(reason));
        assert_eq!(format!("{converted:?}"), format!("Named({reason:?})"));
    }
    use PacTxAbortReason as T;
    for reason in [
        T::RxAckStop,
        T::RxAckSfdTimeout,
        T::RxAckCrcError,
        T::RxAckInvalidLength,
        T::RxAckFilterFail,
        T::RxAckNoRss,
        T::RxAckCoexistenceBreak,
        T::RxAckTypeNotAck,
        T::RxAckRestart,
        T::RxAckTimeout,
        T::TxStop,
        T::TxCoexistenceBreak,
        T::TxSecurityError,
        T::CcaFailed,
        T::CcaBusy,
    ] {
        let converted = tx_abort_reason_from_pac(PacTxAbortReasonObservation::Named(reason));
        assert_eq!(format!("{converted:?}"), format!("Named({reason:?})"));
    }
    assert_eq!(
        rx_abort_reason_from_pac(PacRxAbortReasonObservation::Unclassified),
        Ieee802154RxAbortReasonObservation::Unclassified
    );
    assert_eq!(
        tx_abort_reason_from_pac(PacTxAbortReasonObservation::Unclassified),
        Ieee802154TxAbortReasonObservation::Unclassified
    );
}

#[test]
fn multipan_state_round_trips() {
    for bits in 0..16u8 {
        let bit = |n: u8| bits & (1 << n) != 0;
        let state = Ieee802154MultipanEnableState::new(bit(0), bit(1), bit(2), bit(3));
        assert_eq!(
            multipan_state_from_pac(multipan_state_into_pac(state)),
            state
        );
    }
}
