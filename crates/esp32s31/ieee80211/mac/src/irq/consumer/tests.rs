extern crate std;

use super::*;
use crate::irq::{EVENT_COLLISION, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};
use core::{future::Future, pin::pin, task::Waker};
use std::{
    sync::{Arc, atomic::AtomicUsize},
    task::Wake,
};

#[derive(Default)]
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn finite_batch_preserves_rx_before_tx_under_continuous_publication() {
    let mut storage = IrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    publisher.publish(EVENT_KNOWN_MASK);
    let mut batch = consumer.take_batch();
    for expected in [
        IrqWork::RxSuccess,
        IrqWork::TxComplete,
        IrqWork::TxTimeout,
        IrqWork::Collision,
    ] {
        publisher.publish(EVENT_RX_SUCCESS);
        assert_eq!(batch.next(), Some(expected));
    }
    assert_eq!(batch.next(), None);
    assert_eq!(
        consumer.take_batch().collect::<std::vec::Vec<_>>(),
        [IrqWork::RxSuccess]
    );
}

#[test]
fn cancelled_mac_wait_releases_the_slot_for_the_next_waiter() {
    let mut storage = IrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    let old = Arc::new(WakeCount::default());
    let current = Arc::new(WakeCount::default());
    {
        let waker = Waker::from(old.clone());
        let mut cancelled = pin!(consumer.wait());
        assert!(
            cancelled
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
    }
    let waker = Waker::from(current.clone());
    let mut next = pin!(consumer.wait());
    let mut cx = Context::from_waker(&waker);
    assert!(next.as_mut().poll(&mut cx).is_pending());
    publisher.publish(EVENT_RX_SUCCESS);
    assert_eq!(old.0.load(Ordering::Relaxed), 0);
    assert_eq!(current.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(batch) = next.as_mut().poll(&mut cx) else {
        panic!("published")
    };
    assert_eq!(batch.collect::<std::vec::Vec<_>>(), [IrqWork::RxSuccess]);
}

#[test]
fn cancelled_power_wait_releases_the_slot_for_the_next_waiter() {
    let mut storage = PowerIrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    let old = Arc::new(WakeCount::default());
    let current = Arc::new(WakeCount::default());
    {
        let waker = Waker::from(old.clone());
        let mut cancelled = pin!(consumer.wait());
        assert!(
            cancelled
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
    }
    let waker = Waker::from(current.clone());
    let mut next = pin!(consumer.wait());
    let mut cx = Context::from_waker(&waker);
    assert!(next.as_mut().poll(&mut cx).is_pending());
    publisher.publish(MacPowerInterruptObservation::from_semantic_events(
        false, false, false, false, true, false,
    ));
    assert_eq!(old.0.load(Ordering::Relaxed), 0);
    assert_eq!(current.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(observation) = next.as_mut().poll(&mut cx) else {
        panic!("published")
    };
    assert!(observation.sta_tbtt());
}

#[test]
fn publication_before_registration_and_after_pending_are_both_durable() {
    let mut storage = IrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    publisher.publish(EVENT_TX_COMPLETE);
    assert!(consumer.poll_batch(&mut cx).is_ready());
    assert!(consumer.poll_batch(&mut cx).is_pending());
    std::thread::scope(|s| {
        s.spawn(|| publisher.publish(EVENT_TX_TIMEOUT | EVENT_COLLISION));
    });
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(batch) = consumer.poll_batch(&mut cx) else {
        panic!("published")
    };
    assert_eq!(
        batch.collect::<std::vec::Vec<_>>(),
        [IrqWork::TxTimeout, IrqWork::Collision]
    );
}

#[test]
fn publication_during_waker_registration_cannot_be_lost() {
    // Replacing the registered waker drops its last Arc while registration
    // owns AtomicWaker's slot. The old waker's destructor injects the IRQ.
    struct PublishOnDrop(&'static IrqStorage);
    #[allow(
        clippy::manual_noop_waker,
        reason = "this waker publishes from Drop during registration, which Waker::noop cannot model"
    )]
    impl Wake for PublishOnDrop {
        fn wake(self: Arc<Self>) {}
    }
    impl Drop for PublishOnDrop {
        fn drop(&mut self) {
            self.0.post(EVENT_RX_SUCCESS).wake();
        }
    }
    let storage = std::boxed::Box::leak(std::boxed::Box::new(IrqStorage::new()));
    let (publisher, mut consumer) = storage.split();
    let old = Waker::from(Arc::new(PublishOnDrop(publisher.storage)));
    assert!(
        consumer
            .poll_batch(&mut Context::from_waker(&old))
            .is_pending()
    );
    drop(old);
    let next = Waker::from(Arc::new(WakeCount::default()));
    let Poll::Ready(batch) = consumer.poll_batch(&mut Context::from_waker(&next)) else {
        panic!("registration raced")
    };
    assert_eq!(batch.collect::<std::vec::Vec<_>>(), [IrqWork::RxSuccess]);
}

#[test]
fn hardware_counts_exclude_capacity_and_handoff_and_wrap() {
    let mut storage = IrqStorage::new();
    storage.rx_posts.store(u32::MAX - 1, Ordering::Relaxed);
    let (publisher, mut consumer) = storage.split();
    publisher.notify_rx_capacity();
    publisher.notify_rx_handoff();
    assert_eq!(publisher.diagnostics().rx_post_count(), u32::MAX - 1);
    publisher.publish(EVENT_RX_SUCCESS);
    assert_eq!(publisher.diagnostics().rx_post_count(), u32::MAX);
    publisher.publish(EVENT_RX_SUCCESS);
    assert_eq!(publisher.diagnostics().rx_post_count(), 0);
    let batch = consumer.take_batch();
    assert!(batch.rx_capacity_returned());
    assert_eq!(batch.collect::<std::vec::Vec<_>>(), [IrqWork::RxSuccess]);
}

#[test]
fn power_snapshot_coalesces_all_semantic_facts_and_then_clears() {
    let mut storage = PowerIrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    publisher.publish(MacPowerInterruptObservation::from_semantic_events(
        true, false, true, false, false, true,
    ));
    publisher.publish(MacPowerInterruptObservation::from_semantic_events(
        false, true, false, true, true, false,
    ));
    let observation = consumer.take();
    assert!(
        observation.tsf_timer_0()
            && observation.tsf_timer_1()
            && observation.tsf_timer_2()
            && observation.tsf_timer_3()
    );
    assert!(observation.sta_tbtt() && observation.has_unhandled_event());
    assert!(consumer.take().is_empty());
}

#[test]
fn diagnostics_borrow_their_installation_and_count_unhandled_per_entry() {
    let mut first = IrqStorage::new();
    let mut second = IrqStorage::new();
    let (first, _first_consumer) = first.split();
    let (second, _second_consumer) = second.split();
    let first_view = first.diagnostics();
    let second_view = second.diagnostics();
    first
        .publish_snapshot_deferred(IrqSnapshot {
            had_status: true,
            posted_events: EVENT_RX_SUCCESS,
            had_auxiliary_event: true,
            had_unhandled_event: true,
        })
        .wake();
    first
        .publish_snapshot_deferred(IrqSnapshot::default())
        .wake();
    assert_eq!(
        first_view.snapshot(),
        IrqDiagnosticSnapshot {
            rx_posts: 1,
            mac_irq_entries: 2,
            auxiliary_entries: 1,
            unhandled_entries: 1,
            empty_entries: 1,
            rx_epochs: 1,
            ..IrqDiagnosticSnapshot::default()
        }
    );
    assert_eq!(second_view.snapshot(), IrqDiagnosticSnapshot::default());
}

#[test]
fn diagnostic_epochs_count_empty_to_pending_transitions_not_each_tx_fact() {
    let mut storage = IrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    for events in [
        EVENT_TX_COMPLETE,
        EVENT_TX_TIMEOUT,
        EVENT_COLLISION,
        EVENT_RX_SUCCESS,
    ] {
        publisher
            .publish_snapshot_deferred(IrqSnapshot {
                had_status: true,
                posted_events: events,
                ..IrqSnapshot::default()
            })
            .wake();
    }
    let snapshot = publisher.diagnostics().snapshot();
    assert_eq!(snapshot.mac_irq_entries, 4);
    assert_eq!((snapshot.rx_epochs, snapshot.tx_epochs), (1, 1));
    assert_eq!(
        consumer.take_batch().collect::<std::vec::Vec<_>>(),
        [
            IrqWork::RxSuccess,
            IrqWork::TxComplete,
            IrqWork::TxTimeout,
            IrqWork::Collision,
        ]
    );
    publisher
        .publish_snapshot_deferred(IrqSnapshot {
            had_status: true,
            posted_events: EVENT_RX_SUCCESS | EVENT_TX_TIMEOUT,
            ..IrqSnapshot::default()
        })
        .wake();
    let snapshot = publisher.diagnostics().snapshot();
    assert_eq!((snapshot.rx_epochs, snapshot.tx_epochs), (2, 2));
}

std::thread_local! { static CYCLES: core::cell::Cell<u32> = const { core::cell::Cell::new(0) }; }
pub(super) fn cycles() -> u32 {
    CYCLES.get()
}

#[test]
fn entry_cycles_stop_before_the_owned_waker_runs() {
    struct ExpensiveWake;
    impl Wake for ExpensiveWake {
        fn wake(self: Arc<Self>) {
            CYCLES.set(CYCLES.get().wrapping_add(1000));
        }
    }
    CYCLES.set(10);
    let mut storage = IrqStorage::new();
    let (publisher, mut consumer) = storage.split();
    let waker = Waker::from(Arc::new(ExpensiveWake));
    assert!(
        consumer
            .poll_batch(&mut Context::from_waker(&waker))
            .is_pending()
    );
    let measurement = publisher.begin_mac_entry();
    let wake = publisher.publish_snapshot_deferred(IrqSnapshot {
        had_status: true,
        posted_events: EVENT_RX_SUCCESS,
        ..IrqSnapshot::default()
    });
    CYCLES.set(35);
    let wake = measurement.finish(wake);
    wake.wake();
    assert_eq!(CYCLES.get(), 1035);
    let snapshot = publisher.diagnostics().snapshot();
    assert_eq!(snapshot.mac_irq_entries, 1);
    assert_eq!(snapshot.mac_irq_cycles, 25);
}
