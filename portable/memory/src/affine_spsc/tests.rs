extern crate std;

use super::*;
use std::sync::{Arc, atomic::AtomicUsize};

type Queue = AffineSpscQueue<u8, 3>;

#[test]
fn cursor_domain_wraps_without_changing_fifo_distance() {
    assert_eq!(Queue::cursor_distance(0, 3), 3);
    assert_eq!(Queue::advance(5), 0);
    assert_eq!(Queue::cursor_distance(5, 1), 2);
}

#[test]
fn cursor_distance_does_not_overflow_at_the_largest_capacity() {
    const DEPTH: usize = usize::MAX / 2;
    type LargeQueue = AffineSpscQueue<(), DEPTH>;
    assert_eq!(
        LargeQueue::cursor_distance(DEPTH * 2 - 1, DEPTH / 2),
        DEPTH / 2 + 1
    );
    assert_eq!(LargeQueue::advance(DEPTH * 2 - 1), 0);
}

#[test]
fn returned_endpoints_allow_another_exclusive_epoch() {
    let mut queue = Queue::new();
    {
        let (_producer, _consumer) = queue.split();
    }
    let (producer, consumer) = queue.split();
    producer.try_send(1).unwrap();
    assert_eq!(consumer.try_receive(), Ok(1));
}

#[test]
fn full_and_empty_transitions_preserve_affine_values() {
    let mut queue = AffineSpscQueue::<u8, 2>::new();
    let (producer, consumer) = queue.split();
    producer.try_send(1).unwrap();
    producer.try_send(2).unwrap();
    assert_eq!(producer.try_send(3).unwrap_err().0, 3);
    assert_eq!(consumer.try_receive(), Ok(1));
    assert_eq!(consumer.try_receive(), Ok(2));
    assert_eq!(
        consumer.try_receive(),
        Err(AffineSpscTryReceiveError::Empty)
    );
}

#[test]
fn the_original_receiver_moves_across_a_lifecycle_boundary() {
    fn next_epoch<T, const N: usize>(
        receiver: AffineSpscReceiver<'_, T, N>,
    ) -> AffineSpscReceiver<'_, T, N> {
        receiver
    }
    let mut queue = Queue::new();
    let (producer, consumer) = queue.split();
    producer.try_send(1).unwrap();
    let consumer = next_epoch(consumer);
    assert_eq!(consumer.try_receive(), Ok(1));
    producer.try_send(2).unwrap();
    assert_eq!(consumer.try_receive(), Ok(2));
}

struct Owner {
    id: u8,
    drops: Arc<AtomicUsize>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn nonempty_resplit_preserves_owned_values_and_fifo_across_cursor_wrap() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut queue = AffineSpscQueue::<Owner, 3>::new();
    for epoch in 0..12 {
        {
            let (sender, _receiver) = queue.split();
            for id in 0..3 {
                sender
                    .try_send(Owner {
                        id,
                        drops: Arc::clone(&drops),
                    })
                    .ok()
                    .unwrap();
            }
        }
        assert_eq!(drops.load(Ordering::Relaxed), epoch * 3);
        let (_sender, receiver) = queue.split();
        for id in 0..3 {
            assert_eq!(receiver.try_receive().unwrap().id, id);
        }
        assert_eq!(drops.load(Ordering::Relaxed), (epoch + 1) * 3);
    }
}

#[test]
fn queue_drop_releases_each_leftover_owner_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let mut queue = AffineSpscQueue::<Owner, 2>::new();
        let (sender, _receiver) = queue.split();
        for id in 0..2 {
            sender
                .try_send(Owner {
                    id,
                    drops: Arc::clone(&drops),
                })
                .ok()
                .unwrap();
        }
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }
    assert_eq!(drops.load(Ordering::Relaxed), 2);
}

#[test]
fn endpoint_transfer_between_threads_preserves_fifo() {
    let mut queue = Queue::new();
    let (sender, receiver) = queue.split();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            for expected in 0..128 {
                let mut owner = expected;
                loop {
                    match sender.try_send(owner) {
                        Ok(()) => break,
                        Err(AffineSpscTrySendError(returned)) => owner = returned,
                    }
                    std::thread::yield_now();
                }
            }
        });
        scope.spawn(move || {
            for expected in 0..128 {
                let received = loop {
                    if let Ok(value) = receiver.try_receive() {
                        break value;
                    }
                    std::thread::yield_now();
                };
                assert_eq!(received, expected);
            }
        });
    });
    let (_, receiver) = queue.split();
    assert!(receiver.is_empty());
}

#[test]
fn dropping_a_vacant_permit_preserves_credit_and_refusal_returns_the_sender() {
    let mut queue = AffineSpscQueue::<u8, 1>::new();
    let (mut sender, receiver) = queue.split();
    for value in 0..12 {
        drop(sender.try_reserve().ok().unwrap());
        assert_eq!(sender.free_capacity(), 1);
        assert_eq!(
            receiver.try_receive(),
            Err(AffineSpscTryReceiveError::Empty)
        );
        sender.try_reserve().ok().unwrap().send(value);
        let returned_sender = sender.try_reserve().err().expect("the queue is full");
        assert_eq!(receiver.try_receive(), Ok(value));
        returned_sender.try_reserve().ok().unwrap().send(value + 1);
        assert_eq!(receiver.try_receive(), Ok(value + 1));
    }
}
