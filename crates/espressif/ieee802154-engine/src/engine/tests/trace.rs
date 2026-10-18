//! The engine's trace points, decoded with the host's event set.

use std::{format, string::String, vec::Vec};

use oer_ieee802154_trace::Ieee802154Trace;

use super::*;
use crate::engine::trace::take_recorded;

/// The records emitted since the last call, as the host describes them.
fn described() -> Vec<String> {
    let sets: &[oer_trace::Describer] = &[<Ieee802154Trace as oer_trace::EventSet>::describe];
    take_recorded()
        .iter()
        .map(|record| format!("{}", oer_trace::Described { record, sets }))
        .collect()
}

/// A transmission requesting an ACK records its state changes, the armed
/// ACK timer, both interrupts and the missing-ACK outcome, in order.
#[test]
fn a_missed_ack_traces_the_transmission_to_its_outcome() {
    let mut bench = Bench::enabled();
    bench.env.now = 1_000;
    take_recorded();
    bench.transmit(&DATA_WITH_ACK, false);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    bench.interrupt(&[Ieee802154Event::Timer0Overflow]);
    assert_eq!(
        described(),
        [
            "ieee802154 state Idle -> Tx",
            "ieee802154 interrupt in Tx: tx_done",
            "ieee802154 state Tx -> RxAck",
            "ieee802154 Timer0 armed at 201000 for AckTimeout",
            "ieee802154 interrupt in RxAck: timer0_overflow",
            "ieee802154 Timer0 fired AckTimeout",
            "ieee802154 tx len=12 failed NoAck",
            "ieee802154 state RxAck -> Idle",
            "ieee802154 state Idle -> Sleep",
        ]
    );
}

/// A sampled abort is recorded with the state it arrived in and its named
/// reason before the handler acts on it.
#[test]
fn an_abort_is_traced_with_its_reason() {
    let mut bench = Bench::enabled();
    bench.engine.pib().set_rx_when_idle(true);
    bench.receive();
    take_recorded();
    bench.hw.rx_abort =
        Ieee802154RxAbortReasonObservation::Named(Ieee802154RxAbortReason::CrcError);
    bench.interrupt(&[Ieee802154Event::RxAbort]);
    assert_eq!(
        described(),
        [
            "ieee802154 interrupt in Rx: rx_abort",
            "ieee802154 abort in Rx: rx CrcError",
        ]
    );
}

/// Delivered frames carry their slot and metadata; a frame landing in the
/// stub buffer of a full ring is recorded as dropped.
#[test]
fn a_full_ring_traces_the_dropped_frame() {
    let mut bench = Bench::enabled();
    bench.engine.pib().set_rx_when_idle(true);
    bench.receive();
    for _ in 0..RX_BUFFER_COUNT {
        bench.deliver(&DATA_NO_ACK);
        bench.interrupt(&[Ieee802154Event::RxDone]);
    }
    let outcomes = |described: Vec<String>| -> Vec<String> {
        described
            .into_iter()
            .filter(|line| line.starts_with("ieee802154 rx "))
            .collect()
    };
    let delivered = outcomes(described());
    assert_eq!(delivered.len(), RX_BUFFER_COUNT);
    assert_eq!(
        delivered[1],
        "ieee802154 rx len=12 delivered slot=1 channel=11 rssi=0 lqi=0"
    );

    bench.deliver(&DATA_NO_ACK);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(
        outcomes(described()),
        ["ieee802154 rx len=12 dropped RingFull"]
    );
}
