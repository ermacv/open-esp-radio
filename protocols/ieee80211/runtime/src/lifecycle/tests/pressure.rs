//! Received data the network does not take, each limit on its own: the
//! network's task late while the port lends all it may, the copies filling
//! their queue to the network first, and a receive agreement's control
//! frames while the network is late. A late network holds the ring back
//! for at most [`NETWORK_LATE`], losing nothing; past it the actor walks
//! the ring to its end, refuses what no queue takes, counted by reason,
//! with its buffer back at once, and the station's frames go on.
//!
//! Here the script is the network: it takes nothing until it says so. The
//! model's port lends [`MODEL_RX_EVENTS`] buffers, the copy queue holds
//! [`RX_COPIED_DEPTH`] copies and the queue to the network [`RX_DEPTH`]
//! marks and loans together.

use super::*;
use crate::datapath::NETWORK_LATE;
use crate::station::RX_COPIED_DEPTH;
use crate::test_fixtures::block_ack::{addba, data, ht_beacon, ht_config, response};
use crate::test_fixtures::open_amsdu;
use oer_protocols_ieee80211_testing_model::MODEL_RX_EVENTS;
use oer_protocols_ieee80211_wire::block_ack::frame::{BlockAckAction, encode_block_ack_request};
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// The access point's other station, the source of its data.
const SOURCE: [u8; 6] = [0x02, 0, 0, 0, 0, 0x08];

/// The MSDUs the actor refused for full queues and for pressure on its
/// handoff, and those it copied because the port kept their buffer.
fn pressure(world: &World<'_>) -> (usize, usize, usize) {
    let refused = world.observed.service(|event| match event {
        ServiceEvent::RxMsdusRefused(refused) => Some(refused),
        _ => None,
    });
    let kept = world
        .observed
        .service(|event| match event {
            ServiceEvent::RxMsdusCopied(copied) => Some(usize::from(copied.kept)),
            _ => None,
        })
        .iter()
        .sum();
    (
        refused
            .iter()
            .map(|refused| usize::from(refused.queue_full))
            .sum(),
        refused
            .iter()
            .map(|refused| usize::from(refused.pressure))
            .sum(),
        kept,
    )
}

/// The network's task is late while the access point sends twelve frames
/// of one MSDU each. The port's lend limit fills first: four are lent, the
/// fifth waits in delivery order and the rest wait in the ring, nothing
/// copied or dropped. Once the network has been late for its bound, the
/// actor walks the ring: the waiting frame and the next three are copied
/// by pressure until the copy queue is full, and the last four are refused
/// for pressure on the handoff, each counted and its buffer back at once.
/// Once the network takes its queue, the eight arrive and the loans come
/// back: the next four are lent again.
#[test]
fn a_late_network_holds_the_ring_back_for_its_bound_then_the_ring_is_walked() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                world.settle().await;
                for sequence in 1..=12 {
                    world.air.receive(&open_data(sequence, SOURCE), meta());
                }
                world.settle().await;
                assert_eq!(world.air.pending_rx(), 12 - MODEL_RX_EVENTS - 1);
                assert_eq!(pressure(world), (0, 0, 0), "nothing lost within the bound");
                world.advance_by(NETWORK_LATE);
                world.settle().await;
                assert_eq!(world.air.pending_rx(), 0, "the ring is walked to its end");
                let copied = RX_COPIED_DEPTH;
                assert_eq!(
                    pressure(world),
                    (0, 12 - MODEL_RX_EVENTS - copied, 12 - MODEL_RX_EVENTS)
                );
                assert_eq!(world.delivered(), MODEL_RX_EVENTS + copied);
                assert_eq!(world.lent.get(), MODEL_RX_EVENTS);
                for sequence in 13..=16 {
                    world.air.receive(&open_data(sequence, SOURCE), meta());
                }
                world.settle().await;
                assert_eq!(world.delivered(), 4);
                assert_eq!(world.lent.get(), 2 * MODEL_RX_EVENTS, "the loans came back");
                assert_eq!(pressure(world).1, 12 - MODEL_RX_EVENTS - copied);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The network's task is late while the access point sends A-MSDUs of two
/// MSDUs each. Their copies fill the copy queue before any loan: the third
/// A-MSDU's MSDUs are refused for a full queue, while a following frame of
/// one MSDU is still lent, as the port's lend limit has room. Nothing holds
/// the ring back: it is walked to its end at once, and the network then
/// takes the four copies and the loan.
#[test]
fn copies_fill_their_queue_before_any_loan_without_stopping_the_ring() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                world.settle().await;
                for (sequence, marker) in [(1_u16, 1_u8), (2, 3), (3, 5)] {
                    let mut frame = open_amsdu(SOURCE, &[&[marker; 20], &[marker + 1; 20]]);
                    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
                    world.air.receive(&frame, meta());
                }
                world.air.receive(&open_data(4, SOURCE), meta());
                world.settle().await;
                assert_eq!(world.air.pending_rx(), 0, "the ring is walked to its end");
                assert_eq!(pressure(world), (2, 0, 0), "no copy by pressure");
                let delivered = world.delivered_frames();
                assert_eq!(delivered.len(), RX_COPIED_DEPTH + 1);
                for (frame, marker) in delivered.iter().zip([1_u8, 2, 3, 4]) {
                    assert_eq!(frame[14..], [marker; 20]);
                }
                assert_eq!(world.lent.get(), 1, "the frame of one MSDU is lent");
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// Under a receive agreement the network's task is late: of the access
/// point's eight frames in order four are lent and the fifth waits, and a
/// frame after a gap, the access point's BlockAckReq and its next ADDBA
/// Request wait in the ring behind them. Once the network has been late
/// for its bound the actor walks the ring: four frames are copied by
/// pressure, the frame after the gap waits in the reorder window until the
/// BlockAckReq moves the window past the gap and releases it, and it is
/// refused for pressure, counted; the ADDBA Request is answered while the
/// network is still late. The frame at the BlockAckReq's start is refused
/// the same way. The ring is walked to its end. Once the network takes
/// its queue it has the eight in order, and the agreement goes on in
/// sequence with the loans back.
#[test]
fn a_late_network_leaves_the_receive_agreements_answering() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate_with(&mut seen, ht_beacon(), false).await;
                world.connected().await;
                world.settle().await;
                world.air.receive(&addba(7, 0, 10, 0), meta());
                response(&world.sent(&mut seen, 0x00d0).await);
                for sequence in 10..18 {
                    world
                        .air
                        .receive(&data(sequence, 0, sequence as u8), meta());
                }
                world.air.receive(&data(19, 0, 19), meta());
                let bar =
                    encode_block_ack_request(LOCAL, BSSID, 0, SequenceNumber::new(20).unwrap());
                world.air.receive(&bar, meta());
                world.air.receive(&addba(8, 1, 0, 0), meta());
                world.settle().await;
                assert_ne!(world.air.pending_rx(), 0, "held back within the bound");
                // The wait for the answer moves the clock past the bound.
                assert!(matches!(
                    response(&world.sent(&mut seen, 0x00d0).await),
                    BlockAckAction::AddbaResponse {
                        dialog_token: 8,
                        status: 0,
                        tid: 1,
                        ..
                    }
                ));
                assert_eq!(pressure(world), (0, 1, 5), "the released frame is refused");
                world.air.receive(&data(20, 0, 20), meta());
                world.settle().await;
                assert_eq!(pressure(world), (0, 2, 6));
                assert_eq!(world.air.pending_rx(), 0, "the ring is walked to its end");
                assert_eq!(world.markers().await, (10..18).collect::<Vec<u8>>());
                world.air.receive(&data(21, 0, 21), meta());
                assert_eq!(world.markers().await, [21], "the window went on");
                assert_eq!(world.lent.get(), MODEL_RX_EVENTS + 1);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}
