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
