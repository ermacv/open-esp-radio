extern crate std;
use super::*;
use crate::sink::Sink;
use crate::{NetworkInterfaceId, OwnedEndpointResources};
use core::task::Context;
use oer_portable_memory::HostContext;
use oer_protocols_network_port::TransmitError;
use std::{boxed::Box, vec::Vec};
use xarxa_driver::{PacketPool, PacketPoolStorage};

struct Lent {
    bytes: Box<[u8; 1664]>,
    /// Where the storage starts in `bytes`.
    start: usize,
    dropped: std::sync::Arc<core::sync::atomic::AtomicUsize>,
}
impl oer_protocols_network_port::FrameStorage for Lent {
    fn bytes(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.start..]
    }
    fn advance(
        &mut self,
        by: usize,
    ) -> Result<(), oer_protocols_network_port::StorageAdvanceRefused> {
        let length = self.bytes.len() - self.start;
        if by > length {
            return Err(oer_protocols_network_port::StorageAdvanceRefused { by, length });
        }
        self.start += by;
        Ok(())
    }
}
impl Drop for Lent {
    fn drop(&mut self) {
        self.dropped
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// A lent frame whose payload starts at `start` in its 1664-byte buffer.
fn lent(
    start: usize,
    payload: &[u8],
) -> (
    oer_protocols_network_port::LentFrame<Lent>,
    std::sync::Arc<core::sync::atomic::AtomicUsize>,
) {
    let dropped = std::sync::Arc::new(core::sync::atomic::AtomicUsize::new(0));
    let range = start..start.wrapping_add(payload.len());
    let mut bytes = Box::new([0xee; 1664]);
    bytes[range.clone()].copy_from_slice(payload);
    let storage = Lent {
        bytes,
        start: 0,
        dropped: dropped.clone(),
    };
    let Ok(frame) =
        oer_protocols_network_port::LentFrame::new(storage, [2; 6], [4; 6], 0x0800, range)
    else {
        panic!("the payload lies in the buffer");
    };
    (frame, dropped)
}

fn lent_endpoint() -> (
    crate::OwnedNetworkDevice<'static, HostContext, 2, Sink>,
    crate::OwnedNetworkRunner<'static, HostContext, 2>,
) {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<HostContext, 2>::new()));
    let (stack, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], pool, Sink::with_room(1))
        .unwrap();
    radio.link_controller().set_link_up(true);
    (stack, radio)
}

#[test]
fn a_lent_frame_becomes_a_packet_in_its_own_buffer() {
    let (mut stack, mut radio) = lent_endpoint();
    let slots: &'static LentSlots<Lent, 1> = Box::leak(Box::new(LentSlots::new()));
    let (frame, dropped) = lent(60, &[1, 2, 3, 4]);
    let mut copies = Vec::new();
    publish_lent(radio.rx_publisher(), Some(slots), frame, &mut |reason| {
        copies.push(reason)
    })
    .unwrap();
    assert!(copies.is_empty(), "adopted in place");
    let packet = stack.receive().unwrap();
    assert!(slots.owns(&packet), "the packet is the lent buffer");
    assert_eq!(packet.len(), 18);
    assert_eq!(&packet[..6], &[2; 6]);
    assert_eq!(&packet[6..12], &[4; 6]);
    assert_eq!(&packet[12..14], &[0x08, 0x00]);
    assert_eq!(&packet[14..], &[1, 2, 3, 4]);
    assert_eq!(dropped.load(core::sync::atomic::Ordering::Relaxed), 0);
    drop(packet);
    assert_eq!(
        dropped.load(core::sync::atomic::Ordering::Relaxed),
        0,
        "the packet's drop only releases the slot"
    );
    assert_eq!(slots.reclaim(), 1);
    assert_eq!(
        dropped.load(core::sync::atomic::Ordering::Relaxed),
        1,
        "the reclaim returns the buffer once"
    );
}

#[test]
fn a_lent_frame_the_slots_refuse_is_copied_and_its_buffer_returned() {
    let (mut stack, mut radio) = lent_endpoint();
    let slots: &'static LentSlots<Lent, 1> = Box::leak(Box::new(LentSlots::new()));
    let (first, _) = lent(60, &[1; 4]);
    let (second, dropped) = lent(60, &[2; 4]);
    let mut copies = Vec::new();
    let mut copied = |reason| copies.push(reason);
    publish_lent(radio.rx_publisher(), Some(slots), first, &mut copied).unwrap();
    publish_lent(radio.rx_publisher(), Some(slots), second, &mut copied).unwrap();
    assert_eq!(
        copies,
        [LentCopy::Refused(xarxa_driver::ExternalPacketRefusal::Full)]
    );
    assert_eq!(dropped.load(core::sync::atomic::Ordering::Relaxed), 1);
    let adopted = stack.receive().unwrap();
    let copy = stack.receive().unwrap();
    assert!(slots.owns(&adopted));
    assert!(!slots.owns(&copy));
    assert_eq!(&copy[14..], &[2; 4]);
}

#[test]
fn a_lent_frame_without_headroom_or_slots_is_copied() {
    let (mut stack, mut radio) = lent_endpoint();
    let slots: &'static LentSlots<Lent, 1> = Box::leak(Box::new(LentSlots::new()));
    let mut copies = Vec::new();
    let mut copied = |reason| copies.push(reason);
    let (early, early_dropped) = lent(10, &[3; 4]);
    publish_lent(radio.rx_publisher(), Some(slots), early, &mut copied).unwrap();
    let (unslotted, _) = lent(60, &[5; 4]);
    publish_lent(
        radio.rx_publisher(),
        None::<&'static LentSlots<Lent, 1>>,
        unslotted,
        &mut copied,
    )
    .unwrap();
    assert_eq!(copies, [LentCopy::NoHeadroom, LentCopy::NoSlots]);
    assert_eq!(early_dropped.load(core::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(&stack.receive().unwrap()[14..], &[3; 4]);
    assert_eq!(&stack.receive().unwrap()[14..], &[5; 4]);
}

#[test]
fn a_full_stack_queue_has_no_room_until_the_stack_takes_a_packet() {
    let (mut stack, mut radio) = lent_endpoint();
    let slots: &'static LentSlots<Lent, 2> = Box::leak(Box::new(LentSlots::new()));
    let woken = std::sync::Arc::new(Woken::default());
    let waker = std::task::Waker::from(woken.clone());
    let mut cx = Context::from_waker(&waker);
    for byte in 1..=2 {
        assert!(radio.rx_publisher().poll_queue_room(&mut cx).is_ready());
        let (frame, _) = lent(60, &[byte; 4]);
        publish_lent(radio.rx_publisher(), Some(slots), frame, &mut |_| {}).unwrap();
    }
    assert!(radio.rx_publisher().poll_queue_room(&mut cx).is_pending());
    assert!(!woken.get());
    let packet = stack.receive().unwrap();
    assert!(woken.get(), "the stack's take wakes the waiting bridge");
    assert!(radio.rx_publisher().poll_queue_room(&mut cx).is_ready());
    drop(packet);
}

/// Records whether its waker was woken.
#[derive(Default)]
struct Woken(core::sync::atomic::AtomicBool);

impl Woken {
    fn get(&self) -> bool {
        self.0.load(core::sync::atomic::Ordering::Acquire)
    }
}

impl std::task::Wake for Woken {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.store(true, core::sync::atomic::Ordering::Release);
    }
}

/// A device's transmit memory: boxed bytes, as long as a DMA slot's frame.
struct Memory(Box<[u8; 1600]>);

impl TxStorage for Memory {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0[..]
    }
}

fn memory() -> Memory {
    Memory(Box::new([0; 1600]))
}

/// An endpoint whose link is up, and its receive pool.
/// An endpoint with its link up over a device that keeps two frames.
fn tx_endpoint() -> (
    crate::OwnedNetworkDevice<'static, HostContext, 1, Sink>,
    crate::OwnedNetworkRunner<'static, HostContext, 1>,
    Sink,
) {
    let storage = Box::leak(Box::new(PacketPoolStorage::<1>::new()));
    let receive = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<HostContext, 1>::new()));
    let sink = Sink::with_room(2);
    let (stack, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], receive, sink.clone())
        .unwrap();
    radio.link_controller().set_link_up(true);
    (stack, radio, sink)
}

#[test]
fn a_packet_the_stack_wrote_in_the_devices_memory_goes_out_in_it_without_a_copy() {
    let slots: &'static TxSlots<Memory, 2> = Box::leak(Box::new(TxSlots::new()));
    let general = slots.allocator();
    let mut parked = ParkedTx::new(slots);
    // The first memory is the spare; the next one is parked.
    parked.give(memory());
    assert!(general.try_alloc().is_none());
    parked.give(memory());
    let (mut stack, _radio, sink) = tx_endpoint();
    let mut packet = general.try_alloc().unwrap();
    assert!(general.try_alloc().is_none());
    packet.reserve(4);
    packet.set_len(18);
    packet.fill(7);
    let written_at = packet.as_ptr() as usize;
    assert!(stack.transmit(packet).is_ok());
    let frame = sink.0.borrow_mut().taken.pop_front().unwrap();
    let Ok(mut written) = parked.write(frame) else {
        panic!("a packet of the slots is written");
    };
    assert_eq!((written.start, written.len), (4, 18));
    assert_eq!(written.storage.bytes_mut()[4..22], [7; 18]);
    assert_eq!(
        written.storage.bytes_mut()[4..].as_ptr() as usize,
        written_at
    );
    // The memory is the device's until given back, and given back it parks
    // for the stack again.
    assert!(general.try_alloc().is_none());
    parked.give(written.storage);
    assert!(general.try_alloc().is_some());
}

#[test]
fn a_packet_from_elsewhere_is_copied_into_the_spare_and_the_next_waits_for_memory() {
    let slots: &'static TxSlots<Memory, 1> = Box::leak(Box::new(TxSlots::new()));
    let mut parked = ParkedTx::new(slots);
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let elsewhere = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let (mut stack, _radio, sink) = tx_endpoint();
    let mut send = |fill: u8| {
        let mut packet = elsewhere.try_alloc().unwrap();
        packet.set_len(16);
        packet.fill(fill);
        assert!(stack.transmit(packet).is_ok());
        sink.0.borrow_mut().taken.pop_front().unwrap()
    };
    // No memory yet: the supply says so, and a frame waits, kept.
    assert!(!parked.available());
    let first = match parked.write(send(1)) {
        Err(TxWriteRefused::Kept(refused)) => {
            assert_eq!(refused.reason, TransmitError::QueueFull);
            refused.frame
        }
        _ => panic!("no spare: the frame waits"),
    };
    assert!(!parked.available());
    parked.give(memory());
    assert!(parked.available());
    let Ok(mut written) = parked.write(first) else {
        panic!("the spare takes a copy");
    };
    assert_eq!((written.start, written.len), (0, 16));
    assert_eq!(written.storage.bytes_mut()[..16], [1; 16]);
    // The packet itself went back to its own pool.
    assert!(elsewhere.has_available());
    let second = send(2);
    assert!(matches!(
        parked.write(second),
        Err(TxWriteRefused::Kept(TransmitRefused {
            reason: TransmitError::QueueFull,
            ..
        }))
    ));
    assert!(!parked.available());
    parked.give(written.storage);
    assert!(parked.available());
}
