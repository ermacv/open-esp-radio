extern crate std;
use super::*;
use crate::{NetworkInterfaceId, OwnedEndpointResources};
use core::task::{Context, Poll};
use oer_memory::HostContext;
use oer_network_port::{LinkSnapshot, LinkState, TransmitError};
use std::{boxed::Box, vec::Vec};
use xarxa_driver::{PacketPool, PacketPoolStorage};

struct Port(LinkSnapshot, bool);
struct Frame;
impl oer_network_port::ReceivedFrame for Frame {
    type Storage = core::convert::Infallible;
    type Copied = Self;
    fn into_lent(self) -> Result<oer_network_port::LentFrame<Self::Storage>, Self> {
        Err(self)
    }
}
impl EthernetFrame for Frame {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        Err(FrameLengthError::TooShort)
    }
}
impl NetDevice for Port {
    type RxFrame = Frame;
    fn link(&self) -> LinkSnapshot {
        self.0
    }
    fn poll_link(&mut self, _: &mut Context<'_>) -> Poll<LinkSnapshot> {
        Poll::Pending
    }
    fn receive_queue_len(&self) -> usize {
        0
    }
    fn poll_receive_ready(&mut self, _: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
    fn try_receive(&mut self) -> Option<Frame> {
        None
    }
    fn read_copied<R>(
        &mut self,
        copied: Frame,
        read: impl FnOnce(EthernetFrameView<'_>) -> R,
    ) -> Result<R, FrameLengthError> {
        copied.view().map(read)
    }
    fn poll_transmit_ready(&mut self, _: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }
    fn try_transmit<F: EthernetFrame>(&mut self, frame: F) -> Result<(), TransmitRefused<F>> {
        if self.1 {
            Ok(())
        } else {
            Err(TransmitRefused {
                frame,
                reason: TransmitError::QueueFull,
            })
        }
    }
}

#[test]
fn neutral_refusal_retains_the_packet_and_software_credit_and_epoch_change_reclaims_stale_tx() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<1>::new()));
    let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let mut resources = OwnedEndpointResources::<HostContext, 1, 1>::new();
    let (mut stack, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], pool)
        .unwrap();
    let mut port = Port(
        LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        },
        false,
    );
    let mut previous = LinkSnapshot::DOWN;
    radio.link_controller().follow_device(&port, &mut previous);
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(18);
    packet.fill(7);
    let address = packet.as_ptr();
    assert!(stack.transmit(packet).is_ok());
    let frame = radio.try_receive_tx().unwrap().unwrap();
    let returned = frame.transmit_to(&mut port, |_| {}).err().unwrap();
    assert_eq!(returned.reason, TransmitError::QueueFull);
    assert_eq!(returned.frame.ethernet().as_ptr(), address);
    assert!(!stack.can_transmit());
    assert!(!pool.has_available());
    drop(returned);
    assert!(stack.can_transmit());
    assert!(pool.has_available());
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(18);
    packet.fill(9);
    assert!(stack.transmit(packet).is_ok());
    port.0.epoch = 2;
    radio.link_controller().follow_device(&port, &mut previous);
    assert!(radio.try_receive_tx().unwrap().is_none());
    assert!(pool.has_available());
    assert!(stack.can_transmit());
}

#[test]
fn bridge_keeps_a_refused_packet_until_cancellation_without_spinning_or_releasing_its_credit() {
    use core::future::Future;
    let storage = Box::leak(Box::new(PacketPoolStorage::<1>::new()));
    let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let mut resources = OwnedEndpointResources::<HostContext, 1, 1>::new();
    let (mut stack, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], pool)
        .unwrap();
    let port = Port(
        LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        },
        false,
    );
    radio.link_controller().set_link_up(true);
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(18);
    packet.fill(7);
    assert!(stack.transmit(packet).is_ok());
    let mut bridge = EthernetBridge::new(radio, port, None::<&'static LentSlots<_, 0>>);
    let mut observed = 0;
    {
        let mut future = core::pin::pin!(core::future::poll_fn(|cx| bridge.poll(
            cx,
            |_| {},
            |_| {},
            |_| {},
            |view| {
                assert_eq!(view.payload, &[7; 4]);
                observed += 1;
            }
        )));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        for _ in 0..3 {
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert!(!pool.has_available());
            assert!(!stack.can_transmit());
        }
    }
    assert_eq!(observed, 0);
    drop(bridge);
    assert!(pool.has_available());
    assert!(stack.can_transmit());
}

#[test]
fn successful_tx_is_observed_once_after_admission_and_before_packet_and_credit_return() {
    let storage = Box::leak(Box::new(PacketPoolStorage::<1>::new()));
    let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let mut resources = OwnedEndpointResources::<HostContext, 1, 1>::new();
    let (mut stack, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], pool)
        .unwrap();
    let mut port = Port(
        LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        },
        true,
    );
    radio.link_controller().set_link_up(true);
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(18);
    packet.fill(7);
    assert!(stack.transmit(packet).is_ok());
    let frame = radio.try_receive_tx().unwrap().unwrap();
    let mut observed = 0;
    assert!(
        frame
            .transmit_to(&mut port, |view| {
                assert_eq!(view.payload, &[7; 4]);
                assert!(!pool.has_available());
                assert!(!stack.can_transmit());
                observed += 1;
            })
            .is_ok()
    );
    assert_eq!(observed, 1);
    assert!(pool.has_available());
    assert!(stack.can_transmit());
}

/// A lent DMA-like buffer: its bytes and a count of its drops.
struct Lent {
    bytes: Box<[u8; 1664]>,
    /// Where the storage starts in `bytes`.
    start: usize,
    dropped: std::sync::Arc<core::sync::atomic::AtomicUsize>,
}
impl oer_network_port::FrameStorage for Lent {
    fn bytes(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.start..]
    }
    fn advance(&mut self, by: usize) -> Result<(), oer_network_port::StorageAdvanceRefused> {
        let length = self.bytes.len() - self.start;
        if by > length {
            return Err(oer_network_port::StorageAdvanceRefused { by, length });
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
    oer_network_port::LentFrame<Lent>,
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
    let Ok(frame) = oer_network_port::LentFrame::new(storage, [2; 6], [4; 6], 0x0800, range) else {
        panic!("the payload lies in the buffer");
    };
    (frame, dropped)
}

fn lent_endpoint() -> (
    crate::OwnedNetworkDevice<'static, HostContext, 2, 1>,
    crate::OwnedNetworkRunner<'static, HostContext, 2, 1>,
) {
    let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
    let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<HostContext, 2, 1>::new()));
    let (stack, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], pool)
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
        1,
        "dropping the packet returns the buffer once"
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
        None::<&'static LentSlots<_, 0>>,
        unslotted,
        &mut copied,
    )
    .unwrap();
    assert_eq!(copies, [LentCopy::NoHeadroom, LentCopy::NoSlots]);
    assert_eq!(early_dropped.load(core::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(&stack.receive().unwrap()[14..], &[3; 4]);
    assert_eq!(&stack.receive().unwrap()[14..], &[5; 4]);
}
