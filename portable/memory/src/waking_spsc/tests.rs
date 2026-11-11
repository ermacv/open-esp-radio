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

#[test]
fn a_held_burst_tells_the_consumer_once_and_only_a_waiting_producer_hears_of_room() {
    let (consumer, producer) = (Counted::default(), Counted::default());
    let mut queue = WakingSpscQueue::<u8, 3, _, _>::across_harts(&consumer, &producer);
    let (mut sender, mut receiver) = queue.split();
    let mut cx = Context::from_waker(Waker::noop());

    sender.flush();
    assert_eq!(consumer.counts().1, 0, "nothing was held");
    sender.try_send_held(1).expect("room");
    sender.try_send_held(2).expect("room");
    assert_eq!(consumer.counts().1, 0, "the burst goes on");
    sender.flush();
    assert_eq!(consumer.counts().1, 1, "its end tells once");
    sender.flush();
    assert_eq!(consumer.counts().1, 1, "nothing was held since");

    assert_eq!(receiver.try_receive(), Some(1));
    assert_eq!(producer.counts().1, 0, "the producer does not wait");

    sender.try_send(3).expect("room");
    sender.try_send(4).expect("room");
    assert!(pin!(sender.ready()).poll(&mut cx).is_pending());
    assert_eq!(receiver.try_receive(), Some(2));
    assert_eq!(producer.counts().1, 1, "a waiting producer hears");
    assert_eq!(receiver.try_receive(), Some(3));
    assert_eq!(producer.counts().1, 1, "once");
}

#[test]
fn a_producer_that_found_a_cell_and_waits_for_more_hears_of_the_next_one() {
    let (consumer, producer) = (Counted::default(), Counted::default());
    let mut queue = WakingSpscQueue::<u8, 3, _, _>::across_harts(&consumer, &producer);
    let (mut sender, receiver) = queue.split();
    let mut cx = Context::from_waker(Waker::noop());

    sender.try_send(1).expect("room");
    sender.try_send(2).expect("room");
    // One cell is free, the producer needs two: its poll is ready, and it
    // sleeps on its registration.
    assert!(pin!(sender.ready()).poll(&mut cx).is_ready());
    assert_eq!(receiver.try_receive(), Some(1));
    assert_eq!(producer.counts().1, 1, "the next cell tells it");
    assert_eq!(receiver.try_receive(), Some(2));
    assert_eq!(producer.counts().1, 1, "no longer registered as waiting");
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

/// A producer notify that lets another thread free a cell while the
/// producer's registration is being armed.
struct ArmingNotify<'b> {
    waker: AtomicWaker,
    /// Whether the next registration meets the other thread.
    meet: core::sync::atomic::AtomicBool,
    barrier: &'b std::sync::Barrier,
}

impl QueueNotify for ArmingNotify<'_> {
    fn register(&self, cx: &mut Context<'_>) {
        self.waker.register(cx.waker());
        if self.meet.swap(false, Ordering::SeqCst) {
            // The other thread frees a cell between these two meetings.
            self.barrier.wait();
            self.barrier.wait();
        }
    }
    fn notify(&self) {
        self.waker.wake();
    }
}

#[test]
fn room_freed_before_the_wait_is_found_without_a_wake() {
    let mut queue = WakingSpscQueue::<u8, 1>::new();
    let (mut sender, receiver) = queue.split();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    sender.try_send(1).unwrap();
    assert_eq!(receiver.try_receive(), Some(1));
    assert!(sender.poll_room(&mut cx).is_ready());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    // The poll took no cell: the room is still there for a send.
    assert!(sender.try_send(2).is_ok());
}

#[test]
fn room_freed_after_the_wait_is_armed_wakes_it_once() {
    let mut queue = WakingSpscQueue::<u8, 1>::new();
    let (mut sender, receiver) = queue.split();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    sender.try_send(1).unwrap();
    assert!(sender.poll_room(&mut cx).is_pending());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    assert_eq!(receiver.try_receive(), Some(1));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert!(sender.poll_room(&mut cx).is_ready());
    assert!(sender.try_send(2).is_ok());
}

#[test]
fn room_freed_while_the_wait_is_armed_is_found_by_its_check() {
    let barrier = std::sync::Barrier::new(2);
    let mut queue = WakingSpscQueue::<u8, 1, AtomicWaker, ArmingNotify<'_>>::across_harts(
        AtomicWaker::new(),
        ArmingNotify {
            waker: AtomicWaker::new(),
            meet: core::sync::atomic::AtomicBool::new(false),
            barrier: &barrier,
        },
    );
    let (mut sender, receiver) = queue.split();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    sender.try_send(1).unwrap();
    sender.producer.meet.store(true, Ordering::SeqCst);
    let meeting = &barrier;
    std::thread::scope(|scope| {
        scope.spawn(move || {
            meeting.wait();
            assert_eq!(receiver.try_receive(), Some(1));
            meeting.wait();
        });
        // The cell is freed after the registration and before the wait is
        // marked: the check after the fence finds it.
        assert!(sender.poll_room(&mut cx).is_ready());
    });
    assert!(wakes.0.load(Ordering::Relaxed) <= 1);
    assert!(sender.try_send(2).is_ok());
}

#[test]
fn a_permanently_full_queue_never_wakes_a_room_wait() {
    let mut queue = WakingSpscQueue::<u8, 1>::new();
    let (mut sender, receiver) = queue.split();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    sender.try_send(1).unwrap();
    for _ in 0..4 {
        assert!(sender.poll_room(&mut cx).is_pending());
    }
    // Sends that find no cell and a consumer that takes nothing wake none.
    assert!(sender.try_send(2).is_err());
    assert_eq!(receiver.len(), 1);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
}
