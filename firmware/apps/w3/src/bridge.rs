//! Capacity-aware, bounded bridge between StationService and the owned endpoint.
use crate::observation::Observations;
use core::{
    cell::{Cell, RefCell},
    future::poll_fn,
    task::Poll,
};
use embassy_futures::{select::select, yield_now};
use oer_embassy_net::{NoopRawMutex, OwnedNetworkRunner};
use oer_ieee80211_runtime::station_service::{EthernetFrame, MsduReceiver, MsduSender, TxMsdu};
use oer_ieee80211_wire::{data::EthernetFrameParts, qos::WmmUserPriority};
use oer_time::{Clock, Duration, Timer};

/// Aggregate whole-run work, including empty RX phases. The host computes
/// mean RX per turn from `rx_frames / turns` without target formatting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    pub turns: u32,
    pub rx_frames: u32,
    pub rx_max: u32,
}
impl Counters {
    pub const fn new() -> Self {
        Self {
            turns: 0,
            rx_frames: 0,
            rx_max: 0,
        }
    }
    fn record(&mut self, received: u32) {
        self.turns = self.turns.saturating_add(1);
        self.rx_frames = self.rx_frames.saturating_add(received);
        self.rx_max = self.rx_max.max(received);
    }
}

/// Drain at most each input queue's capacity before yielding.
/// RX capacity is checked before removing an input; a blocked RX publisher
/// never prevents TX from returning the owners that restore that capacity.
pub async fn run<const RX: usize, const TX: usize, T: Clock + Timer>(
    radio: OwnedNetworkRunner<'_, NoopRawMutex, RX, TX>,
    mut tx: MsduSender<'_, TxMsdu, 2>,
    mut rx: MsduReceiver<'_, EthernetFrame, RX>,
    observations: &RefCell<Observations>,
    link: &Cell<bool>,
    clock: &T,
) -> ! {
    let mut state = false;
    loop {
        if state != link.get() {
            state = link.get();
            radio.link_controller().set_link_up(state);
        }
        let publisher = radio.rx_publisher();
        let received = poll_fn(|cx| {
            let mut received = 0u32;
            for _ in 0..RX {
                if publisher.poll_ready(cx).is_pending() {
                    break;
                }
                let Some(frame) = rx.try_receive() else {
                    break;
                };
                received += 1;
                let p = frame.parts();
                observations.borrow_mut().observe(
                    p.ether_type,
                    p.payload,
                    false,
                    clock.now().as_micros(),
                );
                if radio
                    .rx_publisher()
                    .try_send_parts(p.destination, p.source, p.ether_type, p.payload)
                    .is_err()
                {
                    let mut o = observations.borrow_mut();
                    o.queue_drops = o.queue_drops.saturating_add(1);
                }
            }
            Poll::Ready(received)
        })
        .await;
        observations.borrow_mut().bridge.record(received);
        let mut transmitted = false;
        for _ in 0..TX {
            let Some(packet) = radio.try_receive_tx() else {
                break;
            };
            transmitted = true;
            let bytes = packet.ethernet();
            if let Some((header, payload)) = bytes.split_first_chunk::<14>() {
                let [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1] = *header;
                let ether_type = u16::from_be_bytes([e0, e1]);
                match EthernetFrame::from_parts(EthernetFrameParts {
                    destination: [d0, d1, d2, d3, d4, d5],
                    source: [s0, s1, s2, s3, s4, s5],
                    ether_type,
                    payload,
                }) {
                    Ok(frame) => {
                        let permit = tx.ready().await;
                        observations.borrow_mut().observe(
                            ether_type,
                            payload,
                            true,
                            clock.now().as_micros(),
                        );
                        permit.send(TxMsdu {
                            priority: WmmUserPriority::new(0).expect("best effort priority"),
                            frame,
                        });
                    }
                    Err(_) => {
                        let mut o = observations.borrow_mut();
                        o.queue_drops = o.queue_drops.saturating_add(1);
                    }
                }
            }
            drop(packet);
        }
        if received != 0 || transmitted {
            yield_now().await;
        } else {
            // The timer also propagates a link notice when no frame is queued.
            let _ = select(
                async {
                    poll_fn(|cx| publisher.poll_ready(cx)).await;
                    rx.ready().await;
                },
                select(
                    radio.wait_tx_publication(),
                    clock.wait_for(Duration::from_millis(1)),
                ),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::{future::Future, task::Context};
    use oer_embassy_net::{NetworkInterfaceId, OwnedEndpointResources};
    use oer_ieee80211_runtime::station_service::MsduQueue;
    use oer_time_virtual::VirtualClock;
    use std::boxed::Box;
    use xarxa_driver::{PacketBufAllocator, PacketPool, PacketPoolStorage};

    fn allocator<const N: usize>() -> PacketBufAllocator {
        let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
        Box::leak(Box::new(PacketPool::new(storage))).allocator()
    }

    fn frame(marker: u8) -> EthernetFrame {
        EthernetFrame::from_parts(EthernetFrameParts {
            destination: [2; 6],
            source: [4; 6],
            ether_type: 0x88b5,
            payload: &[marker],
        })
        .unwrap()
    }

    #[test]
    fn continuous_rx_advances_tx_within_one_turn_even_when_rx_pool_is_held_by_tx() {
        let clock = VirtualClock::<8>::new();
        let pool = allocator::<1>();
        let mut endpoint = OwnedEndpointResources::<NoopRawMutex, 8, 4>::new();
        let (mut device, radio) = endpoint.split(NetworkInterfaceId::new(0), [2; 6], pool);
        radio.link_controller().set_link_up(true);
        // A native echo reply can retain the sole RX owner in the TX queue.
        radio.try_send_rx(&[2; 15]).unwrap();
        let retained = device.receive().unwrap();
        device.transmit(retained).unwrap();
        assert!(!pool.has_available());
        let mut input = MsduQueue::<EthernetFrame, 8>::new();
        let (source, receiver) = input.split();
        for marker in 0..8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = MsduQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let link = Cell::new(true);
        let mut published = 8;
        let mut received = 0;
        let mut transmitted = 0;
        let mut queued_at = Some(clock.now());
        {
            let mut bridge =
                core::pin::pin!(run(radio, sender, receiver, &observations, &link, &clock));
            let mut cx = Context::from_waker(core::task::Waker::noop());
            for turn in 0..129 {
                clock.advance(Duration::from_micros(100)).unwrap();
                assert!(bridge.as_mut().poll(&mut cx).is_pending());
                if let Some(at) = queued_at.take() {
                    let sent = sink.try_receive().expect("TX progresses in the next turn");
                    assert!(
                        clock.now().saturating_duration_since(at) <= Duration::from_micros(100)
                    );
                    assert_eq!(
                        sent.frame.parts().payload,
                        &[if turn == 0 { 2 } else { transmitted - 1 }]
                    );
                    transmitted += 1;
                }
                assert!(sink.try_receive().is_none());
                if let Some(packet) = device.receive() {
                    assert_eq!(packet[14], received);
                    received += 1;
                    assert!(source.try_send(frame(published)).is_ok());
                    published += 1;
                    assert!(
                        device.receive().is_none(),
                        "only one RX pool owner is available"
                    );
                    device.transmit(packet).unwrap();
                    queued_at = Some(clock.now());
                }
                // No additional RX input was removed while the endpoint was
                // blocked. The continuous backlog stays full after refilling.
                assert!(source.try_send(frame(published)).is_err());
            }
        }
        assert!(received >= 63 && transmitted >= 63);
        assert_eq!(observations.borrow().queue_drops, 0);
        // Every admitted packet has reached TX and returned its RX-pool slot.
        assert!(pool.try_alloc().is_some());
    }

    #[test]
    fn a_full_input_batch_keeps_owners_queued_when_only_one_rx_owner_is_free() {
        let clock = VirtualClock::<8>::new();
        let mut endpoint = OwnedEndpointResources::<NoopRawMutex, 8, 4>::new();
        let (mut device, radio) =
            endpoint.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
        radio.link_controller().set_link_up(true);
        let general = allocator::<1>();
        let mut packet = general.try_alloc().unwrap();
        packet.set_len(15);
        packet.fill(2);
        device.transmit(packet).unwrap();
        let mut input = MsduQueue::<EthernetFrame, 8>::new();
        let (source, receiver) = input.split();
        for marker in 0..8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = MsduQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let link = Cell::new(true);
        let mut bridge =
            core::pin::pin!(run(radio, sender, receiver, &observations, &link, &clock));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert!(sink.try_receive().is_some());
        assert_eq!(device.receive().unwrap()[14], 0);
        assert!(device.receive().is_none());
        assert!(source.try_send(frame(8)).is_ok());
        assert!(source.try_send(frame(9)).is_err());
        assert_eq!(observations.borrow().queue_drops, 0);
        assert!(general.try_alloc().is_some());
    }

    #[test]
    fn a_full_ready_rx_queue_drains_in_one_turn_and_tx_is_served_in_that_turn() {
        const RX: usize = 8;
        let clock = VirtualClock::<8>::new();
        let pool = allocator::<RX>();
        let mut endpoint = OwnedEndpointResources::<NoopRawMutex, RX, 4>::new();
        let (mut device, radio) = endpoint.split(NetworkInterfaceId::new(0), [2; 6], pool);
        radio.link_controller().set_link_up(true);
        let general = allocator::<1>();
        let mut packet = general.try_alloc().unwrap();
        packet.set_len(15);
        packet.fill(2);
        device.transmit(packet).unwrap();
        let mut input = MsduQueue::<EthernetFrame, RX>::new();
        let (source, receiver) = input.split();
        for marker in 0..RX as u8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = MsduQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let link = Cell::new(true);
        let mut bridge =
            core::pin::pin!(run(radio, sender, receiver, &observations, &link, &clock));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        clock.advance(Duration::from_micros(100)).unwrap();
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert_eq!(sink.try_receive().unwrap().frame.parts().payload, &[2]);
        assert!(sink.try_receive().is_none());
        for marker in 0..RX as u8 {
            assert_eq!(device.receive().unwrap()[14], marker);
            source.try_send(frame(marker + RX as u8)).unwrap();
        }
        assert!(device.receive().is_none());
        assert!(source.try_send(frame(16)).is_err());
        let o = observations.borrow();
        assert_eq!(
            o.bridge,
            Counters {
                turns: 1,
                rx_frames: RX as u32,
                rx_max: RX as u32
            }
        );
        assert_eq!(o.queue_drops, 0);
        let mut report = crate::report::Report::new(6, [2; 6], [4; 6]);
        o.report(&mut report);
        let mut recorded = false;
        report
            .notes(|note| {
                if note.topic == oer_diagnostics::Topic::W3Bridge {
                    assert_eq!(note.words(), &[1, RX as u32, RX as u32]);
                    recorded = true;
                }
                Ok::<(), ()>(())
            })
            .unwrap();
        assert!(recorded);
        assert!(general.try_alloc().is_some());
        let returned: std::vec::Vec<_> = (0..RX).map(|_| pool.try_alloc().unwrap()).collect();
        assert!(pool.try_alloc().is_none());
        drop(returned);
    }

    #[test]
    fn continuous_tx_yields_back_to_rx_after_at_most_the_tx_input_capacity() {
        const RX: usize = 8;
        const TX: usize = 4;
        let clock = VirtualClock::<8>::new();
        let mut endpoint = OwnedEndpointResources::<NoopRawMutex, RX, TX>::new();
        let (mut device, radio) =
            endpoint.split(NetworkInterfaceId::new(0), [2; 6], allocator::<RX>());
        radio.link_controller().set_link_up(true);
        let general = allocator::<{ TX + 1 }>();
        let mut input = MsduQueue::<EthernetFrame, RX>::new();
        let (source, receiver) = input.split();
        for marker in 0..RX as u8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = MsduQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let link = Cell::new(true);
        let mut bridge =
            core::pin::pin!(run(radio, sender, receiver, &observations, &link, &clock));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        let mut next_rx = 0u8;
        let mut next_input = RX as u8;
        let mut last_rx = clock.now();
        let mut transmitted = 0;
        for _ in 0..128 {
            // Keep TX nonempty even while the bridge awaits its full output
            // queue. An unbounded TX drain would never reach the next RX phase.
            while device.can_transmit() {
                let mut packet = general.try_alloc().unwrap();
                packet.set_len(15);
                packet.fill(2);
                device.transmit(packet).unwrap();
            }
            clock.advance(Duration::from_micros(100)).unwrap();
            assert!(bridge.as_mut().poll(&mut cx).is_pending());
            if sink.try_receive().is_some() {
                transmitted += 1;
            }
            while let Some(packet) = device.receive() {
                assert_eq!(packet[14], next_rx);
                next_rx = next_rx.wrapping_add(1);
                source.try_send(frame(next_input)).unwrap();
                next_input = next_input.wrapping_add(1);
                last_rx = clock.now();
            }
            assert!(
                clock.now().saturating_duration_since(last_rx)
                    <= Duration::from_micros(((TX + 1) * 100) as u64)
            );
        }
        let o = observations.borrow();
        assert!(transmitted > 100);
        assert!(o.bridge.rx_frames >= 128);
        assert_eq!(o.bridge.rx_max, RX as u32);
        assert_eq!(o.queue_drops, 0);
    }
}
