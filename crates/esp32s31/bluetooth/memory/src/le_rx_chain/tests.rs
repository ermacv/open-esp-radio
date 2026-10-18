use std::{boxed::Box, vec::Vec};

use super::{
    LeRxChain, LeRxChainError, LeRxChainModelAddress, LeRxChainStorage, LeRxSource, LeRxTag,
};
use crate::{
    le_rx_packet::{LeRxError, LeRxOutcome},
    rx_memory_list::RxMemoryListClass,
};

const A: LeRxTag = LeRxTag(1);
const B: LeRxTag = LeRxTag(7);

fn chain() -> LeRxChain<3> {
    let storage = Box::leak(Box::new(LeRxChainStorage::<3>::new()));
    LeRxChain::bind_model(
        storage,
        RxMemoryListClass::NonScanning,
        LeRxChainModelAddress::new(0x2f00_4000).expect("test base is aligned"),
    )
    .expect("test chain binds")
}

fn source(tag: LeRxTag) -> LeRxSource {
    LeRxSource::new(tag, RxMemoryListClass::NonScanning, false)
}

fn pdu(byte: u8) -> [u8; 3] {
    [0x02, 1, byte]
}

fn received(outcome: Option<LeRxOutcome>) -> u8 {
    match outcome {
        Some(LeRxOutcome::Received(pdu)) => pdu.as_bytes()[2],
        other => panic!("expected a received PDU, got {other:?}"),
    }
}

/// Headers in the chain plus the spare always account for every header.
fn assert_conserved(chain: &mut LeRxChain<3>) {
    let mut headers: Vec<_> = chain
        .with_view(|view| view.chain())
        .iter()
        .map(|(header, _, _)| *header)
        .collect();
    headers.extend(chain.ring.spare);
    headers.sort_unstable();
    assert_eq!(headers, [0, 1, 2, 3]);
}

#[test]
fn the_chain_starts_at_a_completed_packetless_cursor() {
    let mut chain = chain();
    assert_eq!(
        chain.with_view(|view| view.chain()),
        [
            (0, false, true),
            (1, true, false),
            (2, true, false),
            (3, true, false)
        ]
    );
    assert_eq!(
        chain.initial_cursor(),
        chain.ring.cursor.controller_address()
    );
    assert_eq!(chain.class(), RxMemoryListClass::NonScanning);
}

#[test]
fn a_controller_reset_returns_the_chain_to_its_bound_image() {
    let mut chain = chain();
    let initial = chain.with_view(|view| view.chain());
    assert!(chain.emulate_receive(&pdu(5), A.number(), 1_000));
    assert!(chain.emulate_receive(&pdu(6), A.number(), 1_000));
    assert_eq!(received(chain.take(source(A)).unwrap()), 5);
    assert_ne!(chain.with_view(|view| view.chain()), initial);

    chain.reset_after_controller_reset(
        &oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation(),
    );

    assert_eq!(chain.with_view(|view| view.chain()), initial);
    assert!(
        matches!(chain.take(source(A)), Ok(None)),
        "no packet survives the reset"
    );
    assert_conserved(&mut chain);
}

#[test]
fn packets_return_in_order_and_the_current_node_keeps_its_header() {
    let mut chain = chain();
    assert_eq!(chain.take(source(A)), Ok(None));
    assert!(chain.emulate_receive(&pdu(1), A.number(), 1_000));
    assert!(chain.emulate_receive(&pdu(2), A.number(), 1_000));

    assert_eq!(received(chain.take(source(A)).unwrap()), 1);
    // The cursor left behind became the spare header.
    assert_eq!(chain.ring.spare, Some(0));
    assert_eq!(received(chain.take(source(A)).unwrap()), 2);
    assert_eq!(chain.take(source(A)), Ok(None));
    assert_conserved(&mut chain);

    // Hardware continues after the packetless header it still holds.
    assert!(chain.emulate_receive(&pdu(3), A.number(), 1_000));
    assert_eq!(received(chain.take(source(A)).unwrap()), 3);
    assert_conserved(&mut chain);
}

#[test]
fn each_tag_takes_only_its_packets() {
    let mut chain = chain();
    assert!(chain.emulate_receive(&pdu(1), A.number(), 1_000));
    assert!(chain.emulate_receive(&pdu(2), B.number(), 1_000));
    assert!(chain.emulate_receive(&pdu(3), A.number(), 1_000));

    assert_eq!(received(chain.take(source(A)).unwrap()), 1);
    assert_eq!(received(chain.take(source(A)).unwrap()), 3);
    assert_eq!(chain.take(source(A)), Ok(None));
    assert_eq!(received(chain.take(source(B)).unwrap()), 2);
    assert_eq!(chain.take(source(B)), Ok(None));
    assert_conserved(&mut chain);
}

#[test]
fn the_chain_rotates_without_losing_nodes() {
    let mut chain = chain();
    for round in 0..20u8 {
        assert!(chain.emulate_receive(&pdu(round), A.number(), 1_000));
        if round % 3 == 0 {
            assert!(chain.emulate_receive(&pdu(round | 0x80), A.number(), 1_000));
            assert_eq!(received(chain.take(source(A)).unwrap()), round);
            assert_eq!(received(chain.take(source(A)).unwrap()), round | 0x80);
        } else {
            assert_eq!(received(chain.take(source(A)).unwrap()), round);
        }
        assert_eq!(chain.take(source(A)), Ok(None));
        assert_conserved(&mut chain);
    }
}

#[test]
fn a_discarded_packet_returns_its_node() {
    let mut chain = chain();
    assert!(chain.emulate_receive(&pdu(1), A.number(), 1_000));
    chain.with_view(|view| view.packet(0).emulate_hardware_discard());
    assert_eq!(chain.take(source(A)), Ok(Some(LeRxOutcome::Discarded)));
    assert_conserved(&mut chain);
}

#[test]
fn a_retained_producer_sentinel_stops_the_chain() {
    let mut chain = chain();
    assert!(chain.emulate_receive(&pdu(1), A.number(), 1_000));
    chain.with_view(|view| {
        view.packet(0).rearm();
        view.packet(0).emulate_receive_tag(A.number());
    });
    assert_eq!(
        chain.take(source(A)),
        Err(LeRxChainError::Packet(LeRxError::ProducerSentinelRetained))
    );
}

#[test]
fn tags_are_twelve_bits() {
    assert_eq!(LeRxTag::new(0x0fff), Some(LeRxTag(0x0fff)));
    assert_eq!(LeRxTag::new(0x1000), None);
}

#[test]
fn a_source_of_the_other_class_is_refused() {
    let mut chain = chain();
    let scanning = LeRxSource::new(A, RxMemoryListClass::Scanning, false);
    assert_eq!(chain.take(scanning), Err(LeRxChainError::ForeignClass));
}
