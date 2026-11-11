#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "tests assert on fixed fixtures"
)]

extern crate std;

use super::*;
use crate::egress::Exchange;
use crate::station::{SameHart, State};
use core::task::Waker;
use oer_portable_memory::WakingSlotQueue;
use oer_protocols_bridges_ieee80211_ethernet::qos::classify_wmm;
use oer_protocols_ieee80211_contracts_port::TxEnd;
use oer_protocols_network_port::LinkSnapshot;
use oer_protocols_network_wire::ip::Dscp;
use std::vec::Vec;

/// One of the network's buffers in a test: its place and its octets.
#[derive(Debug)]
struct Buffer {
    place: u8,
    bytes: Vec<u8>,
}

impl TxStorage for Buffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

/// A pool that publishes a buffer as it is, or loses it when told to.
#[derive(Default)]
struct Pool {
    loses: bool,
}

impl TxPool for Pool {
    type Slot = Buffer;
    type Memory = Buffer;
    fn place(slot: &Buffer) -> u8 {
        slot.place
    }
    fn publish(&mut self, slot: Buffer, length: usize) -> Result<Buffer, PoolRefused<Buffer>> {
        if self.loses {
            return Err(PoolRefused::Lost);
        }
        if length > slot.bytes.len() {
            return Err(PoolRefused::Range(slot));
        }
        Ok(slot)
    }
    fn slot(&mut self, memory: Buffer) -> Buffer {
        memory
    }
}

const SLOTS: usize = 2;
const TX: usize = 2;
type Tx<'c> = LocalTx<'c, Pool, SLOTS, TX, SameHart>;
type Queue = WakingSlotQueue<Option<Outgoing<Buffer>>, TX>;
type Back = WakingSlotQueue<Option<TxEnded<Buffer>>, TX>;

/// Every buffer the network holds, of `octets` each.
fn buffers(octets: usize) -> heapless::Vec<Buffer, SLOTS> {
    buffers_of(SLOTS, octets)
}

/// `count` buffers of `octets` each, at the first places.
fn buffers_of(count: usize, octets: usize) -> heapless::Vec<Buffer, SLOTS> {
    (0..count)
        .map(|place| Buffer {
            place: u8::try_from(place).unwrap(),
            bytes: std::vec![0; octets],
        })
        .collect()
}

fn up(state: &State) {
    state.set_link(LinkSnapshot {
        state: LinkState::Up,
        epoch: 7,
    });
}

/// A frame of `length` octets that is not IP.
const fn plain(length: usize) -> FrameMeta {
    FrameMeta { length, dscp: None }
}

/// A fill's own error.
#[derive(Debug, Eq, PartialEq)]
struct Declined;

fn octets(slot: &mut Slot<'_>) -> Result<(), Declined> {
    slot.open().0.fill(7);
    Ok(())
}

fn unreachable_fill(why: &'static str) -> impl FnOnce(&mut Slot<'_>) -> Result<(), Declined> {
    move |_| panic!("{why}")
}

/// An IPv4 frame of `length` octets whose header carries `dscp`, as the
/// network core writes it.
fn ipv4(dscp: u8) -> impl FnOnce(&mut Slot<'_>) -> Result<(), Declined> {
    move |slot| {
        let bytes = slot.open().0;
        bytes.fill(0);
        bytes[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        // Version 4, five words of header.
        bytes[14] = 0x45;
        bytes[15] = dscp << 2;
        let total = u16::try_from(bytes.len() - 14).unwrap();
        bytes[16..18].copy_from_slice(&total.to_be_bytes());
        bytes[22] = 64;
        bytes[23] = 17;
        Ok(())
    }
}

fn ended(memory: Buffer) -> TxEnded<Buffer> {
    TxEnded {
        memory,
        end: TxEnd::NONE,
    }
}

#[test]
fn a_frame_goes_to_the_station_with_its_length_and_the_priority_its_marking_requests() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(128),
        state.network_link(),
    );
    // CS5 asks for video; an Ethernet frame that is not IP for best effort.
    for (meta, fill) in [
        (
            FrameMeta {
                length: 60,
                dscp: Dscp::new(40),
            },
            ipv4(40),
        ),
        (
            FrameMeta {
                length: 60,
                dscp: Dscp::new(0),
            },
            ipv4(0),
        ),
    ] {
        let ticket = tx.publish(meta, fill).unwrap();
        tx.flush();
        let Ok(Some(outgoing)) = receiver.try_receive_with(Option::take) else {
            panic!("the frame went to the station");
        };
        assert_eq!(outgoing.memory.place, ticket);
        assert_eq!(usize::from(outgoing.length), meta.length);
        // The priority the port was told is the one the written frame asks.
        let written = &outgoing.memory.bytes[..meta.length];
        let ether_type = u16::from_be_bytes([written[12], written[13]]);
        assert_eq!(
            outgoing.priority,
            classify_wmm(ether_type, &written[14..]).user_priority
        );
        assert_eq!(outgoing.priority, station_user_priority(meta.dscp.unwrap()));
    }
}

#[test]
fn a_frame_that_is_not_ip_requests_best_effort() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    tx.publish(plain(42), octets).unwrap();
    tx.flush();
    let Ok(Some(outgoing)) = receiver.try_receive_with(Option::take) else {
        panic!("the frame went to the station");
    };
    assert_eq!(outgoing.priority, WmmUserPriority::UP0);
    assert_eq!(&outgoing.memory.bytes[..42], &[7; 42]);
}

#[test]
fn a_down_link_refuses_before_any_buffer_or_fill() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    let (sender, mut receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    assert_eq!(
        tx.publish(plain(42), unreachable_fill("the link is down")),
        Err(Unpublished::Port(PortRefusal::LinkDown))
    );
    up(&state);
    // Both buffers are still free.
    tx.publish(plain(42), octets).unwrap();
    tx.publish(plain(42), octets).unwrap();
    tx.flush();
    assert!(receiver.try_receive_with(Option::take).is_ok());
}

#[test]
fn a_frame_the_station_cannot_carry_is_refused_before_any_fill() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, _receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(4096),
        state.network_link(),
    );
    let long = ETHERNET_HEADER + MAX_ETHERNET_PAYLOAD_LEN + 1;
    for length in [ETHERNET_HEADER - 1, long] {
        assert_eq!(
            tx.publish(plain(length), unreachable_fill("not a frame to carry")),
            Err(Unpublished::Port(PortRefusal::BadFrame))
        );
    }
    // Longer than its buffer: refused before the fill, the buffer kept.
    let mut queue = Queue::new();
    let mut back = Back::new();
    let (sender, _receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(32),
        state.network_link(),
    );
    assert_eq!(
        tx.publish(plain(60), unreachable_fill("no buffer holds it")),
        Err(Unpublished::Port(PortRefusal::BadFrame))
    );
    tx.publish(plain(32), octets).unwrap();
    tx.publish(plain(32), octets).unwrap();
}

#[test]
fn no_buffer_or_a_full_queue_refuses_before_the_frame_is_written() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    tx.publish(plain(42), octets).unwrap();
    tx.publish(plain(42), octets).unwrap();
    // Both buffers are the station's and the queue is full.
    assert_eq!(
        tx.publish(plain(42), unreachable_fill("no room")),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
    // The station takes one frame: room in the queue, still no buffer.
    tx.flush();
    assert!(receiver.try_receive_with(Option::take).is_ok());
    assert_eq!(
        tx.publish(plain(42), unreachable_fill("no buffer")),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
}

#[test]
fn a_failed_fill_comes_back_as_the_fill_s_and_keeps_the_buffer() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    assert_eq!(
        tx.publish(plain(42), |_| Err(Declined)),
        Err(Unpublished::Fill(Declined))
    );
    tx.flush();
    assert!(matches!(
        receiver.try_receive_with(Option::take),
        Err(_) | Ok(None)
    ));
    tx.publish(plain(42), octets).unwrap();
    tx.publish(plain(42), octets).unwrap();
}

/// A returned buffer and a settled end are two events: the buffer is free
/// again only once its end went on, so no more frames than buffers are
/// ever unsettled.
#[test]
fn a_buffer_frees_only_when_its_end_is_handed_on() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (mut returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    let first = tx.publish(plain(42), octets).unwrap();
    let second = tx.publish(plain(42), octets).unwrap();
    assert_ne!(first, second);
    for _ in 0..2 {
        tx.flush();
        let Ok(Some(outgoing)) = receiver.try_receive_with(Option::take) else {
            panic!("the frame went to the station");
        };
        assert!(
            returned
                .try_send_with(|slot| {
                    *slot = Some(ended(outgoing.memory));
                    Ok::<_, ()>(())
                })
                .is_ok()
        );
    }
    assert!(!tx.notice_waiting(), "nothing is taken back unasked");
    assert_eq!(
        tx.publish(plain(42), unreachable_fill("two unsettled ends")),
        Err(Unpublished::Port(PortRefusal::NoRoom)),
        "two buffers, two unsettled ends: no third frame"
    );
    assert!(tx.notice_waiting(), "the refusal took back what came");
    // The first notice is the epoch's, then each frame's end.
    let mut ends = Vec::new();
    while let Some(notice) = tx.take_notice() {
        if let RadioNotice::Ended(Exchange { ticket, .. }) = notice {
            ends.push(ticket);
            break;
        }
    }
    assert_eq!(ends, [first]);
    let third = tx.publish(plain(42), octets).unwrap();
    assert_eq!(third, first, "the settled buffer goes out again");
    assert!(matches!(
        tx.take_notice(),
        Some(RadioNotice::Ended(Exchange { ticket, .. })) if ticket == second
    ));
    assert!(tx.take_notice().is_none());
}

#[test]
fn a_buffer_the_pool_loses_is_told_lost_and_never_comes_back() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool { loses: true },
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    let lost = tx.publish(plain(42), octets).unwrap();
    tx.flush();
    assert!(
        matches!(receiver.try_receive_with(Option::take), Err(_) | Ok(None)),
        "nothing reaches the station"
    );
    let mut told = false;
    while let Some(notice) = tx.take_notice() {
        told |= notice == RadioNotice::Lost(lost);
    }
    assert!(told);
    // One buffer is left, then none.
    tx.publish(plain(42), octets).unwrap();
    assert_eq!(
        tx.publish(plain(42), unreachable_fill("both buffers are gone")),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
}

/// Each buffer keeps the run its last frame left in it, for the next frame
/// written there.
#[test]
fn a_buffer_keeps_the_run_its_last_frame_left() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (mut returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers_of(1, 64),
        state.network_link(),
    );
    let run = Held {
        at: 20,
        length: 22,
        octet: 9,
    };
    let first = tx
        .publish(plain(42), |slot| {
            assert_eq!(slot.open().1, None, "a new buffer holds no run");
            slot.open().0[20..42].fill(9);
            slot.hold(Some(run));
            Ok::<_, Declined>(())
        })
        .unwrap();
    tx.flush();
    let Ok(Some(outgoing)) = receiver.try_receive_with(Option::take) else {
        panic!("the frame went to the station");
    };
    assert!(
        returned
            .try_send_with(|slot| {
                *slot = Some(ended(outgoing.memory));
                Ok::<_, ()>(())
            })
            .is_ok()
    );
    while tx.take_notice().is_some() {}
    let second = tx
        .publish(plain(42), |slot| {
            assert_eq!(slot.open().1, Some(run), "the buffer's run is kept");
            Ok::<_, Declined>(())
        })
        .unwrap();
    assert_eq!(first, second);
}

#[test]
fn room_is_ready_only_with_a_free_buffer_and_room_to_hand_it_on() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (mut returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    let mut cx = Context::from_waker(Waker::noop());
    assert!(tx.poll_room(&mut cx).is_ready());
    tx.publish(plain(42), octets).unwrap();
    tx.publish(plain(42), octets).unwrap();
    assert!(tx.poll_room(&mut cx).is_pending());
    tx.flush();
    let Ok(Some(outgoing)) = receiver.try_receive_with(Option::take) else {
        panic!("the frame went to the station");
    };
    assert!(
        returned
            .try_send_with(|slot| {
                *slot = Some(ended(outgoing.memory));
                Ok::<_, ()>(())
            })
            .is_ok()
    );
    assert!(
        tx.poll_room(&mut cx).is_pending(),
        "the buffer is back, but frees only with its notice"
    );
    assert!(tx.notice_waiting());
    while tx.take_notice().is_some() {}
    assert!(tx.poll_room(&mut cx).is_ready());
}

/// Every buffer the constructor is given is the network's: none is
/// dropped, and each one carries a frame. More than `SLOTS` cannot be given:
/// the storage's type bounds them.
#[test]
fn every_buffer_given_is_held() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    struct Tracked {
        bytes: [u8; 64],
        place: u8,
        dropped: Arc<AtomicUsize>,
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl TxStorage for Tracked {
        fn bytes_mut(&mut self) -> &mut [u8] {
            &mut self.bytes
        }
    }
    struct Kept;
    impl TxPool for Kept {
        type Slot = Tracked;
        type Memory = Tracked;
        fn place(slot: &Tracked) -> u8 {
            slot.place
        }
        fn publish(&mut self, slot: Tracked, _: usize) -> Result<Tracked, PoolRefused<Tracked>> {
            Ok(slot)
        }
        fn slot(&mut self, memory: Tracked) -> Tracked {
            memory
        }
    }
    let mut queue = WakingSlotQueue::<Option<Outgoing<Tracked>>, TX>::new();
    let mut back = WakingSlotQueue::<Option<TxEnded<Tracked>>, TX>::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (_returned, back) = back.split(|| None);
    let dropped = Arc::new(AtomicUsize::new(0));
    let slots: heapless::Vec<Tracked, SLOTS> = (0..SLOTS)
        .map(|place| Tracked {
            bytes: [0; 64],
            place: u8::try_from(place).unwrap(),
            dropped: dropped.clone(),
        })
        .collect();
    let mut kept = LocalTxState::<Tracked, SLOTS>::new();
    let mut tx: LocalTx<'_, Kept, SLOTS, TX, SameHart> =
        LocalTx::new(sender, back, Kept, &mut kept, slots, state.network_link());
    assert_eq!(dropped.load(Ordering::SeqCst), 0, "no buffer is dropped");
    assert_eq!(tx.buffers(), SLOTS, "every buffer is the network's");
    let mut places = Vec::new();
    for _ in 0..SLOTS {
        places.push(tx.publish(plain(42), octets).unwrap());
    }
    places.sort_unstable();
    assert_eq!(
        places,
        (0..SLOTS)
            .map(|place| u8::try_from(place).unwrap())
            .collect::<Vec<_>>()
    );
    tx.flush();
    while receiver
        .try_receive_with(Option::take)
        .is_ok_and(|frame| frame.is_some())
    {}
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        SLOTS,
        "the station took each one"
    );
}

/// A burst reads the queue's room once and holds its places until its end:
/// a burst that met a full queue refuses for want of room until the burst
/// ends, and the next burst sees the room the station made.
#[test]
fn a_burst_holds_the_places_it_read_until_its_end() {
    let mut queue = Queue::new();
    let mut back = Back::new();
    let state = State::new();
    up(&state);
    let (sender, mut receiver) = queue.split(|| None);
    let (mut returned, back) = back.split(|| None);
    let mut kept = LocalTxState::new();
    let mut tx: Tx<'_> = LocalTx::new(
        sender,
        back,
        Pool::default(),
        &mut kept,
        buffers(64),
        state.network_link(),
    );
    // A refused fill gives its place back to the burst: both places stay.
    assert_eq!(
        tx.publish(plain(42), |_| Err(Declined)),
        Err(Unpublished::Fill(Declined))
    );
    tx.publish(plain(42), octets).unwrap();
    tx.publish(plain(42), octets).unwrap();
    tx.flush();
    // The station takes both and gives them back.
    for _ in 0..TX {
        tx.flush();
        let Ok(Some(outgoing)) = receiver.try_receive_with(Option::take) else {
            panic!("the frame went to the station");
        };
        assert!(
            returned
                .try_send_with(|slot| {
                    *slot = Some(ended(outgoing.memory));
                    Ok::<_, ()>(())
                })
                .is_ok()
        );
    }
    while tx.take_notice().is_some() {}
    // The next burst reads the room again: both places are free.
    tx.publish(plain(42), octets).unwrap();
    tx.publish(plain(42), octets).unwrap();
    assert_eq!(
        tx.publish(plain(42), unreachable_fill("the queue is full")),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
}
