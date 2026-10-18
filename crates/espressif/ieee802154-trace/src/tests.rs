use super::*;

extern crate std;

use std::format;

fn round_trip<E: Event + Copy + PartialEq + core::fmt::Debug>(event: E) {
    assert_eq!(E::decode(event.encode()), Some(event));
}

const STATES: [MacState; 11] = [
    MacState::Disable,
    MacState::Idle,
    MacState::Sleep,
    MacState::Rx,
    MacState::TxAck,
    MacState::TxEnhAck,
    MacState::TxCca,
    MacState::Tx,
    MacState::RxAck,
    MacState::Ed,
    MacState::Cca,
];

const RX_ABORTS: [RxAbortReason; 16] = [
    RxAbortReason::RxStop,
    RxAbortReason::SfdTimeout,
    RxAbortReason::CrcError,
    RxAbortReason::InvalidLength,
    RxAbortReason::FilterFail,
    RxAbortReason::NoRss,
    RxAbortReason::CoexistenceBreak,
    RxAbortReason::UnexpectedAck,
    RxAbortReason::RxRestart,
    RxAbortReason::TxAckTimeout,
    RxAbortReason::TxAckStop,
    RxAbortReason::TxAckCoexistenceBreak,
    RxAbortReason::EnhancedAckSecurityError,
    RxAbortReason::EdAbort,
    RxAbortReason::EdStop,
    RxAbortReason::EdCoexistenceReject,
];

const TX_ABORTS: [TxAbortReason; 15] = [
    TxAbortReason::RxAckStop,
    TxAbortReason::RxAckSfdTimeout,
    TxAbortReason::RxAckCrcError,
    TxAbortReason::RxAckInvalidLength,
    TxAbortReason::RxAckFilterFail,
    TxAbortReason::RxAckNoRss,
    TxAbortReason::RxAckCoexistenceBreak,
    TxAbortReason::RxAckTypeNotAck,
    TxAbortReason::RxAckRestart,
    TxAbortReason::RxAckTimeout,
    TxAbortReason::TxStop,
    TxAbortReason::TxCoexistenceBreak,
    TxAbortReason::TxSecurityError,
    TxAbortReason::CcaFailed,
    TxAbortReason::CcaBusy,
];

const CALLBACKS: [TimerCallback; 4] = [
    TimerCallback::None,
    TimerCallback::AckTimeout,
    TimerCallback::StartReceiveAt,
    TimerCallback::FinishReceiveAt,
];

#[test]
fn every_ieee802154_event_decodes_to_what_it_encoded() {
    for from in STATES {
        for to in STATES {
            round_trip(StateChange { from, to });
        }
        round_trip(Interrupt {
            state: from,
            events: InterruptEvents::TX_DONE
                .with(InterruptEvents::RX_SFD_DONE)
                .with(InterruptEvents::UNCLASSIFIED),
        });
        round_trip(Abort {
            state: from,
            reason: AbortReason::Rx(None),
        });
        round_trip(Abort {
            state: from,
            reason: AbortReason::Tx(None),
        });
    }
    for reason in RX_ABORTS {
        round_trip(Abort {
            state: MacState::Rx,
            reason: AbortReason::Rx(Some(reason)),
        });
    }
    for reason in TX_ABORTS {
        round_trip(Abort {
            state: MacState::RxAck,
            reason: AbortReason::Tx(Some(reason)),
        });
    }
    for result in [
        TxResult::Done { acked: false },
        TxResult::Done { acked: true },
        TxResult::Failed(TxFailure::CcaBusy),
        TxResult::Failed(TxFailure::Abort),
        TxResult::Failed(TxFailure::NoAck),
        TxResult::Failed(TxFailure::InvalidAck),
        TxResult::Failed(TxFailure::Coexist),
        TxResult::Failed(TxFailure::Security),
    ] {
        round_trip(TxOutcome {
            length: 127,
            result,
        });
    }
    for result in [
        RxResult::Delivered {
            slot: 19,
            channel: 26,
            rssi: -104,
            lqi: 255,
        },
        RxResult::Dropped(RxDrop::RingFull),
        RxResult::Dropped(RxDrop::QueueFull),
    ] {
        round_trip(RxOutcome { length: 5, result });
    }
    round_trip(Interrupt {
        state: MacState::Tx,
        events: InterruptEvents::NONE,
    });
    round_trip(PowerSequence {
        pa_on: 0x3ff,
        tx_on: 0x155,
        tx_enable_stop: 0x3f,
        tx_off: 0x2a,
        rx_on: 0x7ff,
        txrx_switch: 0x2aa,
        continuous_rx: 0x3f,
    });
    round_trip(PowerSequence::default());
    round_trip(DcdcControl {
        pre_raise: 0xff,
        drop: 0x10,
        enabled: true,
        raise_for_tx: true,
        reserved_set: 0x81,
    });
    round_trip(DcdcControl::default());
    for timer in [TimerId::Timer0, TimerId::Timer1] {
        for callback in CALLBACKS {
            round_trip(Timer {
                timer,
                op: TimerOp::Armed {
                    callback,
                    at: u32::MAX,
                },
            });
            round_trip(Timer {
                timer,
                op: TimerOp::Fired(callback),
            });
        }
        round_trip(Timer {
            timer,
            op: TimerOp::Stopped,
        });
    }
}

#[test]
fn power_fields_wider_than_the_vendor_field_are_truncated_alone() {
    let sequence = PowerSequence {
        pa_on: 0x400 | 7,
        ..PowerSequence::default()
    };
    assert_eq!(
        PowerSequence::decode(sequence.encode()),
        Some(PowerSequence {
            pa_on: 7,
            ..PowerSequence::default()
        })
    );
}

#[test]
fn reserved_set_names_each_word_with_a_reserved_bit() {
    assert_eq!(DcdcControl::reserved_set_of([0; 8]), 0);
    assert_eq!(
        DcdcControl::reserved_set_of([1 << 10, 0, 0, 0, 0, 0, 0, 1 << 30]),
        0x81
    );
    assert_eq!(
        DcdcControl::reserved_set_of([0, 0, 0, 7, 0, 0, 0, 0]),
        1 << 3
    );
}

#[test]
fn words_no_ieee802154_event_encodes_fail_to_decode() {
    assert_eq!(StateChange::decode([11, 0]), None);
    assert_eq!(StateChange::decode([11 << 8, 0]), None);
    assert_eq!(StateChange::decode([0, 1]), None);
    assert_eq!(StateChange::decode([1 << 16, 0]), None);
    assert_eq!(TxOutcome::decode([256, 0]), None);
    assert_eq!(TxOutcome::decode([0, 3]), None);
    assert_eq!(TxOutcome::decode([0, 1 << 8]), None);
    assert_eq!(TxOutcome::decode([0, 2]), None);
    assert_eq!(TxOutcome::decode([0, 2 | 7 << 8]), None);
    assert_eq!(RxOutcome::decode([2 << 8, 0]), None);
    assert_eq!(RxOutcome::decode([1 << 8 | 2 << 16, 0]), None);
    assert_eq!(RxOutcome::decode([1 << 8, 1]), None);
    assert_eq!(RxOutcome::decode([0, 1 << 24]), None);
    assert_eq!(RxOutcome::decode([1 << 24, 0]), None);
    assert_eq!(Abort::decode([2 << 8, 0]), None);
    assert_eq!(Abort::decode([10 << 16, 0]), None);
    assert_eq!(Abort::decode([1 << 8 | 26 << 16, 0]), None);
    assert_eq!(Abort::decode([11, 0]), None);
    assert_eq!(Abort::decode([0, 1]), None);
    assert_eq!(Interrupt::decode([1 << 12, 0]), None);
    assert_eq!(Interrupt::decode([0, 11]), None);
    assert_eq!(PowerSequence::decode([0, 1 << 27]), None);
    assert_eq!(DcdcControl::decode([1 << 18, 0]), None);
    assert_eq!(DcdcControl::decode([0, 256]), None);
    assert_eq!(Timer::decode([2, 0]), None);
    assert_eq!(Timer::decode([3 << 8, 0]), None);
    assert_eq!(Timer::decode([4 << 16, 0]), None);
    assert_eq!(Timer::decode([1 << 8, 1]), None);
    assert_eq!(Timer::decode([2 << 8, 1]), None);
}

#[test]
fn every_event_has_its_own_kind_and_power_events_share_a_channel() {
    let kinds = [
        StateChange::KIND,
        TxOutcome::KIND,
        RxOutcome::KIND,
        Abort::KIND,
        Interrupt::KIND,
        PowerSequence::KIND,
        DcdcControl::KIND,
        Timer::KIND,
    ];
    for (index, kind) in kinds.iter().enumerate() {
        assert_eq!(kind.domain, Domain::Ieee802154);
        assert!(kinds[index + 1..].iter().all(|other| other != kind));
    }
    let channels = [
        StateChange::CHANNEL,
        TxOutcome::CHANNEL,
        RxOutcome::CHANNEL,
        Abort::CHANNEL,
        Interrupt::CHANNEL,
        PowerSequence::CHANNEL,
        Timer::CHANNEL,
    ];
    for (index, channel) in channels.iter().enumerate() {
        assert!(channels[index + 1..].iter().all(|other| other != channel));
    }
    assert_eq!(DcdcControl::CHANNEL, PowerSequence::CHANNEL);
}

#[test]
fn the_event_set_describes_a_drained_record() {
    let record = |event: &dyn Fn() -> (Kind, [u32; 2])| {
        let (kind, words) = event();
        oer_trace::Record {
            tag: 1,
            kind: kind.raw(),
            t_us: 0,
            words,
        }
    };
    let sets: &[oer_trace::Describer] = &[<Ieee802154Trace as oer_trace::EventSet>::describe];
    let describe =
        |record: &oer_trace::Record| format!("{}", oer_trace::Described { record, sets });
    let interrupt = record(&|| {
        (
            Interrupt::KIND,
            Interrupt {
                state: MacState::RxAck,
                events: InterruptEvents::TX_ABORT.with(InterruptEvents::TIMER0_OVERFLOW),
            }
            .encode(),
        )
    });
    assert_eq!(
        describe(&interrupt),
        "ieee802154 interrupt in RxAck: tx_abort+timer0_overflow"
    );
    let abort = record(&|| {
        (
            Abort::KIND,
            Abort {
                state: MacState::RxAck,
                reason: AbortReason::Tx(Some(TxAbortReason::RxAckTimeout)),
            }
            .encode(),
        )
    });
    assert_eq!(
        describe(&abort),
        "ieee802154 abort in RxAck: tx RxAckTimeout"
    );
    let foreign = oer_trace::Record {
        tag: 1,
        kind: Kind::new(Domain::Ieee802154, 0x7f).raw(),
        t_us: 0,
        words: [1, 2],
    };
    assert!(describe(&foreign).starts_with("ieee802154.127"));
}
