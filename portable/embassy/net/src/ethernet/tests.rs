extern crate std;
use super::*;
use crate::{NetworkInterfaceId, OwnedEndpointResources};
use core::task::{Context, Poll};
use oer_memory::HostContext;
use oer_network_port::{LinkSnapshot, LinkState, TransmitError};
use std::boxed::Box;
use xarxa_driver::{PacketPool, PacketPoolStorage};

struct Port(LinkSnapshot, bool);
struct Frame;
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
    let mut bridge = EthernetBridge::new(radio, port);
    let mut observed = 0;
    {
        let mut future = core::pin::pin!(core::future::poll_fn(|cx| bridge.poll(
            cx,
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
