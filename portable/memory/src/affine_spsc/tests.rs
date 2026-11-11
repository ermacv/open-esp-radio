extern crate std;

use super::*;
use std::sync::{Arc, atomic::AtomicUsize};

type Queue = AffineSpscQueue<u8, 3>;

#[test]
fn local_producer_copies_publish_whole_non_clone_values_in_fifo_order() {
    local_producer_copies_publish_whole_non_clone_values_in_fifo_order_with::<CachedSpscCursor>();
    local_producer_copies_publish_whole_non_clone_values_in_fifo_order_with::<CompactSpscCursor>();
}

fn local_producer_copies_publish_whole_non_clone_values_in_fifo_order_with<C: SpscCursor>() {
    struct Payload(std::boxed::Box<u8>);
    let mut queue = AffineSpscQueue::<Payload, 2, C>::new();
    let (mut producer, mut consumer) = queue.split();
    {
        let first = producer.borrow_local();
        let second = first;
        let receiver = consumer.borrow_local();
        assert!(first.try_send(Payload(std::boxed::Box::new(1))).is_ok());
        assert!(second.try_send(Payload(std::boxed::Box::new(2))).is_ok());
        let refused = first
            .try_send(Payload(std::boxed::Box::new(3)))
            .expect_err("full")
            .0;
        assert_eq!(*refused.0, 3);
        assert_eq!(
            *receiver
                .try_receive()
                .map_err(|_| ())
                .expect("first owner")
                .0,
            1
        );
        assert!(second.try_send(refused).is_ok());
        assert_eq!(
            *receiver
                .try_receive()
                .map_err(|_| ())
                .expect("second owner")
                .0,
            2
        );
    }
    assert_eq!(
        *consumer
            .try_receive()
            .map_err(|_| ())
            .expect("third owner")
            .0,
        3
    );
    assert!(producer.try_send(Payload(std::boxed::Box::new(4))).is_ok());
    assert_eq!(
        *consumer
            .try_receive()
            .map_err(|_| ())
            .expect("original endpoint again")
            .0,
        4
    );
}

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
    nonempty_resplit_preserves_owned_values_and_fifo_across_cursor_wrap_with::<CachedSpscCursor>();
    nonempty_resplit_preserves_owned_values_and_fifo_across_cursor_wrap_with::<CompactSpscCursor>();
}

fn nonempty_resplit_preserves_owned_values_and_fifo_across_cursor_wrap_with<C: SpscCursor>() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut queue = AffineSpscQueue::<Owner, 3, C>::new();
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
    endpoint_transfer_between_threads_preserves_fifo_with::<CachedSpscCursor>();
    endpoint_transfer_between_threads_preserves_fifo_with::<CompactSpscCursor>();
}

fn endpoint_transfer_between_threads_preserves_fifo_with<C: SpscCursor>() {
    let mut queue = AffineSpscQueue::<u8, 3, C>::new();
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

#[test]
#[allow(
    unsafe_code,
    reason = "destroy the one complete queue placed in test-owned storage"
)]
fn cached_placement_transfers_received_owners_and_drops_only_the_queued_owner() {
    struct Payload {
        id: usize,
        dropped: Arc<[AtomicUsize; 2]>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            self.dropped[self.id].fetch_add(1, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new([const { AtomicUsize::new(0) }; 2]);
    let mut storage = MaybeUninit::<AffineSpscQueue<Payload, 2>>::uninit();
    let destination = storage.as_mut_ptr();
    let queue = AffineSpscQueue::init_in_place(&mut storage);
    assert!(core::ptr::eq(queue, destination));
    {
        let (sender, receiver) = queue.split();
        assert!(receiver.is_empty());
        for id in 0..2 {
            assert!(
                sender
                    .try_send(Payload {
                        id,
                        dropped: dropped.clone()
                    })
                    .is_ok()
            );
        }
        let first = receiver
            .try_receive()
            .map_err(|_| ())
            .expect("first affine owner");
        assert_eq!(first.id, 0);
        drop(first);
        assert_eq!(dropped[0].load(Ordering::SeqCst), 1);
        assert_eq!(dropped[1].load(Ordering::SeqCst), 0);
    }
    // SAFETY: placement returned the complete queue at this address, every
    // endpoint loan ended, and this scope owns its sole destruction right.
    unsafe { storage.assume_init_drop() };
    assert_eq!(dropped[0].load(Ordering::SeqCst), 1);
    assert_eq!(dropped[1].load(Ordering::SeqCst), 1);
}

/// Staged values stay unseen until one publication, which shows them all
/// in order; a short batch stops at the free cells and hands the value
/// back; around the ring's end the cursors keep their order.
#[test]
fn a_staged_batch_publishes_once_in_order_around_the_ring() {
    let mut queue = AffineSpscQueue::<u8, 4>::new();
    let (mut sender, mut receiver) = queue.split();
    for round in 0_u8..10 {
        let base = round * 10;
        // Three in, so each round's cells wrap past the ring's end.
        for n in 0..3 {
            assert!(sender.stage(base + n).is_ok());
        }
        assert_eq!(sender.staged(), 3);
        assert_eq!(sender.free_capacity(), 1, "staged cells are not free");
        assert!(receiver.is_empty(), "nothing is seen before publication");
        sender.publish();
        assert_eq!(receiver.len(), 3);
        for n in 0..3 {
            assert_eq!(receiver.take(), Some(base + n));
        }
        assert_eq!(receiver.take(), None);
        // Until released, the taken cells are not the producer's.
        assert_eq!(sender.free_capacity(), 1);
        receiver.release();
        assert_eq!(sender.free_capacity(), 4);
    }
    // A short batch: four cells, a fifth value comes back unstaged.
    for n in 0..4 {
        assert!(sender.stage(n).is_ok());
    }
    assert_eq!(sender.stage(9).unwrap_err().0, 9);
    sender.publish();
    assert_eq!(receiver.try_receive(), Ok(0));
}

/// Ordinary sends and receives keep the order of staged and taken ones: a
/// send publishes what was staged before it, a receive frees what was
/// taken before it, and so do local loans.
#[test]
fn single_operations_follow_the_batches_before_them() {
    let mut queue = AffineSpscQueue::<u8, 4>::new();
    let (mut sender, mut receiver) = queue.split();
    assert!(sender.stage(1).is_ok());
    assert!(sender.try_send(2).is_ok());
    assert_eq!(receiver.take(), Some(1));
    assert_eq!(receiver.try_receive(), Ok(2));
    assert_eq!(sender.free_capacity(), 4, "the taken cell was freed first");
    assert!(sender.stage(3).is_ok());
    assert!(sender.borrow_local().try_send(4).is_ok());
    assert_eq!(receiver.take(), Some(3));
    assert_eq!(receiver.borrow_local().try_receive(), Ok(4));
    assert_eq!(sender.free_capacity(), 4);
}

/// A producer and a consumer on two threads, each in batches: every value
/// arrives once and in order.
#[test]
fn batches_across_threads_deliver_every_value_once_in_order() {
    let queue = std::boxed::Box::leak(std::boxed::Box::new(AffineSpscQueue::<u32, 8>::new()));
    let (mut sender, mut receiver) = queue.split();
    const VALUES: u32 = 20_000;
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut next = 0_u32;
            while next < VALUES {
                let mut batch = 0;
                while batch < 5 && next < VALUES && sender.stage(next).is_ok() {
                    next += 1;
                    batch += 1;
                }
                sender.publish();
                std::thread::yield_now();
            }
        });
        scope.spawn(move || {
            let mut expect = 0_u32;
            while expect < VALUES {
                while let Some(value) = receiver.take() {
                    assert_eq!(value, expect, "in order, once");
                    expect += 1;
                }
                receiver.release();
                std::thread::yield_now();
            }
        });
    });
}

/// Endpoints that end in a batch leave it to the queue: the next split
/// publishes what was staged, once, and frees the taken cells, whose values
/// are not received again.
#[test]
fn a_batch_left_by_ended_endpoints_ends_at_the_next_split() {
    let mut queue = AffineSpscQueue::<u8, 4>::new();
    {
        let (mut sender, mut receiver) = queue.split();
        assert!(sender.try_send(1).is_ok());
        assert!(sender.try_send(2).is_ok());
        assert_eq!(receiver.take(), Some(1));
        assert!(sender.stage(3).is_ok());
        // Both end here, the take unreleased and the stage unpublished.
    }
    let (sender, receiver) = queue.split();
    assert_eq!(
        receiver.try_receive(),
        Ok(2),
        "the taken value is not seen again"
    );
    assert_eq!(
        receiver.try_receive(),
        Ok(3),
        "the staged one is published once"
    );
    assert!(receiver.try_receive().is_err());
    assert_eq!(sender.free_capacity(), 4, "every cell is free");
}
