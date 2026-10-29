//! Capacity-aware, bounded bridge between `station::Runner` and the owned endpoint.
use crate::observation::Observations;
use core::{
    cell::RefCell,
    future::{Future, poll_fn},
    task::Poll,
};
use oer_embassy_net::OwnedNetworkRunner;
use oer_network::{EthernetFrame as _, NetDevice};
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

/// Drain each input queue by at most its capacity per poll, then yield.
/// A full downstream queue keeps its source owner queued and arms its wake.
pub async fn run<
    X: oer_memory::ExecutionContext,
    const RX: usize,
    const TX: usize,
    T: Clock + Timer,
    N: NetDevice,
>(
    mut radio: OwnedNetworkRunner<'_, X, RX, TX>,
    mut net: N,
    observations: &RefCell<Observations>,
    clock: &T,
) -> Result<core::convert::Infallible, oer_embassy_net::AccessRefused> {
    let mut state = oer_network::LinkSnapshot::DOWN;
    let mut pending = None;
    loop {
        let mut timer = core::pin::pin!(clock.wait_for(Duration::from_millis(1)));
        poll_fn(|cx| {
            match drain_step(
                &mut radio,
                &mut net,
                &mut pending,
                observations,
                &mut state,
                clock,
                cx,
            ) {
                Err(error) => Poll::Ready(Err(error)),
                Ok(true) => {
                    // Exactly one bounded step per poll; give the other P1 futures
                    // their turn before retrying work that remains immediately ready.
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Ok(false) => timer.as_mut().poll(cx).map(|_| Ok(())),
            }
        })
        .await?;
    }
}

// Keep Ethernet owner/copy temporaries outside the generated parent poll frame;
// otherwise that frame also remains live while the station service is polled.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn drain_step<
    'r,
    X: oer_memory::ExecutionContext,
    const RX: usize,
    const TX: usize,
    T: Clock,
    N: NetDevice,
>(
    radio: &mut OwnedNetworkRunner<'r, X, RX, TX>,
    net: &mut N,
    pending: &mut Option<(u64, oer_embassy_net::OwnedNetworkTxFrame<'r>)>,
    observations: &RefCell<Observations>,
    state: &mut oer_network::LinkSnapshot,
    clock: &T,
    cx: &mut core::task::Context<'_>,
) -> Result<bool, oer_embassy_net::AccessRefused> {
    radio.link_controller().follow_device(net, state);
    observations
        .borrow_mut()
        .loss
        .sample_queue(net.receive_queue_len() as u32);
    let publisher = radio.rx_publisher();
    let mut received = 0u32;
    for _ in 0..RX {
        let input_ready = net.poll_receive_ready(cx).is_ready();
        if !input_ready {
            break;
        }
        if publisher.poll_ready(cx).is_pending() {
            let mut o = observations.borrow_mut();
            o.loss.publisher_blocked = o.loss.publisher_blocked.saturating_add(1);
            break;
        }
        let Some(frame) = net.try_receive() else {
            break;
        };
        received += 1;
        let Ok(p) = frame.view() else {
            continue;
        };
        observations
            .borrow_mut()
            .observe(p.ether_type, p.payload, false, clock.now().as_micros());
        if oer_embassy_net::ethernet::publish(publisher, frame).is_err() {
            let mut o = observations.borrow_mut();
            o.queue_drops = o.queue_drops.saturating_add(1);
        } else {
            let mut o = observations.borrow_mut();
            o.loss.published = o.loss.published.saturating_add(1);
        }
    }
    observations.borrow_mut().bridge.record(received);
    let mut tx_progress = false;
    for _ in 0..TX {
        let input_ready = pending.is_some() || {
            let mut ready = core::pin::pin!(radio.wait_tx_publication());
            match ready.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    result?;
                    true
                }
                Poll::Pending => false,
            }
        };
        if !input_ready {
            break;
        }
        let Poll::Ready(()) = net.poll_transmit_ready(cx) else {
            break;
        };
        // A coalesced publication may outlive its last queued owner. Consume
        // that hint as work too, so the next poll arms a fresh wait before
        // parking; otherwise a later publication would wait for the timer.
        tx_progress = true;
        let (epoch, packet, first_attempt) = match pending.take() {
            Some((epoch, packet)) => (epoch, packet, false),
            None => {
                let Some(packet) = radio.try_receive_tx()? else {
                    break;
                };
                (net.link().epoch, packet, true)
            }
        };
        if epoch != net.link().epoch {
            // Like a queued old-generation packet, this retained owner returns
            // to its original pool instead of entering a later association.
            drop(packet);
            continue;
        }
        if first_attempt && let Ok(view) = packet.view() {
            observations.borrow_mut().observe(
                view.ether_type,
                view.payload,
                true,
                clock.now().as_micros(),
            );
        }
        if let Err(refused) = packet.transmit_to(net) {
            match refused.reason {
                oer_network::TransmitError::InvalidLength(_) => {
                    let mut o = observations.borrow_mut();
                    o.queue_drops = o.queue_drops.saturating_add(1);
                }
                oer_network::TransmitError::QueueFull | oer_network::TransmitError::LinkDown => {
                    *pending = Some((epoch, refused.frame));
                    break;
                }
            }
        }
    }
    Ok(received != 0 || tx_progress)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use oer_ieee80211_runtime::station::{
        Control, ServiceCommand, ServiceOutput, ServiceOutputSink, State,
    };
    use oer_ieee80211_sta::data::{EthernetFrame, TxMsdu};
    use oer_ieee80211_wire::{data::EthernetFrameParts, qos::WmmUserPriority};
    use oer_memory::{WakingSpscReceiver, WakingSpscSender};

    fn net<'a, const RX: usize>(
        tx: WakingSpscSender<'a, TxMsdu, 2>,
        rx: WakingSpscReceiver<'a, EthernetFrame, RX>,
    ) -> oer_ieee80211_runtime::station::Device<'a, 2, RX> {
        let state = Box::leak(Box::new(State::new()));
        let commands = Box::leak(Box::new(WakingSpscQueue::<ServiceCommand<()>, 1>::new()));
        let (sender, _) = commands.split();
        let (control, _runner) = Control::new(sender, state).unwrap();
        let mut events = control.events(|_: ServiceOutput<()>| {});
        events.emit(ServiceOutput::Notice(
            oer_ieee80211_sta::machine::StaNotice::Connected {
                bssid: oer_ieee80211_wire::address::Bssid::new(
                    oer_ieee80211_wire::address::IndividualAddress::new([2; 6]).unwrap(),
                )
                .unwrap(),
                channel: 6,
            },
        ));
        control.device(tx, rx)
    }
    use core::{future::Future, task::Context};
    use oer_embassy_net::{NetworkInterfaceId, OwnedEndpointResources};
    use oer_memory::{HostContext, WakingSpscQueue};
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
        let mut endpoint = OwnedEndpointResources::<HostContext, 8, 4>::new();
        let (mut device, mut radio) = endpoint
            .split(NetworkInterfaceId::new(0), [2; 6], pool)
            .expect("endpoint split");
        radio.link_controller().set_link_up(true);
        // A native echo reply can retain the sole RX owner in the TX queue.
        radio.try_send_rx(&[2; 15]).unwrap();
        let retained = device.receive().unwrap();
        device.transmit(retained).unwrap();
        assert!(!pool.has_available());
        let mut input = WakingSpscQueue::<EthernetFrame, 8>::new();
        let (source, receiver) = input.split();
        for marker in 0..8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = WakingSpscQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let mut published = 8;
        let mut received = 0;
        let mut transmitted = 0;
        let mut queued_at = Some(clock.now());
        {
            let mut bridge =
                core::pin::pin!(run(radio, net(sender, receiver), &observations, &clock));
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
        let mut endpoint = OwnedEndpointResources::<HostContext, 8, 4>::new();
        let (mut device, mut radio) = endpoint
            .split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>())
            .expect("endpoint split");
        radio.link_controller().set_link_up(true);
        let general = allocator::<1>();
        let mut packet = general.try_alloc().unwrap();
        packet.set_len(15);
        packet.fill(2);
        device.transmit(packet).unwrap();
        let mut input = WakingSpscQueue::<EthernetFrame, 8>::new();
        let (source, receiver) = input.split();
        for marker in 0..8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = WakingSpscQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let mut bridge = core::pin::pin!(run(radio, net(sender, receiver), &observations, &clock));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert!(sink.try_receive().is_some());
        assert_eq!(device.receive().unwrap()[14], 0);
        assert!(device.receive().is_none());
        assert!(source.try_send(frame(8)).is_ok());
        assert!(source.try_send(frame(9)).is_err());
        assert_eq!(observations.borrow().queue_drops, 0);
        assert_eq!(observations.borrow().loss.publisher_blocked, 1);
        assert_eq!(observations.borrow().loss.published, 1);
        assert!(general.try_alloc().is_some());
    }

    #[test]
    fn a_full_ready_rx_queue_drains_in_one_turn_and_tx_is_served_in_that_turn() {
        const RX: usize = 8;
        let clock = VirtualClock::<8>::new();
        let pool = allocator::<RX>();
        let mut endpoint = OwnedEndpointResources::<HostContext, RX, 4>::new();
        let (mut device, mut radio) = endpoint
            .split(NetworkInterfaceId::new(0), [2; 6], pool)
            .expect("endpoint split");
        radio.link_controller().set_link_up(true);
        let general = allocator::<1>();
        let mut packet = general.try_alloc().unwrap();
        packet.set_len(15);
        packet.fill(2);
        device.transmit(packet).unwrap();
        let mut input = WakingSpscQueue::<EthernetFrame, RX>::new();
        let (source, receiver) = input.split();
        for marker in 0..RX as u8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = WakingSpscQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let mut bridge = core::pin::pin!(run(radio, net(sender, receiver), &observations, &clock));
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
        assert_eq!(o.loss.queue_samples, 1);
        assert_eq!(o.loss.queue_sum, RX as u64);
        assert_eq!(o.loss.queue_max, RX as u32);
        assert_eq!(o.loss.published, RX as u32);
        assert_eq!(o.loss.publisher_blocked, 0);
        let mut report = crate::report::Report::new(6, [2; 6], [4; 6]);
        o.report(&mut report);
        let mut recorded = false;
        {
            let mut notes = core::pin::pin!(report.notes(async |note| {
                if note.topic == oer_diagnostics::Topic::W3Bridge {
                    assert_eq!(note.words(), &[1, RX as u32, RX as u32]);
                    recorded = true;
                }
                Ok::<(), ()>(())
            }));
            assert!(matches!(
                notes.as_mut().poll(&mut cx),
                core::task::Poll::Ready(Ok(()))
            ));
        }
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
        let mut endpoint = OwnedEndpointResources::<HostContext, RX, TX>::new();
        let (mut device, mut radio) = endpoint
            .split(NetworkInterfaceId::new(0), [2; 6], allocator::<RX>())
            .expect("endpoint split");
        radio.link_controller().set_link_up(true);
        let general = allocator::<{ TX + 1 }>();
        let mut input = WakingSpscQueue::<EthernetFrame, RX>::new();
        let (source, receiver) = input.split();
        for marker in 0..RX as u8 {
            source.try_send(frame(marker)).unwrap();
        }
        let mut output = WakingSpscQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        let observations = RefCell::new(Observations::new(7));
        let mut bridge = core::pin::pin!(run(radio, net(sender, receiver), &observations, &clock));
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

    #[test]
    fn blocked_step_keeps_owners_and_wakes_on_queue_and_pool_capacity_without_a_timer() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::{
            sync::Arc,
            task::{Wake, Waker},
        };
        struct Wakes(AtomicUsize);
        impl Wake for Wakes {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let clock = VirtualClock::<8>::new();
        let pool = allocator::<1>();
        let general = allocator::<1>();
        let mut endpoint = OwnedEndpointResources::<HostContext, 1, 1>::new();
        let (mut device, mut radio) = endpoint
            .split(NetworkInterfaceId::new(0), [2; 6], pool)
            .expect("endpoint split");
        radio.link_controller().set_link_up(true);
        let packet = |marker| {
            let mut p = general.try_alloc().unwrap();
            p.set_len(15);
            p.fill(marker);
            p
        };
        device.transmit(packet(2)).unwrap();
        let mut input = WakingSpscQueue::<EthernetFrame, 1>::new();
        let (source, receiver) = input.split();
        let mut output = WakingSpscQueue::<TxMsdu, 2>::new();
        let (sender, sink) = output.split();
        for marker in [100, 101] {
            sender
                .try_send(TxMsdu {
                    priority: WmmUserPriority::new(0).unwrap(),
                    frame: frame(marker),
                })
                .unwrap();
        }
        let observations = RefCell::new(Observations::new(7));
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&wakes));
        let mut cx = Context::from_waker(&waker);
        let mut bridge = core::pin::pin!(run(radio, net(sender, receiver), &observations, &clock));
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert!(!device.can_transmit());
        assert!(
            !general.has_available(),
            "the blocked TX owner stays in its input queue"
        );
        let before = wakes.0.load(Ordering::Relaxed);
        assert_eq!(sink.try_receive().unwrap().frame.parts().payload, &[100]);
        assert!(wakes.0.load(Ordering::Relaxed) > before);
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert_eq!(sink.try_receive().unwrap().frame.parts().payload, &[101]);
        assert_eq!(sink.try_receive().unwrap().frame.parts().payload, &[2]);
        assert!(general.has_available());

        // Retire the old publication hint, then arm both source waits. The
        // second poll is quiescent; subsequent progress requires a real wake.
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        let before = wakes.0.load(Ordering::Relaxed);
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert_eq!(wakes.0.load(Ordering::Relaxed), before);
        let before = wakes.0.load(Ordering::Relaxed);
        device.transmit(packet(5)).unwrap();
        assert!(wakes.0.load(Ordering::Relaxed) > before);
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert_eq!(sink.try_receive().unwrap().frame.parts().payload, &[5]);
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        let before = wakes.0.load(Ordering::Relaxed);
        source.try_send(frame(3)).unwrap();
        assert!(wakes.0.load(Ordering::Relaxed) > before);
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        let held = device.receive().unwrap();
        assert_eq!(held[14], 3);
        source.try_send(frame(4)).unwrap();
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        let before = wakes.0.load(Ordering::Relaxed);
        drop(held);
        assert!(wakes.0.load(Ordering::Relaxed) > before);
        assert!(bridge.as_mut().poll(&mut cx).is_pending());
        assert_eq!(device.receive().unwrap()[14], 4);
        assert_eq!(observations.borrow().queue_drops, 0);
        assert_eq!(
            clock.now(),
            oer_time_virtual::START,
            "no fallback timer fired"
        );
    }
}
