extern crate std;

use super::*;
use std::vec::Vec;

#[test]
fn values_are_filled_and_read_in_place_in_their_order() {
    let mut queue = WakingSlotQueue::<Vec<u8>, 2>::new();
    let (mut sender, mut receiver) = queue.split(|| Vec::with_capacity(8));
    for byte in [1_u8, 2] {
        assert_eq!(
            sender.try_send_with(|slot| {
                slot.clear();
                slot.push(byte);
                Ok::<_, ()>(byte)
            }),
            Ok(byte)
        );
    }
    assert_eq!(sender.free_capacity(), 0);
    assert_eq!(
        sender.try_send_with(|_| Ok::<_, ()>(())),
        Err(SlotSendError::Full)
    );
    // The slot keeps its allocation: the value was never moved out.
    let first = receiver.try_receive_with(|slot| (slot.clone(), slot.capacity()));
    assert_eq!(first, Ok((std::vec![1], 8)));
    assert_eq!(receiver.try_receive_with(|slot| slot[0]), Ok(2));
    assert_eq!(
        receiver.try_receive_with(|_| ()),
        Err(SlotReceiveError::Empty)
    );
    assert_eq!(sender.free_capacity(), 2);
}

#[test]
fn a_refused_fill_publishes_nothing_and_keeps_its_slot_free() {
    let mut queue = WakingSlotQueue::<u8, 1>::new();
    let (mut sender, receiver) = queue.split(|| 0);
    assert_eq!(
        sender.try_send_with(|slot| {
            *slot = 9;
            Err::<(), _>("too long")
        }),
        Err(SlotSendError::Refused("too long"))
    );
    assert!(receiver.is_empty());
    assert_eq!(sender.free_capacity(), 1);
    assert_eq!(sender.try_send_with(|slot| Ok::<_, ()>(*slot)), Ok(9));
    assert_eq!(receiver.len(), 1);
}

#[test]
fn a_later_split_keeps_the_filled_values_in_order_and_frees_the_rest() {
    let mut queue = WakingSlotQueue::<u8, 3>::new();
    {
        let (mut sender, _receiver) = queue.split(|| 0);
        for value in [4, 5] {
            let filled = sender.try_send_with(|slot| {
                *slot = value;
                Ok::<_, ()>(())
            });
            assert_eq!(filled, Ok(()));
        }
        // A refused fill leaves the producer holding the last slot.
        assert!(sender.try_send_with(|_| Err::<(), _>(())).is_err());
    }
    let inits = core::cell::Cell::new(0);
    let (sender, mut receiver) = queue.split(|| {
        inits.set(inits.get() + 1);
        0
    });
    assert_eq!(inits.get(), 0, "the slots were installed once");
    assert_eq!(sender.free_capacity(), 1);
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(4));
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(5));
    assert_eq!(sender.free_capacity(), 3);
}

#[test]
fn readiness_waits_for_a_slot_and_for_a_value() {
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};
    let mut queue = WakingSlotQueue::<u8, 1>::new();
    let (mut sender, mut receiver) = queue.split(|| 0);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(pin!(receiver.ready()).poll(&mut cx), Poll::Pending);
    assert_eq!(pin!(sender.ready()).poll(&mut cx), Poll::Ready(()));
    assert_eq!(sender.try_send_with(|_| Ok::<_, ()>(())), Ok(()));
    assert_eq!(pin!(sender.ready()).poll(&mut cx), Poll::Pending);
    assert_eq!(pin!(receiver.ready()).poll(&mut cx), Poll::Ready(()));
}

#[test]
fn a_slot_queue_across_harts_tells_each_end_through_its_own_notify() {
    use core::pin::pin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::{Context, Poll, Waker};

    /// Counts its registrations and notifications.
    #[derive(Default)]
    struct Counted(AtomicUsize, AtomicUsize);
    impl QueueNotify for Counted {
        fn register(&self, _: &mut Context<'_>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn notify(&self) {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }
    let counts = |notify: &Counted| {
        (
            notify.0.load(Ordering::Relaxed),
            notify.1.load(Ordering::Relaxed),
        )
    };

    let (consumer, producer) = (Counted::default(), Counted::default());
    let mut queue = WakingSlotQueue::<u8, 1, _, _>::across_harts(&consumer, &producer);
    let (mut sender, mut receiver) = queue.split(|| 0);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(pin!(receiver.ready()).poll(&mut cx), Poll::Pending);
    assert_eq!(counts(&consumer), (1, 0));
    let filled = sender.try_send_with(|slot| {
        *slot = 4;
        Ok::<_, ()>(())
    });
    assert_eq!(filled, Ok(()));
    assert_eq!(
        counts(&consumer),
        (1, 1),
        "a filled slot tells the consumer"
    );
    assert_eq!(pin!(sender.ready()).poll(&mut cx), Poll::Pending);
    assert_eq!(counts(&producer), (1, 0));
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(4));
    assert_eq!(counts(&producer), (1, 1), "a freed slot tells the producer");
}

#[test]
fn an_end_is_told_only_while_it_waits_once_per_burst() {
    use core::pin::pin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::{Context, Poll, Waker};

    /// Counts its notifications.
    #[derive(Default)]
    struct Counted(AtomicUsize);
    impl QueueNotify for Counted {
        fn register(&self, _: &mut Context<'_>) {}
        fn notify(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let told = |notify: &Counted| notify.0.load(Ordering::Relaxed);
    let fill = |sender: &mut SlotSender<'_, u8, 4, &Counted, &Counted>, value| {
        sender.try_send_with(|slot| {
            *slot = value;
            Ok::<_, ()>(())
        })
    };

    let (consumer, producer) = (Counted::default(), Counted::default());
    let mut queue = WakingSlotQueue::<u8, 4, _, _>::across_harts(&consumer, &producer);
    let (mut sender, mut receiver) = queue.split(|| 0);
    let mut cx = Context::from_waker(Waker::noop());
    // A consumer that never waited is told nothing.
    assert_eq!(fill(&mut sender, 1), Ok(()));
    assert_eq!(told(&consumer), 0);
    // Ready at once: it does not wait, and the next fill tells nothing.
    assert_eq!(pin!(receiver.ready()).poll(&mut cx), Poll::Ready(()));
    assert_eq!(fill(&mut sender, 2), Ok(()));
    assert_eq!(told(&consumer), 0);
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(1));
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(2));
    // Waiting on an empty queue, it is told once for a burst of fills.
    assert_eq!(pin!(receiver.ready()).poll(&mut cx), Poll::Pending);
    for value in 3..7 {
        assert_eq!(fill(&mut sender, value), Ok(()));
    }
    assert_eq!(told(&consumer), 1, "one notify for the burst");
    // The producer waits on the full queue: told once for a burst of reads.
    assert_eq!(pin!(sender.ready()).poll(&mut cx), Poll::Pending);
    for value in 3..7 {
        assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(value));
    }
    assert_eq!(told(&producer), 1, "one notify for the reads");
    assert_eq!(pin!(sender.ready()).poll(&mut cx), Poll::Ready(()));
}

#[test]
fn a_held_burst_tells_a_consumer_of_one_only_when_it_ends() {
    use core::pin::pin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::{Context, Poll, Waker};

    /// Counts its notifications.
    #[derive(Default)]
    struct Counted(AtomicUsize);
    impl QueueNotify for Counted {
        fn register(&self, _: &mut Context<'_>) {}
        fn notify(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let told = |notify: &Counted| notify.0.load(Ordering::Relaxed);
    let hold = |sender: &mut SlotSender<'_, u8, 4, &Counted, &Counted>, value| {
        sender.try_send_held_with(|slot| {
            *slot = value;
            Ok::<_, ()>(())
        })
    };

    let (consumer, producer) = (Counted::default(), Counted::default());
    let mut queue = WakingSlotQueue::<u8, 4, _, _>::across_harts(&consumer, &producer);
    let (mut sender, mut receiver) = queue.split(|| 0);
    let mut cx = Context::from_waker(Waker::noop());
    // Nothing waits: the end of a burst tells nothing.
    sender.flush();
    assert_eq!(told(&consumer), 0);
    // Waiting for one: the burst fills, and only its end tells, once.
    assert_eq!(pin!(receiver.ready()).poll(&mut cx), Poll::Pending);
    assert_eq!(hold(&mut sender, 1), Ok(()));
    assert_eq!(hold(&mut sender, 2), Ok(()));
    assert_eq!(told(&consumer), 0, "a burst does not wake for its first");
    sender.flush();
    assert_eq!(told(&consumer), 1, "the end of the burst does");
    sender.flush();
    assert_eq!(told(&consumer), 1, "no longer waiting");
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(1));
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(2));
    // Waiting for two: the second held fill tells at once.
    assert_eq!(
        pin!(receiver.ready_at_least(2)).poll(&mut cx),
        Poll::Pending
    );
    assert_eq!(hold(&mut sender, 3), Ok(()));
    assert_eq!(told(&consumer), 1, "below the count");
    assert_eq!(hold(&mut sender, 4), Ok(()));
    assert_eq!(told(&consumer), 2, "at the count, within the burst");
    // Below the count, the end of a burst tells nothing either.
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(3));
    assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(4));
    assert_eq!(
        pin!(receiver.ready_at_least(2)).poll(&mut cx),
        Poll::Pending
    );
    assert_eq!(hold(&mut sender, 5), Ok(()));
    sender.flush();
    assert_eq!(told(&consumer), 2);
    // A consumer that registers after a held fill sees it itself.
    assert_eq!(pin!(receiver.ready()).poll(&mut cx), Poll::Ready(()));
}

#[test]
fn a_consumer_waiting_for_several_is_told_once_they_are_filled() {
    use core::pin::pin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::{Context, Poll, Waker};

    /// Counts its notifications.
    #[derive(Default)]
    struct Counted(AtomicUsize);
    impl QueueNotify for Counted {
        fn register(&self, _: &mut Context<'_>) {}
        fn notify(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let told = |notify: &Counted| notify.0.load(Ordering::Relaxed);
    let fill = |sender: &mut SlotSender<'_, u8, 4, &Counted, &Counted>, value| {
        sender.try_send_with(|slot| {
            *slot = value;
            Ok::<_, ()>(())
        })
    };

    let (consumer, producer) = (Counted::default(), Counted::default());
    let mut queue = WakingSlotQueue::<u8, 4, _, _>::across_harts(&consumer, &producer);
    let (mut sender, mut receiver) = queue.split(|| 0);
    let mut cx = Context::from_waker(Waker::noop());
    // Waiting for three: the first two fills tell nothing, the third once.
    assert_eq!(
        pin!(receiver.ready_at_least(3)).poll(&mut cx),
        Poll::Pending
    );
    assert_eq!(fill(&mut sender, 1), Ok(()));
    assert_eq!(fill(&mut sender, 2), Ok(()));
    assert_eq!(told(&consumer), 0, "below the count nothing is told");
    assert_eq!(fill(&mut sender, 3), Ok(()));
    assert_eq!(told(&consumer), 1, "told at the count");
    assert_eq!(fill(&mut sender, 4), Ok(()));
    assert_eq!(told(&consumer), 1, "no longer waiting");
    // Already holding the count: ready at once, and nothing more is told.
    assert_eq!(
        pin!(receiver.ready_at_least(2)).poll(&mut cx),
        Poll::Ready(())
    );
    for value in 1..5 {
        assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(value));
    }
    // A count beyond the depth waits for a full queue; zero waits for one.
    assert_eq!(
        pin!(receiver.ready_at_least(9)).poll(&mut cx),
        Poll::Pending
    );
    for value in 5..9 {
        assert_eq!(fill(&mut sender, value), Ok(()));
    }
    assert_eq!(told(&consumer), 2, "told when the queue is full");
    for value in 5..9 {
        assert_eq!(receiver.try_receive_with(|slot| *slot), Ok(value));
    }
    assert_eq!(
        pin!(receiver.ready_at_least(0)).poll(&mut cx),
        Poll::Pending
    );
    assert_eq!(fill(&mut sender, 9), Ok(()));
    assert_eq!(told(&consumer), 3, "zero counts as one");
}
