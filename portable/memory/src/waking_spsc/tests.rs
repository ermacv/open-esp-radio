extern crate std;

use super::*;
use core::future::Future;
use core::pin::pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::Context;
use std::sync::Arc;
use std::task::{Wake, Waker};

struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn occupancy_observations_preserve_owners_and_fifo() {
    let mut queue = WakingSpscQueue::<u8, 2>::new();
    let (sender, receiver) = queue.split();
    assert!(receiver.is_empty());
    sender.try_send(1).unwrap();
    sender.try_send(2).unwrap();
    assert_eq!(receiver.len(), 2);
    assert_eq!(receiver.len(), 2);
    assert_eq!(receiver.try_receive(), Some(1));
    assert_eq!(receiver.len(), 1);
    assert_eq!(receiver.try_receive(), Some(2));
    assert!(receiver.is_empty());
}

#[test]
fn full_queue_returns_the_same_non_clone_owner_and_keeps_fifo() {
    struct Owner(u8);
    let mut queue = WakingSpscQueue::<Owner, 2>::new();
    let (sender, receiver) = queue.split();
    assert!(sender.try_send(Owner(1)).is_ok());
    assert!(sender.try_send(Owner(2)).is_ok());
    assert_eq!(sender.try_send(Owner(3)).err().unwrap().0.0, 3);
    assert_eq!(receiver.try_receive().unwrap().0, 1);
    assert!(sender.try_send(Owner(4)).is_ok());
    assert_eq!(receiver.try_receive().unwrap().0, 2);
    assert_eq!(receiver.try_receive().unwrap().0, 4);
    assert!(receiver.try_receive().is_none());
}

#[test]
fn publication_wakes_the_consumer_and_reclaim_wakes_the_blocked_producer() {
    struct Owner(u8);
    let mut queue = WakingSpscQueue::<Owner, 1>::new();
    let (mut sender, mut receiver) = queue.split();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    {
        let mut ready = core::pin::pin!(receiver.ready());
        assert!(ready.as_mut().poll(&mut cx).is_pending());
        assert!(sender.try_send(Owner(1)).is_ok());
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
        assert!(ready.as_mut().poll(&mut cx).is_ready());
    }
    let owner = Owner(2);
    {
        let mut ready = core::pin::pin!(sender.ready());
        assert!(ready.as_mut().poll(&mut cx).is_pending());
        assert_eq!(receiver.try_receive().unwrap().0, 1);
        assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
        let Poll::Ready(permit) = ready.as_mut().poll(&mut cx) else {
            panic!("the receiver returned the credit");
        };
        permit.send(owner);
    }
    assert_eq!(receiver.try_receive().unwrap().0, 2);
    assert!(receiver.try_receive().is_none());
}

#[test]
fn cancellation_keeps_the_owner_and_both_endpoints_can_wait_again() {
    struct Owner(u8);
    let mut queue = WakingSpscQueue::<Owner, 1>::new();
    let (mut sender, mut receiver) = queue.split();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(wakes);
    let mut cx = Context::from_waker(&waker);
    assert!(sender.try_send(Owner(1)).is_ok());
    let owner = Owner(2);
    {
        let mut wait = core::pin::pin!(sender.ready());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(owner.0, 2);
    assert_eq!(receiver.try_receive().unwrap().0, 1);
    {
        let mut wait = core::pin::pin!(sender.ready());
        // Reclamation before registration is seen without another wake.
        assert!(wait.as_mut().poll(&mut cx).is_ready());
    }
    {
        let mut wait = core::pin::pin!(receiver.ready());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
    }
    assert!(sender.try_send(owner).is_ok());
    {
        let mut wait = core::pin::pin!(receiver.ready());
        // Publication before registration is seen without another wake.
        assert!(wait.as_mut().poll(&mut cx).is_ready());
    }
    assert_eq!(receiver.try_receive().unwrap().0, 2);
}

#[test]
fn resplitting_keeps_the_queued_owner_until_the_next_receiver_takes_it() {
    struct Owner(Arc<AtomicUsize>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let mut queue = WakingSpscQueue::<Owner, 1>::new();
        {
            let (sender, _receiver) = queue.split();
            assert!(sender.try_send(Owner(Arc::clone(&drops))).is_ok());
        }
        let (_next_sender, next_receiver) = queue.split();
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        let owner = next_receiver.try_receive().unwrap();
        assert!(next_receiver.try_receive().is_none());
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(owner);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the queue has credit or a published owner"),
    }
}

#[test]
fn a_dropped_send_permit_publishes_nothing_and_can_be_reserved_again() {
    let mut queue = WakingSpscQueue::<u8, 1>::new();
    let (mut sender, receiver) = queue.split();
    drop(ready(sender.ready()));
    assert!(receiver.try_receive().is_none());
    ready(sender.ready()).send(9);
    assert_eq!(receiver.try_receive(), Some(9));
}

#[test]
fn a_producer_on_another_thread_sends_each_owner_once_through_its_permit() {
    struct Owner(usize, Arc<AtomicUsize>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }
    const COUNT: usize = 512;
    let drops = Arc::new(AtomicUsize::new(0));
    let mut queue = WakingSpscQueue::<Owner, 1>::new();
    let (mut sender, mut receiver) = queue.split();
    let published = std::sync::Barrier::new(2);
    let consumed = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let (published, consumed) = (&published, &consumed);
        let producer_drops = Arc::clone(&drops);
        let producer = scope.spawn(move || {
            for id in 0..COUNT {
                ready(sender.ready()).send(Owner(id, Arc::clone(&producer_drops)));
                published.wait();
                consumed.wait();
            }
        });
        for expected in 0..COUNT {
            published.wait();
            ready(receiver.ready());
            let owner = receiver.try_receive().unwrap();
            assert_eq!(owner.0, expected);
            drop(owner);
            consumed.wait();
        }
        producer.join().unwrap();
    });
    assert!(receiver.try_receive().is_none());
    assert_eq!(drops.load(Ordering::Relaxed), COUNT);
}

#[test]
fn try_reserve_keeps_the_value_with_its_caller_until_it_sends() {
    let mut queue = WakingSpscQueue::<u8, 1>::new();
    let (mut sender, receiver) = queue.split();
    sender.try_reserve().expect("a vacant cell").send(7);
    assert!(
        sender.try_reserve().is_none(),
        "the full queue reserves nothing"
    );
    // A dropped permit publishes nothing.
    assert_eq!(receiver.try_receive(), Some(7));
    drop(sender.try_reserve().expect("the cell is vacant again"));
    assert_eq!(receiver.try_receive(), None);
}

/// A notify that counts its registrations and notifications.
#[derive(Default)]
struct Counted {
    registered: AtomicUsize,
    notified: AtomicUsize,
}

impl Counted {
    fn counts(&self) -> (usize, usize) {
        (
            self.registered.load(Ordering::Relaxed),
            self.notified.load(Ordering::Relaxed),
        )
    }
}

impl QueueNotify for Counted {
    fn register(&self, _: &mut Context<'_>) {
        self.registered.fetch_add(1, Ordering::Relaxed);
    }
    fn notify(&self) {
        self.notified.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_queue_across_harts_registers_and_tells_each_end_through_its_own_notify() {
    let (consumer, producer) = (Counted::default(), Counted::default());
    let mut queue = WakingSpscQueue::<u8, 1, _, _>::across_harts(&consumer, &producer);
    let (mut sender, mut receiver) = queue.split();
    let mut cx = Context::from_waker(Waker::noop());

    assert!(pin!(receiver.ready()).poll(&mut cx).is_pending());
    assert_eq!(consumer.counts(), (1, 0), "the waiting consumer registers");
    sender.try_send(1).expect("room");
    assert_eq!(
        consumer.counts(),
        (1, 1),
        "a publication tells the consumer"
    );

    assert!(pin!(sender.ready()).poll(&mut cx).is_pending());
    assert_eq!(producer.counts(), (1, 0), "the waiting producer registers");
    assert_eq!(receiver.try_receive(), Some(1));
    assert_eq!(producer.counts(), (1, 1), "a freed cell tells the producer");

    sender.ready().now_or_never().expect("a free cell").send(2);
    assert_eq!(
        consumer.counts(),
        (1, 2),
        "a permit's send tells the consumer"
    );
}

/// Poll a future once with a no-op waker.
trait NowOrNever: Future + Sized {
    fn now_or_never(self) -> Option<Self::Output> {
        let mut cx = Context::from_waker(Waker::noop());
        match pin!(self).poll(&mut cx) {
            Poll::Ready(value) => Some(value),
            Poll::Pending => None,
        }
    }
}

impl<F: Future> NowOrNever for F {}
